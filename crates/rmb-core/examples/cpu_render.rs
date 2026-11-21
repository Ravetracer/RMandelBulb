//! Renders a scene on the CPU: `cpu_render [scene] out.png [width] [samples]`.
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (scene, out) = if args.len() > 2 && !args[2].parse::<u32>().is_ok() {
        (rmb_core::io::load(std::path::Path::new(&args[1])).expect("load"), args[2].clone())
    } else {
        (rmb_core::Scene::default(), args.get(1).cloned().unwrap_or("out.png".into()))
    };
    let k = if args.len() > 2 && !args[2].parse::<u32>().is_ok() { 3 } else { 2 };
    let w: u32 = args.get(k).and_then(|s| s.parse().ok()).unwrap_or(480);
    let n: u32 = args.get(k + 1).and_then(|s| s.parse().ok()).unwrap_or(4);
    let h = (w as f64 / scene.aspect()).round() as u32;
    let t = std::time::Instant::now();
    let mut r = rmb_core::render::cpu::CpuRenderer::new(scene.clone(), w, h);
    for _ in 0..n {
        r.render_pass();
    }
    eprintln!("{w}x{h}, {n} samples, {:.2}s, missing: {:?}", t.elapsed().as_secs_f64(), r.program().missing);
    rmb_core::io::image::write_png(std::path::Path::new(&out), w, h, &r.rgba8(), Some(&scene)).expect("png");
}
