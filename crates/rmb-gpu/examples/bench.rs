//! Times GPU render passes with features switched off one at a time, to see where the pass
//! time goes: `bench [scene…] [-W width] [-H height] [-s samples]` (default scene without
//! arguments).
use std::time::{Duration, Instant};

use rmb_core::scene::AoMode;
use rmb_core::Scene;
use rmb_gpu::renderer::{GpuContext, GpuRenderer};

type Variant = (&'static str, Box<dyn Fn(&mut Scene)>);

fn main() {
    let mut scenes = Vec::new();
    let (mut w, mut h, mut samples) = (1920u32, 1080u32, 8u32);
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "-W" => w = it.next().and_then(|v| v.parse().ok()).expect("width"),
            "-H" => h = it.next().and_then(|v| v.parse().ok()).expect("height"),
            "-s" => samples = it.next().and_then(|v| v.parse().ok()).expect("samples"),
            p => scenes.push((p.to_string(), rmb_core::io::load(std::path::Path::new(p)).expect("load"))),
        }
    }
    if scenes.is_empty() {
        scenes.push(("default".into(), Scene::default()));
    }
    for (name, scene) in &scenes {
        rmb_core::compose::load_models(scene);
        println!("{name} ({w}x{h}, {samples} passes)");
        let variants: Vec<Variant> = vec![
            ("full", Box::new(|_| {})),
            ("no AO", Box::new(|s| s.lighting.ao_mode = AoMode::Off)),
            ("no shadows", Box::new(|s| s.lighting.lights.iter_mut().for_each(|l| l.shadows = false))),
            ("no lights", Box::new(|s| s.lighting.lights.clear())),
            ("no AO, no lights", Box::new(|s| {
                s.lighting.ao_mode = AoMode::Off;
                s.lighting.lights.clear();
            })),
            ("no refine", Box::new(|s| s.march.refine_steps = 0)),
            ("no normal smoothing", Box::new(|s| s.march.smooth_normals = 0)),
        ];
        let mut base = 0.0;
        for (label, change) in &variants {
            let mut s = scene.clone();
            change(&mut s);
            let ctx = GpuContext::new_headless().expect("gpu");
            let mut r = GpuRenderer::new(ctx);
            let t = Instant::now();
            r.set_scene(&s, w, h, false);
            r.wait_for_pipeline().expect("pipeline");
            let compile = t.elapsed().as_secs_f64();
            // One pass to warm up, then the timed passes.
            while r.samples() < 1 {
                r.work(Duration::from_secs(10), 1);
            }
            let t = Instant::now();
            while r.samples() < samples + 1 {
                r.work(Duration::from_secs(10), samples + 1);
            }
            let pass = t.elapsed().as_secs_f64() / samples as f64;
            if *label == "full" {
                base = pass;
            }
            println!("  {label:<18} {:>8.1} ms/pass  {:>5.0}%   (compile {compile:.1} s)", pass * 1000.0, pass / base * 100.0);
        }
    }
}
