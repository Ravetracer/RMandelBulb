// SPDX-License-Identifier: LGPL-2.1-or-later

//! RMandelbulb3D command line: render scenes without the user interface.

use std::path::PathBuf;
use std::process::ExitCode;

mod net;

const USAGE: &str = "\
RMandelbulb3D command line

USAGE:
  rmb-cli render <scene> [-o out.png|out.exr] [-W width] [-H height] [-s samples] [--cpu]
                 [-t threads] [--denoise [blend] | --no-denoise] [--16bit] [--tiles size|off]
  rmb-cli animate <scene> [-o out.mp4|folder] [-W width] [-H height] [-s samples] [--cpu]
                 [--fps n] [--from frame] [--to frame] [--denoise [blend] | --no-denoise]
                 [--ffmpeg path] [--audio file [--audio-offset s] | --no-audio]
                 [--shutter fraction] [--16bit | --exr]
  rmb-cli mesh <scene> [-o out.ply|.obj|.stl] [-r resolution] [--box x0,y0,z0,x1,y1,z1]
                 [--thickness cells] [--no-colors] [--no-refine] [--y-up] [--min-piece percent]
                 [--reduce percent]
                 [--cpu]
  rmb-cli voxels <scene> [-o folder|out.vox] [-r resolution] [--box …] [--thickness cells]
                 [--no-colors] [--hollow] [--cpu]
  rmb-cli --network [--name name] [--key key] [--port n] [--cpu]
                                           Render client for network rendering
  rmb-cli net-render <scene> [-o out.mp4|folder|out.png|out.exr] [-W width] [-H height]
                 [-s samples] [--from frame] [--to frame] [--still] [--clients all|a,b]
                 [--client host[:port]] [--local] [--key key] [--wait s] [--keep-frames]
                 [--16bit | --exr] [--denoise | --no-denoise] [--ffmpeg path] [--tiles size|off]
                                           Render on the clients of the network
  rmb-cli net-clients [--wait s]           List the clients on the network
  rmb-cli batch <queue.json | scene…> [-o folder] [-s samples] [--cpu] [--ffmpeg path]
                                           Render queue: the waiting jobs of a queue file (from
                                           the app's render queue) or every scene given
  rmb-cli convert <scene> -o out.rmb       Import (e.g. a Mandelbulb3D .m3p) and save as .rmb
  rmb-cli info <scene>                     Formulas, camera and import notes
  rmb-cli formulas                         List all formulas
  rmb-cli m3f <dir|file>… [-o out.mbf] [-v] Translate Mandelbulb3D formulas (.m3f) into the formula
                                           language; writes all that translate into one file

<scene> is a .rmb file, a Mandelbulb3D .m3p file, a text file with Mandelbulb3D clipboard
parameters, or a PNG rendered by RMandelbulb3D. Width and height default to the scene's;
giving only one keeps the aspect ratio. Rendering uses the GPU unless --cpu is given.
--denoise runs the AI denoiser (Intel Open Image Denoise; blend 0..1, default 1) whatever the
scene says, --no-denoise skips it. --16bit writes a 16-bit PNG. An .exr output gets the linear
image before the post processing (denoised as the scene says) with the passes coverage, Z
(distance from the camera), albedo and normal. --tiles renders on the GPU in tiles of about
that many pixels per side (images larger than the GPU memory holds); by default images above
about 16 megapixels use tiles of 2048, --tiles off never. The library is loaded at runtime: RMB_OIDN_DIR, next to the
executable (lib/ on Linux), ./oidn (python3 tools/fetch_oidn.py) or the system.

animate renders the scene's keyframe animation. The output's extension picks the format
(.mp4 H.264, .mov ProRes, .webm VP9, .gif; anything else: a folder of numbered PNG files);
movies need ffmpeg (on the search path, or --ffmpeg). --from/--to limit the frames (inclusive).
The scene's sound track (Animation › Sound) goes under movies (not GIF); --audio replaces it,
--audio-offset sets where in the sound frame 0 starts (seconds), --no-audio leaves it out.
--shutter sets the motion blur (exposure as a fraction of the frame time, 0 = off, 0.5 = 180°
shutter) instead of the scene's. For a folder of images, --16bit writes 16-bit PNG files and
--exr EXR files (linear, with passes).

mesh exports the scene's surface as a closed triangle mesh (surface nets on a grid of distance
estimates). The extension picks the format: .ply (binary, normals and vertex colors), .obj
(normals, vertex colors as `v x y z r g b`), .stl (binary, no colors). -r sets the grid cells
along the longest side of the box (default 256); --box the sampled box (default: fitted to the
surface); --thickness how many cells outside the fractal the surface lies (default 0.5; more
thickens thin parts); --y-up writes y up instead of z up; --min-piece removes loose pieces
with fewer triangles than that percentage of the mesh; --reduce keeps that percentage of the
triangles (merging vertices where the surface is flat). The distances come from the GPU
unless --cpu is given.

voxels writes the same grid as voxels: a folder of PNG images, one per z layer from the bottom
(colors with transparency, or white on black with --no-colors), or a MagicaVoxel model when -o
ends in .vox (at most 256 voxels per side, colors in a 255-color palette). --hollow keeps only
the voxels next to empty space.

Network rendering: start `rmb-cli --network` on every machine that should help (same version;
UDP port 47810 and TCP port 47811 open in the firewall). `net-render` on the server finds them
by their announcements, sends the scene with its files and renders the animation frames (or,
for a still image or with --still, ranges of its samples, or tiles for images above about 16
megapixels or with --tiles) on all clients or the ones named
with --clients; --local renders on this machine too, --client adds one by address where the
network blocks the announcements. Frames go into the output folder (movies are encoded after
the last frame; --keep-frames keeps the PNG files). A shared --key on both sides keeps other
servers out.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("render") => render(&args[1..]),
        Some("animate") => animate(&args[1..]),
        Some("--network" | "client") => net::client(&args[1..]),
        Some("net-render") => net::render(&args[1..]),
        Some("net-clients") => net::list(&args[1..]),
        Some("batch") => batch(&args[1..]),
        Some("mesh") => mesh(&args[1..], false),
        Some("voxels") => mesh(&args[1..], true),
        Some("convert") => convert(&args[1..]),
        Some("info") => info(&args[1..]),
        Some("m3f") => m3f(&args[1..]),
        Some("formulas") => {
            formulas();
            Ok(())
        }
        Some("-h" | "--help" | "help") | None => {
            println!("{USAGE}");
            Ok(())
        }
        Some(o) => Err(format!("unknown command '{o}'\n\n{USAGE}")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

struct Opts {
    input: Option<PathBuf>,
    output: Option<PathBuf>,
    width: Option<u32>,
    height: Option<u32>,
    samples: u32,
    cpu: bool,
    threads: Option<usize>,
    /// Denoiser override: on with a blend, or off.
    denoise: Option<Option<f64>>,
    fps: Option<f64>,
    from: Option<usize>,
    to: Option<usize>,
    ffmpeg: Option<PathBuf>,
    /// Sound track override: a file, or none.
    audio: Option<Option<PathBuf>>,
    audio_offset: Option<f64>,
    shutter: Option<f64>,
    sixteen: bool,
    /// Animation frames as EXR files.
    exr: bool,
    /// Tile size: None automatic, Some(0) off.
    tiles: Option<u32>,
}

fn parse(args: &[String]) -> Result<Opts, String> {
    let mut o = Opts { input: None, output: None, width: None, height: None, samples: 64, cpu: false, threads: None, denoise: None, fps: None, from: None, to: None, ffmpeg: None, audio: None, audio_offset: None, shutter: None, sixteen: false, exr: false, tiles: None };
    let mut it = args.iter().peekable();
    while let Some(a) = it.next() {
        let mut val = |name: &str| it.next().cloned().ok_or_else(|| format!("{name} needs a value"));
        match a.as_str() {
            "-o" => o.output = Some(PathBuf::from(val("-o")?)),
            "-W" => o.width = Some(val("-W")?.parse().map_err(|_| "bad width")?),
            "-H" => o.height = Some(val("-H")?.parse().map_err(|_| "bad height")?),
            "-s" => o.samples = val("-s")?.parse().map_err(|_| "bad sample count")?,
            "-t" => o.threads = Some(val("-t")?.parse().map_err(|_| "bad thread count")?),
            "--cpu" => o.cpu = true,
            "--16bit" => o.sixteen = true,
            "--exr" => o.exr = true,
            "--tiles" => {
                let v = val("--tiles")?;
                o.tiles = Some(if v == "off" { 0 } else { v.parse().map_err(|_| "bad tile size")? });
            }
            "--fps" => o.fps = Some(val("--fps")?.parse().map_err(|_| "bad frame rate")?),
            "--from" => o.from = Some(val("--from")?.parse().map_err(|_| "bad frame number")?),
            "--to" => o.to = Some(val("--to")?.parse().map_err(|_| "bad frame number")?),
            "--ffmpeg" => o.ffmpeg = Some(PathBuf::from(val("--ffmpeg")?)),
            "--audio" => o.audio = Some(Some(PathBuf::from(val("--audio")?))),
            "--audio-offset" => o.audio_offset = Some(val("--audio-offset")?.parse().map_err(|_| "bad audio offset")?),
            "--no-audio" => o.audio = Some(None),
            "--shutter" => o.shutter = Some(val("--shutter")?.parse().map_err(|_| "bad shutter")?),
            "--denoise" => {
                let blend = it.peek().and_then(|v| v.parse::<f64>().ok());
                if blend.is_some() {
                    it.next();
                }
                o.denoise = Some(Some(blend.unwrap_or(1.0)));
            }
            "--no-denoise" => o.denoise = Some(None),
            s if s.starts_with('-') => return Err(format!("unknown option '{s}'")),
            s => o.input = Some(PathBuf::from(s)),
        }
    }
    Ok(o)
}

fn mesh(args: &[String], voxels: bool) -> Result<(), String> {
    use rmb_core::mesh_export::{MeshFormat, MeshSettings};
    use rmb_core::voxel_export::{VoxelFormat, VoxelSettings};
    let mut s = MeshSettings::default();
    let mut vs = VoxelSettings::default();
    let (mut input, mut output, mut bounds, mut cpu) = (None, None, None, false);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = |name: &str| it.next().cloned().ok_or_else(|| format!("{name} needs a value"));
        match a.as_str() {
            "-o" => output = Some(PathBuf::from(val("-o")?)),
            "-r" => s.resolution = val("-r")?.parse().map_err(|_| "bad resolution")?,
            "--thickness" => s.thickness = val("--thickness")?.parse().map_err(|_| "bad thickness")?,
            "--box" => {
                let v: Vec<f64> = val("--box")?.split(',').map(|x| x.trim().parse::<f64>()).collect::<Result<_, _>>().map_err(|_| "bad box")?;
                let [x0, y0, z0, x1, y1, z1] = v[..] else { return Err("--box needs six numbers".into()) };
                bounds = Some((rmb_core::math::v3(x0, y0, z0), rmb_core::math::v3(x1, y1, z1)));
            }
            "--no-colors" => {
                s.colors = false;
                vs.colors = false;
            }
            "--hollow" if voxels => vs.hollow = true,
            "--no-refine" => s.refine = false,
            "--min-piece" => s.min_piece = val("--min-piece")?.parse().map_err(|_| "bad piece size")?,
            "--reduce" => s.reduce = val("--reduce")?.parse::<f64>().map_err(|_| "bad --reduce percentage")?.clamp(0.1, 100.0),
            "--y-up" => s.y_up = true,
            "--cpu" => cpu = true,
            o if o.starts_with('-') => return Err(format!("unknown option '{o}'")),
            o => input = Some(PathBuf::from(o)),
        }
    }
    let input = input.ok_or("no scene file given")?;
    let scene = rmb_core::io::load(&input)?;
    for e in rmb_core::compose::load_models(&scene) {
        eprintln!("warning: 3D model not loaded: {e}");
    }
    let output = output.unwrap_or_else(|| if voxels { input.with_extension("") } else { input.with_extension("ply") });
    if voxels {
        let vox = output.extension().is_some_and(|e| e.eq_ignore_ascii_case("vox"));
        vs.format = if vox { VoxelFormat::MagicaVoxel } else { VoxelFormat::PngStack };
    } else {
        s.format = MeshFormat::of_path(&output).ok_or("the output must end in .ply, .obj or .stl")?;
    }
    let (lo, hi) = match bounds {
        Some(b) => b,
        None => rmb_core::mesh_export::fit_bounds(&scene).ok_or("no surface found within ±64; give --box")?,
    };
    (s.min, s.max) = (lo, hi);
    if voxels {
        s = rmb_core::voxel_export::limited(&s, &vs);
    }
    let g = rmb_core::mesh_export::Grid::new(&s);
    eprintln!("box ({:.4}, {:.4}, {:.4}) .. ({:.4}, {:.4}, {:.4}), grid {} × {} × {}", lo.x, lo.y, lo.z, hi.x, hi.y, hi.z, g.n[0], g.n[1], g.n[2]);
    let start = std::time::Instant::now();
    let mut gpu = if cpu {
        None
    } else {
        match rmb_gpu::GpuContext::new_headless().and_then(|ctx| rmb_gpu::GridField::new(ctx, &scene, g)) {
            Ok(f) => Some(f),
            Err(e) => {
                eprintln!("GPU not available ({e}), using the CPU");
                None
            }
        }
    };
    let mut last = -1i32;
    let field = gpu.as_mut().map(|f| f as &mut dyn rmb_core::mesh_export::Field);
    let mut report = |f: f32| {
        let pc = (f * 100.0) as i32;
        if pc != last {
            last = pc;
            eprint!("\r{pc:3} %");
        }
        true
    };
    let summary = if voxels {
        let (n, files) = rmb_core::voxel_export::export(&scene, &s, &vs, field, &output, &mut report)?;
        format!("{n} voxels in {} file{}", files.len(), if files.len() == 1 { "" } else { "s" })
    } else {
        let (v, t, pieces) = rmb_core::mesh_export::export(&scene, &s, field, &output, &mut report)?;
        let removed = if pieces > 0 { format!(", {pieces} loose pieces removed") } else { String::new() };
        format!("{v} vertices, {t} triangles{removed}")
    };
    eprintln!();
    println!("{} ({summary}, {:.1} s)", output.display(), start.elapsed().as_secs_f64());
    Ok(())
}

/// `rmb-cli batch`: renders the waiting jobs of a queue file (and marks them done or failed in
/// the file), or one job per scene file (its animation when it has keyframes, else an image;
/// next to the scene or in the folder given with -o).
fn batch(args: &[String]) -> Result<(), String> {
    use rmb_core::queue::{JobState, Queue, QueueJob};
    let o = parse(args)?;
    let inputs: Vec<PathBuf> = args.iter().filter(|a| !a.starts_with('-')).map(PathBuf::from).filter(|p| p.is_file()).collect();
    if inputs.is_empty() {
        return Err("no queue file or scenes given".into());
    }
    let queue_file = (inputs.len() == 1 && inputs[0].extension().is_some_and(|e| e.eq_ignore_ascii_case("json"))).then(|| inputs[0].clone());
    let mut queue = match &queue_file {
        Some(f) => Queue::load(f)?,
        None => {
            let samples = if args.iter().any(|a| a == "-s") { o.samples } else { 128 };
            Queue { jobs: inputs.iter().map(|p| QueueJob::for_file(p, o.output.as_deref(), samples)).collect::<Result<_, _>>()? }
        }
    };
    let ctx = if o.cpu { None } else { Some(rmb_gpu::GpuContext::new_headless()?) };
    let ffmpeg = rmb_core::video::ffmpeg_program(o.ffmpeg.as_deref());
    let total = queue.jobs.iter().filter(|j| j.state == JobState::Waiting).count();
    let (mut n, mut failed) = (0, 0);
    let start = std::time::Instant::now();
    while let Some(i) = queue.next_waiting() {
        n += 1;
        let job = queue.jobs[i].clone();
        eprintln!("[{n}/{total}] {}: {}", job.name, job.summary());
        queue.jobs[i].state = JobState::Running;
        if let Some(f) = &queue_file {
            queue.save(f)?;
        }
        let t0 = std::time::Instant::now();
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let mut last = u32::MAX;
        let done_before = (n - 1) as f32;
        let r = rmb_gpu::batch::run_job(&job, ctx.clone(), &ffmpeg, &cancel, |p| {
            let pct = (p * 100.0) as u32;
            if pct != last {
                last = pct;
                let all = (done_before + p) / total.max(1) as f32;
                eprint!("\r  job {:3}%   queue {:3}%", pct, (all * 100.0) as u32);
            }
        });
        eprintln!();
        queue.jobs[i].state = match r {
            Ok(summary) => {
                eprintln!("  {summary}");
                JobState::Done { seconds: t0.elapsed().as_secs_f64() }
            }
            Err(e) => {
                eprintln!("  failed: {e}");
                failed += 1;
                JobState::Failed { error: e }
            }
        };
        if let Some(f) = &queue_file {
            queue.save(f)?;
        }
    }
    eprintln!("{n} job{} in {}{}", if n == 1 { "" } else { "s" }, rmb_core::queue::duration(start.elapsed().as_secs_f64()), if failed > 0 { format!(", {failed} failed") } else { String::new() });
    if failed > 0 { Err(format!("{failed} job(s) failed")) } else { Ok(()) }
}

fn load(o: &Opts) -> Result<rmb_core::Scene, String> {
    let p = o.input.as_ref().ok_or("no scene file given")?;
    let scene = rmb_core::io::load(p)?;
    for e in rmb_core::compose::load_models(&scene) {
        eprintln!("warning: 3D model not loaded: {e}");
    }
    Ok(scene)
}

/// Applies the denoiser override of the options.
fn override_denoise(o: &Opts, post: &mut rmb_core::scene::Post) {
    match o.denoise {
        Some(Some(blend)) => {
            post.denoise = true;
            post.denoise_blend = blend.clamp(0.0, 1.0);
        }
        Some(None) => post.denoise = false,
        None => {}
    }
}

/// Image size from the options (one given: keeps the scene's aspect ratio).
fn image_size(o: &Opts, scene: &rmb_core::Scene) -> (u32, u32) {
    match (o.width, o.height) {
        (Some(w), Some(h)) => (w, h),
        (Some(w), None) => (w, ((w as f64) / scene.aspect()).round().max(1.0) as u32),
        (None, Some(h)) => (((h as f64) * scene.aspect()).round().max(1.0) as u32, h),
        (None, None) => (scene.width, scene.height),
    }
}

fn animate(args: &[String]) -> Result<(), String> {
    let o = parse(args)?;
    let scene = load(&o)?;
    let mut anim = scene.animation.clone().ok_or("the scene has no animation")?;
    if let Some(fps) = o.fps {
        anim.fps = fps;
    }
    if let Some(a) = &o.audio {
        anim.audio = a.clone();
    }
    if let Some(off) = o.audio_offset {
        anim.audio_offset_s = off;
    }
    if let Some(sh) = o.shutter {
        anim.shutter = sh;
    }
    // Audio-reactive drivers: the sound's levels.
    if anim.ensure_audio_levels(o.input.as_ref().and_then(|p| p.parent()), &rmb_core::video::ffmpeg_program(o.ffmpeg.as_deref()))? {
        eprintln!("analysed the sound for the drivers");
    }
    let plan = rmb_core::animation::AnimationPlan::new(&anim)?;
    let (w, h) = image_size(&o, &scene);
    let last = plan.frame_count() - 1;
    let (from, to) = (o.from.unwrap_or(0).min(last), o.to.unwrap_or(last).min(last));
    if from > to {
        return Err("--from is after --to".into());
    }
    let out = o.output.clone().unwrap_or_else(|| o.input.as_ref().map(|p| p.with_extension("mp4")).unwrap_or_else(|| "animation.mp4".into()));
    let mut format = rmb_core::video::VideoFormat::from_path(&out);
    // Image folders: 16-bit PNG or EXR on request.
    if format.is_images() {
        if o.exr {
            format = rmb_core::video::VideoFormat::ExrSequence;
        } else if o.sixteen {
            format = rmb_core::video::VideoFormat::Png16Sequence;
        }
    }
    let ffmpeg = rmb_core::video::ffmpeg_program(o.ffmpeg.as_deref());
    if format.needs_ffmpeg() && !rmb_core::video::ffmpeg_available(&ffmpeg) {
        return Err(format!("{} needs ffmpeg, which was not found (install it or give --ffmpeg)", format.label()));
    }
    let ctx = if o.cpu { None } else { Some(rmb_gpu::GpuContext::new_headless()?) };
    if let Some(c) = &ctx {
        eprintln!("GPU: {}", c.adapter_name);
    }
    // A sound given on the command line is relative to the working folder, the scene's to the
    // scene file.
    let base = if matches!(o.audio, Some(Some(_))) { None } else { o.input.as_ref().and_then(|p| p.parent()) };
    let audio = anim.audio_track(from, base);
    if let Some(a) = audio.as_ref().filter(|_| !format.has_audio()) {
        eprintln!("note: {} has no sound; {} left out", format.label(), a.path.display());
    }
    let mut sink = rmb_core::video::VideoSink::new(&out, format, w, h, plan.fps(), &ffmpeg, audio.as_ref())?;
    let start = std::time::Instant::now();
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let mut last_pct = u32::MAX;
    let frame_scene = |t: f64| {
        let mut s = plan.scene_at_time(t);
        override_denoise(&o, &mut s.post);
        s
    };
    let result = rmb_gpu::render_animation(
        ctx,
        from..to + 1,
        w,
        h,
        o.samples,
        anim.shutter,
        frame_scene,
        &cancel,
        |p| {
            let pct = (p * 100.0) as u32;
            if pct != last_pct {
                last_pct = pct;
                eprint!("\r{pct:3}%");
            }
        },
        format.hdr_frames(),
        |f, frame| sink.write(f - from, frame),
    );
    eprintln!();
    result?;
    sink.finish()?;
    let n = to + 1 - from;
    eprintln!("{} ({n} frames, {w} x {h}, {} samples, {:.1} s)", out.display(), o.samples, start.elapsed().as_secs_f64());
    Ok(())
}

fn render(args: &[String]) -> Result<(), String> {
    let o = parse(args)?;
    let mut scene = load(&o)?;
    override_denoise(&o, &mut scene.post);
    let (w, h) = image_size(&o, &scene);
    let out = o.output.clone().unwrap_or_else(|| o.input.as_ref().map(|p| p.with_extension("png")).unwrap_or_else(|| "out.png".into()));
    let prog = rmb_core::Program::for_scene(&scene);
    if !prog.missing.is_empty() {
        eprintln!("warning: formulas not available: {}", prog.missing.join(", "));
    }
    let start = std::time::Instant::now();
    let kind = rmb_core::io::image::ImageKind::of_path(&out, o.sixteen);
    let tile = match o.tiles {
        Some(0) => None,
        Some(t) => Some(t),
        None => rmb_core::io::image::auto_tile(w, h),
    };
    if let (Some(t), false) = (tile, o.cpu) {
        let ctx = rmb_gpu::GpuContext::new_headless()?;
        eprintln!("GPU: {}, tiles of {t} px ({} × {})", ctx.adapter_name, w.div_ceil(t.clamp(64, 16384)), h.div_ceil(t.clamp(64, 16384)));
        let mut last = 0;
        let (image, denoised) = rmb_gpu::render_tiled_hdr(ctx, &scene, w, h, o.samples, t, kind == rmb_core::io::image::ImageKind::Exr, |p| {
            let pct = (p * 100.0) as u32;
            if pct != last {
                last = pct;
                eprint!("\r{pct:3}%");
            }
            true
        })?;
        eprintln!();
        rmb_core::io::image::save_render_ex(&out, image, &scene, kind, true)?;
        if scene.post.denoise {
            eprintln!("{}", denoised.describe());
        }
        eprintln!("{} ({w} x {h}, {} samples, {:.1} s)", out.display(), o.samples, start.elapsed().as_secs_f64());
        return Ok(());
    }
    let image = if o.cpu {
        if let Some(t) = o.threads {
            let _ = rayon::ThreadPoolBuilder::new().num_threads(t).build_global();
        }
        let mut r = rmb_core::render::cpu::CpuRenderer::new(scene.clone(), w, h);
        for i in 0..o.samples.max(1) {
            r.render_pass();
            eprint!("\rsample {}/{}", i + 1, o.samples);
        }
        eprintln!();
        r.hdr_image()
    } else {
        let ctx = rmb_gpu::GpuContext::new_headless()?;
        eprintln!("GPU: {}", ctx.adapter_name);
        let mut last = 0;
        let r = rmb_gpu::render_blocking_hdr(ctx, &scene, w, h, o.samples, |p| {
            let pct = (p * 100.0) as u32;
            if pct != last {
                last = pct;
                eprint!("\r{pct:3}%");
            }
            true
        })?;
        eprintln!();
        r
    };
    let denoised = rmb_core::io::image::save_render(&out, image, &scene, kind)?;
    if scene.post.denoise {
        eprintln!("{}", denoised.describe());
    }
    eprintln!("{} ({w} x {h}, {} samples, {:.1} s)", out.display(), o.samples, start.elapsed().as_secs_f64());
    Ok(())
}

fn convert(args: &[String]) -> Result<(), String> {
    let o = parse(args)?;
    let scene = load(&o)?;
    let out = o.output.clone().ok_or("-o <file.rmb> is required")?;
    rmb_core::io::save(&scene, &out)?;
    eprintln!("saved {}", out.display());
    Ok(())
}

fn info(args: &[String]) -> Result<(), String> {
    let o = parse(args)?;
    let s = load(&o)?;
    println!("Image      {} x {}", s.width, s.height);
    let c = &s.camera;
    println!("Camera     position ({:.6}, {:.6}, {:.6}), fov {:.1}", c.position.x, c.position.y, c.position.z, c.fov);
    println!("Iteration  max {} bailout {} julia {}", s.fractal.max_iterations, s.fractal.bailout, s.fractal.julia);
    for (i, sl) in s.fractal.slots.iter().enumerate() {
        let known = rmb_core::formula::find(&sl.formula).is_some();
        println!("Slot {}     {} x{}{}  {:?}", i + 1, sl.formula, sl.iterations, if known { "" } else { "  (missing)" }, sl.params);
    }
    if !s.notes.is_empty() {
        println!("\n{}", s.notes);
    }
    Ok(())
}

fn formulas() {
    let lib = rmb_core::formula::library();
    for cat in lib.categories() {
        println!("{cat}");
        for f in lib.all().iter().filter(|f| f.def.category == cat) {
            let aliases: Vec<&str> = f.def.aliases.iter().map(|a| a.name.as_str()).collect();
            let al = if aliases.is_empty() { String::new() } else { format!("  [MB3D: {}]", aliases.join(", ")) };
            println!("  {:<26}{}{al}", f.def.name, f.def.description);
        }
    }
}

fn m3f(args: &[String]) -> Result<(), String> {
    let mut inputs = Vec::new();
    let mut output = None;
    let mut verbose = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-o" => output = it.next().map(PathBuf::from),
            "-v" => verbose = true,
            s => inputs.push(PathBuf::from(s)),
        }
    }
    if inputs.is_empty() {
        return Err("no folder or file given".into());
    }
    let mut files: Vec<PathBuf> = Vec::new();
    for input in &inputs {
        if input.is_dir() {
            let mut f: Vec<PathBuf> = std::fs::read_dir(input).map_err(|e| e.to_string())?.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("m3f"))).collect();
            f.sort();
            files.extend(f);
        } else {
            files.push(input.clone());
        }
    }
    let mut ok = String::new();
    let (mut n_ok, mut n_fail) = (0, 0);
    let mut reasons: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for f in &files {
        let name = f.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let res = rmb_core::formula::m3f::translate_file(f).and_then(|t| match rmb_core::formula::lang::parse_library(&t.source) {
            Ok(_) => Ok(t.source),
            Err(e) => {
                if verbose && files.len() == 1 {
                    println!("{}", t.source);
                }
                Err(format!("generated source does not parse: {e}"))
            }
        });
        match res {
            Ok(src) => {
                n_ok += 1;
                if verbose && files.len() == 1 {
                    println!("{src}");
                }
                ok.push_str(&src);
                ok.push('\n');
            }
            Err(e) => {
                n_fail += 1;
                // Group by the reason without the instruction address.
                let key = e.split(" at ").next().unwrap_or(&e).to_string();
                if verbose {
                    eprintln!("{name}: {e}");
                }
                reasons.entry(key).or_default().push(name);
            }
        }
    }
    eprintln!("translated {n_ok} of {} formulas ({n_fail} failed)", files.len());
    let mut rs: Vec<_> = reasons.into_iter().collect();
    rs.sort_by_key(|(_, v)| std::cmp::Reverse(v.len()));
    for (r, names) in rs.iter().take(40) {
        eprintln!("{:4}  {r}  (e.g. {})", names.len(), names.iter().take(3).cloned().collect::<Vec<_>>().join(", "));
    }
    if let Some(o) = output {
        let header = "// Generated by `rmb-cli m3f` from the Mandelbulb3D formula files (M3Formulas, GNU LGPL 2.1 or\n// later, by the Mandelbulb3D authors and formula contributors). Do not edit; regenerate.\n\n";
        let ok = format!("{header}{ok}");
        std::fs::write(&o, ok).map_err(|e| format!("{}: {e}", o.display()))?;
        eprintln!("wrote {}", o.display());
    }
    Ok(())
}
