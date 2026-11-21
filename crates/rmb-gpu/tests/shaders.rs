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
            let mut prog = Program::new(&s.fractal, &s.cut);
            validate(&rmb_gpu::codegen::march_shader(&prog), &f.def.name);
            prog.reflections = true;
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

#[test]
fn gpu_matches_cpu_with_mandelbulb3d_shading() {
    // Display-space shading, range fog from a vertical background, glow with two colors,
    // camera-relative ambient light and Mandelbulb3D's iteration colors.
    use rmb_core::scene::{BackgroundMode, ColorMode, FogMode};
    let mut scene = Scene::default();
    scene.lighting.ao_mode = rmb_core::scene::AoMode::Fast;
    scene.lighting.ambient_camera_relative = true;
    scene.post.display_space = true;
    scene.post.tone_map = false;
    scene.background.mode = BackgroundMode::Vertical;
    scene.background.bottom = [0.8, 0.5, 0.2];
    scene.background.curve = 2.0;
    scene.fog.mode = FogMode::Range;
    scene.fog.strength = 0.8;
    scene.fog.start = 0.1;
    scene.fog.curve = 40.0;
    scene.fog.squared = true;
    scene.fog.from_background = true;
    scene.fog.glow = 0.6;
    scene.fog.glow_offset = 3.0;
    scene.fog.glow_mix_steps = 40.0;
    scene.fog.glow_color2 = [1.0, 0.2, 0.1];
    scene.fog.glow_iteration = 4;
    scene.lighting.lights[0].diffuse = rmb_core::scene::DiffuseCurve::Squared;
    // A visible directional light in view (behind the camera's view direction).
    scene.lighting.lights[1].visible = true;
    scene.lighting.lights[1].camera_relative = true;
    scene.lighting.lights[1].azimuth = 175.0;
    scene.lighting.lights[1].elevation = 5.0;
    scene.lighting.lights[1].glow_size = 15.0;
    // Smoothed normals with roughness.
    scene.march.smooth_normals = 3;
    scene.lighting.roughness = 1.0;
    // Two bounces of reflections.
    scene.material.reflection = 0.6;
    scene.material.reflection_bounces = 2;
    // A panorama background image, if Mandelbulb3D's maps are there.
    if rmb_core::maps::get_file("1031.jpg").is_some() {
        scene.background.image = "1031.jpg".into();
        scene.background.image_rotation = [20.0, 150.0, 10.0];
    }
    scene.material.color_mode = ColorMode::IterationsRatio;
    scene.material.color_scale = -0.3;
    scene.material.color_offset = 1.7;
    scene.material.detail_shift = 0.24;
    // A shadowed point light and highlight colors per gradient stop.
    scene.lighting.lights.push(rmb_core::scene::Light {
        kind: rmb_core::scene::LightKind::Point,
        position: rmb_core::v3(1.2, -1.5, 1.0),
        intensity: 3.0,
        color: [1.0, 0.6, 0.3],
        diffuse: rmb_core::scene::DiffuseCurve::Wrap,
        occlusion: 0.7,
        shadows: false,
        visible: true,
        glow_shape: rmb_core::scene::GlowShape::Ball,
        glow_size: 0.4,
        ..Default::default()
    });
    for (i, st) in scene.material.gradient.iter_mut().enumerate() {
        st.specular = [0.2 * i as f32, 1.0, 0.5];
    }
    parity(scene);
}

#[test]
fn hdr_readback_matches_display_and_cpu_helper_images() {
    let Ok(ctx) = rmb_gpu::GpuContext::new_headless() else {
        eprintln!("no GPU, skipped");
        return;
    };
    let mut scene = Scene::default();
    scene.lighting.ao_mode = rmb_core::scene::AoMode::Fast;
    let (w, h) = (96, 64);
    // The HDR image post processed on the CPU equals the GPU display pass.
    let shown = rmb_gpu::render_blocking(ctx.clone(), &scene, w, h, 1, |_| true).expect("gpu render");
    let img = rmb_gpu::render_blocking_hdr(ctx, &scene, w, h, 1, |_| true).expect("gpu render");
    let mine = img.to_rgba8(&scene.post);
    let worst = shown.iter().zip(&mine).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap();
    assert!(worst <= 1, "display differs by {worst}");
    // Albedo and normals agree with the CPU renderer.
    let mut cpu = rmb_core::render::cpu::CpuRenderer::new(scene, w, h);
    cpu.render_pass();
    let c = cpu.hdr_image();
    for (name, a, b) in [("albedo", &img.albedo, &c.albedo), ("normal", &img.normal, &c.normal)] {
        let mean = a.iter().zip(b.iter()).map(|(x, y)| (x - y).abs()).sum::<f32>() / a.len() as f32;
        eprintln!("{name}: mean difference {mean:.4}");
        assert!(mean < 0.01, "{name} differs by {mean}");
    }
}

#[test]
fn gpu_matches_cpu_with_a_height_map() {
    // Mandelbulb3D's "dIFS heart shrub": HeightMapIFS reads map 10 (from ../mb3d/M3Maps).
    if rmb_core::maps::get(10).is_none() {
        eprintln!("map 10 not found, skipped");
        return;
    }
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../mb3d/M3Parameter/dIFS heart shrub.m3p");
    let Ok(bytes) = std::fs::read(&path) else {
        eprintln!("sample not found, skipped");
        return;
    };
    let mut scene = rmb_core::io::m3p::import_binary(&bytes).expect("import").scene;
    scene.lighting.ao_mode = rmb_core::scene::AoMode::Fast;
    assert!(Program::new(&scene.fractal, &scene.cut).map_numbers().contains(&10));
    parity(scene);
}

#[test]
fn gpu_matches_cpu_with_an_interpolation_hybrid() {
    let mut scene = Scene::default();
    scene.lighting.ao_mode = rmb_core::scene::AoMode::Fast;
    // Two bulb variants (a bulb blended with a box is too chaotic for f32 against f64).
    scene.fractal.slots = vec![Slot::new("Mandelbulb"), Slot::new("Mandelbulb (cosine)")];
    scene.fractal.blend = Some([0.6, 0.4]);
    assert!(Program::new(&scene.fractal, &scene.cut).blend.is_some());
    parity(scene);
}
