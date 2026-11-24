// SPDX-License-Identifier: LGPL-2.1-or-later
//! The render server: sends a job to the chosen clients and collects the results.
//!
//! Clients pull work: an idle client asks for the next piece, so fast machines take more
//! pieces than slow ones. When nothing is left, an idle client also starts the piece whose
//! client will need the longest (by its measured speed, or its hardware before the first
//! piece); the first result wins and the other client is stopped. Frames are written as soon
//! as they arrive; sample ranges of a still image are added up and finished at the end.

use std::collections::{HashMap, HashSet, VecDeque};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rmb_core::Scene;
use rmb_core::render::denoise::HdrImage;

use crate::proto::{JobKind, JobSpec, Msg, Specs, Unit, recv, send, sha256};

/// Where a job's results go.
#[derive(Clone, Debug)]
pub enum Output {
    /// Animation frames: a movie (`format`) or a folder of PNG files; `ffmpeg` encodes movies
    /// after the last frame.
    Animation { path: PathBuf, format: rmb_core::video::VideoFormat, ffmpeg: PathBuf, keep_frames: bool },
    /// A still image (PNG 8/16 bit or EXR by `kind`).
    Still { path: PathBuf, kind: rmb_core::io::image::ImageKind },
}

#[derive(Clone, Debug)]
pub struct JobRequest {
    pub scene: Scene,
    /// Folder of the scene file (relative sound paths of movies).
    pub scene_dir: Option<PathBuf>,
    pub width: u32,
    pub height: u32,
    pub samples: u32,
    pub kind: JobKind,
    pub output: Output,
    /// Shared key for clients that need one.
    pub key: Option<String>,
}

/// A client to render on.
#[derive(Clone, Debug)]
pub struct Target {
    pub name: String,
    pub addr: SocketAddr,
    pub specs: Specs,
}

/// State of one client during a job (for the display).
#[derive(Clone, Debug, Default)]
pub struct ClientStatus {
    pub name: String,
    pub specs: Specs,
    pub state: String,
    pub unit: Option<Unit>,
    pub fraction: f32,
    pub units_done: usize,
    /// Seconds per unit of work weight (see `Unit::weight`), once measured.
    pub speed: Option<f64>,
}

/// State of a job (for the display).
#[derive(Clone, Debug, Default)]
pub struct Status {
    pub clients: Vec<ClientStatus>,
    pub done: usize,
    pub total: usize,
    pub started: Option<Instant>,
    /// Encoding the movie, finishing the image…
    pub phase: String,
    pub finished: bool,
    pub error: Option<String>,
    /// Samples per frame (or of the still image).
    pub samples: u32,
    pub animation: bool,
}

impl ClientStatus {
    /// Measured speed as text: seconds per frame, or per sample of a still image.
    pub fn speed_text(&self, status: &Status) -> String {
        match self.speed {
            Some(v) if status.animation => format!("{:.1} s/frame", v * status.samples.max(1) as f64),
            Some(v) => format!("{:.2} s/sample", v),
            None => String::new(),
        }
    }
}

impl Status {
    /// Share of the job done, counting the pieces in progress.
    pub fn fraction(&self) -> f32 {
        if self.total == 0 {
            return 0.0;
        }
        let running: f32 = self.clients.iter().filter(|c| c.unit.is_some()).map(|c| c.fraction).sum();
        ((self.done as f32 + running.min((self.total - self.done) as f32)) / self.total as f32).min(1.0)
    }
}

/// Shared work state.
struct Work {
    queue: VecDeque<Unit>,
    /// Units in progress: client index, start time, fraction.
    running: HashMap<Unit, Vec<(usize, Instant, f32)>>,
    done: HashSet<Unit>,
    total: usize,
    /// Seconds per weight unit per client (measured) and a guess from the hardware.
    speed: Vec<Option<f64>>,
    guess: Vec<f64>,
    samples: u32,
}

impl Work {
    fn finished(&self) -> bool {
        self.done.len() >= self.total
    }

    fn time_per_weight(&self, c: usize) -> f64 {
        self.speed[c].unwrap_or(self.guess[c])
    }

    /// The next unit for client `c`: the queue first, then a duplicate of the unit whose client
    /// needs the longest when `c` would clearly be faster.
    fn next(&mut self, c: usize) -> Option<Unit> {
        if let Some(u) = self.queue.pop_front() {
            self.running.entry(u).or_default().push((c, Instant::now(), 0.0));
            return Some(u);
        }
        let mine = self.time_per_weight(c);
        let mut best: Option<(Unit, f64)> = None;
        for (u, workers) in &self.running {
            if workers.len() >= 2 || workers.iter().any(|w| w.0 == c) {
                continue;
            }
            let (owner, start, frac) = workers[0];
            let weight = u.weight(self.samples);
            let total = weight * self.time_per_weight(owner);
            let left = (total * (1.0 - frac as f64)).max(total - start.elapsed().as_secs_f64());
            let mine_total = weight * mine;
            if mine_total < 0.7 * left && best.is_none_or(|b| left > b.1) {
                best = Some((*u, left));
            }
        }
        let (u, _) = best?;
        self.running.entry(u).or_default().push((c, Instant::now(), 0.0));
        Some(u)
    }

    /// Unit `u` of client `c` failed or the connection broke: back into the queue unless
    /// someone else works on it.
    fn give_back(&mut self, c: usize, u: Unit) {
        if let Some(w) = self.running.get_mut(&u) {
            w.retain(|x| x.0 != c);
            if w.is_empty() {
                self.running.remove(&u);
                if !self.done.contains(&u) {
                    self.queue.push_front(u);
                }
            }
        }
    }
}

/// Sum of the sample ranges of a still image.
struct StillSum {
    n: f64,
    color: Vec<f64>,
    albedo: Vec<f64>,
    normal: Vec<f64>,
    hits: Vec<f64>,
    depth: Vec<f64>,
}

/// A still image's sample range as bytes: per pixel color, albedo, normal (3 floats each),
/// coverage and depth.
pub fn encode_hdr(img: &HdrImage) -> Vec<u8> {
    let n = (img.width * img.height) as usize;
    let mut out = Vec::with_capacity(8 + n * 11 * 4);
    out.extend(img.width.to_le_bytes());
    out.extend(img.height.to_le_bytes());
    let get = |v: &[f32], i: usize| v.get(i).copied().unwrap_or(0.0);
    for i in 0..n {
        for k in 0..3 {
            out.extend(get(&img.color, i * 3 + k).to_le_bytes());
        }
        for k in 0..3 {
            out.extend(get(&img.albedo, i * 3 + k).to_le_bytes());
        }
        for k in 0..3 {
            out.extend(get(&img.normal, i * 3 + k).to_le_bytes());
        }
        out.extend(get(&img.alpha, i).to_le_bytes());
        out.extend(img.depth.get(i).copied().unwrap_or(f32::INFINITY).to_le_bytes());
    }
    out
}

impl StillSum {
    fn new(w: u32, h: u32) -> StillSum {
        let n = (w * h) as usize;
        StillSum { n: 0.0, color: vec![0.0; n * 3], albedo: vec![0.0; n * 3], normal: vec![0.0; n * 3], hits: vec![0.0; n], depth: vec![0.0; n] }
    }

    /// Adds a range of `count` samples (see `encode_hdr`).
    fn add(&mut self, bytes: &[u8], count: u32) -> Result<(), String> {
        let n = self.hits.len();
        if bytes.len() != 8 + n * 44 {
            return Err("an image of the wrong size arrived".into());
        }
        let f = |i: usize| f32::from_le_bytes([bytes[8 + i * 4], bytes[9 + i * 4], bytes[10 + i * 4], bytes[11 + i * 4]]) as f64;
        let c = count as f64;
        for i in 0..n {
            let b = i * 11;
            for k in 0..3 {
                self.color[i * 3 + k] += f(b + k) * c;
                self.albedo[i * 3 + k] += f(b + 3 + k) * c;
                self.normal[i * 3 + k] += f(b + 6 + k) * c;
            }
            let (a, d) = (f(b + 9), f(b + 10));
            self.hits[i] += a * c;
            if a > 0.0 && d.is_finite() {
                self.depth[i] += d * a * c;
            }
        }
        self.n += c;
        Ok(())
    }

    fn image(&self, w: u32, h: u32) -> HdrImage {
        let k = 1.0 / self.n.max(1.0);
        HdrImage {
            width: w,
            height: h,
            color: self.color.iter().map(|v| (v * k) as f32).collect(),
            albedo: self.albedo.iter().map(|v| (v * k) as f32).collect(),
            normal: self.normal.iter().map(|v| (v * k) as f32).collect(),
            alpha: self.hits.iter().map(|v| (v * k) as f32).collect(),
            depth: self.depth.iter().zip(&self.hits).map(|(d, h)| if *h > 0.0 { (d / h) as f32 } else { f32::INFINITY }).collect(),
        }
    }
}

/// A rough time per weight unit from the hardware (only until the first piece is measured):
/// graphics cards far ahead of processors.
fn guess_time(s: &Specs) -> f64 {
    if s.gpu.is_empty() { 1.0 / (s.threads.max(1) as f64) * 50.0 } else { 0.05 }
}

struct Shared {
    work: Mutex<Work>,
    status: Arc<Mutex<Status>>,
    still: Option<Mutex<StillSum>>,
    cancel: Arc<AtomicBool>,
    /// Frames folder (animations).
    frames_dir: Option<PathBuf>,
    from: usize,
}

/// Runs a job on `targets` (blocking). `status` is updated for a display; setting `cancel`
/// stops the job. Returns a summary of what was written.
pub fn run_job(req: JobRequest, targets: Vec<Target>, status: Arc<Mutex<Status>>, cancel: Arc<AtomicBool>) -> Result<String, String> {
    if targets.is_empty() {
        return Err("no clients selected".into());
    }
    let set_phase = |p: &str| {
        if let Ok(mut s) = status.lock() {
            s.phase = p.to_string();
        }
    };
    set_phase("Preparing the files");
    // Audio-reactive drivers: the clients get the sound's levels with the scene.
    let mut req = req;
    if let Output::Animation { ffmpeg, .. } = &req.output
        && let Some(a) = req.scene.animation.as_mut()
    {
        a.ensure_audio_levels(req.scene_dir.as_deref(), ffmpeg)?;
    }
    rmb_core::compose::load_models(&req.scene);
    let prepared = crate::assets::prepare(&req.scene)?;
    let job_id = crate::proto::random_u64();
    let units: Vec<Unit> = match req.kind {
        JobKind::Animation { from, to } => (from..=to).map(|f| Unit::Frame { f }).collect(),
        JobKind::Still => {
            // About four pieces per client, so faster ones can take more.
            let n = req.samples.max(1);
            let per = n.div_ceil((targets.len() as u32 * 4).max(1)).max(1);
            (0..n).step_by(per as usize).map(|start| Unit::Samples { start, count: per.min(n - start) }).collect()
        }
    };
    let (frames_dir, from) = match (&req.output, req.kind) {
        (Output::Animation { path, format, .. }, JobKind::Animation { from, .. }) => {
            let dir = if format.needs_ffmpeg() { path.with_extension("frames") } else { path.clone() };
            std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            (Some(dir), from)
        }
        _ => (None, 0),
    };
    let spec = JobSpec { job_id, scene: prepared.scene.to_json(), width: req.width, height: req.height, samples: req.samples, kind: req.kind, files: prepared.files.iter().map(|f| f.1.clone()).collect() };
    let total = units.len();
    {
        let mut s = status.lock().map_err(|_| "status lock")?;
        *s = Status { clients: targets.iter().map(|t| ClientStatus { name: t.name.clone(), specs: t.specs.clone(), state: "connecting".into(), ..Default::default() }).collect(), done: 0, total, started: Some(Instant::now()), phase: "Rendering".into(), finished: false, error: None, samples: req.samples, animation: matches!(req.kind, JobKind::Animation { .. }) };
    }
    let shared = Arc::new(Shared {
        work: Mutex::new(Work { queue: units.into(), running: HashMap::new(), done: HashSet::new(), total, speed: vec![None; targets.len()], guess: targets.iter().map(|t| guess_time(&t.specs)).collect(), samples: req.samples }),
        status: status.clone(),
        still: matches!(req.kind, JobKind::Still).then(|| Mutex::new(StillSum::new(req.width, req.height))),
        cancel: cancel.clone(),
        frames_dir: frames_dir.clone(),
        from,
    });
    let files: Arc<HashMap<String, PathBuf>> = Arc::new(prepared.files.iter().map(|(p, e)| (e.hash.clone(), p.clone())).collect());
    let spec = Arc::new(spec);
    let handles: Vec<_> = targets
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let (shared, files, spec, key, t) = (shared.clone(), files.clone(), spec.clone(), req.key.clone(), t.clone());
            std::thread::spawn(move || client_thread(i, t, &shared, &spec, &files, key.as_deref()))
        })
        .collect();
    // Wait until every piece is done, the job is cancelled or no client is left.
    let mut gave_up = false;
    loop {
        std::thread::sleep(Duration::from_millis(200));
        let finished = shared.work.lock().map(|w| w.finished()).unwrap_or(true);
        if finished || cancel.load(Ordering::Relaxed) {
            break;
        }
        if handles.iter().all(|h| h.is_finished()) {
            gave_up = true;
            break;
        }
    }
    let done_all = shared.work.lock().map(|w| w.finished()).unwrap_or(false);
    if !done_all {
        cancel.store(true, Ordering::Relaxed);
    }
    for h in handles {
        let _ = h.join();
    }
    if cancel.load(Ordering::Relaxed) && !done_all {
        let reason = if gave_up { "no client could continue (see their states)" } else { "cancelled" };
        if let Ok(mut s) = status.lock() {
            s.error = Some(reason.into());
            s.finished = true;
        }
        return Err(reason.into());
    }
    let summary = finish(&req, &shared, &set_phase);
    if let Ok(mut s) = status.lock() {
        s.finished = true;
        s.phase = "Finished".into();
        if let Err(e) = &summary {
            s.error = Some(e.clone());
        }
    }
    summary
}

/// Writes the movie or the still image after the last piece.
fn finish(req: &JobRequest, shared: &Shared, set_phase: &impl Fn(&str)) -> Result<String, String> {
    match (&req.output, req.kind) {
        (Output::Animation { path, format, ffmpeg, keep_frames }, JobKind::Animation { from, to }) => {
            let format = *format;
            let dir = shared.frames_dir.clone().ok_or("no frames folder")?;
            let n = to + 1 - from;
            if !format.needs_ffmpeg() {
                return Ok(format!("{} frames in {}", n, dir.display()));
            }
            set_phase("Encoding the movie");
            let anim = req.scene.animation.clone().ok_or("the scene has no animation")?;
            let plan = rmb_core::animation::AnimationPlan::new(&anim)?;
            let audio = anim.audio_track(from, req.scene_dir.as_deref());
            let mut sink = rmb_core::video::VideoSink::new(path, format, req.width, req.height, plan.fps(), ffmpeg, audio.as_ref())?;
            for i in 0..n {
                let file = dir.join(frame_name(i));
                let img = image::open(&file).map_err(|e| format!("{}: {e}", file.display()))?.to_rgba8();
                sink.write_frame(i, img.as_raw())?;
            }
            sink.finish()?;
            if !keep_frames {
                let _ = std::fs::remove_dir_all(&dir);
            }
            Ok(format!("{} ({n} frames)", path.display()))
        }
        (Output::Still { path, kind }, JobKind::Still) => {
            set_phase("Finishing the image");
            let sum = shared.still.as_ref().ok_or("no image")?.lock().map_err(|_| "image lock")?;
            let img = sum.image(req.width, req.height);
            let outcome = rmb_core::io::image::save_render(path, img, &req.scene, *kind)?;
            let note = if req.scene.post.denoise { format!(", {}", outcome.describe()) } else { String::new() };
            Ok(format!("{} ({} × {}, {} samples{note})", path.display(), req.width, req.height, req.samples))
        }
        _ => Err("the output does not match the job".into()),
    }
}

fn frame_name(i: usize) -> String {
    format!("frame_{i:05}.png")
}

fn set_client(shared: &Shared, i: usize, f: impl FnOnce(&mut ClientStatus)) {
    if let Ok(mut s) = shared.status.lock()
        && let Some(c) = s.clients.get_mut(i)
    {
        f(c);
    }
}

/// Talks to one client until the job is done; reconnects after errors (a restarted client).
fn client_thread(i: usize, t: Target, shared: &Shared, spec: &JobSpec, files: &HashMap<String, PathBuf>, key: Option<&str>) {
    let mut failures = 0;
    loop {
        if shared.cancel.load(Ordering::Relaxed) || shared.work.lock().map(|w| w.finished()).unwrap_or(true) {
            set_client(shared, i, |c| {
                c.state = "done".into();
                c.unit = None;
            });
            return;
        }
        let mut current: Option<Unit> = None;
        let r = session(i, &t, shared, spec, files, key, &mut current);
        if let Some(u) = current
            && let Ok(mut w) = shared.work.lock()
        {
            w.give_back(i, u);
        }
        match r {
            Ok(()) => {
                set_client(shared, i, |c| {
                    c.state = "done".into();
                    c.unit = None;
                });
                return;
            }
            Err(Fatal(e)) => {
                set_client(shared, i, |c| {
                    c.state = e;
                    c.unit = None;
                });
                return;
            }
            Err(Retry(e)) => {
                failures += 1;
                set_client(shared, i, |c| {
                    c.state = format!("{e}; trying again");
                    c.unit = None;
                });
                // Back off a little more after every failure (up to 20 s).
                let wait = Duration::from_secs((2 * failures).min(20));
                let until = Instant::now() + wait;
                while Instant::now() < until && !shared.cancel.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(200));
                }
            }
        }
    }
}

enum SessionError {
    /// Give up on this client (wrong version or key).
    Fatal(String),
    /// Try again later (connection lost, client busy or restarting).
    Retry(String),
}
use SessionError::{Fatal, Retry};

fn retry<T>(r: std::io::Result<T>) -> Result<T, SessionError> {
    r.map_err(|e| Retry(format!("connection: {e}")))
}

#[allow(clippy::too_many_arguments)]
fn session(i: usize, t: &Target, shared: &Shared, spec: &JobSpec, files: &HashMap<String, PathBuf>, key: Option<&str>, current: &mut Option<Unit>) -> Result<(), SessionError> {
    set_client(shared, i, |c| c.state = "connecting".into());
    let mut s = retry(TcpStream::connect_timeout(&t.addr, Duration::from_secs(4)))?;
    let _ = s.set_nodelay(true);
    // A client busy with another server does not answer: give up on it quickly.
    retry(s.set_read_timeout(Some(Duration::from_secs(10))))?;
    let (m, _) = recv(&mut s).map_err(|_| Retry("busy or not answering".into()))?;
    retry(s.set_read_timeout(Some(Duration::from_secs(300))))?;
    let Msg::Welcome { version, nonce, key: needs_key, .. } = m else { return Err(Retry("unexpected message".into())) };
    if version != crate::version() {
        return Err(Fatal(format!("version {version} (needs {})", crate::version())));
    }
    let auth = match (needs_key, key) {
        (true, Some(k)) => Some(crate::proto::auth(&nonce, k)),
        (true, None) => return Err(Fatal("needs a key (--key)".into())),
        _ => None,
    };
    let server = crate::host_name();
    retry(send(&mut s, &Msg::Hello { version: crate::version(), server, auth }, None))?;
    match retry(recv(&mut s))?.0 {
        Msg::Accepted => {}
        Msg::Rejected { reason } => return Err(Fatal(format!("rejected: {reason}"))),
        // A client rendering for another server: try again later.
        _ => return Err(Retry("busy".into())),
    }
    set_client(shared, i, |c| c.state = "sending files".into());
    retry(send(&mut s, &Msg::Job(Box::new(spec.clone())), None))?;
    let (m, _) = retry(recv(&mut s))?;
    let Msg::Need { hashes } = m else { return Err(Retry("unexpected message".into())) };
    for (k, h) in hashes.iter().enumerate() {
        let Some(path) = files.get(h) else { continue };
        let bytes = std::fs::read(path).map_err(|e| Fatal(format!("{}: {e}", path.display())))?;
        retry(send(&mut s, &Msg::File { hash: h.clone(), bytes: bytes.len() as u64 }, Some(&bytes)))?;
        set_client(shared, i, |c| c.state = format!("sending files ({}/{})", k + 1, hashes.len()));
    }
    retry(send(&mut s, &Msg::FilesDone, None))?;
    set_client(shared, i, |c| c.state = "ready".into());
    loop {
        let (m, blob) = retry(recv(&mut s))?;
        match m {
            Msg::Ready => {
                if shared.cancel.load(Ordering::Relaxed) {
                    let _ = send(&mut s, &Msg::Done, None);
                    return Ok(());
                }
                let (next, finished, done, total) = {
                    let mut w = shared.work.lock().map_err(|_| Fatal("lock".into()))?;
                    let n = w.next(i);
                    (n, w.finished(), w.done.len(), w.total)
                };
                match next {
                    Some(u) => {
                        *current = Some(u);
                        set_client(shared, i, |c| {
                            c.state = "rendering".into();
                            c.unit = Some(u);
                            c.fraction = 0.0;
                        });
                        retry(send(&mut s, &Msg::Work { unit: u, done, total }, None))?;
                    }
                    None if finished => {
                        retry(send(&mut s, &Msg::Done, None))?;
                        let _ = recv(&mut s);
                        return Ok(());
                    }
                    None => {
                        set_client(shared, i, |c| {
                            c.state = "waiting for the others".into();
                            c.unit = None;
                        });
                        retry(send(&mut s, &Msg::Wait, None))?;
                    }
                }
            }
            Msg::Progress { unit, fraction } => {
                let (cancelled, done, total) = {
                    let mut w = shared.work.lock().map_err(|_| Fatal("lock".into()))?;
                    if let Some(ws) = w.running.get_mut(&unit) {
                        for x in ws.iter_mut().filter(|x| x.0 == i) {
                            x.2 = fraction;
                        }
                    }
                    (w.done.contains(&unit) || shared.cancel.load(Ordering::Relaxed), w.done.len(), w.total)
                };
                set_client(shared, i, |c| c.fraction = fraction);
                if cancelled {
                    if let Ok(mut w) = shared.work.lock() {
                        w.give_back(i, unit);
                    }
                    *current = None;
                    retry(send(&mut s, &Msg::Cancel, None))?;
                } else {
                    retry(send(&mut s, &Msg::Continue { done, total }, None))?;
                }
            }
            Msg::Result { unit, hash, .. } => {
                if sha256(&blob) != hash {
                    retry(send(&mut s, &Msg::Ack { unit, ok: false }, None))?;
                    continue;
                }
                store(i, shared, unit, &blob).map_err(Fatal)?;
                retry(send(&mut s, &Msg::Ack { unit, ok: true }, None))?;
                if *current == Some(unit) {
                    *current = None;
                }
            }
            Msg::Failed { unit, error } => {
                if let Ok(mut w) = shared.work.lock() {
                    w.give_back(i, unit);
                }
                *current = None;
                set_client(shared, i, |c| {
                    c.state = format!("failed: {error}");
                    c.unit = None;
                });
                return Err(Fatal(format!("failed: {error}")));
            }
            Msg::Bye => return Ok(()),
            other => return Err(Retry(format!("unexpected message {other:?}"))),
        }
    }
}

/// Stores a result (once per unit; later duplicates are dropped) and updates the counts and
/// the client's speed.
fn store(i: usize, shared: &Shared, unit: Unit, blob: &[u8]) -> Result<(), String> {
    let started = {
        let w = shared.work.lock().map_err(|_| "lock")?;
        if w.done.contains(&unit) {
            return Ok(());
        }
        w.running.get(&unit).and_then(|ws| ws.iter().find(|x| x.0 == i).map(|x| x.1))
    };
    match unit {
        Unit::Frame { f } => {
            let dir = shared.frames_dir.as_ref().ok_or("no frames folder")?;
            let file = dir.join(frame_name(f - shared.from));
            let tmp = file.with_extension("part");
            std::fs::write(&tmp, blob).and_then(|_| std::fs::rename(&tmp, &file)).map_err(|e| format!("{}: {e}", file.display()))?;
        }
        Unit::Samples { count, .. } => {
            shared.still.as_ref().ok_or("no image")?.lock().map_err(|_| "image lock")?.add(blob, count)?;
        }
    }
    let mut w = shared.work.lock().map_err(|_| "lock")?;
    w.done.insert(unit);
    w.running.remove(&unit);
    if let Some(t0) = started {
        let per = t0.elapsed().as_secs_f64() / unit.weight(w.samples);
        // Smoothed: the newest piece counts half.
        w.speed[i] = Some(w.speed[i].map_or(per, |s| 0.5 * s + 0.5 * per));
    }
    let (done, speed) = (w.done.len(), w.speed[i]);
    drop(w);
    if let Ok(mut s) = shared.status.lock() {
        s.done = done;
        if let Some(c) = s.clients.get_mut(i) {
            c.units_done += 1;
            c.speed = speed;
            c.unit = None;
            c.fraction = 0.0;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn work(clients: usize, units: usize) -> Work {
        Work { queue: (0..units).map(|f| Unit::Frame { f }).collect(), running: HashMap::new(), done: HashSet::new(), total: units, speed: vec![None; clients], guess: vec![1.0; clients], samples: 4 }
    }

    #[test]
    fn idle_fast_clients_take_over_the_slowest_piece_at_the_end() {
        let mut w = work(2, 1);
        w.speed = vec![Some(10.0), Some(1.0)];
        // The slow client takes the only frame; the fast one gets a duplicate of it.
        assert_eq!(w.next(0), Some(Unit::Frame { f: 0 }));
        assert_eq!(w.next(1), Some(Unit::Frame { f: 0 }));
        // No third copy, and nothing for a client that already has it.
        assert_eq!(w.next(1), None);
        // Equal speeds: no duplicate.
        let mut w = work(2, 1);
        w.speed = vec![Some(1.0), Some(1.0)];
        w.next(0);
        assert_eq!(w.next(1), None);
    }

    #[test]
    fn failed_pieces_go_back_into_the_queue() {
        let mut w = work(1, 2);
        let u = w.next(0).unwrap();
        w.give_back(0, u);
        assert_eq!(w.queue.front(), Some(&u));
    }

    #[test]
    fn sample_ranges_add_up_to_the_mean() {
        let img = |v: f32| HdrImage { width: 1, height: 1, color: vec![v; 3], albedo: vec![v; 3], normal: vec![0.0, 0.0, 1.0], alpha: vec![1.0], depth: vec![2.0] };
        let mut s = StillSum::new(1, 1);
        s.add(&encode_hdr(&img(1.0)), 1).unwrap();
        s.add(&encode_hdr(&img(4.0)), 3).unwrap();
        let m = s.image(1, 1);
        assert!((m.color[0] - 3.25).abs() < 1e-6 && (m.depth[0] - 2.0).abs() < 1e-6 && m.alpha[0] == 1.0);
    }
}
