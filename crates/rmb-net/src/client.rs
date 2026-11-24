// SPDX-License-Identifier: LGPL-2.1-or-later
//! The render client: announces itself, waits for a server to connect, receives the job and
//! its files, renders the pieces the server hands out and sends them back. A finished piece
//! stays in the client's spool folder until the server confirmed it; after a broken
//! connection the server gets the spooled pieces first when it comes back.

use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rmb_core::Scene;
use rmb_gpu::GpuContext;

use crate::assets::Cache;
use crate::discovery::Announce;
use crate::proto::{JobKind, JobSpec, Msg, Specs, Unit, recv, send, sha256};

#[derive(Clone, Debug, Default)]
pub struct ClientOptions {
    /// Name shown on the server (default: the host name).
    pub name: Option<String>,
    /// TCP port (default `CLIENT_PORT`; another free one if it is taken).
    pub port: Option<u16>,
    /// Shared key the server must know.
    pub key: Option<String>,
    /// Render on the CPU even with a GPU.
    pub cpu: bool,
    /// Cache folder (default: the user's cache folder).
    pub cache: Option<PathBuf>,
    /// Do not announce (tests; the server connects by address).
    pub silent: bool,
    /// Stop after one job (tests).
    pub once: bool,
}

/// What the client is doing (for its display).
#[derive(Clone, Debug)]
pub enum ClientEvent {
    Started { name: String, port: u16, specs: Specs },
    Waiting,
    Connected { server: String },
    Files { done: usize, total: usize },
    Rendering { unit: Unit, fraction: f32, done: usize, total: usize },
    Sent { unit: Unit },
    JobFinished,
    Error(String),
}

/// Runs the client until it fails to start (or after one job with `once`).
pub fn run_client(opts: ClientOptions, mut on_event: impl FnMut(ClientEvent)) -> Result<(), String> {
    let ctx = if opts.cpu { None } else { GpuContext::new_headless().ok() };
    let specs = Specs::of_this_machine(ctx.as_ref().map(|c| c.adapter_name.as_str()).unwrap_or(""));
    let name = opts.name.clone().unwrap_or_else(crate::host_name);
    let listener = TcpListener::bind((Ipv4Addr::UNSPECIFIED, opts.port.unwrap_or(crate::CLIENT_PORT))).or_else(|_| TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0))).map_err(|e| format!("cannot open a TCP port: {e}"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let id = crate::proto::random_u64();
    let info = Arc::new(Mutex::new(Announce::new(id, name.clone(), port, opts.key.is_some(), specs.clone())));
    let stop = Arc::new(AtomicBool::new(false));
    if !opts.silent {
        crate::discovery::announce(info.clone(), stop.clone()).map_err(|e| format!("cannot announce on the network: {e}"))?;
    }
    on_event(ClientEvent::Started { name: name.clone(), port, specs: specs.clone() });
    let cache = Cache::new(opts.cache.clone());
    let mut session = Session { ctx, cache, opts: opts.clone(), name, id, specs, job: None };
    let result = loop {
        on_event(ClientEvent::Waiting);
        let (stream, from) = match listener.accept() {
            Ok(s) => s,
            Err(e) => break Err(format!("accept failed: {e}")),
        };
        set_busy(&info, true);
        on_event(ClientEvent::Connected { server: from.ip().to_string() });
        let r = session.serve(stream, &mut on_event);
        set_busy(&info, false);
        match r {
            Ok(finished) => {
                if finished {
                    on_event(ClientEvent::JobFinished);
                    if opts.once {
                        break Ok(());
                    }
                }
            }
            Err(e) => on_event(ClientEvent::Error(e)),
        }
    };
    stop.store(true, Ordering::Relaxed);
    result
}

/// Starts a client in this process (the server's own machine renders too) and returns it as a
/// target. It runs until the program ends.
pub fn start_local(key: Option<String>) -> Result<crate::server::Target, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let name = format!("{} (this machine)", crate::host_name());
    let opts = ClientOptions { name: Some(name.clone()), port: Some(0), key, silent: true, ..Default::default() };
    std::thread::spawn(move || {
        let _ = run_client(opts, |e| {
            if let ClientEvent::Started { port, specs, .. } = e {
                let _ = tx.send((port, specs));
            }
        });
    });
    let (port, specs) = rx.recv_timeout(Duration::from_secs(30)).map_err(|_| "the local client did not start".to_string())?;
    Ok(crate::server::Target { name, addr: ([127, 0, 0, 1], port).into(), specs })
}

fn set_busy(info: &Mutex<Announce>, busy: bool) {
    if let Ok(mut i) = info.lock() {
        i.busy = busy;
    }
}

/// The current job and its renderer (kept over reconnections of the same job).
struct Job {
    spec: JobSpec,
    frames: Option<FramesJob>,
    still: Option<rmb_gpu::GpuRenderer>,
    still_cpu: Option<rmb_core::render::cpu::CpuRenderer>,
}

struct FramesJob {
    plan: rmb_core::animation::AnimationPlan,
    renderer: rmb_gpu::FrameRenderer,
}

struct Session {
    ctx: Option<GpuContext>,
    cache: Cache,
    opts: ClientOptions,
    name: String,
    id: u64,
    specs: Specs,
    job: Option<Job>,
}

type Io<T> = Result<T, String>;

fn io<T>(r: std::io::Result<T>) -> Io<T> {
    r.map_err(|e| format!("connection: {e}"))
}

impl Session {
    /// One connection of a server; true when the job finished.
    fn serve(&mut self, mut s: TcpStream, ev: &mut impl FnMut(ClientEvent)) -> Io<bool> {
        let _ = s.set_nodelay(true);
        io(s.set_read_timeout(Some(Duration::from_secs(120))))?;
        let nonce = format!("{:016x}", crate::proto::random_u64());
        io(send(&mut s, &Msg::Welcome { version: crate::version(), name: self.name.clone(), id: self.id, nonce: nonce.clone(), key: self.opts.key.is_some(), busy: false, specs: self.specs.clone() }, None))?;
        let (m, _) = io(recv(&mut s))?;
        let Msg::Hello { version, auth, .. } = m else { return Err("unexpected message".into()) };
        if version != crate::version() {
            let _ = send(&mut s, &Msg::Rejected { reason: format!("version {} (the client has {})", version, crate::version()) }, None);
            return Err(format!("server has version {version}, this client {}", crate::version()));
        }
        if let Some(k) = &self.opts.key
            && auth.as_deref() != Some(crate::proto::auth(&nonce, k).as_str())
        {
            let _ = send(&mut s, &Msg::Rejected { reason: "wrong key".into() }, None);
            return Err("a server with a wrong key tried to connect".into());
        }
        io(send(&mut s, &Msg::Accepted, None))?;
        let (m, _) = io(recv(&mut s))?;
        let Msg::Job(spec) = m else { return Err("expected a job".into()) };
        self.receive_files(&mut s, &spec, ev)?;
        // A new job: set it up (the same job after a reconnection keeps its renderer).
        if self.job.as_ref().is_none_or(|j| j.spec.job_id != spec.job_id) {
            self.job = Some(self.setup(*spec)?);
        }
        let job_id = self.job.as_ref().map(|j| j.spec.job_id).unwrap_or(0);
        self.send_spooled(&mut s, job_id)?;
        let mut last_counts;
        loop {
            io(send(&mut s, &Msg::Ready, None))?;
            let (m, _) = io(recv(&mut s))?;
            match m {
                Msg::Work { unit, done, total } => {
                    last_counts = (done, total);
                    ev(ClientEvent::Rendering { unit, fraction: 0.0, done, total });
                    match self.render(&mut s, unit, &mut last_counts, ev) {
                        Ok(Some(bytes)) => self.deliver(&mut s, job_id, unit, bytes, ev)?,
                        // Cancelled: another client was faster.
                        Ok(None) => {}
                        Err(e) => {
                            io(send(&mut s, &Msg::Failed { unit, error: e.clone() }, None))?;
                            ev(ClientEvent::Error(e));
                        }
                    }
                }
                Msg::Wait => std::thread::sleep(Duration::from_millis(500)),
                Msg::Done => {
                    let _ = send(&mut s, &Msg::Bye, None);
                    self.finish_job(job_id);
                    return Ok(true);
                }
                other => return Err(format!("unexpected message {other:?}")),
            }
        }
    }

    fn receive_files(&mut self, s: &mut TcpStream, spec: &JobSpec, ev: &mut impl FnMut(ClientEvent)) -> Io<()> {
        let mut need: Vec<String> = spec.files.iter().filter(|f| !self.cache.has(&f.hash)).map(|f| f.hash.clone()).collect();
        need.sort();
        need.dedup();
        let total = need.len();
        io(send(s, &Msg::Need { hashes: need }, None))?;
        let mut done = 0;
        loop {
            let (m, blob) = io(recv(s))?;
            match m {
                Msg::File { hash, .. } => {
                    self.cache.store(&hash, &blob)?;
                    done += 1;
                    ev(ClientEvent::Files { done, total });
                }
                Msg::FilesDone => break,
                other => return Err(format!("unexpected message {other:?}")),
            }
        }
        self.cache.materialize(spec.job_id, &spec.files)?;
        Ok(())
    }

    fn setup(&mut self, spec: JobSpec) -> Io<Job> {
        let mut scene = Scene::from_json(&spec.scene)?;
        self.cache.rewrite(spec.job_id, &mut scene);
        if let Some(a) = scene.animation.as_mut() {
            for k in &mut a.keyframes {
                self.cache.rewrite(spec.job_id, &mut k.scene);
            }
        }
        // The job's maps come first.
        let maps = self.cache.job_dir(spec.job_id).join("maps");
        rmb_core::maps::set_dirs(vec![maps]);
        rmb_core::compose::load_models(&scene);
        let mut job = Job { spec: spec.clone(), frames: None, still: None, still_cpu: None };
        match spec.kind {
            JobKind::Animation { from, to } => {
                let anim = scene.animation.clone().ok_or("the scene has no animation")?;
                let plan = rmb_core::animation::AnimationPlan::new(&anim)?;
                let fs = |t: f64| plan.scene_at_time(t);
                let renderer = rmb_gpu::FrameRenderer::new(self.ctx.clone(), from..to + 1, spec.width, spec.height, spec.samples, anim.shutter, &fs);
                job.frames = Some(FramesJob { plan, renderer });
            }
            JobKind::Still => match &self.ctx {
                Some(ctx) => {
                    let mut r = rmb_gpu::GpuRenderer::new(ctx.clone());
                    r.set_band_target(Duration::from_millis(50));
                    r.set_baking(Some(Vec::new()));
                    r.set_uniform_baking(Some(Vec::new()));
                    r.set_scene(&scene, spec.width, spec.height, false);
                    r.wait_for_pipeline()?;
                    job.still = Some(r);
                }
                None => job.still_cpu = Some(rmb_core::render::cpu::CpuRenderer::new(scene.clone(), spec.width, spec.height)),
            },
        }
        Ok(job)
    }

    /// Renders a piece; None when the server cancelled it.
    fn render(&mut self, s: &mut TcpStream, unit: Unit, counts: &mut (usize, usize), ev: &mut impl FnMut(ClientEvent)) -> Io<Option<Vec<u8>>> {
        let job = self.job.as_mut().ok_or("no job")?;
        let cancel = AtomicBool::new(false);
        let mut last = Instant::now();
        let mut net_error: Option<String> = None;
        // Progress goes to the server (at most four times a second); its answer may cancel.
        let mut report = |p: f32| {
            ev(ClientEvent::Rendering { unit, fraction: p, done: counts.0, total: counts.1 });
            if last.elapsed() < Duration::from_millis(250) || net_error.is_some() {
                return;
            }
            last = Instant::now();
            let r = send(s, &Msg::Progress { unit, fraction: p }, None).and_then(|_| recv(s));
            match r {
                Ok((Msg::Continue { done, total }, _)) => *counts = (done, total),
                Ok((Msg::Cancel, _)) => cancel.store(true, Ordering::Relaxed),
                Ok((m, _)) => net_error = Some(format!("unexpected message {m:?}")),
                Err(e) => {
                    net_error = Some(format!("connection: {e}"));
                    cancel.store(true, Ordering::Relaxed);
                }
            }
        };
        let (w, h) = (job.spec.width, job.spec.height);
        let result: Io<Vec<u8>> = match unit {
            Unit::Frame { f } => {
                let fj = job.frames.as_mut().ok_or("not an animation job")?;
                let plan = &fj.plan;
                let fs = |t: f64| plan.scene_at_time(t);
                fj.renderer.render(f, &fs, &cancel, &mut report).and_then(|rgba| png_bytes(w, h, &rgba))
            }
            Unit::Samples { start, count } => {
                let img = if let Some(r) = job.still.as_mut() {
                    r.start_at_sample(start);
                    while r.samples() < start + count {
                        r.work(Duration::from_millis(100), start + count);
                        if let Some(e) = r.last_error() {
                            return Err(e);
                        }
                        report((r.samples() - start) as f32 / count as f32 + r.pass_progress() / count as f32);
                        if cancel.load(Ordering::Relaxed) {
                            break;
                        }
                    }
                    r.read_hdr()
                } else if let Some(r) = job.still_cpu.as_mut() {
                    r.start_at_sample(start);
                    for i in 0..count {
                        if cancel.load(Ordering::Relaxed) {
                            break;
                        }
                        r.render_pass();
                        report((i + 1) as f32 / count as f32);
                    }
                    Some(r.hdr_image())
                } else {
                    None
                };
                img.map(|i| crate::server::encode_hdr(&i)).ok_or_else(|| "reading the image back failed".to_string())
            }
        };
        if let Some(e) = net_error {
            return Err(e);
        }
        if cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }
        result.map(Some)
    }

    /// Spools a result, sends it and deletes it once the server confirmed it.
    fn deliver(&mut self, s: &mut TcpStream, job: u64, unit: Unit, bytes: Vec<u8>, ev: &mut impl FnMut(ClientEvent)) -> Io<()> {
        let dir = self.cache.spool_dir(job);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let file = dir.join(spool_name(unit));
        std::fs::write(&file, &bytes).map_err(|e| format!("{}: {e}", file.display()))?;
        self.send_result(s, unit, &bytes, &file)?;
        ev(ClientEvent::Sent { unit });
        Ok(())
    }

    fn send_result(&mut self, s: &mut TcpStream, unit: Unit, bytes: &[u8], file: &std::path::Path) -> Io<()> {
        let hash = sha256(bytes);
        for _ in 0..3 {
            io(send(s, &Msg::Result { unit, hash: hash.clone(), bytes: bytes.len() as u64 }, Some(bytes)))?;
            let (m, _) = io(recv(s))?;
            match m {
                Msg::Ack { ok: true, .. } => {
                    // Stored on the server: only now the local copy goes.
                    let _ = std::fs::remove_file(file);
                    return Ok(());
                }
                Msg::Ack { ok: false, .. } => continue,
                other => return Err(format!("unexpected message {other:?}")),
            }
        }
        Err(format!("{} was not accepted; kept in {}", unit.label(), file.display()))
    }

    /// Results of the job that were finished but not confirmed (broken connection).
    fn send_spooled(&mut self, s: &mut TcpStream, job: u64) -> Io<()> {
        let dir = self.cache.spool_dir(job);
        let Ok(rd) = std::fs::read_dir(&dir) else { return Ok(()) };
        for e in rd.flatten() {
            let p = e.path();
            let Some(unit) = p.file_name().and_then(|n| n.to_str()).and_then(parse_spool_name) else { continue };
            let Ok(bytes) = std::fs::read(&p) else { continue };
            self.send_result(s, unit, &bytes, &p)?;
        }
        Ok(())
    }

    fn finish_job(&mut self, job: u64) {
        self.job = None;
        let _ = std::fs::remove_dir_all(self.cache.job_dir(job));
        let _ = std::fs::remove_dir(self.cache.spool_dir(job));
        rmb_core::maps::set_dirs(Vec::new());
    }
}

fn spool_name(u: Unit) -> String {
    match u {
        Unit::Frame { f } => format!("frame_{f}.png"),
        Unit::Samples { start, count } => format!("samples_{start}_{count}.bin"),
    }
}

fn parse_spool_name(n: &str) -> Option<Unit> {
    if let Some(f) = n.strip_prefix("frame_").and_then(|r| r.strip_suffix(".png")) {
        return f.parse().ok().map(|f| Unit::Frame { f });
    }
    let r = n.strip_prefix("samples_")?.strip_suffix(".bin")?;
    let (a, b) = r.split_once('_')?;
    Some(Unit::Samples { start: a.parse().ok()?, count: b.parse().ok()? })
}

/// A finished frame as PNG bytes.
fn png_bytes(w: u32, h: u32, rgba: &[u8]) -> Io<Vec<u8>> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut wr = enc.write_header().map_err(|e| e.to_string())?;
        wr.write_image_data(rgba).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spool_names_round_trip() {
        for u in [Unit::Frame { f: 12 }, Unit::Samples { start: 4, count: 8 }] {
            assert_eq!(parse_spool_name(&spool_name(u)), Some(u));
        }
    }
}
