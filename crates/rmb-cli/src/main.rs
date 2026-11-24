// SPDX-License-Identifier: LGPL-2.1-or-later

//! RMandelbulb3D command line: render scenes without the user interface.

use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "\
RMandelbulb3D command line

USAGE:
  rmb-cli render <scene> [-o out.png|out.exr] [-W width] [-H height] [-s samples] [--cpu]
                 [-t threads] [--denoise [blend] | --no-denoise] [--16bit]
  rmb-cli animate <scene> [-o out.mp4|folder] [-W width] [-H height] [-s samples] [--cpu]
                 [--fps n] [--from frame] [--to frame] [--denoise [blend] | --no-denoise]
                 [--ffmpeg path] [--audio file [--audio-offset s] | --no-audio]
                 [--shutter fraction]
  rmb-cli mesh <scene> [-o out.ply|.obj|.stl] [-r resolution] [--box x0,y0,z0,x1,y1,z1]
                 [--thickness cells] [--no-colors] [--no-refine] [--y-up] [--cpu]
  rmb-cli voxels <scene> [-o folder|out.vox] [-r resolution] [--box …] [--thickness cells]
                 [--no-colors] [--hollow] [--cpu]
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
(distance from the camera), albedo and normal. The library is loaded at runtime: RMB_OIDN_DIR, next to the
executable (lib/ on Linux), ./oidn (python3 tools/fetch_oidn.py) or the system.

animate renders the scene's keyframe animation. The output's extension picks the format
(.mp4 H.264, .mov ProRes, .webm VP9, .gif; anything else: a folder of numbered PNG files);
movies need ffmpeg (on the search path, or --ffmpeg). --from/--to limit the frames (inclusive).
The scene's sound track (Animation › Sound) goes under movies (not GIF); --audio replaces it,
--audio-offset sets where in the sound frame 0 starts (seconds), --no-audio leaves it out.
--shutter sets the motion blur (exposure as a fraction of the frame time, 0 = off, 0.5 = 180°
shutter) instead of the scene's.

mesh exports the scene's surface as a closed triangle mesh (surface nets on a grid of distance
estimates). The extension picks the format: .ply (binary, normals and vertex colors), .obj
(normals, vertex colors as `v x y z r g b`), .stl (binary, no colors). -r sets the grid cells
along the longest side of the box (default 256); --box the sampled box (default: fitted to the
surface); --thickness how many cells outside the fractal the surface lies (default 0.5; more
thickens thin parts); --y-up writes y up instead of z up. The distances come from the GPU
unless --cpu is given.

voxels writes the same grid as voxels: a folder of PNG images, one per z layer from the bottom
(colors with transparency, or white on black with --no-colors), or a MagicaVoxel model when -o
ends in .vox (at most 256 voxels per side, colors in a 255-color palette). --hollow keeps only
the voxels next to empty space.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("render") => render(&args[1..]),
        Some("animate") => animate(&args[1..]),
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
}

fn parse(args: &[String]) -> Result<Opts, String> {
    let mut o = Opts { input: None, output: None, width: None, height: None, samples: 64, cpu: false, threads: None, denoise: None, fps: None, from: None, to: None, ffmpeg: None, audio: None, audio_offset: None, shutter: None, sixteen: false };
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
        let (v, t) = rmb_core::mesh_export::export(&scene, &s, field, &output, &mut report)?;
        format!("{v} vertices, {t} triangles")
    };
    eprintln!();
    println!("{} ({summary}, {:.1} s)", output.display(), start.elapsed().as_secs_f64());
    Ok(())
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
    let plan = rmb_core::animation::AnimationPlan::new(&anim)?;
    let (w, h) = image_size(&o, &scene);
    let last = plan.frame_count() - 1;
    let (from, to) = (o.from.unwrap_or(0).min(last), o.to.unwrap_or(last).min(last));
    if from > to {
        return Err("--from is after --to".into());
    }
    let out = o.output.clone().unwrap_or_else(|| o.input.as_ref().map(|p| p.with_extension("mp4")).unwrap_or_else(|| "animation.mp4".into()));
    let format = rmb_core::video::VideoFormat::from_path(&out);
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
        |f, rgba| sink.write_frame(f - from, &rgba),
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
    let kind = rmb_core::io::image::ImageKind::of_path(&out, o.sixteen);
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
