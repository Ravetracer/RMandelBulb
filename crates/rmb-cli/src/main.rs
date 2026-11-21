// SPDX-License-Identifier: LGPL-2.1-or-later

//! RMandelbulb3D command line: render scenes without the user interface.

use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "\
RMandelbulb3D command line

USAGE:
  rmb-cli render <scene> [-o out.png] [-W width] [-H height] [-s samples] [--cpu] [-t threads]
  rmb-cli convert <scene> -o out.rmb       Import (e.g. a Mandelbulb3D .m3p) and save as .rmb
  rmb-cli info <scene>                     Formulas, camera and import notes
  rmb-cli formulas                         List all formulas
  rmb-cli m3f <dir|file>… [-o out.mbf] [-v] Translate Mandelbulb3D formulas (.m3f) into the formula
                                           language; writes all that translate into one file

<scene> is a .rmb file, a Mandelbulb3D .m3p file, a text file with Mandelbulb3D clipboard
parameters, or a PNG rendered by RMandelbulb3D. Width and height default to the scene's;
giving only one keeps the aspect ratio. Rendering uses the GPU unless --cpu is given.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("render") => render(&args[1..]),
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
}

fn parse(args: &[String]) -> Result<Opts, String> {
    let mut o = Opts { input: None, output: None, width: None, height: None, samples: 64, cpu: false, threads: None };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = |name: &str| it.next().cloned().ok_or_else(|| format!("{name} needs a value"));
        match a.as_str() {
            "-o" => o.output = Some(PathBuf::from(val("-o")?)),
            "-W" => o.width = Some(val("-W")?.parse().map_err(|_| "bad width")?),
            "-H" => o.height = Some(val("-H")?.parse().map_err(|_| "bad height")?),
            "-s" => o.samples = val("-s")?.parse().map_err(|_| "bad sample count")?,
            "-t" => o.threads = Some(val("-t")?.parse().map_err(|_| "bad thread count")?),
            "--cpu" => o.cpu = true,
            s if s.starts_with('-') => return Err(format!("unknown option '{s}'")),
            s => o.input = Some(PathBuf::from(s)),
        }
    }
    Ok(o)
}

fn load(o: &Opts) -> Result<rmb_core::Scene, String> {
    let p = o.input.as_ref().ok_or("no scene file given")?;
    rmb_core::io::load(p)
}

fn render(args: &[String]) -> Result<(), String> {
    let o = parse(args)?;
    let scene = load(&o)?;
    let (w, h) = match (o.width, o.height) {
        (Some(w), Some(h)) => (w, h),
        (Some(w), None) => (w, ((w as f64) / scene.aspect()).round().max(1.0) as u32),
        (None, Some(h)) => (((h as f64) * scene.aspect()).round().max(1.0) as u32, h),
        (None, None) => (scene.width, scene.height),
    };
    let out = o.output.clone().unwrap_or_else(|| o.input.as_ref().map(|p| p.with_extension("png")).unwrap_or_else(|| "out.png".into()));
    let prog = rmb_core::Program::new(&scene.fractal, &scene.cut);
    if !prog.missing.is_empty() {
        eprintln!("warning: formulas not available: {}", prog.missing.join(", "));
    }
    let start = std::time::Instant::now();
    let rgba = if o.cpu {
        if let Some(t) = o.threads {
            let _ = rayon::ThreadPoolBuilder::new().num_threads(t).build_global();
        }
        let mut r = rmb_core::render::cpu::CpuRenderer::new(scene.clone(), w, h);
        for i in 0..o.samples.max(1) {
            r.render_pass();
            eprint!("\rsample {}/{}", i + 1, o.samples);
        }
        eprintln!();
        r.rgba8()
    } else {
        let ctx = rmb_gpu::GpuContext::new_headless()?;
        eprintln!("GPU: {}", ctx.adapter_name);
        let mut last = 0;
        let r = rmb_gpu::render_blocking(ctx, &scene, w, h, o.samples, |p| {
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
    rmb_core::io::image::write_png(&out, w, h, &rgba, Some(&scene))?;
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
