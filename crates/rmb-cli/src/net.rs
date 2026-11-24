// SPDX-License-Identifier: LGPL-2.1-or-later
//! Network rendering commands: `--network` (render client), `net-render` (server) and
//! `net-clients` (list the clients on the network).

use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rmb_net::client::{ClientEvent, ClientOptions};
use rmb_net::proto::{JobKind, Unit};
use rmb_net::server::{JobRequest, Output, Status, Target};

/// A text progress bar of `width` characters.
fn bar(f: f32, width: usize) -> String {
    let n = ((f.clamp(0.0, 1.0) * width as f32).round() as usize).min(width);
    format!("[{}{}]", "#".repeat(n), "-".repeat(width - n))
}

fn clock(s: f64) -> String {
    let s = s.max(0.0).round() as u64;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

/// `rmb-cli --network`: runs a render client until Ctrl+C.
pub fn client(args: &[String]) -> Result<(), String> {
    let mut opts = ClientOptions::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = |name: &str| it.next().cloned().ok_or_else(|| format!("{name} needs a value"));
        match a.as_str() {
            "--network" | "client" => {}
            "--name" => opts.name = Some(val("--name")?),
            "--key" => opts.key = Some(val("--key")?),
            "--port" => opts.port = Some(val("--port")?.parse().map_err(|_| "bad port")?),
            "--cache" => opts.cache = Some(PathBuf::from(val("--cache")?)),
            "--cpu" => opts.cpu = true,
            o => return Err(format!("unknown option '{o}'")),
        }
    }
    let tty = std::io::stderr().is_terminal();
    let mut line_open = false;
    let mut last_draw = Instant::now() - Duration::from_secs(1);
    let mut started = Instant::now();
    let end_line = |open: &mut bool| {
        if *open {
            eprintln!();
            *open = false;
        }
    };
    rmb_net::client::run_client(opts, |e| match e {
        ClientEvent::Started { name, port, specs } => {
            eprintln!("RMandelbulb3D render client \"{name}\" on port {port} ({})", specs.summary());
            eprintln!("Announcing on the network (UDP port {}); stop with Ctrl+C.", rmb_net::DISCOVERY_PORT);
        }
        ClientEvent::Waiting => {
            end_line(&mut line_open);
            eprintln!("Waiting for a server…");
        }
        ClientEvent::Connected { server } => {
            started = Instant::now();
            eprintln!("Server {server} connected");
        }
        ClientEvent::Files { done, total } => {
            if tty {
                eprint!("\r\x1b[2KReceiving files {done}/{total} {}", bar(done as f32 / total.max(1) as f32, 20));
                line_open = true;
            }
        }
        ClientEvent::Rendering { unit, fraction, done, total } => {
            if last_draw.elapsed() < Duration::from_millis(200) && fraction > 0.0 && fraction < 1.0 {
                return;
            }
            last_draw = Instant::now();
            let job = if total > 0 { done as f32 / total as f32 } else { 0.0 };
            let text = format!("{:<16} {} {:3.0}%   job {} {:3.0}% ({done}/{total})", unit.label(), bar(fraction, 20), fraction * 100.0, bar(job, 20), job * 100.0);
            if tty {
                eprint!("\r\x1b[2K{text}");
                line_open = true;
            } else if fraction == 0.0 {
                eprintln!("{text}");
            }
        }
        ClientEvent::Sent { unit } => {
            if !tty {
                eprintln!("{} sent", unit.label());
            }
        }
        ClientEvent::JobFinished => {
            end_line(&mut line_open);
            eprintln!("Job finished ({})", clock(started.elapsed().as_secs_f64()));
        }
        ClientEvent::Error(msg) => {
            end_line(&mut line_open);
            eprintln!("error: {msg}");
        }
    })
}

struct NetOpts {
    input: Option<PathBuf>,
    output: Option<PathBuf>,
    width: Option<u32>,
    height: Option<u32>,
    samples: u32,
    from: Option<usize>,
    to: Option<usize>,
    still: bool,
    clients: Option<Vec<String>>,
    addresses: Vec<String>,
    key: Option<String>,
    local: bool,
    wait: f64,
    sixteen: bool,
    exr: bool,
    keep_frames: bool,
    ffmpeg: Option<PathBuf>,
    denoise: Option<bool>,
    tiles: Option<u32>,
}

fn parse(args: &[String]) -> Result<NetOpts, String> {
    let mut o = NetOpts { input: None, output: None, width: None, height: None, samples: 64, from: None, to: None, still: false, clients: None, addresses: Vec::new(), key: None, local: false, wait: 3.0, sixteen: false, exr: false, keep_frames: false, ffmpeg: None, denoise: None, tiles: None };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = |name: &str| it.next().cloned().ok_or_else(|| format!("{name} needs a value"));
        match a.as_str() {
            "-o" => o.output = Some(PathBuf::from(val("-o")?)),
            "-W" => o.width = Some(val("-W")?.parse().map_err(|_| "bad width")?),
            "-H" => o.height = Some(val("-H")?.parse().map_err(|_| "bad height")?),
            "-s" => o.samples = val("-s")?.parse().map_err(|_| "bad sample count")?,
            "--from" => o.from = Some(val("--from")?.parse().map_err(|_| "bad frame number")?),
            "--to" => o.to = Some(val("--to")?.parse().map_err(|_| "bad frame number")?),
            "--still" => o.still = true,
            "--clients" => {
                let v = val("--clients")?;
                o.clients = (v != "all").then(|| v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect());
            }
            "--client" => o.addresses.push(val("--client")?),
            "--key" => o.key = Some(val("--key")?),
            "--local" => o.local = true,
            "--wait" => o.wait = val("--wait")?.parse().map_err(|_| "bad wait time")?,
            "--16bit" => o.sixteen = true,
            "--exr" => o.exr = true,
            "--keep-frames" => o.keep_frames = true,
            "--tiles" => {
                let v = val("--tiles")?;
                o.tiles = Some(if v == "off" { 0 } else { v.parse().map_err(|_| "bad tile size")? });
            }
            "--ffmpeg" => o.ffmpeg = Some(PathBuf::from(val("--ffmpeg")?)),
            "--denoise" => o.denoise = Some(true),
            "--no-denoise" => o.denoise = Some(false),
            s if s.starts_with('-') => return Err(format!("unknown option '{s}'")),
            s => o.input = Some(PathBuf::from(s)),
        }
    }
    Ok(o)
}

/// `rmb-cli net-clients`: the clients heard within a few seconds.
pub fn list(args: &[String]) -> Result<(), String> {
    let o = parse(args)?;
    let d = rmb_net::discovery::Discovery::start()?;
    eprintln!("Listening for {:.0} s…", o.wait);
    let seen = d.wait(Duration::from_secs_f64(o.wait));
    if seen.is_empty() {
        println!("No clients found. Start them with: rmb-cli --network");
    }
    for s in seen {
        let mut flags = Vec::new();
        if s.info.busy {
            flags.push("busy".to_string());
        }
        if s.info.key {
            flags.push("needs a key".to_string());
        }
        if s.info.version != rmb_net::version() {
            flags.push(format!("version {}", s.info.version));
        }
        let flags = if flags.is_empty() { String::new() } else { format!("  [{}]", flags.join(", ")) };
        println!("{:<20} {:<22} {}{flags}", s.info.name, s.addr, s.info.specs.summary());
    }
    Ok(())
}

/// `rmb-cli net-render`: renders a scene on the clients of the network.
pub fn render(args: &[String]) -> Result<(), String> {
    let o = parse(args)?;
    let input = o.input.clone().ok_or("no scene file given")?;
    let mut scene = rmb_core::io::load(&input)?;
    if let Some(d) = o.denoise {
        scene.post.denoise = d;
    }
    let (w, h) = match (o.width, o.height) {
        (Some(w), Some(h)) => (w, h),
        (Some(w), None) => (w, ((w as f64) / scene.aspect()).round().max(1.0) as u32),
        (None, Some(h)) => (((h as f64) * scene.aspect()).round().max(1.0) as u32, h),
        (None, None) => (scene.width, scene.height),
    };
    let animated = scene.animation.as_ref().is_some_and(|a| !a.keyframes.is_empty()) && !o.still;
    let (kind, output) = if animated {
        let anim = scene.animation.clone().unwrap_or_default();
        let plan = rmb_core::animation::AnimationPlan::new(&anim)?;
        let last = plan.frame_count() - 1;
        let (from, to) = (o.from.unwrap_or(0).min(last), o.to.unwrap_or(last).min(last));
        if from > to {
            return Err("--from is after --to".into());
        }
        let path = o.output.clone().unwrap_or_else(|| input.with_extension("mp4"));
        let ffmpeg = rmb_core::video::ffmpeg_program(o.ffmpeg.as_deref());
        let mut format = rmb_core::video::VideoFormat::from_path(&path);
        if format.is_images() {
            if o.exr {
                format = rmb_core::video::VideoFormat::ExrSequence;
            } else if o.sixteen {
                format = rmb_core::video::VideoFormat::Png16Sequence;
            }
        }
        if format.needs_ffmpeg() && !rmb_core::video::ffmpeg_available(&ffmpeg) {
            return Err(format!("{} needs ffmpeg, which was not found (install it or give --ffmpeg)", format.label()));
        }
        (JobKind::Animation { from, to }, Output::Animation { path, format, ffmpeg, keep_frames: o.keep_frames })
    } else {
        let path = o.output.clone().unwrap_or_else(|| input.with_extension("png"));
        (JobKind::Still, Output::Still { kind: rmb_core::io::image::ImageKind::of_path(&path, o.sixteen), path })
    };
    let mut targets = find_targets(&o)?;
    if o.local {
        targets.push(rmb_net::client::start_local(o.key.clone())?);
    }
    if targets.is_empty() {
        return Err("no clients to render on (start them with: rmb-cli --network, or add --local)".into());
    }
    eprintln!("Rendering on {} client{}:", targets.len(), if targets.len() == 1 { "" } else { "s" });
    for t in &targets {
        eprintln!("  {:<20} {}", t.name, t.specs.summary());
    }
    let req = JobRequest { scene, scene_dir: input.parent().map(|p| p.to_path_buf()), width: w, height: h, samples: o.samples, kind, output, key: o.key.clone(), tiles: o.tiles };
    let status = Arc::new(Mutex::new(Status::default()));
    let cancel = Arc::new(AtomicBool::new(false));
    let st = status.clone();
    let worker = std::thread::spawn(move || rmb_net::server::run_job(req, targets, st, cancel));
    let mut display = Display::default();
    while !worker.is_finished() {
        std::thread::sleep(Duration::from_millis(250));
        if let Ok(s) = status.lock() {
            display.draw(&s, animated);
        }
    }
    if let Ok(s) = status.lock() {
        display.draw(&s, animated);
    }
    let result = worker.join().map_err(|_| "the job stopped unexpectedly".to_string())?;
    let summary = result?;
    let took = status.lock().ok().and_then(|s| s.started).map(|t| t.elapsed().as_secs_f64()).unwrap_or(0.0);
    eprintln!("{summary}, {}", clock(took));
    Ok(())
}

/// The clients to render on: found on the network (all, or the ones named) and given by
/// address.
fn find_targets(o: &NetOpts) -> Result<Vec<Target>, String> {
    let mut targets = Vec::new();
    let wanted_network = o.addresses.is_empty() || o.clients.is_some();
    if wanted_network {
        let d = rmb_net::discovery::Discovery::start()?;
        eprintln!("Looking for clients ({:.0} s)…", o.wait);
        for s in d.wait(Duration::from_secs_f64(o.wait)) {
            if let Some(names) = &o.clients
                && !names.iter().any(|n| n.eq_ignore_ascii_case(&s.info.name))
            {
                continue;
            }
            if s.info.version != rmb_net::version() {
                eprintln!("  skipped {}: version {} (this server: {})", s.info.name, s.info.version, rmb_net::version());
                continue;
            }
            if s.info.busy {
                eprintln!("  skipped {}: busy with another job", s.info.name);
                continue;
            }
            targets.push(Target { name: s.info.name, addr: s.addr, specs: s.info.specs });
        }
        if let Some(names) = &o.clients {
            for n in names {
                if !targets.iter().any(|t| t.name.eq_ignore_ascii_case(n)) {
                    eprintln!("  client '{n}' not found");
                }
            }
        }
    }
    for a in &o.addresses {
        let addr = rmb_net::discovery::parse_address(a)?;
        targets.push(Target { name: a.clone(), addr, specs: Default::default() });
    }
    Ok(targets)
}

/// Progress bars on the terminal (redrawn in place), or a line every few seconds otherwise.
#[derive(Default)]
struct Display {
    lines: usize,
    last_plain: Option<Instant>,
}

impl Display {
    fn draw(&mut self, s: &Status, animated: bool) {
        let what = if animated { "frames" } else { "pieces" };
        let f = s.fraction();
        let el = s.started.map(|t| t.elapsed().as_secs_f64()).unwrap_or(0.0);
        let eta = if f > 0.02 && f < 1.0 { format!(", about {} left", clock(el / f as f64 - el)) } else { String::new() };
        let mut out = vec![format!("{:<11} {} {:3.0}%  {}/{} {what}  {}{eta}", s.phase, bar(f, 30), f * 100.0, s.done, s.total, clock(el))];
        for c in &s.clients {
            let piece = match (c.unit, c.state.as_str()) {
                (Some(u), "rendering") => format!("{:<14} {} {:3.0}%", unit_short(u), bar(c.fraction, 16), c.fraction * 100.0),
                _ => format!("{:<37}", truncate(&c.state, 37)),
            };
            let speed = c.speed_text(s);
            let speed = if speed.is_empty() { speed } else { format!("  {speed}") };
            out.push(format!("  {:<18} {piece}  {:>3} done{speed}", truncate(&c.name, 18), c.units_done));
        }
        if std::io::stderr().is_terminal() {
            let mut e = std::io::stderr().lock();
            if self.lines > 0 {
                let _ = write!(e, "\x1b[{}A", self.lines);
            }
            for l in &out {
                let _ = writeln!(e, "\x1b[2K{l}");
            }
            self.lines = out.len();
        } else if self.last_plain.is_none_or(|t| t.elapsed() > Duration::from_secs(5)) || s.finished {
            self.last_plain = Some(Instant::now());
            eprintln!("{}", out[0]);
        }
    }
}

fn unit_short(u: Unit) -> String {
    u.label()
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() } else { s.chars().take(n.saturating_sub(1)).collect::<String>() + "…" }
}
