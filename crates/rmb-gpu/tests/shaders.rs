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

fn parity(scene: Scene) {
    let Ok(ctx) = rmb_gpu::GpuContext::new_headless() else {
        eprintln!("no GPU, skipped");
        return;
    };
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
fn gpu_matches_cpu() {
    let mut scene = Scene::default();
    scene.lighting.ao_mode = rmb_core::scene::AoMode::Fast;
    parity(scene);
}

#[test]
fn gpu_matches_cpu_with_de_combination_and_4d_rotation() {
    let mut scene = Scene::default();
    scene.lighting.ao_mode = rmb_core::scene::AoMode::Fast;
    scene.fractal.slots = vec![Slot::new("Mandelbulb"), Slot::new("Quaternion"), Slot::new("Menger sponge")];
    scene.fractal.julia = true;
    scene.fractal.julia_c = [-0.2, 0.6, 0.2, 0.1];
    scene.fractal.rotation_4d = [20.0, 10.0, 5.0];
    scene.fractal.combine = Some(rmb_core::scene::DeCombine { mode: rmb_core::scene::CombineMode::Smooth, start: 2, repeat_from: 2, max_iterations: 6, smooth: 0.2 });
    parity(scene);
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

#[test]
fn gpu_matches_cpu_inside_rendering() {
    let mut scene = Scene::default();
    scene.lighting.ao_mode = rmb_core::scene::AoMode::Fast;
    scene.fractal.inside = true;
    scene.camera.position = rmb_core::v3(0.05, -0.1, 0.02);
    scene.march.max_steps = 600;
    parity(scene);
}
