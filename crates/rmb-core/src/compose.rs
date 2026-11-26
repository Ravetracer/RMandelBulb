// SPDX-License-Identifier: LGPL-2.1-or-later

//! Scene composition (like Mandelbulber's boolean mode): objects after the main fractal, each a
//! fractal, a primitive or a 3D model with its own placement, material and boolean operation.
//!
//! The objects are combined with the scene's distance estimate in list order: union, subtract
//! or intersect, also smooth (polynomial smooth minimum over the blend distance). An object is
//! evaluated in its own coordinates: `local = R⁻¹ (p − position) / scale`, its distance is
//! scaled back by `scale`. Primitive distances are exact (signed) distances; the water surface
//! is divided by the steepness of its waves so it never overestimates.
//!
//! These functions are mirrored in the GPU kernel (march.wgsl); keep them in sync.

use serde::{Deserialize, Serialize};

use crate::math::{V3, v3};
use crate::scene::{ColorMode, ColorStop, Fractal};

/// Most objects a scene can have besides the main fractal.
pub const MAX_OBJECTS: usize = 8;

/// Distance of a model that is not baked (yet): far away.
pub const NO_MODEL: f64 = 1.0e10;

/// Bakes the 3D models of the scene and loads their textures (blocking; renders that must show
/// them call this first).
/// Returns the models that failed, with the reason.
pub fn load_models(scene: &crate::Scene) -> Vec<String> {
    let mut failed = Vec::new();
    for o in scene.objects.iter().filter(|o| o.enabled) {
        if let Shape::Model(m) = &o.shape {
            match crate::mesh::get(&m.path, &m.options) {
                Ok(_) => crate::mesh::load_textures(m),
                Err(e) => failed.push(format!("{}: {e}", o.name)),
            }
        }
    }
    failed
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SceneObject {
    pub name: String,
    pub enabled: bool,
    pub shape: Shape,
    pub transform: Transform,
    pub op: BoolOp,
    /// Blend distance of the smooth operations (world units).
    pub smooth: f64,
    pub material: ObjectMaterial,
    /// Repetition: spacing of the copies along the object's own x, y and z axes, in its own
    /// units (before the scale; 0: no copies along that axis).
    pub repeat: [f64; 3],
    /// Copies on each side along each axis (0: endless).
    pub repeat_limit: [u32; 3],
}

impl Default for SceneObject {
    fn default() -> Self {
        SceneObject {
            name: "Sphere".into(),
            enabled: true,
            shape: Shape::Primitive(Primitive::Sphere { radius: 0.5 }),
            transform: Transform::default(),
            op: BoolOp::Union,
            smooth: 0.1,
            material: ObjectMaterial::default(),
            repeat: [0.0; 3],
            repeat_limit: [0; 3],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Shape {
    Fractal(Box<Fractal>),
    Primitive(Primitive),
    /// A 3D model file, baked into a distance grid (mesh.rs).
    Model(ModelRef),
}

/// A 3D model of the scene: the file and how it is baked.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelRef {
    pub path: String,
    pub options: crate::mesh::ModelOptions,
    /// Changes to the file's materials (by material name).
    pub materials: Vec<crate::mesh::MaterialOverride>,
    /// Images near the model were assigned to its materials once (on import).
    pub auto_assigned: bool,
}

/// Primitive shapes in their own coordinates (z is up).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Primitive {
    /// The ground: everything below z = 0 is solid.
    Plane,
    /// Box of the given full size, edges rounded by `rounding`.
    Box { size: [f64; 3], rounding: f64 },
    Sphere { radius: f64 },
    /// Upright cylinder (along z), centered on the origin.
    Cylinder { radius: f64, height: f64, rounding: f64 },
    /// Ring around the z axis.
    Torus { major: f64, minor: f64 },
    /// A water surface at z = 0 with ocean waves (`water_height`): many waves around the
    /// wind direction, their crests sharpened by drag. It reflects by itself
    /// (Fresnel, `reflect`) and shows foam on the crests. `phase` is the time of the waves;
    /// animations add `speed` per second.
    Water {
        amplitude: f64,
        wavelength: f64,
        phase: f64,
        angle: f64,
        /// Sharpness of the crests (0 = round swell, 1 = like the sea, 2 = very choppy).
        #[serde(default = "water_choppy")]
        choppy: f64,
        /// Number of waves (4-64): more give a richer sea with finer ripples.
        #[serde(default = "water_detail")]
        detail: f64,
        /// Foam on the crests (0-1).
        #[serde(default)]
        foam: f64,
        /// Strength of the reflection (1 = like water).
        #[serde(default = "water_reflect")]
        reflect: f64,
        /// Movement of the waves per second in animations.
        #[serde(default)]
        speed: f64,
        /// Waves running in toward the other objects of the scene and breaking on them, with
        /// foam along the coast (0 = none, 1 = normal, 2 = strong).
        #[serde(default = "water_shore_default")]
        shore: f64,
        /// Drops splashing up where the waves break (0 = none, 1 = normal, 2 = high).
        #[serde(default = "water_spray_default")]
        spray: f64,
        /// Size of the spray drops (1 = normal, 0.2-3).
        #[serde(default = "water_drop_size_default")]
        drop_size: f64,
        /// How much the drop sizes vary (0 = all alike, 1 = from tiny to full size).
        #[serde(default = "water_drop_variation_default")]
        drop_variation: f64,
        /// The water as a medium the camera and rays can enter (see-through, underwater view).
        #[serde(default)]
        medium: WaterMedium,
    },
    /// Upright pill: a cylinder with half-sphere ends, `height` from end to end.
    Capsule { radius: f64, height: f64 },
    /// Upright cone from the bottom radius to the top radius (0: pointed), centered on the origin.
    Cone { bottom: f64, top: f64, height: f64 },
    /// Upright six-sided prism; `radius` from the axis to the middle of a side.
    HexPrism { radius: f64, height: f64 },
    /// Upright prism with an equilateral triangle as its section; `side` is its side length.
    TriPrism { side: f64, height: f64 },
}

impl Primitive {
    /// Names by [`Primitive::index`].
    pub const NAMES: [&'static str; 10] = ["Plane", "Box", "Sphere", "Cylinder", "Torus", "Water", "Capsule", "Cone", "Hexagonal prism", "Triangular prism"];

    /// The primitives with sensible start values (in menu order).
    pub fn defaults() -> [Primitive; 10] {
        [
            Primitive::Plane,
            Primitive::Box { size: [1.0, 1.0, 1.0], rounding: 0.0 },
            Primitive::Sphere { radius: 0.5 },
            Primitive::Cylinder { radius: 0.3, height: 1.0, rounding: 0.0 },
            Primitive::Capsule { radius: 0.25, height: 1.0 },
            Primitive::Cone { bottom: 0.4, top: 0.0, height: 1.0 },
            Primitive::Torus { major: 0.6, minor: 0.15 },
            Primitive::HexPrism { radius: 0.4, height: 1.0 },
            Primitive::TriPrism { side: 0.8, height: 1.0 },
            Primitive::water(),
        ]
    }

    pub fn index(&self) -> u32 {
        match self {
            Primitive::Plane => 0,
            Primitive::Box { .. } => 1,
            Primitive::Sphere { .. } => 2,
            Primitive::Cylinder { .. } => 3,
            Primitive::Torus { .. } => 4,
            Primitive::Water { .. } => 5,
            Primitive::Capsule { .. } => 6,
            Primitive::Cone { .. } => 7,
            Primitive::HexPrism { .. } => 8,
            Primitive::TriPrism { .. } => 9,
        }
    }

    pub fn name(&self) -> &'static str {
        Self::NAMES[self.index() as usize]
    }

    /// Water with good start values (moving waves in animations).
    pub fn water() -> Primitive {
        Primitive::Water {
            amplitude: 0.08,
            wavelength: 2.0,
            phase: 0.0,
            angle: 0.0,
            choppy: water_choppy(),
            detail: water_detail(),
            foam: 0.3,
            reflect: water_reflect(),
            speed: 1.0,
            shore: water_shore_default(),
            spray: water_spray_default(),
            drop_size: water_drop_size_default(),
            drop_variation: water_drop_variation_default(),
            medium: WaterMedium { see_through: true, ..WaterMedium::default() },
        }
    }

    /// The medium settings of water (None for other primitives).
    pub fn medium(&self) -> Option<&WaterMedium> {
        match self {
            Primitive::Water { medium, .. } => Some(medium),
            _ => None,
        }
    }

    /// The parameters in a fixed order (GPU buffer: `prim`, `prim2`, `prim3`).
    pub fn params(&self) -> [f64; 12] {
        let p4 = |a: [f64; 4]| [a[0], a[1], a[2], a[3], 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        match *self {
            Primitive::Plane => [0.0; 12],
            Primitive::Box { size, rounding } => p4([size[0], size[1], size[2], rounding]),
            Primitive::Sphere { radius } => p4([radius, 0.0, 0.0, 0.0]),
            Primitive::Cylinder { radius, height, rounding } => p4([radius, height, rounding, 0.0]),
            Primitive::Torus { major, minor } => p4([major, minor, 0.0, 0.0]),
            Primitive::Water { amplitude, wavelength, phase, angle, choppy, detail, foam, reflect, shore, spray, drop_size, drop_variation, .. } => [amplitude, wavelength, phase, angle, choppy, detail, foam, reflect, shore, spray, drop_size, drop_variation],
            Primitive::Capsule { radius, height } => p4([radius, height, 0.0, 0.0]),
            Primitive::Cone { bottom, top, height } => p4([bottom, top, height, 0.0]),
            Primitive::HexPrism { radius, height } => p4([radius, height, 0.0, 0.0]),
            Primitive::TriPrism { side, height } => p4([side, height, 0.0, 0.0]),
        }
    }

    /// Signed distance from the local point `q` (negative inside).
    pub fn distance(&self, q: V3) -> f64 {
        primitive_distance(self.index(), self.params(), q)
    }
}

/// Water as a medium (the first see-through water of a scene): rays refract into it, the camera
/// can dive, and light and sight fade with the distance travelled in it, red first.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WaterMedium {
    /// Rays pass through the surface (refraction) and the camera can go below it. Off: the
    /// opaque, reflecting water of older scenes.
    pub see_through: bool,
    /// Refraction index (water 1.33).
    pub ior: f64,
    /// Color of deep water near the surface (what distant things fade into).
    pub color: [f32; 3],
    /// Color left of white light after `visibility` in the water (absorption per channel:
    /// the darker a channel, the faster it fades; red goes first in real water).
    pub filter: [f32; 3],
    /// Distance (scene units) after which light is filtered down to `filter`.
    pub visibility: f64,
    /// Fading of every color alike (floating particles): 0 = clear, 1 = murky.
    pub murk: f64,
    /// How much light from above weakens with depth (1 = like the view through the water;
    /// 0 = equally bright at every depth).
    pub depth_dark: f64,
}

impl Default for WaterMedium {
    fn default() -> Self {
        WaterMedium { see_through: false, ior: 1.33, color: [0.03, 0.22, 0.3], filter: [0.12, 0.6, 0.78], visibility: 3.0, murk: 0.25, depth_dark: 1.0 }
    }
}

impl WaterMedium {
    /// Fading per scene unit of the view through the water, per channel: absorption from the
    /// filter color plus the murk (the same for all channels).
    pub fn extinction(&self) -> [f64; 3] {
        let vis = self.visibility.max(1e-6);
        self.filter.map(|f| -(f as f64).clamp(1e-4, 1.0).ln() / vis + self.murk.max(0.0) * 3.0 / vis)
    }

    /// Fading per unit of depth of the light from above.
    pub fn depth_fade(&self) -> [f64; 3] {
        self.extinction().map(|e| e * self.depth_dark.max(0.0))
    }
}

/// Signed distance of primitive `kind` with parameters `a` at the local point `q`.
pub fn primitive_distance(kind: u32, a: [f64; 12], q: V3) -> f64 {
    match kind {
        0 => q.z,
        1 => {
            let r = a[3].max(0.0);
            let h = v3((a[0] * 0.5 - r).max(0.0), (a[1] * 0.5 - r).max(0.0), (a[2] * 0.5 - r).max(0.0));
            let d = v3(q.x.abs() - h.x, q.y.abs() - h.y, q.z.abs() - h.z);
            let outside = v3(d.x.max(0.0), d.y.max(0.0), d.z.max(0.0)).length();
            outside + d.x.max(d.y).max(d.z).min(0.0) - r
        }
        2 => q.length() - a[0],
        3 => {
            let r = a[2].max(0.0);
            let dx = (q.x * q.x + q.y * q.y).sqrt() - (a[0] - r).max(0.0);
            let dz = q.z.abs() - (a[1] * 0.5 - r).max(0.0);
            dx.max(dz).min(0.0) + (dx.max(0.0) * dx.max(0.0) + dz.max(0.0) * dz.max(0.0)).sqrt() - r
        }
        4 => {
            let xy = (q.x * q.x + q.y * q.y).sqrt() - a[0];
            (xy * xy + q.z * q.z).sqrt() - a[1]
        }
        6 => {
            let r = a[0].max(0.0);
            let hh = (a[1] * 0.5 - r).max(0.0);
            v3(q.x, q.y, q.z - q.z.clamp(-hh, hh)).length() - r
        }
        7 => {
            // Capped cone (exact; after Inigo Quilez).
            let (r1, r2, h) = (a[0].max(0.0), a[1].max(0.0), (a[2] * 0.5).max(1e-9));
            let (qx, qy) = ((q.x * q.x + q.y * q.y).sqrt(), q.z);
            let (k1x, k1y, k2x, k2y) = (r2, h, r2 - r1, 2.0 * h);
            let cax = qx - qx.min(if qy < 0.0 { r1 } else { r2 });
            let cay = qy.abs() - h;
            let f = (((k1x - qx) * k2x + (k1y - qy) * k2y) / (k2x * k2x + k2y * k2y)).clamp(0.0, 1.0);
            let (cbx, cby) = (qx - k1x + k2x * f, qy - k1y + k2y * f);
            let sign = if cbx < 0.0 && cay < 0.0 { -1.0 } else { 1.0 };
            sign * (cax * cax + cay * cay).min(cbx * cbx + cby * cby).sqrt()
        }
        8 => {
            // Hexagonal prism (exact; after Inigo Quilez).
            let (kx, ky, kz) = (-0.866_025_403_784_438_6, 0.5, 0.577_350_269_189_625_8);
            let (r, h) = (a[0].max(0.0), a[1] * 0.5);
            let (mut px, mut py) = (q.x.abs(), q.y.abs());
            let m = 2.0 * (kx * px + ky * py).min(0.0);
            px -= m * kx;
            py -= m * ky;
            let ex = px - px.clamp(-kz * r, kz * r);
            let dx = (ex * ex + (py - r) * (py - r)).sqrt() * (py - r).signum();
            let dz = q.z.abs() - h;
            dx.max(dz).min(0.0) + (dx.max(0.0) * dx.max(0.0) + dz.max(0.0) * dz.max(0.0)).sqrt()
        }
        9 => {
            // Equilateral triangle (exact; after Inigo Quilez), extruded along z.
            let k = 3f64.sqrt();
            let r = a[0].max(0.0) * 0.5;
            let (mut px, mut py) = (q.x.abs() - r, q.y + r / k);
            if px + k * py > 0.0 {
                (px, py) = ((px - k * py) * 0.5, (-k * px - py) * 0.5);
            }
            px -= px.clamp(-2.0 * r, 0.0);
            let dx = -(px * px + py * py).sqrt() * py.signum();
            let dz = q.z.abs() - a[1] * 0.5;
            dx.max(dz).min(0.0) + (dx.max(0.0) * dx.max(0.0) + dz.max(0.0) * dz.max(0.0)).sqrt()
        }
        _ => water_distance(&a, q, NO_SHORE),
    }
}

fn water_choppy() -> f64 {
    1.0
}

fn water_detail() -> f64 {
    32.0
}

fn water_reflect() -> f64 {
    1.0
}

fn water_shore_default() -> f64 {
    1.0
}

fn water_spray_default() -> f64 {
    1.0
}

fn water_drop_size_default() -> f64 {
    1.0
}

fn water_drop_variation_default() -> f64 {
    0.5
}

/// Waves of a water surface: their number grows with the detail.
pub fn water_waves(detail: f64) -> i32 {
    (detail.round() as i32).clamp(4, 64)
}

/// Wave height ∝ wave length ^ this: 1 gives every size band about the same slope, as on the
/// sea (ripples on swells).
const WATER_FALLOFF: f64 = 1.0;

/// Shore distance of water with no other object near it.
pub const NO_SHORE: f64 = 1.0e30;

/// How far out the shore waves reach, in wave lengths.
const SHORE_RANGE: f64 = 0.6;
/// Length of the shore waves, in wave lengths (waves grow shorter in shallow water).
const SHORE_LEN: f64 = 0.2;
/// How far out drops splash up, in wave lengths.
const SPRAY_RANGE: f64 = 0.25;

/// The open sea at (`x`, `y`) around 0.46 on a 0..1 scale (not clamped; after "Very fast
/// procedural ocean" by afl_ext; mirrors march.wgsl `water_sea01`): a sum of waves
/// exp(sin - 1), from swells longer than the wave length down to small ripples, each with
/// its own random phase, a direction around the wind (small waves spread wider) and the speed
/// of deep water waves (longer ones run faster). Every wave drags the point along its slope,
/// which sharpens the crests (`choppy`). Over large distances the crests bend (slow domain
/// warping) and wave groups make the sea calmer and rougher in turns, so a wide view never
/// repeats.
fn water_sea01(a: &[f64; 12], x: f64, y: f64) -> f64 {
    let (len, t, wind) = (a[1].max(1e-6), a[2], a[3].to_radians());
    let drag = a[4].max(0.0) * 0.38;
    // Slow bends: the points move by up to 0.8 wave lengths on a scale of 5.
    let (gx, gy) = (x / (len * 5.0), y / (len * 5.0));
    let wx = (water_noise(gx, gy) * 2.0 - 1.0) * len * 0.8;
    let wy = (water_noise(gx + 31.7, gy + 17.3) * 2.0 - 1.0) * len * 0.8;
    let freq0 = std::f64::consts::TAU / len;
    let (mut px, mut py) = ((x + wx) * freq0, (y + wy) * freq0);
    let n = water_waves(a[5]);
    // From 2.5 times the wave length down to about an eightieth of it.
    let ratio = (200f64).powf(1.0 / (n - 1).max(1) as f64);
    let (mut f, mut w, mut sum, mut weights, mut squares) = (0.4, 1.0, 0.0, 0.0, 0.0);
    for i in 0..n {
        let spread = 0.5 + 1.5 * i as f64 / n as f64;
        let ang = wind + spread * (crate::render::hash3(i, 0, 11) as f64 * 2.0 - 1.0);
        let (dx, dy) = (ang.cos(), ang.sin());
        let ph = (px * dx + py * dy) * f + t * f.sqrt() + crate::render::hash3(i, 1, 11) as f64 * std::f64::consts::TAU;
        let wave = (ph.sin() - 1.0).exp();
        let slope = -wave * ph.cos();
        px += dx * slope * w * drag;
        py += dy * slope * w * drag;
        sum += wave * w;
        weights += w;
        squares += w * w;
        w *= ratio.powf(-WATER_FALLOFF);
        f *= ratio;
    }
    // Standardized by the mean and spread of exp(sin - 1) (0.4658, 0.3025), so the height
    // keeps its range for any number of waves; wave groups grow and calm the waves over about
    // seven wave lengths.
    let z = (sum - 0.4658 * weights) / (0.3025 * squares.sqrt());
    let group = 0.45 + 0.55 * water_noise(x / (len * 7.0) + 5.1, y / (len * 7.0) + 9.3);
    0.46 + 0.14 * z * group
}

/// Phase of the shore waves where they reach the coast at (`x`, `y`): they run in by the
/// speed of their length, their crests bent along the coast by noise.
fn water_shore_phase(a: &[f64; 12], x: f64, y: f64) -> f64 {
    let len = a[1].max(1e-6);
    let bend = water_noise(x / len * 1.3 + 3.7, y / len * 1.3 + 1.9);
    a[2] * (1.0 / SHORE_LEN).sqrt() + bend * std::f64::consts::TAU
}

/// Waves running in toward the shore, `s` (local units) away from the rest of the scene
/// (mirrors march.wgsl `water_shore`): their addition to the 0..1 height and how much they
/// break there (0..1). They grow steeper and higher toward the coast and collapse right at
/// it.
fn water_shore(a: &[f64; 12], x: f64, y: f64, s: f64) -> (f64, f64) {
    let (len, strength) = (a[1].max(1e-6), a[8].clamp(0.0, 2.0));
    let range = len * SHORE_RANGE;
    if strength <= 0.0 || s >= range {
        return (0.0, 0.0);
    }
    let s = s.max(0.0);
    let near = 1.0 - s / range;
    let ph = s * std::f64::consts::TAU / (len * SHORE_LEN) + water_shore_phase(a, x, y);
    let wave = ((ph.sin() - 1.0) * (1.0 + 2.0 * near)).exp();
    let collapse = (s / (len * 0.15)).min(1.0);
    let env = near * near * (3.0 - 2.0 * near) * collapse * (2.0 - collapse);
    // Higher and lower stretches along the coast.
    let height = 0.6 + 0.4 * water_noise(x / len * 1.3 + 8.1, y / len * 1.3 + 2.3);
    (strength * 0.22 * env * height * (wave - 0.2), near * near * wave)
}

/// Height of the water at (`x`, `y`) in 0..1, `s` away from the shore (mirrors march.wgsl
/// `water_wave01`).
fn water_wave01(a: &[f64; 12], x: f64, y: f64, s: f64) -> f64 {
    (water_sea01(a, x, y) + water_shore(a, x, y, s).0).clamp(0.0, 1.0)
}

/// Height of the waves at (`x`, `y`) (mirrors march.wgsl `water_height`): `a` = wave height,
/// wave length, phase (time), wind direction (degrees), choppiness, detail, foam, reflection,
/// shore waves, spray; `s` the distance to the rest of the scene (`NO_SHORE`: none). -1..1
/// times the wave height (crests up).
pub fn water_height(a: &[f64; 12], x: f64, y: f64, s: f64) -> f64 {
    (water_wave01(a, x, y, s) * 2.0 - 1.0) * a[0]
}

/// The steepest slope the waves reach, times wave height × 2π / wave length. Measured: about
/// 1.3 without choppiness at any detail; with choppiness 1 / 2: 1.4 / 2.2 at 32 waves,
/// 2.2 / 5.2 at 64. Bound: 1.6 + choppy² × waves / 40 (checked by
/// `water_slopes_stay_below_the_distance_bound`).
fn water_steepness(a: &[f64; 12]) -> f64 {
    let c = a[4].max(0.0);
    a[0].abs() * std::f64::consts::TAU / a[1].max(1e-6) * (1.6 + c * c * water_waves(a[5]) as f64 / 40.0)
}

/// Bound of how fast the shore waves change the height per unit of shore distance or
/// position: wave height × strength / wave length × 16 (measured: 13.7; checked by
/// `water_with_a_shore_stays_a_distance_bound`).
fn water_shore_steepness(a: &[f64; 12]) -> f64 {
    a[0].abs() * a[8].clamp(0.0, 2.0) / a[1].max(1e-6) * 16.0
}

/// Distance bound of the water surface, `s` away from the rest of the scene: above every
/// crest the distance to the highest one, near the waves the height difference scaled down by
/// the steepest slope (steeper near the coast, where the shore waves run), and the spray.
pub fn water_distance(a: &[f64; 12], q: V3, s: f64) -> f64 {
    water_body_distance(a, q, s).min(water_spray(a, q, s))
}

/// [`water_distance`] without the spray: the surface of the water body (seen from inside, the
/// medium negates it).
pub fn water_body_distance(a: &[f64; 12], q: V3, s: f64) -> f64 {
    let amp = a[0].abs();
    let above = q.z - amp;
    if above > amp {
        return above;
    }
    let range = a[1].max(1e-6) * SHORE_RANGE;
    let open = (1.0 + water_steepness(a).powi(2)).sqrt();
    let dz = q.z - water_height(a, q.x, q.y, s);
    // The same bound above and below the surface (by the height difference's size): the
    // water medium seen from inside negates it.
    let h = dz.abs();
    let mut d = h / open;
    let x = s - range;
    if a[8] > 0.0 && h > x * open {
        // Within the shore waves' reach their slope (also along the shore distance, which
        // changes by at most 1 per unit) adds to the open sea's. From x beyond the reach, the
        // first x of the way to the waves there still has the open sea's slope, so they are at
        // least |dz| / steep + x (1 - open / steep) away.
        let steep = open + water_shore_steepness(a);
        d = h / steep + x.max(0.0) * (1.0 - open / steep);
    }
    let d = d.copysign(dz);
    if above > 0.0 { d.max(above) } else { d }
}

/// Sideways drift of the spray at (`x`, `y`) (mirrors march.wgsl `water_spray_drift`): a
/// direction around the wind and a distance (up to `drift`) from slow noise, so neighboring
/// drops fly together, scaled by the flight time from the shore waves' phase: out during the
/// flights, back while no drop flies. Returns the offset and the drift velocity (per cycle).
fn water_spray_drift(a: &[f64; 12], x: f64, y: f64, drift: f64) -> (f64, f64, f64, f64) {
    let len = a[1].max(1e-6);
    let (gx, gy) = (x / (len * 1.5), y / (len * 1.5));
    let ang = a[3].to_radians() + (water_noise(gx + 13.1, gy + 7.7) * 2.0 - 1.0) * 2.5;
    let far = drift * (0.4 + 0.6 * water_noise(gx + 2.9, gy + 21.4));
    let cycle = (water_shore_phase(a, x, y) - std::f64::consts::FRAC_PI_2) / std::f64::consts::TAU;
    let c = cycle - cycle.floor();
    let t = if c < 0.65 { c / 0.65 } else { (1.0 - c) / 0.35 };
    (ang.cos() * far * t, ang.sin() * far * t, ang.cos() * far / 0.65, ang.sin() * far / 0.65)
}

/// How fast the spray drift changes per unit of position: drift / wave length × 6 (measured:
/// 5.1; checked by `water_with_a_shore_stays_a_distance_bound`).
fn water_spray_warp(drift: f64, len: f64) -> f64 {
    drift / len * 6.0
}

/// Drops splashing up where the shore waves break (mirrors march.wgsl `water_spray`): one drop
/// at most in every cell of a grid on the surface, flying in an arc after each wave reaches
/// the coast: up and back down, and sideways with the drift (`water_spray_drift`, the same
/// for nearby drops), smaller farther from the coast. `a[10]` scales the drops, `a[11]` how
/// much their sizes vary. The drift moves the grid (a smooth warp, so the bound divides by
/// 1 + its rate of change). The cell of the warped point and its eight neighbors are checked;
/// drops beyond them are at least the distance to that block's edge away (drops keep a margin
/// inside their cell).
fn water_spray(a: &[f64; 12], q: V3, s: f64) -> f64 {
    let (amp, len, amount) = (a[0].abs().max(1e-9), a[1].max(1e-6), a[9].clamp(0.0, 2.0));
    if amount <= 0.0 || a[8] <= 0.0 {
        return NO_SHORE;
    }
    let (size, vary) = (a[10].clamp(0.2, 3.0), a[11].clamp(0.0, 1.0));
    let range = len * SPRAY_RANGE;
    let (top, cell, r0) = (amp * 3.0 * amount, amp * 0.6 * size.max(1.0), amp * 0.07 * size);
    let drift = top * 0.6;
    // Outside the zone of the drops (they drift up to `drift` beyond the spray range): at least
    // the distance to it, and never less than the bound from the cells (at most 1.6 cells), so
    // the zone's edge never looks like a surface.
    let outside = (s - range - drift).max(q.z - top - r0);
    if outside >= cell * 1.6 {
        return outside;
    }
    let (wx, wy, vx, vy) = water_spray_drift(a, q.x, q.y, drift);
    let p = v3(q.x - wx, q.y - wy, q.z);
    let (fx, fy) = ((p.x / cell).floor(), (p.y / cell).floor());
    let margin = cell * 0.1;
    let mut d = cell * 1.5 - (p.x - (fx + 0.5) * cell).abs().max((p.y - (fy + 0.5) * cell).abs()) + margin;
    // The radius shrinks with the shore distance (squared): at most 2 r0 / range per unit.
    let fall = (1.0 - s / range).clamp(0.0, 1.0).powi(2);
    let lip = 1.0 + 2.0 * r0 / range;
    for k in 0..9 {
        let (i, j) = (fx as i32 + k % 3 - 1, fy as i32 + k / 3 - 1);
        let h = |n: i32| crate::render::hash3(i, j, n) as f64;
        if h(31) > 0.55 * amount.min(1.0) {
            continue;
        }
        let (cx, cy) = ((i as f64 + 0.5) * cell, (j as f64 + 0.5) * cell);
        // The flight starts when a crest reaches the coast and lasts half a wave period.
        let cycle = (water_shore_phase(a, cx, cy) - std::f64::consts::FRAC_PI_2) / std::f64::consts::TAU;
        let u = (cycle - cycle.floor() - 0.15 * h(32)) / 0.5;
        if !(0.0..1.0).contains(&u) {
            continue;
        }
        let reach = cell * 0.5 - r0 - margin;
        let height = top * (0.35 + 0.65 * h(35));
        let c = v3(cx + (h(33) * 2.0 - 1.0) * reach, cy + (h(34) * 2.0 - 1.0) * reach, height * 4.0 * u * (1.0 - u));
        let r = r0 * (1.0 - vary * h(36)) * (1.0 - 0.5 * u) * fall;
        // Stretched along the flight while fast: an ellipsoid along the velocity (sideways
        // drift and the rise or fall), whose bound scales that component down.
        let v = v3(vx, vy, height * 4.0 * (1.0 - 2.0 * u) / 0.5).normalize();
        let stretch = 1.0 + 1.2 * (1.0 - 2.0 * u).abs();
        let e = p - c;
        let e = e - v * (e.dot(v) * (1.0 - 1.0 / stretch));
        d = d.min((e.length() - r) / lip);
    }
    (d / (1.0 + water_spray_warp(drift, len))).max(outside)
}

/// How water reflects light arriving at the cosine `cos_i` to its normal (Schlick, water's
/// 2 % straight on), scaled by `strength` (mirrors march.wgsl `water_fresnel`).
pub fn water_fresnel(cos_i: f64, strength: f64) -> f64 {
    let x = (1.0 - cos_i.clamp(0.0, 1.0)).powi(5);
    ((0.02 + 0.98 * x) * strength.max(0.0)).min(1.0)
}

/// Share of light reflected by a smooth surface between refraction indices (`eta` = index on
/// the ray's side / index beyond) at the cosine `cos_i` (exact Fresnel, unpolarized; 1 at total
/// internal reflection; mirrors march.wgsl `fresnel_exact`).
pub fn fresnel_exact(cos_i: f64, eta: f64) -> f64 {
    let ci = cos_i.clamp(0.0, 1.0);
    let k = 1.0 - eta * eta * (1.0 - ci * ci);
    if k <= 0.0 {
        return 1.0;
    }
    let ct = k.sqrt();
    let rs = (ci - eta * ct) / (ci + eta * ct);
    let rp = (ct - eta * ci) / (ct + eta * ci);
    ((rs * rs + rp * rp) * 0.5).min(1.0)
}

/// Value noise 0..1 on the integer grid (mirrors march.wgsl `water_noise`).
fn water_noise(x: f64, y: f64) -> f64 {
    let (ix, iy) = (x.floor(), y.floor());
    let (fx, fy) = (x - ix, y - iy);
    let (ux, uy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
    let h = |dx: i32, dy: i32| crate::render::hash3(ix as i32 + dx, iy as i32 + dy, 7) as f64;
    let (a, b) = (h(0, 0) + (h(1, 0) - h(0, 0)) * ux, h(0, 1) + (h(1, 1) - h(0, 1)) * ux);
    a + (b - a) * uy
}

/// The look of water at the local point `q`, `s` away from the rest of the scene (mirrors
/// march.wgsl `water_look`): its height 0..1 (crests brighten the water: light passing
/// through them) and the foam share. Foam: on the highest crests by `a[6]`, along the coast
/// and where the shore waves break (by the shore strength), broken up by noise that drifts
/// with the wind; drops are white water.
pub fn water_look(a: &[f64; 12], q: V3, s: f64) -> (f64, f64) {
    let (sea, (lift, breaking)) = (water_sea01(a, q.x, q.y), water_shore(a, q.x, q.y, s));
    let h = (sea + lift).clamp(0.0, 1.0);
    let len = a[1].max(1e-6);
    // A drop: nearer to one than to the waves below.
    if water_spray(a, q, s) < q.z - (h * 2.0 - 1.0) * a[0] {
        return (1.0, 0.6);
    }
    // The top ~5 % of the heights (90 % lie below 0.61, 98 % below 0.70).
    let crest = ((h - 0.62) / 0.16).clamp(0.0, 1.0);
    let crest_foam = crest * crest * a[6].max(0.0);
    let strength = a[8].clamp(0.0, 1.0);
    let mut shore_foam = 0.0;
    if strength > 0.0 && s < len * SHORE_RANGE {
        let near = 1.0 - s.max(0.0) / (len * SHORE_RANGE);
        let band = len * 0.08 * (0.4 + 1.2 * water_noise(q.x / len * 4.0, q.y / len * 4.0));
        let line = 1.0 - s.max(0.0) / band;
        shore_foam = strength * line.max((breaking - 0.25) / 0.35).max(near.powi(3) * 0.7).clamp(0.0, 1.0);
    }
    if crest_foam.max(shore_foam) <= 0.0 {
        return (h, 0.0);
    }
    let f = std::f64::consts::TAU / len;
    let wind = a[3].to_radians();
    let (u, v) = ((q.x * wind.cos() + q.y * wind.sin()) * f - a[2] * 0.5, (q.y * wind.cos() - q.x * wind.sin()) * f);
    // Streaks along the wind: the noise is stretched along it.
    let n = 0.6 * water_noise(u * 1.5, v * 4.0) + 0.4 * water_noise(u * 4.0, v * 11.0);
    let broken = ((n - 0.3) / 0.35).clamp(0.0, 1.0);
    (h, (crest_foam.max(shore_foam) * broken).min(1.0))
}

/// Position, rotation (degrees about x, then y, then z), uniform scale and stretch along the
/// object's own axes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Transform {
    pub position: V3,
    pub rotation: [f64; 3],
    pub scale: f64,
    /// Extra scale along the object's x, y and z axes (on top of `scale`).
    pub stretch: [f64; 3],
}

impl Default for Transform {
    fn default() -> Self {
        Transform { position: V3::ZERO, rotation: [0.0; 3], scale: 1.0, stretch: [1.0; 3] }
    }
}

impl Transform {
    /// No move, turn, scale or stretch.
    pub fn is_identity(&self) -> bool {
        *self == Transform::default()
    }

    /// Rows of the inverse rotation (world → local).
    pub fn inverse_rotation(&self) -> [[f64; 3]; 3] {
        let [x, y, z] = self.rotation.map(f64::to_radians);
        let (sx, cx) = x.sin_cos();
        let (sy, cy) = y.sin_cos();
        let (sz, cz) = z.sin_cos();
        // R = Rz · Ry · Rx; its inverse is the transpose.
        let r = [
            [cz * cy, cz * sy * sx - sz * cx, cz * sy * cx + sz * sx],
            [sz * cy, sz * sy * sx + cz * cx, sz * sy * cx - cz * sx],
            [-sy, cy * sx, cy * cx],
        ];
        [[r[0][0], r[1][0], r[2][0]], [r[0][1], r[1][1], r[2][1]], [r[0][2], r[1][2], r[2][2]]]
    }

    /// Rows of the rotation (local → world): the transpose of `inverse_rotation`.
    pub fn rotation_matrix(&self) -> [[f64; 3]; 3] {
        let i = self.inverse_rotation();
        [[i[0][0], i[1][0], i[2][0]], [i[0][1], i[1][1], i[2][1]], [i[0][2], i[1][2], i[2][2]]]
    }

    /// Sets `rotation` from the rows of a rotation matrix (R = Rz · Ry · Rx).
    pub fn set_rotation_matrix(&mut self, r: [[f64; 3]; 3]) {
        let sy = (-r[2][0]).clamp(-1.0, 1.0);
        let y = sy.asin();
        let (x, z) = if sy.abs() < 1.0 - 1e-9 {
            (r[2][1].atan2(r[2][2]), r[1][0].atan2(r[0][0]))
        } else {
            // Gimbal lock: only x - z (or x + z) is defined; keep x at zero.
            (0.0, (-r[0][1]).atan2(r[1][1]))
        };
        // No negative zeros or rounding dust in the fields.
        self.rotation = [x, y, z].map(|v| if v.abs() < 1e-9 { 0.0 } else { v.to_degrees() });
    }

    pub fn scale(&self) -> f64 {
        self.scale.abs().max(1e-9)
    }

    /// Scale along the object's axes: `scale` times `stretch`.
    pub fn size(&self) -> V3 {
        let s = self.stretch.map(|v| v.abs().max(1e-6));
        v3(s[0], s[1], s[2]) * self.scale()
    }
}

/// World point `p` in the coordinates of an object with inverse rotation `inv`, `pos` and
/// `size` (scale per own axis).
pub fn to_local(inv: &[[f64; 3]; 3], pos: V3, size: V3, p: V3) -> V3 {
    let r = rotate(inv, p - pos);
    v3(r.x / size.x, r.y / size.y, r.z / size.z)
}

/// A local point folded into the copy around the origin (repetition: `cell` spacing per axis,
/// 0 = none; `limit` copies on each side, 0 = endless). Same on the GPU (`obj_local`).
pub fn repeat_fold(q: V3, cell: [f64; 3], limit: [u32; 3]) -> V3 {
    let fold = |x: f64, c: f64, n: u32| {
        if c <= 0.0 {
            return x;
        }
        let k = (x / c + 0.5).floor();
        let k = if n > 0 { k.clamp(-(n as f64), n as f64) } else { k };
        x - c * k
    };
    v3(fold(q.x, cell[0], limit[0]), fold(q.y, cell[1], limit[1]), fold(q.z, cell[2], limit[2]))
}

/// `v` times the matrix with rows `m`.
pub fn rotate(m: &[[f64; 3]; 3], v: V3) -> V3 {
    v3(m[0][0] * v.x + m[0][1] * v.y + m[0][2] * v.z, m[1][0] * v.x + m[1][1] * v.y + m[1][2] * v.z, m[2][0] * v.x + m[2][1] * v.y + m[2][2] * v.z)
}

/// A world normal in an object's coordinates (normals scale inversely to points).
pub fn normal_to_local(inv: &[[f64; 3]; 3], size: V3, n: V3) -> V3 {
    let r = rotate(inv, n);
    v3(r.x * size.x, r.y * size.y, r.z * size.z).normalize()
}

/// An object's normal in world coordinates (the inverse of `normal_to_local`).
pub fn normal_to_world(inv: &[[f64; 3]; 3], size: V3, n: V3) -> V3 {
    let m = v3(n.x / size.x, n.y / size.y, n.z / size.z);
    // The transpose undoes the inverse rotation.
    v3(inv[0][0] * m.x + inv[1][0] * m.y + inv[2][0] * m.z, inv[0][1] * m.x + inv[1][1] * m.y + inv[2][1] * m.z, inv[0][2] * m.x + inv[1][2] * m.y + inv[2][2] * m.z).normalize()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum BoolOp {
    #[default]
    Union,
    /// Cuts the object out of everything before it.
    Subtract,
    /// Keeps only what is inside both.
    Intersect,
    SmoothUnion,
    SmoothSubtract,
    SmoothIntersect,
}

impl BoolOp {
    pub const ALL: [BoolOp; 6] = [BoolOp::Union, BoolOp::Subtract, BoolOp::Intersect, BoolOp::SmoothUnion, BoolOp::SmoothSubtract, BoolOp::SmoothIntersect];

    pub fn label(self) -> &'static str {
        match self {
            BoolOp::Union => "Add",
            BoolOp::Subtract => "Cut away",
            BoolOp::Intersect => "Intersect",
            BoolOp::SmoothUnion => "Add smoothly",
            BoolOp::SmoothSubtract => "Cut away smoothly",
            BoolOp::SmoothIntersect => "Intersect smoothly",
        }
    }

    pub fn smooth(self) -> bool {
        matches!(self, BoolOp::SmoothUnion | BoolOp::SmoothSubtract | BoolOp::SmoothIntersect)
    }

    pub fn index(self) -> u32 {
        self as u32
    }
}

/// Combines the distance so far `a` with an object's distance `b`; true when the object forms
/// the surface there (its material colors it).
pub fn combine(op: u32, a: f64, b: f64, k: f64) -> (f64, bool) {
    let k = k.max(1e-12);
    let smin = |a: f64, b: f64| {
        let h = (0.5 + 0.5 * (b - a) / k).clamp(0.0, 1.0);
        (b + (a - b) * h - k * h * (1.0 - h), h < 0.5)
    };
    match op {
        0 => (a.min(b), b < a),
        1 => (a.max(-b), -b > a),
        2 => (a.max(b), b > a),
        3 => smin(a, b),
        4 => {
            let (d, w) = smin(-a, b);
            (-d, w)
        }
        _ => {
            let (d, w) = smin(-a, -b);
            (-d, w)
        }
    }
}

/// How a primitive picks its gradient color.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Pattern {
    /// The color at the gradient offset.
    #[default]
    Solid,
    /// Alternating squares: the gradient's ends.
    Checker,
    /// Along the local height.
    Height,
    /// With the distance from the object's center.
    Radial,
    /// 3D models: the colors of the model file (vertex colors, material colors, textures).
    Model,
}

impl Pattern {
    pub const ALL: [Pattern; 4] = [Pattern::Solid, Pattern::Checker, Pattern::Height, Pattern::Radial];
    /// The patterns of 3D models.
    pub const FOR_MODELS: [Pattern; 5] = [Pattern::Model, Pattern::Solid, Pattern::Checker, Pattern::Height, Pattern::Radial];

    pub fn label(self) -> &'static str {
        match self {
            Pattern::Solid => "Solid",
            Pattern::Checker => "Checker",
            Pattern::Height => "By height",
            Pattern::Radial => "From the center",
            Pattern::Model => "Model colors",
        }
    }

    pub fn index(self) -> u32 {
        self as u32
    }
}

/// Color value (gradient position before scale and offset) of a primitive at local `q`.
pub fn pattern_value(pattern: u32, size: f64, q: V3) -> f64 {
    let s = size.abs().max(1e-9);
    match pattern {
        0 => 0.0,
        1 => {
            // Cells centered on the surface (z = 0 stays in one cell for the ground).
            let c = (q.x / s).floor() + (q.y / s).floor() + (q.z / s + 0.5).floor();
            c.rem_euclid(2.0)
        }
        2 => q.z / s,
        3 => q.length() / s,
        // Model colors (the gradient's offset color where the model has none).
        _ => 0.0,
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ObjectMaterial {
    /// Use these colors (else the main material's).
    pub own: bool,
    pub gradient: Vec<ColorStop>,
    /// Glowing stripes over the gradient.
    pub bands: Vec<crate::gradient::GlowBand>,
    /// Animation: the gradient turns by this many lengths per second (moves `color_offset`).
    pub rotation: f64,
    /// Fractals: which value of the orbit picks the color.
    pub color_mode: ColorMode,
    pub color_scale: f64,
    pub color_offset: f64,
    pub repeat: bool,
    /// Primitives: how the color is picked.
    pub pattern: Pattern,
    /// Size of the pattern (checker cells, height and radius per gradient length).
    pub pattern_size: f64,
    /// Texture layers over the colors, bottom first.
    pub layers: Vec<crate::texture::TextureLayer>,
}

impl Default for ObjectMaterial {
    fn default() -> Self {
        ObjectMaterial {
            own: true,
            gradient: crate::gradient::preset("Bone").unwrap_or_default(),
            bands: Vec::new(),
            rotation: 0.0,
            color_mode: ColorMode::Iterations,
            color_scale: 0.15,
            color_offset: 0.0,
            repeat: true,
            pattern: Pattern::Solid,
            pattern_size: 1.0,
            layers: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primitive_distances_are_exact_or_safe() {
        let sphere = Primitive::Sphere { radius: 0.5 };
        assert!((sphere.distance(v3(2.0, 0.0, 0.0)) - 1.5).abs() < 1e-12);
        let bx = Primitive::Box { size: [2.0, 2.0, 2.0], rounding: 0.0 };
        assert!((bx.distance(v3(3.0, 0.0, 0.0)) - 2.0).abs() < 1e-12);
        assert!((bx.distance(v3(2.0, 2.0, 0.0)) - 2f64.sqrt()).abs() < 1e-12);
        assert!(bx.distance(V3::ZERO) < 0.0);
        let cyl = Primitive::Cylinder { radius: 1.0, height: 2.0, rounding: 0.0 };
        assert!((cyl.distance(v3(3.0, 0.0, 0.0)) - 2.0).abs() < 1e-12);
        assert!((cyl.distance(v3(0.0, 0.0, 3.0)) - 2.0).abs() < 1e-12);
        let torus = Primitive::Torus { major: 1.0, minor: 0.25 };
        assert!((torus.distance(v3(1.0, 0.0, 1.0)) - 0.75).abs() < 1e-12);
        assert!((Primitive::Plane.distance(v3(5.0, 3.0, 2.0)) - 2.0).abs() < 1e-12);
        let cap = Primitive::Capsule { radius: 0.5, height: 3.0 };
        assert!((cap.distance(v3(0.0, 0.0, 3.0)) - 1.5).abs() < 1e-12);
        assert!((cap.distance(v3(2.0, 0.0, 0.3)) - 1.5).abs() < 1e-12);
        let cone = Primitive::Cone { bottom: 1.0, top: 0.0, height: 2.0 };
        assert!((cone.distance(v3(0.0, 0.0, -3.0)) - 2.0).abs() < 1e-12);
        assert!((cone.distance(v3(0.0, 0.0, 2.0)) - 1.0).abs() < 1e-12);
        assert!(cone.distance(v3(0.0, 0.0, 0.0)) < 0.0);
        let hex = Primitive::HexPrism { radius: 1.0, height: 2.0 };
        assert!((hex.distance(v3(0.0, 3.0, 0.0)) - 2.0).abs() < 1e-12);
        assert!((hex.distance(v3(0.0, 0.0, 4.0)) - 3.0).abs() < 1e-12);
        let tri = Primitive::TriPrism { side: 2.0, height: 2.0 };
        // The side opposite the top corner lies 1/sqrt(3) below the center.
        assert!((tri.distance(v3(0.0, -1.0 / 3f64.sqrt() - 1.0, 0.0)) - 1.0).abs() < 1e-12);
        assert!((tri.distance(v3(0.0, 0.0, 3.0)) - 2.0).abs() < 1e-12);
        // Never more than the true distance: sample points against a dense surface sampling.
        for p in [cap, cone, hex, tri, Primitive::Cone { bottom: 0.3, top: 0.8, height: 1.5 }] {
            let surface: Vec<V3> = (0..40_000)
                .map(|i| {
                    let (u, v, w) = ((i % 40) as f64 / 39.0, ((i / 40) % 40) as f64 / 39.0, (i / 1600) as f64 / 24.0);
                    v3(u * 4.0 - 2.0, v * 4.0 - 2.0, w * 4.0 - 2.0)
                })
                .filter(|q| p.distance(*q).abs() < 0.03)
                .collect();
            for i in 0..50 {
                let q = v3((i as f64 * 0.37).sin() * 3.0, (i as f64 * 0.73).cos() * 3.0, (i as f64 * 0.51).sin() * 3.0);
                let d = p.distance(q);
                let nearest = surface.iter().map(|s| (*s - q).length()).fold(f64::MAX, f64::min);
                assert!(d.abs() <= nearest + 0.05, "{p:?} at {q:?}: {d} vs {nearest}");
            }
        }
        // Water: never more than the vertical distance to the surface.
        let w = Primitive::Water { amplitude: 0.05, wavelength: 0.3, phase: 0.4, angle: 20.0, choppy: 1.0, detail: 16.0, foam: 0.3, reflect: 1.0, speed: 0.0, shore: 1.0, spray: 1.0, drop_size: 1.0, drop_variation: 0.5, medium: WaterMedium::default() };
        let a = w.params();
        for i in 0..200 {
            let (x, y) = (i as f64 * 0.013, i as f64 * 0.007);
            let h = water_height(&a, x, y, NO_SHORE);
            let d = w.distance(v3(x, y, 0.5));
            assert!(d <= 0.5 - h + 1e-12 && d > 0.0);
        }
    }

    #[test]
    fn water_slopes_stay_below_the_distance_bound() {
        // The bound divides by the steepest slope: measure the real slopes.
        for (amp, len, choppy, detail) in [(0.05, 0.3, 1.0, 24.0), (0.2, 1.0, 0.0, 8.0), (0.02, 0.5, 2.0, 64.0), (0.1, 2.0, 1.5, 32.0)] {
            let a = [amp, len, 0.7, 25.0, choppy, detail, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0];
            let steep = water_steepness(&a);
            let e = len * 1e-5;
            let mut worst: f64 = 0.0;
            for i in 0..20000 {
                let (x, y) = ((i as f64 * 0.6180339) % 1.0 * len * 30.0, (i as f64 * 0.7548776) % 1.0 * len * 30.0);
                let gx = (water_height(&a, x + e, y, NO_SHORE) - water_height(&a, x - e, y, NO_SHORE)) / (2.0 * e);
                let gy = (water_height(&a, x, y + e, NO_SHORE) - water_height(&a, x, y - e, NO_SHORE)) / (2.0 * e);
                worst = worst.max((gx * gx + gy * gy).sqrt());
            }
            assert!(worst < steep, "slope {worst} above the bound {steep} (ratio {:.2}; amp {amp}, len {len}, choppy {choppy})", worst / (amp * std::f64::consts::TAU / len));
            let range = (0..2000).map(|i| water_height(&a, i as f64 * 0.0137, i as f64 * 0.0071, NO_SHORE) / amp).fold((f64::MAX, f64::MIN), |r, h| (r.0.min(h), r.1.max(h)));
            assert!(range.0 >= -1.0 - 1e-9 && range.1 <= 1.0 + 1e-9, "{range:?}");
        }
    }


    #[test]
    fn water_with_a_shore_stays_a_distance_bound() {
        // A sphere in the water as the rest of the scene. No point within the distance bound
        // of a point lies below the surface or inside a drop (the sign of the distance tells).
        let shore = |q: V3| q.length() - 0.4;
        let mut worst_slope: f64 = 0.0;
        let mut worst_warp: f64 = 0.0;
        for (amp, len, strength, spray, t, size, vary) in [(0.08, 1.0, 1.0, 1.0, 0.3, 1.0, 0.5), (0.05, 0.5, 2.0, 2.0, 1.7, 3.0, 1.0), (0.1, 2.0, 2.0, 1.0, 4.1, 0.2, 0.0), (0.03, 0.4, 0.5, 2.0, 2.6, 1.5, 0.8)] {
            let a = [amp, len, t, 25.0, 1.0, 24.0, 0.3, 1.0, strength, spray, size, vary];
            let d_at = |q: V3| water_distance(&a, q, shore(q));
            let mut drops = 0;
            for i in 0..6000 {
                let r = 0.4 + (i as f64 * 0.6180339) % 1.0 * len * 1.8;
                let ang = (i as f64 * 0.7548776) % 1.0 * std::f64::consts::TAU;
                let z = -amp + (i as f64 * 0.5698403) % 1.0 * amp * 7.0;
                let p = v3(r * ang.cos(), r * ang.sin(), z);
                let d = d_at(p);
                // Drops: dense samples in the spray zone.
                let zone = v3(ang.cos(), ang.sin(), 0.0) * (0.4 + (i as f64 * 0.3819660) % 1.0 * len * SPRAY_RANGE) + v3(0.0, 0.0, z.max(0.0) * 0.5);
                for k in 0..32 {
                    let q = zone + v3(crate::render::hash3(i, k, 6) as f64 - 0.5, crate::render::hash3(i, k, 7) as f64 - 0.5, 0.0) * (amp * 0.9);
                    // Without a drop in the 3 × 3 cells the bound is above a cell.
                    if water_spray(&a, q, shore(q)) < amp * 0.3 {
                        drops += 1;
                    }
                }
                if d <= 0.0 {
                    continue;
                }
                for k in 0..24 {
                    let u = crate::render::hash3(i, k, 3) as f64;
                    let (th, ph) = (crate::render::hash3(i, k, 4) as f64 * std::f64::consts::TAU, (crate::render::hash3(i, k, 5) as f64 * 2.0 - 1.0).acos());
                    let v = v3(ph.sin() * th.cos(), ph.sin() * th.sin(), ph.cos()) * (d * 0.999 * u.cbrt());
                    let e = d_at(p + v);
                    assert!(e > 0.0, "amp {amp} len {len}: {p:?} bound {d}, {:?} at {} has {e}", p + v, v.length());
                }
                // Straight down: the surface's own slope bound.
                let below = d_at(p - v3(0.0, 0.0, d * 0.999));
                assert!(below > 0.0, "{p:?} bound {d}: below {below}");
            }
            assert!(drops > 0, "no drops at amp {amp} len {len}");
            // Shore waves change the height at most by water_shore_steepness per unit.
            let steep = water_shore_steepness(&a);
            let e = len * 1e-5;
            for i in 0..20000 {
                let (x, y, s) = ((i as f64 * 0.6180339) % 1.0 * len * 10.0, (i as f64 * 0.7548776) % 1.0 * len * 10.0, (i as f64 * 0.5698403) % 1.0 * len * SHORE_RANGE);
                let shore_h = |x: f64, y: f64, s: f64| water_shore(&a, x, y, s).0 * 2.0 * amp;
                let gs = (shore_h(x, y, s + e) - shore_h(x, y, (s - e).max(0.0))) / (s + e - (s - e).max(0.0));
                let gx = (shore_h(x + e, y, s) - shore_h(x - e, y, s)) / (2.0 * e);
                let gy = (shore_h(x, y + e, s) - shore_h(x, y - e, s)) / (2.0 * e);
                worst_slope = worst_slope.max((gs.abs() + (gx * gx + gy * gy).sqrt()) / steep);
            }
            // The spray drift changes at most by water_spray_warp per unit (any direction).
            let drift = amp * 3.0 * spray * 0.6;
            let warp = water_spray_warp(drift, len);
            for i in 0..20000 {
                let (x, y) = ((i as f64 * 0.6180339) % 1.0 * len * 10.0, (i as f64 * 0.7548776) % 1.0 * len * 10.0);
                let w = |x: f64, y: f64| {
                    let (wx, wy, _, _) = water_spray_drift(&a, x, y, drift);
                    v3(wx, wy, 0.0)
                };
                let (gx, gy) = ((w(x + e, y) - w(x - e, y)) * (0.5 / e), (w(x, y + e) - w(x, y - e)) * (0.5 / e));
                // Norm of the 2 × 2 Jacobian, bounded by its Frobenius norm.
                worst_warp = worst_warp.max((gx.dot(gx) + gy.dot(gy)).sqrt() / warp);
            }
        }
        assert!(worst_slope < 1.0, "shore slope {worst_slope} of the bound");
        assert!(worst_warp < 1.0, "spray warp {worst_warp} of the bound");
    }

    #[test]
    fn rotation_matrix_round_trip() {
        for rot in [[10.0, 20.0, 30.0], [-170.0, 45.0, 95.0], [0.0, 0.0, -55.0], [30.0, 89.0, 0.0]] {
            let t = Transform { rotation: rot, ..Default::default() };
            let mut u = Transform::default();
            u.set_rotation_matrix(t.rotation_matrix());
            let (a, b) = (t.rotation_matrix(), u.rotation_matrix());
            for r in 0..3 {
                for c in 0..3 {
                    assert!((a[r][c] - b[r][c]).abs() < 1e-9, "{rot:?} -> {:?}", u.rotation);
                }
            }
        }
        // Stretch: points scale per own axis, normals inversely.
        let t = Transform { stretch: [2.0, 1.0, 0.5], ..Default::default() };
        let (inv, size) = (t.inverse_rotation(), t.size());
        assert_eq!(to_local(&inv, V3::ZERO, size, v3(2.0, 1.0, 0.5)), v3(1.0, 1.0, 1.0));
        let n = normal_to_local(&inv, size, v3(1.0, 1.0, 0.0).normalize());
        let back = normal_to_world(&inv, size, n);
        assert!((back - v3(1.0, 1.0, 0.0).normalize()).length() < 1e-12);
    }

    #[test]
    fn transform_moves_rotates_and_scales() {
        let t = Transform { position: v3(1.0, 2.0, 3.0), rotation: [0.0, 0.0, 90.0], scale: 2.0, ..Default::default() };
        let inv = t.inverse_rotation();
        // The world point one unit along +y from the position is local +x (rotated 90° about z)
        // at half the distance (scale 2).
        let q = to_local(&inv, t.position, t.size(), v3(1.0, 4.0, 3.0));
        assert!((q - v3(1.0, 0.0, 0.0)).length() < 1e-12, "{q:?}");
        // Rotations stay orthonormal.
        let t2 = Transform { rotation: [30.0, -50.0, 120.0], ..Default::default() };
        let m = t2.inverse_rotation();
        for i in 0..3 {
            for j in 0..3 {
                let d: f64 = (0..3).map(|k| m[i][k] * m[j][k]).sum();
                assert!((d - if i == j { 1.0 } else { 0.0 }).abs() < 1e-12);
            }
        }
    }

    #[test]
    fn operations_and_which_object_forms_the_surface() {
        assert_eq!(combine(0, 1.0, 0.5, 0.0), (0.5, true));
        assert_eq!(combine(0, 0.2, 0.5, 0.0), (0.2, false));
        // Subtract: inside the cutter the cut face belongs to it.
        assert_eq!(combine(1, -1.0, -0.3, 0.0), (0.3, true));
        assert_eq!(combine(2, 0.1, 0.4, 0.0), (0.4, true));
        // Smooth union is below both near the seam and equals the plain union far away.
        let (d, _) = combine(3, 0.05, 0.05, 0.2);
        assert!(d < 0.05);
        assert!((combine(3, 1.0, 5.0, 0.2).0 - 1.0).abs() < 1e-12);
    }
}

#[cfg(test)]
mod scene_tests {
    use super::*;
    use crate::Scene;

    fn with_objects() -> Scene {
        let mut s = Scene::default();
        s.objects.push(SceneObject { name: "Ball".into(), transform: Transform { position: v3(3.0, 0.0, 0.0), scale: 2.0, ..Default::default() }, ..Default::default() });
        s.objects.push(SceneObject { name: "Ground".into(), shape: Shape::Primitive(Primitive::Plane), transform: Transform { position: v3(0.0, 0.0, -1.5), ..Default::default() }, ..Default::default() });
        s
    }

    #[test]
    fn objects_join_the_distance_estimate() {
        let s = with_objects();
        let prog = crate::Program::for_scene(&s);
        assert_eq!(prog.objects.len(), 2);
        let mut ev = prog.evaluator();
        // Near the ball (radius 0.5 · scale 2 = 1 around x = 3): its distance and its id.
        let o = ev.orbit(v3(4.5, 0.0, 0.0), 1e-6);
        assert!((o.de - 0.5).abs() < 1e-9, "{}", o.de);
        assert_eq!(o.object, 1);
        // Above the ground, away from everything else.
        let o = ev.orbit(v3(-6.0, 6.0, -1.0), 1e-6);
        assert!((o.de - 0.5).abs() < 1e-9 && o.object == 2, "{o:?}");
        // The bulb itself stays object 0.
        assert_eq!(ev.orbit(v3(1.3, 0.0, 0.0), 1e-6).object, 0);
    }

    #[test]
    fn subtracting_a_sphere_carves_the_bulb() {
        let mut s = Scene::default();
        s.objects.push(SceneObject { op: BoolOp::Subtract, shape: Shape::Primitive(Primitive::Sphere { radius: 0.6 }), ..Default::default() });
        let prog = crate::Program::for_scene(&s);
        let mut ev = prog.evaluator();
        // The center of the bulb is now empty: 0.6 from the cut face.
        let o = ev.orbit(V3::ZERO, 1e-6);
        assert!((o.de - 0.6).abs() < 1e-9 && o.object == 1, "{o:?}");
    }

    #[test]
    fn composed_scene_round_trips_and_old_scenes_load() {
        let s = with_objects();
        let back = Scene::from_json(&s.to_json()).unwrap();
        assert_eq!(back.objects, s.objects);
        let old = Scene::from_json(&Scene::default().to_json().replace("\"objects\": [],", "")).unwrap();
        assert!(old.objects.is_empty());
    }

    #[test]
    fn objects_render_in_their_own_material() {
        // A red ball in front of the camera: its pixels are red.
        let mut s = Scene::default();
        let cam = s.camera.clone();
        let mut ball = SceneObject { transform: Transform { position: cam.position + cam.forward * 1.2, ..Default::default() }, ..Default::default() };
        ball.material.gradient = vec![crate::scene::ColorStop::new(0.0, [1.0, 0.0, 0.0]), crate::scene::ColorStop::new(1.0, [1.0, 0.0, 0.0])];
        s.objects.push(ball);
        let mut r = crate::render::cpu::CpuRenderer::new(s, 16, 16);
        r.render_pass();
        let px = r.rgba8();
        let c = &px[(8 * 16 + 8) * 4..(8 * 16 + 8) * 4 + 3];
        assert!(c[0] > 60 && c[1] < c[0] / 3 && c[2] < c[0] / 3, "{c:?}");
    }
}


