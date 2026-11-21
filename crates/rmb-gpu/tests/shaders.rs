//! Validates the generated WGSL of every library formula with naga, and compares a GPU render
//! with the CPU reference when a GPU is available.

use rmb_core::Scene;
use rmb_core::fractal::Program;
use rmb_core::scene::Slot;

fn validate(src: &str, what: &str) {
    let module = naga::front::wgsl::parse_str(src).unwrap_or_else(|e| panic!("{what}: {}", e.emit_to_string(src)));
    naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty())
        .validate(&module)
        .unwrap_or_else(|e| panic!("{what}: {e:?}"));
}

#[test]
fn every_formula_compiles_to_valid_wgsl() {
    let lib = rmb_core::formula::library();
    for f in lib.all() {
        let mut s = Scene::default();
        s.fractal.slots = vec![Slot::new(&f.def.name), Slot::new("Amazing Box")];
        for numeric in [false, true] {
            s.fractal.de_mode = if numeric { rmb_core::scene::DeMode::NumericLog } else { rmb_core::scene::DeMode::Auto };
            let prog = Program::new(&s.fractal, &s.cut);
            validate(&rmb_gpu::codegen::march_shader(&prog), &f.def.name);
        }
    }
    validate(include_str!("../src/display.wgsl"), "display");
}

#[test]
fn gpu_matches_cpu() {
    let Ok(ctx) = rmb_gpu::GpuContext::new_headless() else {
        eprintln!("no GPU, skipped");
        return;
    };
    let mut scene = Scene::default();
    scene.lighting.ao_mode = rmb_core::scene::AoMode::Fast;
    let (w, h) = (96, 64);
    let gpu = rmb_gpu::render_blocking(ctx, &scene, w, h, 1, |_| true).expect("gpu render");
    let mut cpu = rmb_core::render::cpu::CpuRenderer::new(scene, w, h);
    cpu.render_pass();
    let cpu = cpu.rgba8();
    let diff: Vec<i32> = gpu.iter().zip(&cpu).map(|(a, b)| (*a as i32 - *b as i32).abs()).collect();
    let mean = diff.iter().sum::<i32>() as f64 / diff.len() as f64;
    let big = diff.iter().filter(|d| **d > 40).count();
    eprintln!("mean abs diff {mean:.2}, {big} channels off by more than 40");
    assert!(mean < 4.0, "mean difference {mean}");
    assert!(big < diff.len() / 50, "{big} large differences");
}

#[test]
fn every_formula_renders_on_the_gpu() {
    let Ok(ctx) = rmb_gpu::GpuContext::new_headless() else {
        eprintln!("no GPU, skipped");
        return;
    };
    let lib = rmb_core::formula::library();
    for f in lib.all() {
        let mut s = Scene::default();
        s.fractal.slots = vec![Slot::new(&f.def.name)];
        if f.def.transform {
            s.fractal.slots.push(Slot::new("Mandelbulb"));
        }
        let img = rmb_gpu::render_blocking(ctx.clone(), &s, 16, 16, 1, |_| true).unwrap_or_else(|e| panic!("{}: {e}", f.def.name));
        assert_eq!(img.len(), 16 * 16 * 4, "{}", f.def.name);
    }
}
