//! Times GPU render passes with features switched off one at a time, to see where the pass
//! time goes: `bench [scene…] [-W width] [-H height] [-s samples] [--bake] [--bake-uniforms] [--full]`
//! (default scene without arguments; `--bake`: formula parameters as shader constants, as final
//! renders do; `--bake-uniforms`: the uniform settings too; `--full`: no feature toggles;
//! `--only a,b`: only the toggles whose label contains one of the words).
use std::time::{Duration, Instant};

use rmb_core::scene::AoMode;
use rmb_core::Scene;
use rmb_gpu::renderer::{GpuContext, GpuRenderer};

type Variant = (&'static str, Box<dyn Fn(&mut Scene)>);

/// Changes the water medium of `s` (the first see-through water), if any.
fn medium(s: &mut Scene, f: impl Fn(&mut rmb_core::compose::WaterMedium)) {
    use rmb_core::compose::{Primitive, Shape};
    for o in &mut s.objects {
        if let Shape::Primitive(Primitive::Water { medium, .. }) = &mut o.shape
            && medium.see_through
        {
            f(medium);
            return;
        }
    }
}

/// Changes the seabeds of `s`.
fn seabed(s: &mut Scene, f: impl Fn(&mut rmb_core::compose::SceneObject)) {
    for o in &mut s.objects {
        if matches!(o.shape, rmb_core::compose::Shape::Primitive(rmb_core::compose::Primitive::Seabed { .. })) {
            f(o);
        }
    }
}

fn main() {
    let mut scenes = Vec::new();
    let (mut w, mut h, mut samples, mut bake, mut bake_u, mut full_only) = (1920u32, 1080u32, 8u32, false, false, false);
    let mut only: Option<Vec<String>> = None;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "-W" => w = it.next().and_then(|v| v.parse().ok()).expect("width"),
            "-H" => h = it.next().and_then(|v| v.parse().ok()).expect("height"),
            "-s" => samples = it.next().and_then(|v| v.parse().ok()).expect("samples"),
            "--bake" => bake = true,
            "--bake-uniforms" => bake_u = true,
            "--full" => full_only = true,
            "--only" => only = it.next().map(|v| v.split(',').map(str::to_string).collect()),
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
            ("no caustics", Box::new(|s| medium(s, |m| m.caustics = 0.0))),
            ("no light rays", Box::new(|s| medium(s, |m| m.light_rays = 0.0))),
            ("no bubbles/part.", Box::new(|s| medium(s, |m| (m.bubbles, m.particles) = (0.0, 0.0)))),
            ("opaque water", Box::new(|s| medium(s, |m| m.see_through = false))),
            ("no plants", Box::new(|s| seabed(s, |o| o.plants.clear()))),
            ("plants, no rocks", Box::new(|s| seabed(s, |o| if let rmb_core::compose::Shape::Primitive(rmb_core::compose::Primitive::Seabed { rocks, .. }) = &mut o.shape { *rocks = 0.0 }))),
            ("plants, no pebbles", Box::new(|s| seabed(s, |o| if let rmb_core::compose::Shape::Primitive(rmb_core::compose::Primitive::Seabed { pebbles, .. }) = &mut o.shape { *pebbles = 0.0 }))),
            ("plants, no seaweed", Box::new(|s| seabed(s, |o| if let rmb_core::compose::Shape::Primitive(rmb_core::compose::Primitive::Seabed { seaweed, .. }) = &mut o.shape { *seaweed = 0.0 }))),
            ("seabed, no ripples", Box::new(|s| seabed(s, |o| if let rmb_core::compose::Shape::Primitive(rmb_core::compose::Primitive::Seabed { ripples, .. }) = &mut o.shape { *ripples = 0.0 }))),
            ("no seabed parts", Box::new(|s| {
                seabed(s, |o| {
                    o.plants.clear();
                    if let rmb_core::compose::Shape::Primitive(rmb_core::compose::Primitive::Seabed { rocks, pebbles, seaweed, .. }) = &mut o.shape {
                        (*rocks, *pebbles, *seaweed) = (0.0, 0.0, 0.0);
                    }
                })
            })),
        ];
        let mut base = 0.0;
        for (label, change) in variants.iter().take(if full_only { 1 } else { usize::MAX }) {
            if only.as_ref().is_some_and(|o| *label != "full" && !o.iter().any(|n| label.contains(n.as_str()))) {
                continue;
            }
            let mut s = scene.clone();
            change(&mut s);
            let ctx = GpuContext::new_headless().expect("gpu");
            let mut r = GpuRenderer::new(ctx);
            if bake {
                r.set_baking(Some(Vec::new()));
            }
            if bake_u {
                r.set_uniform_baking(Some(Vec::new()));
            }
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
