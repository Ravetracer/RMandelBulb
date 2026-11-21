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
    rmb_gpu::render_animation(Some(ctx), 0..3, 48, 32, 1, |f| plan.scene_at(f), &cancel, |_| {}, |f, rgba| {
        frames.push((f, rgba));
        Ok(())
    })
    .expect("render");
    assert_eq!(frames.iter().map(|f| f.0).collect::<Vec<_>>(), vec![0, 1, 2]);
    let diff = |x: &[u8], y: &[u8]| x.iter().zip(y).filter(|(p, q)| p != q).count();
    assert!(diff(&frames[0].1, &frames[1].1) > 100 && diff(&frames[1].1, &frames[2].1) > 100);
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
            transform: Transform { position: p(1.4, 0.4, 0.2), rotation: [10.0, 25.0, 40.0], scale: 0.25 },
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

/// Writes a torus as OBJ + MTL with two colored halves (materials "warm" and "cool").
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
        }
    }
    for half in [0, 1] {
        let _ = writeln!(s, "usemtl {}", if half == 0 { "warm" } else { "cool" });
        for i in (half * n / 2)..((half + 1) * n / 2) {
            for j in 0..m {
                let v = |i: usize, j: usize| (i % n) * m + (j % m) + 1;
                let _ = writeln!(s, "f {} {} {}", v(i, j), v(i + 1, j), v(i + 1, j + 1));
                let _ = writeln!(s, "f {} {} {}", v(i, j), v(i + 1, j + 1), v(i, j + 1));
            }
        }
    }
    let p = dir.join("torus.obj");
    std::fs::write(&p, s).expect("obj");
    p
}

#[test]
fn gpu_matches_cpu_with_a_3d_model() {
    use rmb_core::compose::{BoolOp, ModelRef, ObjectMaterial, Pattern, SceneObject, Shape, Transform};
    let path = write_test_obj(&std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("rmb_model_test"));
    let model = ModelRef { path: path.to_string_lossy().into_owned(), options: rmb_core::mesh::ModelOptions { resolution: 64, ..Default::default() } };
    let mut scene = Scene::default();
    scene.lighting.ao_mode = rmb_core::scene::AoMode::Fast;
    scene.objects = vec![
        // The colored ring around the bulb, tilted.
        SceneObject {
            name: "Ring".into(),
            shape: Shape::Model(model.clone()),
            transform: Transform { rotation: [60.0, 10.0, 30.0], scale: 1.8, ..Default::default() },
            material: ObjectMaterial { pattern: Pattern::Model, ..Default::default() },
            ..Default::default()
        },
        // The same model cuts into the bulb from the side, smoothly.
        SceneObject { name: "Cut".into(), op: BoolOp::SmoothSubtract, smooth: 0.1, shape: Shape::Model(model), transform: Transform { position: rmb_core::v3(0.9, -0.5, 0.3), scale: 0.35, ..Default::default() }, ..Default::default() },
    ];
    assert!(rmb_core::compose::load_models(&scene).is_empty());
    let prog = Program::for_scene(&scene);
    assert!(matches!(&prog.objects[0].shape, rmb_core::fractal::ObjectShape::Model(Some(v)) if !v.colors.is_empty()));
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
    parity(scene);
}
