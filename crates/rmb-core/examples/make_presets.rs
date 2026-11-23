//! Writes the explore lane's preset scenes: `make_presets <folder>` (crates/rmb-app/presets).
use rmb_core::palette::{Harmony, Order, PaletteOptions};
use rmb_core::scene::{ColorMode, ColorStop, Slot};
use rmb_core::{Scene, V3, v3};

/// Camera at `pos` looking at `target`, then `approach` (0..1) of the way to the surface at the
/// view center.
fn camera(s: &mut Scene, pos: V3, target: V3, approach: f64) {
    s.camera.position = pos;
    s.camera.look_at(target, v3(0.0, 0.0, 1.0));
    if approach > 0.0
        && let Some((t, _)) = rmb_core::render::cpu::pick(s, 0.0, 0.0, 256, 144)
    {
        s.camera.position += s.camera.forward * (t * approach);
    }
}

fn base(notes: &str) -> Scene {
    Scene { notes: notes.into(), ..Default::default() }
}

fn slot(name: &str, params: &[(&str, f64)]) -> Slot {
    let mut s = Slot::new(name);
    for (k, v) in params {
        s.params.insert((*k).into(), *v);
    }
    s
}

fn palette(harmony: Harmony, hue: f64, colors: usize, seed: u64) -> Vec<ColorStop> {
    rmb_core::palette::generate(&PaletteOptions { harmony, hue, random_hue: false, colors, order: Order::Alternate, seed, ..Default::default() })
}

fn rgb(stops: &[(f64, [u8; 3])]) -> Vec<ColorStop> {
    stops.iter().map(|(p, c)| ColorStop::new(*p, c.map(rmb_core::gradient::srgb_to_linear))).collect()
}

fn mandelbulb() -> Scene {
    let mut s = base("Preset: the classic power 8 Mandelbulb.");
    s.fractal.max_iterations = 14;
    s.material.gradient = rgb(&[(0.0, [30, 14, 8]), (0.3, [190, 98, 50]), (0.55, [250, 205, 150]), (0.8, [120, 60, 35]), (1.0, [30, 14, 8])]);
    camera(&mut s, v3(1.7, -2.0, 1.1), V3::ZERO, 0.0);
    s
}

fn amazing_box() -> Scene {
    let mut s = base("Preset: an Amazing Box (Mandelbox, scale -1.77) seen from inside its structure.");
    s.fractal.slots = vec![slot("Amazing Box", &[("scale", -1.77), ("min_r", 0.5)])];
    s.fractal.max_iterations = 22;
    s.fractal.bailout = 16.0;
    s.material.gradient = palette(Harmony::Complementary, 215.0, 5, 3);
    s.material.color_scale = 0.12;
    s.fog.density = 0.08;
    camera(&mut s, v3(3.2, -2.6, 1.8), v3(0.0, 0.0, 0.2), 0.55);
    s
}

fn menger() -> Scene {
    let mut s = base("Preset: a Menger sponge, lit from the side.");
    s.fractal.slots = vec![slot("Menger sponge", &[])];
    s.fractal.max_iterations = 8;
    s.material.gradient = rgb(&[(0.0, [70, 66, 60]), (0.5, [196, 188, 172]), (1.0, [90, 84, 76])]);
    s.material.color_mode = ColorMode::OrbitTrap;
    camera(&mut s, v3(2.4, -2.9, 2.2), V3::ZERO, 0.0);
    s
}

fn kifs() -> Scene {
    let mut s = base("Preset: a kaleidoscopic IFS with rotations between the folds.");
    s.fractal.slots = vec![slot("Kaleidoscopic IFS", &[("scale", 2.1), ("ox", 1.0), ("oy", 0.3), ("oz", 0.5), ("ax", 12.0), ("ay", -18.0)])];
    s.fractal.max_iterations = 14;
    s.material.gradient = palette(Harmony::Triadic, 280.0, 6, 11);
    s.material.color_mode = ColorMode::OrbitTrap;
    s.material.color_scale = 0.8;
    camera(&mut s, v3(2.0, -2.4, 1.6), V3::ZERO, 0.0);
    s
}

fn quaternion_julia() -> Scene {
    let mut s = base("Preset: a quaternion Julia set with glossy reflections.");
    s.fractal.slots = vec![slot("Quaternion", &[])];
    s.fractal.max_iterations = 14;
    s.fractal.julia = true;
    s.fractal.julia_c = [-0.745, 0.113, 0.05, 0.05];
    s.fractal.max_iterations = 24;
    s.material.gradient = palette(Harmony::Analogous, 170.0, 5, 5);
    s.material.reflection = 0.35;
    camera(&mut s, v3(2.2, -2.6, 1.5), V3::ZERO, 0.0);
    s
}

fn neon_box_city() -> Scene {
    let mut s = base("Preset: dark box structures with thin neon lines in a blue haze (in the style of Julius Horsthuis' films): a dark gradient with emissive glow bands, fractal haze, bloom.");
    s.fractal.slots = vec![slot("Amazing Box", &[("scale", -1.5), ("min_r", 0.5)])];
    s.fractal.max_iterations = 24;
    s.fractal.bailout = 16.0;
    // Dark, slightly blue structure; the light comes from three thin glowing bands.
    s.material.gradient = rgb(&[(0.0, [10, 12, 20]), (0.5, [28, 34, 52]), (1.0, [10, 12, 20])]);
    let band = |pos: f64, color: [f32; 3]| rmb_core::gradient::GlowBand { pos, width: 0.012, softness: 0.4, color, emission: 4.0, ..Default::default() };
    s.material.bands = vec![band(0.2, [0.1, 0.9, 1.0]), band(0.52, [1.0, 0.25, 0.8]), band(0.8, [1.0, 0.6, 0.15])];
    s.material.color_mode = ColorMode::OrbitTrap;
    s.material.color_scale = 0.45;
    s.material.specular = 0.6;
    s.lighting.ambient = 0.25;
    s.lighting.sky_color = [0.2, 0.35, 0.8];
    s.lighting.lights.iter_mut().for_each(|l| l.intensity *= 0.25);
    s.background.top = [0.0, 0.01, 0.03];
    s.background.horizon = [0.02, 0.05, 0.12];
    s.background.bottom = [0.0, 0.0, 0.02];
    s.fog.density = 0.1;
    s.fog.color = [0.01, 0.03, 0.09];
    s.fog.haze = 0.8;
    s.fog.haze_size = 0.02;
    s.fog.haze_glow = 0.8;
    s.fog.haze_color = [0.05, 0.25, 1.0];
    s.post.bloom = 0.6;
    s.post.bloom_threshold = 0.8;
    s.post.chromatic = 0.5;
    camera(&mut s, v3(4.2, -5.0, 1.2), v3(0.0, 0.0, 0.3), 0.2);
    s
}

fn glass_bulb() -> Scene {
    let mut s = base("Preset: a Mandelbulb of colored glass.");
    s.fractal.slots = vec![slot("Mandelbulb", &[("power", 6.0)])];
    s.fractal.max_iterations = 10;
    let mut g = palette(Harmony::Analogous, 200.0, 4, 9);
    g.iter_mut().for_each(|c| c.transparency = 0.92);
    s.material.gradient = g;
    s.material.transmission = true;
    s.material.reflection = 1.0;
    s.material.reflection_bounces = 4;
    s.material.absorption = 0.4;
    s.background.top = [0.05, 0.06, 0.09];
    s.background.horizon = [0.5, 0.45, 0.4];
    s.background.bottom = [0.08, 0.07, 0.06];
    camera(&mut s, v3(1.8, -2.1, 1.0), V3::ZERO, 0.0);
    s
}

fn hybrid() -> Scene {
    let mut s = base("Preset: a hybrid of a rotation, an Amazing Box and a Mandelbulb iteration.");
    s.fractal.slots = vec![slot("Amazing Box", &[("scale", -1.8), ("min_r", 0.5)]), slot("Rotate", &[("ax", 8.0), ("az", 12.0)])];
    s.fractal.slots[0].iterations = 2;
    s.fractal.max_iterations = 18;
    s.fractal.bailout = 16.0;
    s.material.gradient = palette(Harmony::SplitComplementary, 30.0, 6, 17);
    s.material.color_scale = 0.12;
    camera(&mut s, v3(4.0, -4.8, 2.6), V3::ZERO, 0.45);
    s
}

fn main() {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).expect("output folder"));
    std::fs::create_dir_all(&dir).expect("folder");
    type Make = fn() -> Scene;
    let presets: [(&str, Make); 8] = [
        ("mandelbulb", mandelbulb),
        ("amazing-box", amazing_box),
        ("menger", menger),
        ("kifs", kifs),
        ("quaternion-julia", quaternion_julia),
        ("neon-box-city", neon_box_city),
        ("glass-bulb", glass_bulb),
        ("hybrid", hybrid),
    ];
    for (name, make) in presets {
        let s = make();
        std::fs::write(dir.join(format!("{name}.rmb")), s.to_json()).expect("write");
        println!("{name}: coverage {:.2}", rmb_core::explore::coverage(&s, 6));
    }
}
