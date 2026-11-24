// SPDX-License-Identifier: LGPL-2.1-or-later

//! Starting points for the explore lane: random scenes and mutations of a scene. Random scenes
//! come from formulas that give good shapes over most of their parameter range, are framed by
//! the camera and dropped when the fractal fills too little or too much of the view.

use crate::palette::{Harmony, Order, PaletteOptions};
use crate::scene::{ColorMode, Scene, Slot};
use crate::{V3, v3};

/// Kinds of random scenes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RandomKind {
    Any,
    Bulbs,
    Boxes,
    Ifs,
    Hybrids,
}

impl RandomKind {
    pub const ALL: [RandomKind; 5] = [RandomKind::Any, RandomKind::Bulbs, RandomKind::Boxes, RandomKind::Ifs, RandomKind::Hybrids];

    pub fn label(self) -> &'static str {
        match self {
            RandomKind::Any => "Any",
            RandomKind::Bulbs => "Bulbs",
            RandomKind::Boxes => "Boxes",
            RandomKind::Ifs => "IFS",
            RandomKind::Hybrids => "Hybrids",
        }
    }
}

/// What a mutation changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mutation {
    All,
    Parameters,
    Colors,
    Camera,
    Formulas,
}

impl Mutation {
    pub const ALL: [Mutation; 5] = [Mutation::All, Mutation::Parameters, Mutation::Colors, Mutation::Camera, Mutation::Formulas];

    pub fn label(self) -> &'static str {
        match self {
            Mutation::All => "Everything",
            Mutation::Parameters => "Formula parameters",
            Mutation::Colors => "Colors",
            Mutation::Camera => "Camera",
            Mutation::Formulas => "Formulas",
        }
    }
}

/// Small deterministic random stream (splitmix64).
pub struct Rand(u64);

impl Rand {
    pub fn new(seed: u64) -> Rand {
        Rand(seed ^ 0x9e37_79b9_7f4a_7c15)
    }

    pub fn unit(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^= z >> 31;
        (z >> 11) as f64 / (1u64 << 53) as f64
    }

    pub fn range(&mut self, a: f64, b: f64) -> f64 {
        a + (b - a) * self.unit()
    }

    pub fn chance(&mut self, p: f64) -> bool {
        self.unit() < p
    }

    pub fn pick<'a, T>(&mut self, list: &'a [T]) -> &'a T {
        &list[((self.unit() * list.len() as f64) as usize).min(list.len() - 1)]
    }

    pub fn seed(&mut self) -> u64 {
        (self.unit() * 9.0e15) as u64
    }
}

/// A formula with parameter ranges that give good shapes, its iterations and how far the
/// camera starts away.
struct Good {
    name: &'static str,
    params: &'static [(&'static str, f64, f64)],
    iterations: (u32, u32),
    distance: f64,
}

const BULBS: &[Good] = &[
    Good { name: "Mandelbulb", params: &[("power", 3.0, 10.0), ("phi_phase", -40.0, 40.0)], iterations: (8, 14), distance: 2.6 },
    Good { name: "Mandelbulb (cosine)", params: &[("power", 3.0, 9.0)], iterations: (8, 14), distance: 2.6 },
    Good { name: "Xenodreambuie", params: &[("power", 3.0, 9.0), ("alpha", -90.0, 90.0), ("beta", -90.0, 90.0)], iterations: (8, 12), distance: 2.6 },
    Good { name: "Folding bulb", params: &[("power", 2.0, 4.0), ("fold", 0.6, 2.0)], iterations: (8, 14), distance: 3.0 },
    Good { name: "Mandelbar bulb", params: &[("power", 2.0, 6.0)], iterations: (8, 14), distance: 2.6 },
    Good { name: "Quaternion (MB3D)", params: &[], iterations: (10, 16), distance: 2.6 },
];

const BOXES: &[Good] = &[
    Good { name: "Amazing Box", params: &[("scale", -2.4, -1.5), ("min_r", 0.2, 0.7)], iterations: (14, 30), distance: 8.0 },
    Good { name: "Amazing Box", params: &[("scale", 1.6, 2.8), ("min_r", 0.2, 0.7)], iterations: (12, 24), distance: 9.0 },
    Good { name: "Amazing Surf", params: &[("scale", 1.6, 2.4), ("min_r", 0.3, 0.7)], iterations: (12, 24), distance: 7.0 },
    Good { name: "ABox Mod Kali", params: &[("scale", -2.2, -1.6), ("fold", 1.2, 2.4)], iterations: (12, 24), distance: 7.0 },
    Good { name: "Kali box", params: &[("scale", 1.8, 2.3), ("min_r", 0.3, 0.8), ("tx", -0.5, 0.5)], iterations: (12, 24), distance: 7.0 },
    Good { name: "Mandelbox 4D", params: &[("scale", -2.2, -1.6), ("cw_add", -0.5, 0.5)], iterations: (14, 28), distance: 8.0 },
];

const IFS: &[Good] = &[
    Good { name: "Menger sponge", params: &[("scale", 2.6, 3.2), ("ox", 0.8, 1.2), ("oz", 0.8, 1.2)], iterations: (6, 12), distance: 3.6 },
    Good { name: "Sierpinski tetrahedron", params: &[("scale", 1.8, 2.2)], iterations: (10, 18), distance: 3.0 },
    Good {
        name: "Kaleidoscopic IFS",
        params: &[("scale", 1.7, 2.6), ("ox", 0.4, 1.4), ("oy", 0.0, 1.0), ("oz", 0.0, 1.0), ("ax", -30.0, 30.0), ("ay", -30.0, 30.0)],
        iterations: (10, 18),
        distance: 3.6,
    },
    Good { name: "Octahedron", params: &[("scale", 1.8, 2.4), ("ox", 0.7, 1.3)], iterations: (10, 18), distance: 3.0 },
];

/// Transforms mixed into hybrids, with parameter ranges.
const TRANSFORMS: &[Good] = &[
    Good { name: "Rotate", params: &[("ax", -30.0, 30.0), ("ay", -30.0, 30.0), ("az", -30.0, 30.0)], iterations: (1, 1), distance: 0.0 },
    Good { name: "Box fold", params: &[], iterations: (1, 1), distance: 0.0 },
    Good { name: "Abs", params: &[], iterations: (1, 1), distance: 0.0 },
];

fn slot_of(g: &Good, rnd: &mut Rand) -> Slot {
    let mut s = Slot::new(g.name);
    for (p, a, b) in g.params {
        s.params.insert((*p).to_string(), rnd.range(*a, *b));
    }
    s
}

/// A random scene of `kind` (not yet framed; see [`random_framed`]).
pub fn random_scene(kind: RandomKind, rnd: &mut Rand) -> Scene {
    let mut scene = Scene::default();
    let kind = if kind == RandomKind::Any { *rnd.pick(&RandomKind::ALL[1..]) } else { kind };
    let pool: &[Good] = match kind {
        RandomKind::Bulbs => BULBS,
        RandomKind::Boxes => BOXES,
        RandomKind::Ifs => IFS,
        _ => rnd.pick(&[BULBS, BOXES, IFS]),
    };
    let main = rnd.pick(pool);
    let f = &mut scene.fractal;
    f.slots = vec![slot_of(main, rnd)];
    f.max_iterations = rnd.range(main.iterations.0 as f64, main.iterations.1 as f64 + 0.99) as u32;
    if kind == RandomKind::Hybrids {
        // A second formula of the same family, or a transform before the main formula.
        if rnd.chance(0.5) {
            let other = rnd.pick(pool);
            let mut s = slot_of(other, rnd);
            s.iterations = 1 + (rnd.unit() * 2.0) as u32;
            f.slots.push(s);
        }
        if rnd.chance(0.6) || f.slots.len() == 1 {
            f.slots.insert(0, slot_of(rnd.pick(TRANSFORMS), rnd));
            f.repeat_from = 0;
        }
    }
    // Box fractals are bigger: a larger bailout keeps their detail.
    if std::ptr::eq(pool, BOXES) {
        f.bailout = 16.0;
    }
    if rnd.chance(0.2) && std::ptr::eq(pool, BULBS) {
        f.julia = true;
        f.julia_c = [rnd.range(-0.8, 0.8), rnd.range(-0.8, 0.8), rnd.range(-0.8, 0.8), 0.0];
    }
    randomize_colors(&mut scene, rnd);
    // Camera: a random direction, looking at the origin from the formula's distance.
    let dir = random_direction(rnd);
    let cam = &mut scene.camera;
    cam.position = dir * main.distance * rnd.range(0.8, 1.2);
    cam.look_at(V3::ZERO, v3(0.0, 0.0, 1.0));
    scene
}

fn random_direction(rnd: &mut Rand) -> V3 {
    let z = rnd.range(-0.6, 0.9);
    let a = rnd.range(0.0, std::f64::consts::TAU);
    let r = (1.0 - z * z).sqrt();
    v3(r * a.cos(), r * a.sin(), z)
}

/// New colors: a generated palette, a color mode and sometimes glowing neon parts.
pub fn randomize_colors(scene: &mut Scene, rnd: &mut Rand) {
    let neon = rnd.chance(0.25);
    let harmony = if neon { Harmony::DarkNeon } else { *rnd.pick(&Harmony::ALL[..7]) };
    let opts = PaletteOptions {
        colors: 3 + (rnd.unit() * 5.0) as usize,
        harmony,
        random_hue: true,
        hue_jitter: rnd.range(0.0, 25.0),
        order: *rnd.pick(&[Order::Alternate, Order::Harmony, Order::Lightness]),
        hardness: if rnd.chance(0.2) { rnd.range(0.5, 1.0) } else { 0.0 },
        emissive_share: if neon { 0.34 } else { 0.0 },
        emission: rnd.range(1.5, 4.0),
        seed: rnd.seed() | 1,
        ..PaletteOptions::default()
    };
    let m = &mut scene.material;
    m.gradient = crate::palette::generate(&opts);
    m.color_mode = *rnd.pick(&[ColorMode::Iterations, ColorMode::Iterations, ColorMode::OrbitTrap, ColorMode::IterationsRatio]);
    m.color_scale = match m.color_mode {
        ColorMode::OrbitTrap => rnd.range(0.3, 1.5),
        _ => rnd.range(0.05, 0.3),
    };
    m.color_offset = rnd.unit();
    if neon {
        scene.post.bloom = 0.5;
        scene.lighting.ambient *= 0.5;
    } else {
        scene.post.bloom = 0.0;
    }
}

/// Share (0..1) of an `n` × `n` grid of view rays that hit the fractal.
pub fn coverage(scene: &Scene, n: usize) -> f64 {
    let mut hits = 0;
    for j in 0..n {
        for i in 0..n {
            let sx = (i as f64 + 0.5) / n as f64 * 2.0 - 1.0;
            let sy = (j as f64 + 0.5) / n as f64 * 2.0 - 1.0;
            if crate::render::cpu::pick(scene, sx, sy, 96, 54).is_some() {
                hits += 1;
            }
        }
    }
    hits as f64 / (n * n) as f64
}

/// Frames the camera on the fractal: moves along the view toward the surface at the center
/// (or turns toward the origin when the center ray misses), so the fractal fills a good part
/// of the view. False when it cannot be framed (nothing to see, or only surface).
pub fn frame(scene: &mut Scene, rnd: &mut Rand) -> bool {
    scene.march.near = 0.0;
    let Some((t, _)) = crate::render::cpu::pick(scene, 0.0, 0.0, 96, 54) else { return false };
    // Somewhere between far away and close up.
    let keep = rnd.range(0.35, 0.85);
    scene.camera.position += scene.camera.forward * (t * (1.0 - keep));
    let c = coverage(scene, 5);
    (0.25..=0.97).contains(&c)
}

/// How a thumbnail looks, from two renders of it (RGBA8): `half` after half the samples and
/// `full` after all. Returns `contrast` (standard deviation of the brightness, 0..0.5) and
/// `noise` (mean brightness change between the two, relative to the contrast). Real detail
/// stays when more samples come in; sub-pixel dust and grain change, so dusty and grainy
/// scenes have much noise, empty or flat ones little contrast.
pub fn thumb_quality(half: &[u8], full: &[u8]) -> (f64, f64) {
    let lum = |p: &[u8]| (0.2126 * p[0] as f64 + 0.7152 * p[1] as f64 + 0.0722 * p[2] as f64) / 255.0;
    let n = (full.len() / 4).min(half.len() / 4);
    if n == 0 {
        return (0.0, 0.0);
    }
    let l: Vec<f64> = full.as_chunks::<4>().0.iter().take(n).map(|p| lum(p)).collect();
    let mean = l.iter().sum::<f64>() / n as f64;
    let contrast = (l.iter().map(|v| (v - mean) * (v - mean)).sum::<f64>() / n as f64).sqrt();
    let change = half.as_chunks::<4>().0.iter().zip(&l).map(|(p, v)| (lum(p) - v).abs()).sum::<f64>() / n as f64;
    (contrast, change / (contrast + 0.02))
}

/// A random scene that frames well (up to `tries` attempts; the last attempt otherwise).
pub fn random_framed(kind: RandomKind, seed: u64, tries: usize) -> Scene {
    let mut rnd = Rand::new(seed);
    let mut last = Scene::default();
    for _ in 0..tries.max(1) {
        let mut s = random_scene(kind, &mut rnd);
        if frame(&mut s, &mut rnd) {
            return s;
        }
        last = s;
    }
    last
}

/// A variant of `base`: `strength` (0..1) scales how far it goes.
pub fn mutate(base: &Scene, what: Mutation, strength: f64, seed: u64) -> Scene {
    let mut rnd = Rand::new(seed);
    let mut s = base.clone();
    let k = strength.clamp(0.0, 1.0);
    let what = if what == Mutation::All { *rnd.pick(&Mutation::ALL[1..]) } else { what };
    match what {
        Mutation::Parameters | Mutation::All => {
            for slot in &mut s.fractal.slots {
                let Some(f) = crate::formula::find(&slot.formula) else { continue };
                for p in &f.def.params {
                    if matches!(p.kind, crate::formula::ParamKind::Bool | crate::formula::ParamKind::Int) || rnd.chance(0.4) {
                        continue;
                    }
                    let v = slot.value(p);
                    let span = (p.max - p.min).abs().max(1e-6);
                    slot.params.insert(p.ident.clone(), v + rnd.range(-1.0, 1.0) * span * 0.08 * k);
                }
            }
            if rnd.chance(0.3 * k) {
                let f = &mut s.fractal;
                f.max_iterations = (f.max_iterations as i64 + rnd.range(-3.0, 3.0) as i64).clamp(2, 100) as u32;
            }
        }
        Mutation::Colors => {
            if rnd.chance(0.5) {
                randomize_colors(&mut s, &mut rnd);
            } else {
                s.material.color_offset = (s.material.color_offset + rnd.range(-0.5, 0.5) * k).rem_euclid(1.0);
                s.material.gradient = crate::palette::adjust(&s.material.gradient, crate::palette::Adjust { hue: rnd.range(-120.0, 120.0) * k, saturation: rnd.range(-0.3, 0.3) * k, ..Default::default() });
            }
        }
        Mutation::Camera => {
            // Orbit around the point the camera looks at, and a little closer or farther.
            let cam = s.camera.clone();
            let t = crate::render::cpu::pick(&s, 0.0, 0.0, 96, 54).map_or(1.0, |(t, _)| t);
            let target = cam.position + cam.forward * t;
            let off = cam.position - target;
            let yaw = rnd.range(-1.0, 1.0) * 0.8 * k;
            let (sn, cs) = yaw.sin_cos();
            let rotated = v3(off.x * cs - off.y * sn, off.x * sn + off.y * cs, off.z + rnd.range(-0.3, 0.3) * k * off.length());
            s.camera.position = target + rotated * rnd.range(1.0 - 0.4 * k, 1.0 + 0.3 * k);
            s.camera.look_at(target, cam.up);
        }
        Mutation::Formulas => {
            let f = &mut s.fractal;
            if !f.slots.is_empty() && rnd.chance(0.5) && f.slots.len() < crate::scene::MAX_SLOTS {
                f.slots.insert(0, slot_of(rnd.pick(TRANSFORMS), &mut rnd));
            } else if let Some(slot) = f.slots.iter_mut().find(|s| crate::formula::find(&s.formula).is_some_and(|f| !f.def.transform)) {
                // Another formula of a family that matches the current one.
                let pool: &[Good] = [BULBS, BOXES, IFS].into_iter().find(|p| p.iter().any(|g| g.name == slot.formula)).unwrap_or(BULBS);
                *slot = slot_of(rnd.pick(pool), &mut rnd);
            }
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_scenes_use_known_formulas_and_frame() {
        for name in BULBS.iter().chain(BOXES).chain(IFS).chain(TRANSFORMS).map(|g| g.name) {
            assert!(crate::formula::find(name).is_some(), "{name} is not in the library");
        }
        let mut framed = 0;
        for seed in 0..6 {
            let s = random_framed(RandomKind::Any, seed, 4);
            assert!(!s.fractal.slots.is_empty());
            if (0.25..=0.97).contains(&coverage(&s, 5)) {
                framed += 1;
            }
        }
        assert!(framed >= 3, "only {framed} of 6 random scenes framed");
    }

    #[test]
    fn mutations_change_the_scene() {
        let base = random_framed(RandomKind::Bulbs, 3, 4);
        for what in Mutation::ALL {
            let changed = (0..4).any(|seed| mutate(&base, what, 0.8, seed) != base);
            assert!(changed, "{what:?} changes nothing");
        }
    }
}
