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
            let mut prog = Program::for_scene(&s);
            validate(&rmb_gpu::codegen::march_shader(&prog), &f.def.name);
            // Final renders: the parameters as constants.
            let baked: Vec<Option<f64>> = prog.constants.iter().map(|v| Some(*v)).collect();
            validate(&rmb_gpu::codegen::march_shader_baked(&prog, &baked), &f.def.name);
            // Deep zoom: double-float iteration (also baked: the low parts become constants).
            prog.deep = Some([0.1, -0.2, 0.3]);
            validate(&rmb_gpu::codegen::march_shader(&prog), &format!("{} (deep)", f.def.name));
            validate(&rmb_gpu::codegen::march_shader_baked(&prog, &baked), &format!("{} (deep, baked)", f.def.name));
            prog.deep = None;
            prog.reflections = true;
            validate(&rmb_gpu::codegen::march_shader(&prog), &f.def.name);
        }
    }
    validate(include_str!("../src/display.wgsl"), "display");
}

fn parity(scene: Scene) {
    if let Some((mean, big, len)) = gpu_cpu_difference(scene) {
        assert!(mean < 4.0, "mean difference {mean}");
        assert!(big < len / 50, "{big} large differences");
    }
}

/// Mean absolute difference of a GPU and a CPU render, the channels off by more than 40 and
/// the channel count (`None` without a GPU).
fn gpu_cpu_difference(scene: Scene) -> Option<(f64, usize, usize)> {
    let Ok(ctx) = rmb_gpu::GpuContext::new_headless() else {
        eprintln!("no GPU, skipped");
        return None;
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
    Some((mean, big, diff.len()))
}

/// The default bulb seen from `zoom` times closer to the surface at the image center (ten
/// times closer per step, measuring the distance again each time).
fn deep_zoom(mut s: Scene, zoom: f64) -> Scene {
    let mut left = zoom;
    while left > 1.0 + 1e-9 {
        let f = left.min(10.0);
        let (t, _) = rmb_core::render::cpu::pick(&s, 0.0, 0.0, 1024, 1024).expect("surface at the center");
        s.camera.position += s.camera.forward * (t - t / f);
        left /= f;
    }
    s
}

#[test]
fn gpu_matches_cpu_in_deep_zoom() {
    use rmb_core::scene::{DeMode, Precision};
    // Analytic and numeric distance estimates; zoom 1e7 is far beyond single precision.
    for numeric in [false, true] {
        let mut scene = Scene::default();
        scene.lighting.ao_mode = rmb_core::scene::AoMode::Fast;
        if numeric {
            scene.fractal.de_mode = DeMode::NumericLog;
        }
        let mut scene = deep_zoom(scene, 1e7);
        let prog = Program::for_scene(&scene);
        assert!(rmb_core::render::zoom_ratio(&scene, &prog, 64) < rmb_core::render::DEEP_ZOOM_RATIO, "Auto must pick double precision");
        scene.march.precision = Precision::Double;
        parity(scene.clone());
        // Single precision fails there (the test can tell).
        scene.march.precision = Precision::Single;
        if let Some((mean, _, _)) = gpu_cpu_difference(scene) {
            assert!(mean > 4.0, "single precision should fail at zoom 1e7 (mean difference {mean})");
        }
    }
}

#[test]
fn gpu_matches_cpu() {
    let mut scene = Scene::default();
    scene.lighting.ao_mode = rmb_core::scene::AoMode::Fast;
    parity(scene);
}

#[test]
fn gpu_matches_cpu_with_position_coloring() {
    for mode in [rmb_core::scene::ColorMode::Plane, rmb_core::scene::ColorMode::Sphere] {
        let mut scene = Scene::default();
        scene.lighting.ao_mode = rmb_core::scene::AoMode::Fast;
        scene.material.color_mode = mode;
        scene.material.color_scale = 1.3;
        scene.material.color_center = [0.2, -0.1, 0.3];
        scene.material.color_direction = [1.0, 0.5, 0.2];
        scene.material.bands = vec![rmb_core::gradient::GlowBand { pos: 0.4, width: 0.1, emission: 5.0, ..Default::default() }];
        parity(scene);
    }
}

#[test]
fn gpu_matches_cpu_with_mb3d_second_choice() {
    use rmb_core::scene::SecondChoice;
    for (k, choice) in SecondChoice::ALL.into_iter().enumerate() {
        let mut scene = Scene::default();
        scene.lighting.ao_mode = rmb_core::scene::AoMode::Fast;
        scene.material.color_mode = rmb_core::scene::ColorMode::SecondChoice;
        scene.material.second_choice = choice;
        scene.material.color_scale = 3.0;
        scene.material.lli_multiplier = 2.0;
        // Color on iteration for some of them (0: the constant).
        scene.material.color_on_iteration = [-1, 3, -1, 0, -1, 5][k];
        parity(scene);
    }
}

#[test]
fn gpu_matches_cpu_path_traced() {
    let mut scene = Scene::default();
    scene.lighting.render_mode = rmb_core::scene::RenderMode::PathTraced;
    scene.lighting.path_bounces = 3;
    scene.material.reflection = 0.3;
    scene.material.bands = vec![rmb_core::gradient::GlowBand { pos: 0.3, width: 0.2, emission: 3.0, ..Default::default() }];
    let cpu = |s: &Scene| {
        let mut r = rmb_core::render::cpu::CpuRenderer::new(s.clone(), 48, 32);
        r.render_pass();
        r.rgba8()
    };
    let mut classic = scene.clone();
    classic.lighting.render_mode = rmb_core::scene::RenderMode::Classic;
    let changed = cpu(&scene).iter().zip(cpu(&classic)).filter(|(a, b)| (**a as i32 - *b as i32).abs() > 8).count();
    assert!(changed > 300, "path tracing changes only {changed} channels");
    parity(scene);
}

#[test]
fn gpu_matches_cpu_with_glow_bands_bloom_streaks_and_steam() {
    let mut scene = Scene::default();
    scene.lighting.ao_mode = rmb_core::scene::AoMode::Fast;
    scene.material.bands = vec![
        // Two halves of the gradient: one glows, one has a hard-edged plain color.
        rmb_core::gradient::GlowBand { pos: 0.25, width: 0.5, emission: 6.0, ..Default::default() },
        rmb_core::gradient::GlowBand { pos: 0.75, width: 0.45, softness: 0.0, color: [1.0, 0.2, 0.05], emission: 0.0, ..Default::default() },
    ];
    scene.material.emission_light = 1.0;
    scene.material.emission_rays = 3;
    scene.post.bloom = 0.8;
    scene.post.bloom_threshold = 0.6;
    scene.post.streak = 0.7;
    scene.post.streak_threshold = 0.5;
    scene.post.streak_count = 2;
    scene.post.streak_angle = 20.0;
    scene.material.steam = 1.5;
    scene.material.steam_size = 0.15;
    scene.material.steam_grain = 0.2;
    let cpu = |s: &Scene| {
        let mut r = rmb_core::render::cpu::CpuRenderer::new(s.clone(), 48, 32);
        r.render_pass();
        r.rgba8()
    };
    let mut plain = scene.clone();
    plain.material.bands.clear();
    plain.post.bloom = 0.0;
    plain.post.streak = 0.0;
    plain.material.steam = 0.0;
    let changed = cpu(&scene).iter().zip(cpu(&plain)).filter(|(a, b)| (**a as i32 - *b as i32).abs() > 8).count();
    assert!(changed > 300, "bands and bloom change only {changed} channels");
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
    scene.fractal.combine = Some(rmb_core::scene::DeCombine { mode: rmb_core::scene::CombineMode::Smooth, start: 2, repeat_from: 2, max_iterations: 6, smooth: 0.2, ..Default::default() });
    if rmb_core::maps::get(2).is_some() {
        scene.material.color_map = rmb_core::scene::ColorMap { image: "2".into(), mode: rmb_core::scene::MapMode::Normal, rotation: [30.0, 60.0, 0.0], luminance_only: true, ..Default::default() };
    }
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
    scene.material.depth_shift = 0.3;
    scene.material.depth_reference = 3.0;
    // A color map projected in 3D, softened by roughness.
    if rmb_core::maps::get(12).is_some() {
        scene.material.color_map = rmb_core::scene::ColorMap { image: "12".into(), mode: rmb_core::scene::MapMode::Triplanar, scale: 3.0, ..Default::default() };
    }
    // Interior colors on a cut face.
    scene.material.interior = rmb_core::gradient::preset("Ice").unwrap_or_default();
    scene.material.interior_scale = 0.8;
    scene.material.interior_offset = 0.1;
    scene.cut.z = rmb_core::scene::CutPlane { enabled: true, position: 0.1, keep_above: false };
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
    assert!(Program::for_scene(&scene).map_numbers().contains(&10));
    parity(scene);
}

#[test]
fn gpu_matches_cpu_with_an_interpolation_hybrid() {
    let mut scene = Scene::default();
    scene.lighting.ao_mode = rmb_core::scene::AoMode::Fast;
    // Two bulb variants (a bulb blended with a box is too chaotic for f32 against f64).
    scene.fractal.slots = vec![Slot::new("Mandelbulb"), Slot::new("Mandelbulb (cosine)")];
    scene.fractal.blend = Some([0.6, 0.4]);
    if rmb_core::maps::get(12).is_some() {
        scene.material.color_map = rmb_core::scene::ColorMap { image: "12".into(), mode: rmb_core::scene::MapMode::Orbit, scale: 2.0, rotation: [0.0, 0.0, 30.0], ..Default::default() };
    }
    assert!(Program::for_scene(&scene).blend.is_some());
    parity(scene);
}

#[test]
fn gpu_matches_cpu_with_a_difs_mix() {
    // Mandelbulb3D's "genQuat helix mix": GeneralQuat transforms, helixIFS measures.
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../mb3d/M3Parameter/genQuat helix mix.m3p");
    let Ok(bytes) = std::fs::read(&path) else {
        eprintln!("sample not found, skipped");
        return;
    };
    let mut scene = rmb_core::io::m3p::import_binary(&bytes).expect("import").scene;
    scene.lighting.ao_mode = rmb_core::scene::AoMode::Fast;
    assert!(Program::for_scene(&scene).is_mix());
    parity(scene);
}

#[test]
fn gpu_matches_cpu_with_object_fractals_with_de_combination() {
    use rmb_core::compose::{SceneObject, Shape, Transform};
    use rmb_core::scene::{CombineMode, DeCombine, Fractal, Slot};
    let mut scene = Scene::default();
    scene.lighting.ao_mode = rmb_core::scene::AoMode::Fast;
    let p = |x: f64, y: f64, z: f64| rmb_core::v3(x, y, z);
    // Mandelbulb and Amazing Box as one object, smoothly united; a Mandelbulb/Amazing Box
    // interpolation as another.
    let combined = Fractal {
        slots: vec![Slot::new("Mandelbulb"), Slot::new("Amazing Box")],
        bailout: 16.0,
        combine: Some(DeCombine { mode: CombineMode::Smooth, start: 1, smooth: 0.2, ..Default::default() }),
        ..Default::default()
    };
    let blended = Fractal { slots: vec![Slot::new("Mandelbulb"), Slot::new("Amazing Box")], bailout: 16.0, blend: Some([0.7, 0.3]), ..Default::default() };
    scene.objects = vec![
        SceneObject { name: "Combined".into(), shape: Shape::Fractal(Box::new(combined)), transform: Transform { position: p(1.2, -0.5, 0.3), scale: 0.3, ..Default::default() }, ..Default::default() },
        SceneObject { name: "Blended".into(), shape: Shape::Fractal(Box::new(blended)), transform: Transform { position: p(-1.0, -0.8, 0.5), scale: 0.3, ..Default::default() }, ..Default::default() },
    ];
    // A dIFS mix (Mandelbulb3D's "genQuat helix mix") as a third object.
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../mb3d/M3Parameter/genQuat helix mix.m3p");
    if let Ok(bytes) = std::fs::read(&path) {
        let mix = rmb_core::io::m3p::import_binary(&bytes).expect("import").scene.fractal;
        assert!(mix.combine.as_ref().is_some_and(|c| c.mode == CombineMode::Mix));
        scene.objects.push(SceneObject { name: "Mix".into(), shape: Shape::Fractal(Box::new(mix)), transform: Transform { position: p(0.3, -1.0, -0.6), scale: 0.4, ..Default::default() }, ..Default::default() });
    }
    let prog = Program::for_scene(&scene);
    for o in &prog.objects {
        let rmb_core::fractal::ObjectShape::Fractal(f) = &o.shape else { panic!("fractal objects") };
        assert!(f.part2.is_some() || f.blend.is_some());
    }
    validate(&rmb_gpu::codegen::march_shader(&prog), "object fractals with DE combination");
    parity(scene);
}

#[test]
fn gpu_matches_cpu_with_glass() {
    // Refraction, Fresnel reflection, absorption and scattering inside the bulb.
    let mut scene = Scene::default();
    scene.lighting.ao_mode = rmb_core::scene::AoMode::Fast;
    for (i, s) in scene.material.gradient.iter_mut().enumerate() {
        s.transparency = if i % 2 == 0 { 1.0 } else { 0.6 };
    }
    scene.material.reflection = 1.0;
    scene.material.reflection_bounces = 4;
    scene.material.transmission = true;
    scene.material.refraction_index = 1.4;
    scene.material.absorption = 0.8;
    scene.material.scattering = 30.0;
    let cpu = |s: &Scene| {
        let mut r = rmb_core::render::cpu::CpuRenderer::new(s.clone(), 48, 32);
        r.render_pass();
        r.rgba8()
    };
    let mut opaque = scene.clone();
    opaque.material.transmission = false;
    let changed = cpu(&scene).iter().zip(cpu(&opaque)).filter(|(a, b)| (**a as i32 - *b as i32).abs() > 8).count();
    assert!(changed > 100, "glass changes only {changed} channels");
    parity(scene);
}

#[test]
fn gpu_matches_cpu_with_mandelbulb3d_glass() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../mb3d/M3Parameter/toricaleggs.m3p");
    let Ok(bytes) = std::fs::read(&path) else {
        eprintln!("sample not found, skipped");
        return;
    };
    let mut scene = rmb_core::io::m3p::import_binary(&bytes).expect("import").scene;
    scene.lighting.ao_mode = rmb_core::scene::AoMode::Fast;
    assert!(scene.material.transmission);
    parity(scene);
}

/// Writes a 24-bit BMP with a colorful pattern.
fn write_test_bmp(path: &std::path::Path, w: u32, h: u32) {
    let row = (w * 3).div_ceil(4) * 4;
    let size = 54 + row * h;
    let mut b = Vec::with_capacity(size as usize);
    b.extend_from_slice(b"BM");
    for v in [size, 0, 54, 40, w, h] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&24u16.to_le_bytes());
    for v in [0u32, row * h, 2835, 2835, 0, 0] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    for y in 0..h {
        for x in 0..w {
            let (fx, fy) = (x as f32 / w as f32, y as f32 / h as f32);
            b.extend_from_slice(&[(255.0 * fy) as u8, (128.0 + 127.0 * (fx * 12.0).sin()) as u8, (255.0 * fx) as u8]);
        }
        b.resize(b.len() + (row - w * 3) as usize, 0);
    }
    std::fs::write(path, b).expect("write bmp");
}

#[test]
fn gpu_matches_cpu_with_background_image_options() {
    // Ambient light from the image (its 16 × 8 copy), spherical conversion, fitted borders,
    // brightness, fog on top of the image, screen coordinates, reflections that see the image.
    use rmb_core::scene::FogMode;
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("rmb_bg_test.bmp");
    write_test_bmp(&path, 96, 48);
    let mut scene = Scene::default();
    scene.lighting.ao_mode = rmb_core::scene::AoMode::Fast;
    scene.post.display_space = true;
    scene.background.image = path.to_string_lossy().into_owned();
    scene.background.image_rotation = [10.0, 40.0, 0.0];
    scene.background.image_intensity = 1.3;
    scene.background.image_spherical = true;
    scene.background.image_fit_border = true;
    scene.background.image_fog_add = true;
    scene.background.image_screen = true;
    scene.lighting.ambient_from_background = true;
    scene.lighting.ambient = 0.8;
    scene.fog.mode = FogMode::Range;
    scene.fog.strength = 0.4;
    scene.material.reflection = 0.5;
    scene.material.reflection_bounces = 1;
    let (big, small, _) = rmb_core::render::background_maps(&scene).expect("test image loads");
    assert_eq!((big.width, big.height, small.width, small.height), (96, 48, 16, 8));
    parity(scene);
}

#[test]
fn animation_frames_render_and_differ() {
    use rmb_core::animation::{Animation, AnimationPlan, Keyframe};
    let Ok(ctx) = rmb_gpu::GpuContext::new_headless() else {
        eprintln!("no GPU, skipped");
        return;
    };
    let a = Scene::default();
    let mut b = a.clone();
    b.camera.position = rmb_core::v3(2.6, -0.8, 1.0);
    b.camera.look_at(rmb_core::V3::ZERO, rmb_core::v3(0.0, 0.0, 1.0));
    b.lighting.ambient = 0.6;
    let anim = Animation { keyframes: vec![Keyframe { scene: a, frames: 2, ..Default::default() }, Keyframe { scene: b, ..Default::default() }], ..Default::default() };
    let plan = AnimationPlan::new(&anim).expect("plan");
    assert_eq!(plan.frame_count(), 3);
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let mut frames = Vec::new();
    rmb_gpu::render_animation(Some(ctx), 0..3, 48, 32, 1, 0.0, |t| plan.scene_at_time(t), &cancel, |_| {}, |f, rgba| {
        frames.push((f, rgba));
        Ok(())
    })
    .expect("render");
    assert_eq!(frames.iter().map(|f| f.0).collect::<Vec<_>>(), vec![0, 1, 2]);
    let diff = |x: &[u8], y: &[u8]| x.iter().zip(y).filter(|(p, q)| p != q).count();
    assert!(diff(&frames[0].1, &frames[1].1) > 100 && diff(&frames[1].1, &frames[2].1) > 100);
}

#[test]
fn gpu_matches_cpu_with_motion_blur() {
    let mut scene = Scene::default();
    scene.lighting.ao_mode = rmb_core::scene::AoMode::Fast;
    // The camera moves sideways, turns a little and zooms during the exposure.
    let mut end = scene.camera.clone();
    end.position += end.right * 0.4;
    end.rotate_local(0.08, 0.02, 0.0);
    end.fov *= 0.9;
    scene.motion_end = Some(end);
    if let Some((mean, big, n)) = gpu_cpu_difference(scene) {
        assert!(mean < 4.0 && big * 100 < n, "mean {mean}, {big} of {n} channels far off");
    }
}

#[test]
fn gpu_matches_cpu_with_light_scattering_in_fog() {
    use rmb_core::scene::{FogMode, Light, LightKind};
    for mode in [FogMode::Exponential, FogMode::Range] {
        let mut scene = Scene::default();
        scene.lighting.ao_mode = rmb_core::scene::AoMode::Fast;
        scene.fog.mode = mode;
        scene.fog.density = 0.25;
        scene.fog.strength = 0.8;
        scene.fog.curve = 4.0;
        scene.fog.scatter = 3.0;
        // A spot light behind the fractal shines through its gaps.
        let behind = scene.camera.position * -1.0;
        scene.lighting.lights.push(Light { kind: LightKind::Spot, position: behind, target: rmb_core::V3::ZERO, cone_angle: 25.0, intensity: 4.0, ..Default::default() });
        if let Some((mean, big, n)) = gpu_cpu_difference(scene) {
            assert!(mean < 4.0 && big * 100 < n, "{mode:?}: mean {mean}, {big} of {n} channels far off");
        }
    }
}

#[test]
fn gpu_matches_cpu_with_glossy_reflections_and_frosted_glass() {
    for glass in [false, true] {
        let mut scene = Scene::default();
        scene.lighting.ao_mode = rmb_core::scene::AoMode::Fast;
        scene.material.reflection = 0.8;
        scene.material.gloss = 0.4;
        scene.material.transmission = glass;
        if glass {
            for s in &mut scene.material.gradient {
                s.transparency = 0.7;
            }
        }
        if let Some((mean, big, n)) = gpu_cpu_difference(scene) {
            assert!(mean < 4.0 && big * 100 < n, "glass {glass}: mean {mean}, {big} of {n} channels far off");
        }
    }
}

#[test]
fn gpu_matches_cpu_with_shimmer_and_color_map_emission() {
    let bmp = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("rmb_layer_test.bmp");
    write_test_bmp(&bmp, 64, 64);
    let mut scene = Scene::default();
    scene.lighting.ao_mode = rmb_core::scene::AoMode::Fast;
    scene.material.gradient[0].emission = 1.5;
    scene.material.shimmer = 0.8;
    scene.material.shimmer_size = 0.08;
    scene.material.shimmer_phase = 2.4;
    scene.material.color_map.image = bmp.to_string_lossy().into_owned();
    scene.material.color_map.mode = rmb_core::scene::MapMode::Triplanar;
    scene.material.color_map.scale = 3.0;
    scene.material.color_map.emission = 1.2;
    let cpu = |s: &Scene| {
        let mut r = rmb_core::render::cpu::CpuRenderer::new(s.clone(), 48, 32);
        r.render_pass();
        r.rgba8()
    };
    let mut steady = scene.clone();
    steady.material.shimmer = 0.0;
    let changed = cpu(&scene).iter().zip(cpu(&steady)).filter(|(a, b)| (**a as i32 - *b as i32).abs() > 8).count();
    assert!(changed > 200, "the shimmer changes only {changed} channels");
    parity(scene);
}

#[test]
fn motion_blur_smears_a_camera_pan() {
    use rmb_core::animation::{Animation, AnimationPlan, Keyframe};
    let Ok(ctx) = rmb_gpu::GpuContext::new_headless() else {
        eprintln!("no GPU, skipped");
        return;
    };
    let a = Scene::default();
    let mut b = a.clone();
    b.camera.position += b.camera.right * 0.6;
    let anim = Animation { keyframes: vec![Keyframe { scene: a, frames: 1, ..Default::default() }, Keyframe { scene: b, ..Default::default() }], ..Default::default() };
    let plan = AnimationPlan::new(&anim).expect("plan");
    let (w, h) = (96u32, 64u32);
    // Sum of horizontal differences: lower where the image is smeared sideways.
    let sharpness = |shutter: f64| {
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let mut out = Vec::new();
        rmb_gpu::render_animation(Some(ctx.clone()), 0..1, w, h, 16, shutter, |t| plan.scene_at_time(t), &cancel, |_| {}, |_, rgba| {
            out = rgba;
            Ok(())
        })
        .expect("render");
        let mut sum = 0i64;
        for y in 0..h as usize {
            for x in 1..w as usize {
                let i = (y * w as usize + x) * 4;
                sum += (out[i + 1] as i64 - out[i - 3] as i64).abs();
            }
        }
        sum
    };
    let (sharp, blurred) = (sharpness(0.0), sharpness(1.0));
    eprintln!("horizontal detail: sharp {sharp}, blurred {blurred}");
    assert!((blurred as f64) < 0.8 * sharp as f64);
}

#[test]
fn gpu_matches_cpu_with_spot_area_and_line_lights() {
    use rmb_core::scene::{GlowShape, Light, LightKind};
    let mut scene = Scene::default();
    scene.lighting.ao_mode = rmb_core::scene::AoMode::Fast;
    scene.lighting.lights.truncate(1);
    scene.lighting.lights[0].intensity = 0.3;
    let p = |x: f64, y: f64, z: f64| rmb_core::v3(x, y, z);
    scene.lighting.lights.push(Light { kind: LightKind::Spot, position: p(1.5, -1.5, 1.5), target: rmb_core::V3::ZERO, cone_angle: 18.0, cone_softness: 0.5, intensity: 6.0, color: [1.0, 0.7, 0.4], ..Default::default() });
    scene.lighting.lights.push(Light {
        kind: LightKind::Area,
        position: p(-1.2, -1.6, 1.4),
        target: rmb_core::V3::ZERO,
        width: 1.0,
        height: 0.6,
        intensity: 5.0,
        color: [0.4, 0.7, 1.0],
        visible: true,
        glow_intensity: 2.0,
        ..Default::default()
    });
    scene.lighting.lights.push(Light {
        kind: LightKind::Line,
        position: p(-1.5, 1.2, 1.6),
        target: p(1.5, 1.2, 1.6),
        intensity: 3.0,
        color: [0.6, 1.0, 0.6],
        visible: true,
        glow_shape: GlowShape::Ball,
        glow_size: 0.05,
        ..Default::default()
    });
    // Seen from where the area light is in view.
    scene.camera.position = p(0.3, -3.6, 2.2);
    scene.camera.look_at(p(-0.3, 0.0, 0.6), p(0.0, 0.0, 1.0));
    parity(scene);
}

#[test]
fn gpu_matches_cpu_with_emissive_colors() {
    // Self-shining colors, the light they cast (one hemisphere ray) and their glow.
    let mut scene = Scene::default();
    scene.lighting.ao_mode = rmb_core::scene::AoMode::Raytraced;
    scene.lighting.lights[0].intensity = 0.3;
    for (i, s) in scene.material.gradient.iter_mut().enumerate() {
        s.emission = if i % 2 == 1 { 2.0 } else { 0.0 };
    }
    scene.material.emission_light = 1.5;
    scene.material.emission_glow = 0.8;
    let cpu = |s: &Scene| {
        let mut r = rmb_core::render::cpu::CpuRenderer::new(s.clone(), 48, 32);
        r.render_pass();
        r.rgba8()
    };
    let mut plain = scene.clone();
    for s in plain.material.gradient.iter_mut() {
        s.emission = 0.0;
    }
    let changed = cpu(&scene).iter().zip(cpu(&plain)).filter(|(a, b)| (**a as i32 - *b as i32).abs() > 8).count();
    assert!(changed > 100, "emission changes only {changed} channels");
    parity(scene);
}

#[test]
fn gpu_matches_cpu_with_composed_objects() {
    use rmb_core::compose::{BoolOp, ObjectMaterial, Pattern, Primitive, SceneObject, Shape, Transform};
    use rmb_core::scene::{ColorStop, Fractal, Slot};
    let mut scene = Scene::default();
    scene.lighting.ao_mode = rmb_core::scene::AoMode::Fast;
    let p = |x: f64, y: f64, z: f64| rmb_core::v3(x, y, z);
    let mut box_frac = Fractal { slots: vec![Slot::new("Amazing Box")], max_iterations: 10, bailout: 16.0, ..Default::default() };
    box_frac.de_mode = rmb_core::scene::DeMode::Linear;
    let two = |a: [f32; 3], b: [f32; 3]| vec![ColorStop::new(0.0, a), ColorStop::new(1.0, b)];
    let mut emissive = two([0.2, 0.6, 1.0], [1.0, 0.4, 0.1]);
    emissive[0].emission = 1.5;
    scene.objects = vec![
        // A second fractal, moved, turned and shrunk, in its own colors.
        SceneObject {
            name: "Box".into(),
            shape: Shape::Fractal(Box::new(box_frac)),
            transform: Transform { position: p(1.4, 0.4, 0.2), rotation: [10.0, 25.0, 40.0], scale: 0.25, ..Default::default() },
            material: ObjectMaterial { gradient: emissive, color_scale: 0.3, ..Default::default() },
            ..Default::default()
        },
        // Checkered ground below the bulb.
        SceneObject {
            name: "Ground".into(),
            shape: Shape::Primitive(Primitive::Plane),
            transform: Transform { position: p(0.0, 0.0, -1.1), ..Default::default() },
            material: ObjectMaterial { gradient: two([0.9, 0.9, 0.85], [0.15, 0.15, 0.2]), pattern: Pattern::Checker, pattern_size: 0.5, repeat: false, ..Default::default() },
            ..Default::default()
        },
        // A sphere carved out of the bulb, a torus melted onto it, water in the back.
        SceneObject { name: "Cut".into(), op: BoolOp::Subtract, shape: Shape::Primitive(Primitive::Sphere { radius: 0.5 }), transform: Transform { position: p(0.6, -0.6, 0.6), ..Default::default() }, ..Default::default() },
        SceneObject {
            name: "Ring".into(),
            op: BoolOp::SmoothUnion,
            smooth: 0.2,
            shape: Shape::Primitive(Primitive::Torus { major: 0.9, minor: 0.08 }),
            transform: Transform { rotation: [70.0, 0.0, 30.0], ..Default::default() },
            material: ObjectMaterial { own: false, ..Default::default() },
            ..Default::default()
        },
        SceneObject { name: "Sea".into(), shape: Shape::Primitive(Primitive::Water { amplitude: 0.03, wavelength: 0.4, phase: 1.0, angle: 30.0 }), transform: Transform { position: p(0.0, 3.0, -0.9), ..Default::default() }, ..Default::default() },
    ];
    let prog = Program::for_scene(&scene);
    assert_eq!(prog.objects.len(), 5);
    assert!(prog.missing.is_empty(), "{:?}", prog.missing);
    validate(&rmb_gpu::codegen::march_shader(&prog), "composed objects");
    let cpu = |s: &Scene| {
        let mut r = rmb_core::render::cpu::CpuRenderer::new(s.clone(), 48, 32);
        r.render_pass();
        r.rgba8()
    };
    let mut plain = scene.clone();
    plain.objects.clear();
    let changed = cpu(&scene).iter().zip(cpu(&plain)).filter(|(a, b)| (**a as i32 - *b as i32).abs() > 8).count();
    assert!(changed > 500, "the objects change only {changed} channels");
    parity(scene);
}

#[test]
fn gpu_matches_cpu_with_placed_fractal_repeated_objects_and_new_primitives() {
    use rmb_core::compose::{Primitive, SceneObject, Shape, Transform};
    let mut scene = Scene::default();
    scene.lighting.ao_mode = rmb_core::scene::AoMode::Fast;
    let p = |x: f64, y: f64, z: f64| rmb_core::v3(x, y, z);
    // The bulb moved, turned and stretched.
    scene.fractal.transform = Transform { position: p(0.2, -0.1, 0.15), rotation: [20.0, -15.0, 35.0], scale: 0.8, stretch: [1.0, 1.3, 1.0] };
    // A floor of capsules (endless along x, three on each side along y) and the other new
    // shapes in a row.
    scene.objects = vec![
        SceneObject {
            name: "Capsules".into(),
            shape: Shape::Primitive(Primitive::Capsule { radius: 0.1, height: 0.5 }),
            transform: Transform { position: p(0.0, 0.0, -1.2), rotation: [90.0, 0.0, 0.0], ..Default::default() },
            repeat: [0.35, 0.0, 0.35],
            repeat_limit: [0, 0, 3],
            ..Default::default()
        },
        SceneObject { name: "Cone".into(), shape: Shape::Primitive(Primitive::Cone { bottom: 0.25, top: 0.05, height: 0.5 }), transform: Transform { position: p(1.2, -0.6, 0.0), ..Default::default() }, ..Default::default() },
        SceneObject { name: "Hex".into(), shape: Shape::Primitive(Primitive::HexPrism { radius: 0.2, height: 0.4 }), transform: Transform { position: p(1.2, 0.2, 0.4), rotation: [0.0, 30.0, 0.0], ..Default::default() }, ..Default::default() },
        SceneObject { name: "Tri".into(), shape: Shape::Primitive(Primitive::TriPrism { side: 0.4, height: 0.3 }), transform: Transform { position: p(-1.1, -0.8, 0.5), ..Default::default() }, ..Default::default() },
    ];
    let prog = Program::for_scene(&scene);
    assert!(prog.main_transform.is_some());
    validate(&rmb_gpu::codegen::march_shader(&prog), "placed fractal and repeated objects");
    parity(scene.clone());
    // Deep zoom on the placed fractal.
    scene.objects.clear();
    let mut deep = deep_zoom(scene, 1e6);
    deep.march.precision = rmb_core::scene::Precision::Double;
    parity(deep);
}

#[test]
fn gpu_matches_cpu_with_texture_layers() {
    use rmb_core::compose::{ObjectMaterial, Primitive, SceneObject, Shape, Transform};
    use rmb_core::texture::{BlendMode, Mapping, Target, TextureLayer};
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("rmb_layer_test.bmp");
    write_test_bmp(&path, 64, 64);
    let img = path.to_string_lossy().into_owned();
    let mut scene = Scene::default();
    scene.lighting.ao_mode = rmb_core::scene::AoMode::Fast;
    scene.material.emission_glow = 0.5;
    scene.material.layers = vec![
        // The image multiplied over the gradient, projected three ways.
        TextureLayer { image: img.clone(), blend: BlendMode::Multiply, scale: 1.5, rotation: [0.0, 20.0, 10.0], ..Default::default() },
        // A solid color painted through the image as a mask.
        TextureLayer { color: [0.1, 0.8, 0.3], mask: img.clone(), mask_contrast: 3.0, opacity: 0.8, scale: 0.7, ..Default::default() },
        // Relief from the image.
        TextureLayer { image: img.clone(), target: Target::Bump, strength: 6.0, scale: 2.0, ..Default::default() },
        // Glow where the mask is bright.
        TextureLayer { target: Target::Emission, mask: img.clone(), mask_invert: true, mask_contrast: 4.0, strength: 1.5, mapping: Mapping::Spherical, scale: 3.0, ..Default::default() },
    ];
    scene.objects = vec![SceneObject {
        name: "Ground".into(),
        shape: Shape::Primitive(Primitive::Plane),
        transform: Transform { position: rmb_core::v3(0.0, 0.0, -1.1), rotation: [0.0, 0.0, 35.0], ..Default::default() },
        material: ObjectMaterial {
            layers: vec![TextureLayer { image: img.clone(), mapping: Mapping::PlanarXY, blend: BlendMode::Overlay, scale: 0.5, ..Default::default() }],
            ..Default::default()
        },
        ..Default::default()
    }];
    let cpu = |s: &Scene| {
        let mut r = rmb_core::render::cpu::CpuRenderer::new(s.clone(), 48, 32);
        r.render_pass();
        r.rgba8()
    };
    let mut plain = scene.clone();
    plain.material.layers.clear();
    plain.objects[0].material.layers.clear();
    let changed = cpu(&scene).iter().zip(cpu(&plain)).filter(|(a, b)| (**a as i32 - *b as i32).abs() > 8).count();
    assert!(changed > 500, "the layers change only {changed} channels");
    parity(scene);
}

#[test]
fn gpu_matches_cpu_with_alpha_normal_specular_and_roughness_layers() {
    use rmb_core::compose::{ObjectMaterial, Primitive, SceneObject, Shape, Transform};
    use rmb_core::texture::{Mapping, Target, TextureLayer};
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"));
    // Red dots on a transparent background, and a wavy normal map.
    let (w, h) = (64u32, 64u32);
    let dots: Vec<u8> = (0..w * h)
        .flat_map(|i| {
            let (x, y) = ((i % w) as f64 / 8.0, (i / w) as f64 / 8.0);
            let inside = (x - x.floor() - 0.5).powi(2) + (y - y.floor() - 0.5).powi(2) < 0.1;
            [230, 40, 30, if inside { 255 } else { 0 }]
        })
        .collect();
    let normals: Vec<u8> = (0..w * h)
        .flat_map(|i| {
            let (x, y) = ((i % w) as f64 / w as f64, (i / w) as f64 / h as f64);
            let (nx, ny) = (0.6 * (x * 12.0).sin(), 0.6 * (y * 9.0).cos());
            let nz = (1.0 - nx * nx - ny * ny).max(0.0).sqrt();
            [((nx * 0.5 + 0.5) * 255.0) as u8, ((ny * 0.5 + 0.5) * 255.0) as u8, ((nz * 0.5 + 0.5) * 255.0) as u8, 255]
        })
        .collect();
    let (dots_path, normal_path) = (dir.join("rmb_alpha_dots.png"), dir.join("rmb_normals.png"));
    rmb_core::io::image::write_png(&dots_path, w, h, &dots, None).expect("write");
    rmb_core::io::image::write_png(&normal_path, w, h, &normals, None).expect("write");
    let (dots_img, normal_img) = (dots_path.to_string_lossy().into_owned(), normal_path.to_string_lossy().into_owned());
    let bmp = dir.join("rmb_layer_test.bmp");
    write_test_bmp(&bmp, 64, 64);
    let gray = bmp.to_string_lossy().into_owned();
    let mut scene = Scene::default();
    scene.lighting.ao_mode = rmb_core::scene::AoMode::Fast;
    scene.lighting.roughness = 1.0;
    scene.material.layers = vec![
        TextureLayer { image: dots_img.clone(), scale: 2.0, ..Default::default() },
        TextureLayer { image: normal_img.clone(), target: Target::Normal, strength: 1.5, scale: 1.5, ..Default::default() },
        TextureLayer { image: gray.clone(), target: Target::Specular, scale: 0.8, ..Default::default() },
        TextureLayer { image: gray.clone(), target: Target::Roughness, opacity: 0.7, scale: 1.2, rotation: [0.0, 0.0, 30.0], ..Default::default() },
    ];
    // A ground with the normal map in planar and the dots in world space.
    scene.objects = vec![SceneObject {
        name: "Ground".into(),
        shape: Shape::Primitive(Primitive::Plane),
        transform: Transform { position: rmb_core::v3(0.0, 0.0, -1.1), rotation: [0.0, 0.0, 25.0], ..Default::default() },
        material: ObjectMaterial {
            layers: vec![
                TextureLayer { image: normal_img, target: Target::Normal, mapping: Mapping::PlanarXY, scale: 0.5, ..Default::default() },
                TextureLayer { image: dots_img, mapping: Mapping::PlanarXY, space: rmb_core::texture::Space::World, ..Default::default() },
            ],
            ..Default::default()
        },
        ..Default::default()
    }];
    let cpu = |s: &Scene| {
        let mut r = rmb_core::render::cpu::CpuRenderer::new(s.clone(), 48, 32);
        r.render_pass();
        r.rgba8()
    };
    let mut no_alpha = scene.clone();
    no_alpha.material.layers[0].use_alpha = false;
    let changed = cpu(&scene).iter().zip(cpu(&no_alpha)).filter(|(a, b)| (**a as i32 - *b as i32).abs() > 8).count();
    assert!(changed > 300, "the alpha channel changes only {changed} channels");
    parity(scene);
}

/// Writes a torus as OBJ + MTL with two colored halves (materials "warm" and "cool"), texture
/// coordinates and a striped image `stripes.png` next to it.
fn write_test_obj(dir: &std::path::Path) -> std::path::PathBuf {
    use std::fmt::Write;
    std::fs::create_dir_all(dir).expect("test dir");
    std::fs::write(dir.join("t.mtl"), "newmtl warm\nKd 0.9 0.4 0.1\nnewmtl cool\nKd 0.1 0.5 0.9\n").expect("mtl");
    let (n, m) = (48usize, 16usize);
    let mut s = String::from("mtllib t.mtl\n");
    for i in 0..n {
        for j in 0..m {
            let (a, b) = (i as f64 / n as f64 * std::f64::consts::TAU, j as f64 / m as f64 * std::f64::consts::TAU);
            let r = 1.0 + 0.35 * b.cos();
            let _ = writeln!(s, "v {} {} {}", r * a.cos(), 0.35 * b.sin(), r * a.sin());
            let _ = writeln!(s, "vt {} {}", i as f64 / n as f64 * 3.0, j as f64 / m as f64);
        }
    }
    for half in [0, 1] {
        let _ = writeln!(s, "usemtl {}", if half == 0 { "warm" } else { "cool" });
        for i in (half * n / 2)..((half + 1) * n / 2) {
            for j in 0..m {
                let v = |i: usize, j: usize| (i % n) * m + (j % m) + 1;
                let _ = writeln!(s, "f {0}/{0} {1}/{1} {2}/{2}", v(i, j), v(i + 1, j), v(i + 1, j + 1));
                let _ = writeln!(s, "f {0}/{0} {1}/{1} {2}/{2}", v(i, j), v(i + 1, j + 1), v(i, j + 1));
            }
        }
    }
    let img = image::RgbImage::from_fn(16, 8, |x, y| if (x / 2 + y / 2) % 2 == 0 { image::Rgb([250, 240, 40]) } else { image::Rgb([30, 90, 200]) });
    img.save(dir.join("stripes.png")).expect("png");
    let p = dir.join("torus.obj");
    std::fs::write(&p, s).expect("obj");
    p
}

#[test]
fn gpu_matches_cpu_with_a_3d_model() {
    use rmb_core::compose::{BoolOp, ModelRef, ObjectMaterial, Pattern, SceneObject, Shape, Transform};
    let path = write_test_obj(&std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("rmb_model_test"));
    let model = ModelRef { path: path.to_string_lossy().into_owned(), options: rmb_core::mesh::ModelOptions { resolution: 64, ..Default::default() }, ..Default::default() };
    let mut scene = Scene::default();
    scene.lighting.ao_mode = rmb_core::scene::AoMode::Fast;
    scene.objects = vec![
        // The colored ring around the bulb, tilted.
        SceneObject {
            name: "Ring".into(),
            shape: Shape::Model(model.clone()),
            transform: Transform { rotation: [60.0, 10.0, 30.0], scale: 1.8, stretch: [1.3, 0.8, 1.0], ..Default::default() },
            material: ObjectMaterial { pattern: Pattern::Model, ..Default::default() },
            ..Default::default()
        },
        // The same model cuts into the bulb from the side, smoothly.
        SceneObject { name: "Cut".into(), op: BoolOp::SmoothSubtract, smooth: 0.1, shape: Shape::Model(model), transform: Transform { position: rmb_core::v3(0.9, -0.5, 0.3), scale: 0.35, ..Default::default() }, ..Default::default() },
    ];
    // The cool half gets a striped texture, the warm one a new color.
    if let Shape::Model(m) = &mut scene.objects[0].shape {
        m.materials = vec![
            rmb_core::mesh::MaterialOverride { name: "cool".into(), texture: Some("stripes.png".into()), ..Default::default() },
            rmb_core::mesh::MaterialOverride { name: "warm".into(), color: Some([0.8, 0.05, 0.3]), bump: "stripes.png".into(), bump_strength: 3.0, specular: "stripes.png".into(), ..Default::default() },
        ];
    }
    assert!(rmb_core::compose::load_models(&scene).is_empty());
    let prog = Program::for_scene(&scene);
    assert!(matches!(&prog.objects[0].shape, rmb_core::fractal::ObjectShape::Model(Some(m)) if m.look.colored && m.look.maps && m.look.textures.len() == 1));
    validate(&rmb_gpu::codegen::march_shader(&prog), "3D model");
    let cpu = |s: &Scene| {
        let mut r = rmb_core::render::cpu::CpuRenderer::new(s.clone(), 48, 32);
        r.render_pass();
        r.rgba8()
    };
    let mut plain = scene.clone();
    plain.objects.clear();
    let changed = cpu(&scene).iter().zip(cpu(&plain)).filter(|(a, b)| (**a as i32 - *b as i32).abs() > 8).count();
    assert!(changed > 500, "the model changes only {changed} channels");
    parity(scene.clone());
    let before = cpu(&scene);
    // A normal map on the cool half (the stripes as normals), the warm half glowing.
    if let Shape::Model(m) = &mut scene.objects[0].shape {
        m.materials[0].normal = "stripes.png".into();
        m.materials[0].normal_strength = 2.0;
        m.materials[1].emission = Some([1.5, 0.6, 0.1]);
    }
    assert!(rmb_core::compose::load_models(&scene).is_empty());
    let changed = cpu(&scene).iter().zip(&before).filter(|(a, b)| (**a as i32 - **b as i32).abs() > 8).count();
    assert!(changed > 200, "normal map and emission change only {changed} channels");
    parity(scene);
}

/// Renders with the formula parameters and the uniform settings baked into the shader (final
/// renders) look the same as the preview's, which reads them from buffers.
#[test]
fn baked_settings_render_like_the_preview() {
    let Ok(ctx) = rmb_gpu::GpuContext::new_headless() else {
        eprintln!("no GPU, skipped");
        return;
    };
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../mb3d/M3Parameter");
    let names = ["AboxShapes-Misty", "Dainbramage - Hydra", "6 AM - Torii temple", "MarkJayBee - Curvichrome", "toricaleggs"];
    let (w, h) = (96, 64);
    for name in names {
        let path = dir.join(format!("{name}.m3p"));
        let Ok(scene) = rmb_core::io::load(&path) else {
            eprintln!("{name}: not found, skipped");
            continue;
        };
        let mut r = rmb_gpu::GpuRenderer::new(ctx.clone());
        r.set_scene(&scene, w, h, false);
        r.wait_for_pipeline().expect("pipeline");
        while r.samples() < 1 {
            r.work(std::time::Duration::from_secs(10), 1);
        }
        let preview = r.read_rgba8().expect("read back");
        let baked = rmb_gpu::render_blocking(ctx.clone(), &scene, w, h, 1, |_| true).expect("gpu render");
        let diff: Vec<i32> = preview.iter().zip(&baked).map(|(a, b)| (*a as i32 - *b as i32).abs()).collect();
        let mean = diff.iter().sum::<i32>() as f64 / diff.len() as f64;
        let big = diff.iter().filter(|d| **d > 40).count();
        eprintln!("{name}: mean difference {mean:.3}, {big} channels off by more than 40");
        assert!(mean < 1.0 && big * 200 < diff.len(), "{name}: mean {mean}, {big} far off");
    }
}

/// A resting preview switches to the baked shader and keeps accumulating the same image.
#[test]
fn resting_preview_continues_with_the_baked_shader() {
    let Ok(ctx) = rmb_gpu::GpuContext::new_headless() else {
        eprintln!("no GPU, skipped");
        return;
    };
    let scene = Scene::default();
    let (w, h) = (96, 64);
    // With the baked shader taking over (rest) or the buffer shader only, `min` samples at least.
    let render = |rest: bool, min: u32| {
        let mut r = rmb_gpu::GpuRenderer::new(ctx.clone());
        r.set_rest_baking(rest.then_some(std::time::Duration::ZERO));
        r.set_scene(&scene, w, h, false);
        r.wait_for_pipeline().expect("pipeline");
        let start = std::time::Instant::now();
        // Passes until the baked shader took over (or a timeout), then 4 more.
        while rest && !r.rest_baked() && start.elapsed().as_secs() < 60 {
            r.work(std::time::Duration::from_millis(5), u32::MAX);
        }
        assert_eq!(r.rest_baked(), rest, "baked shader in use");
        let target = (r.samples() + 4).max(min);
        while r.samples() < target {
            r.work(std::time::Duration::from_secs(10), target);
        }
        (r.read_rgba8().expect("read back"), r.samples())
    };
    let (baked, n) = render(true, 0);
    assert!(n >= 5, "accumulation went on ({n} samples)");
    let (plain, _) = render(false, n);
    let mean = baked.iter().zip(&plain).map(|(a, b)| (*a as i32 - *b as i32).abs()).sum::<i32>() as f64 / baked.len() as f64;
    eprintln!("mean difference {mean:.3} after {n} samples");
    assert!(mean < 1.0, "mean difference {mean}");
}

#[test]
fn gpu_matches_cpu_with_fractal_haze() {
    use rmb_core::scene::HazeShape;
    for (shape, gradient) in [(HazeShape::Distance, false), (HazeShape::Distance, true), (HazeShape::Iterations, true)] {
        let mut scene = Scene::default();
        scene.lighting.ao_mode = rmb_core::scene::AoMode::Fast;
        // The bulb's iteration counts only climb right at the surface: a thicker haze.
        scene.fog.haze = if shape == HazeShape::Iterations { 40.0 } else { 4.0 };
        scene.fog.haze_shape = shape;
        scene.fog.haze_size = 0.06;
        scene.fog.haze_start = 0.0;
        scene.fog.haze_glow = 1.5;
        scene.fog.haze_gradient = gradient;
        // The haze must change the image.
        let mut plain = scene.clone();
        plain.fog.haze = 0.0;
        let w = (48, 32);
        if let Ok(ctx) = rmb_gpu::GpuContext::new_headless() {
            let a = rmb_gpu::render_blocking(ctx.clone(), &scene, w.0, w.1, 1, |_| true).expect("gpu render");
            let b = rmb_gpu::render_blocking(ctx, &plain, w.0, w.1, 1, |_| true).expect("gpu render");
            let diff = a.iter().zip(&b).map(|(x, y)| (*x as i32 - *y as i32).abs()).sum::<i32>() as f64 / a.len() as f64;
            eprintln!("{shape:?} gradient {gradient}: haze changes the image by {diff:.2}");
            assert!(diff > 2.0, "{shape:?}: the haze must change the image (mean difference {diff})");
        }
        if let Some((mean, big, n)) = gpu_cpu_difference(scene) {
            assert!(mean < 4.0 && big * 100 < n, "{shape:?} gradient {gradient}: mean {mean}, {big} of {n} channels far off");
        }
    }
}

/// Chromatic aberration, white balance and split toning look the same in the GPU display pass
/// and in the CPU post processing of exports.
#[test]
fn display_matches_cpu_with_chromatic_aberration_and_grading() {
    let Ok(ctx) = rmb_gpu::GpuContext::new_headless() else {
        eprintln!("no GPU, skipped");
        return;
    };
    let mut scene = Scene::default();
    scene.lighting.ao_mode = rmb_core::scene::AoMode::Fast;
    let p = &mut scene.post;
    p.chromatic = 1.5;
    p.temperature = 0.4;
    p.tint = -0.3;
    p.shadow_amount = 0.6;
    p.highlight_amount = 0.5;
    let (w, h) = (96, 64);
    let shown = rmb_gpu::render_blocking(ctx.clone(), &scene, w, h, 1, |_| true).expect("gpu render");
    let mut img = rmb_gpu::render_blocking_hdr(ctx.clone(), &scene, w, h, 1, |_| true).expect("gpu render");
    rmb_core::render::chromatic_aberration(&mut img.color, w, h, scene.post.chromatic);
    let mine = img.to_rgba8(&scene.post);
    let worst = shown.iter().zip(&mine).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap();
    assert!(worst <= 2, "display differs by {worst}");
    // The effects change the image.
    let mut plain = scene.clone();
    plain.post = rmb_core::scene::Post::default();
    let base = rmb_gpu::render_blocking(ctx, &plain, w, h, 1, |_| true).expect("gpu render");
    let diff = shown.iter().zip(&base).map(|(a, b)| (*a as i32 - *b as i32).abs()).sum::<i32>() as f64 / shown.len() as f64;
    assert!(diff > 3.0, "mean change {diff}");
}

/// Mandelbulber's per-slot settings (iteration range, weight, add C, bailout check), the
/// minimum iterations and the constant factor: valid shaders (also in double-float) and the
/// GPU matches the CPU.
#[test]
fn gpu_matches_cpu_with_mandelbulber_slot_settings() {
    let mut scene = Scene::default();
    scene.lighting.ao_mode = rmb_core::scene::AoMode::Fast;
    let f = &mut scene.fractal;
    f.slots = vec![Slot::new("Mandelbulb"), Slot::new("Mandelbulb"), Slot::new("Rotate")];
    f.slots[0].stop = Some(2);
    f.slots[1].start = 1;
    f.slots[1].weight = 0.7;
    f.slots[1].add_c = false;
    f.slots[2].check_bailout = false;
    f.min_iterations = 2;
    f.c_factor = [1.1, 1.0, 0.9];
    f.max_iterations = 10;
    let mut prog = Program::for_scene(&scene);
    assert!(prog.slot_extras());
    validate(&rmb_gpu::codegen::march_shader(&prog), "slot settings");
    prog.deep = Some([0.1, -0.2, 0.3]);
    validate(&rmb_gpu::codegen::march_shader(&prog), "slot settings (deep)");
    // The settings change the fractal.
    let mut plain = scene.clone();
    for s in &mut plain.fractal.slots {
        let formula = s.formula.clone();
        *s = Slot::new(&formula);
    }
    plain.fractal.min_iterations = 0;
    plain.fractal.c_factor = [1.0; 3];
    if let Ok(ctx) = rmb_gpu::GpuContext::new_headless() {
        let a = rmb_gpu::render_blocking(ctx.clone(), &scene, 48, 32, 1, |_| true).expect("gpu render");
        let b = rmb_gpu::render_blocking(ctx, &plain, 48, 32, 1, |_| true).expect("gpu render");
        let diff = a.iter().zip(&b).map(|(x, y)| (*x as i32 - *y as i32).abs()).sum::<i32>() as f64 / a.len() as f64;
        assert!(diff > 2.0, "settings change the image ({diff})");
    }
    parity(scene);
}

/// A renderer that switched scenes renders the second one like a fresh renderer (no colors,
/// parameters or buffers left over from the first).
#[test]
fn switching_scenes_leaves_nothing_behind() {
    let Ok(ctx) = rmb_gpu::GpuContext::new_headless() else {
        eprintln!("no GPU, skipped");
        return;
    };
    let render = |r: &mut rmb_gpu::GpuRenderer, s: &Scene| {
        r.set_scene(s, 96, 54, false);
        r.wait_for_pipeline().expect("pipeline");
        while r.samples() < 2 {
            r.work(std::time::Duration::from_secs(5), 2);
        }
        r.read_rgba8().expect("read back")
    };
    for seed in 0..4u64 {
        let a = rmb_core::explore::random_framed(rmb_core::explore::RandomKind::Any, seed * 2, 4);
        let b = rmb_core::explore::random_framed(rmb_core::explore::RandomKind::Any, seed * 2 + 1, 4);
        let mut shared = rmb_gpu::GpuRenderer::new(ctx.clone());
        render(&mut shared, &a);
        let after = render(&mut shared, &b);
        let fresh = render(&mut rmb_gpu::GpuRenderer::new(ctx.clone()), &b);
        let baked = rmb_gpu::render_blocking(ctx.clone(), &b, 96, 54, 2, |_| true).expect("baked");
        let mb = baked.iter().zip(&fresh).map(|(x, y)| (*x as i32 - *y as i32).abs()).sum::<i32>() as f64 / baked.len() as f64;
        eprintln!("seed {seed}: baked vs preview {mb:.3}");
        let mean = after.iter().zip(&fresh).map(|(x, y)| (*x as i32 - *y as i32).abs()).sum::<i32>() as f64 / after.len() as f64;
        eprintln!("seed {seed}: mean difference {mean:.3}");
        assert!(mean < 0.5, "seed {seed}: {mean}");
    }
}
