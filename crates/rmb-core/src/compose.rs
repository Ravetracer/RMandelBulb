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

/// Most objects that act as bubble sources (march.wgsl `u.wmed[10..14]`).
pub const MAX_BUBBLE_SOURCES: usize = 8;

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
    /// Bubbles rise from this object in a plume (water medium with bubbles from vents).
    pub bubble_source: bool,
    /// Seabed: plants grown by L-systems (at most `plants::MAX_SPECIES` enabled).
    pub plants: Vec<crate::plants::PlantSpecies>,
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
            bubble_source: false,
            plants: Vec::new(),
        }
    }
}

// Scenes hold a handful of objects: the water's settings inline cost nothing worth a box.
#[allow(clippy::large_enum_variant)]
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
// The water carries its medium settings; a scene has at most a few primitives, so the size
// does not matter and the type stays Copy.
#[allow(clippy::large_enum_variant)]
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
    /// An ocean floor at z = 0 (everything below is solid): rolling hills from noise and sand
    /// ripples across `angle` (degrees), `ripple_length` apart (`seabed_height`), with rocks,
    /// pebbles and swaying seaweed on it (`seabed_parts`). `phase` is the time of the
    /// swaying; animations add `sway_speed` per second.
    Seabed {
        hills: f64,
        hill_size: f64,
        ripples: f64,
        ripple_length: f64,
        angle: f64,
        /// Share of the ground with a rock (0 = none, 1 = many).
        #[serde(default)]
        rocks: f64,
        /// Size of the largest rocks (local units).
        #[serde(default = "seabed_rock_size")]
        rock_size: f64,
        /// Small stones (0 = none, 1 = many).
        #[serde(default)]
        pebbles: f64,
        /// Seaweed in meadows (0 = none, 1 = dense).
        #[serde(default)]
        seaweed: f64,
        /// Height of the tallest seaweed (local units).
        #[serde(default = "seabed_weed_height")]
        weed_height: f64,
        /// How far the seaweed sways (0 = still, 1 = strongly).
        #[serde(default = "seabed_sway")]
        sway: f64,
        /// Swaying per second in animations.
        #[serde(default = "seabed_sway_speed")]
        sway_speed: f64,
        #[serde(default)]
        phase: f64,
    },
}

impl Primitive {
    /// Names by [`Primitive::index`].
    pub const NAMES: [&'static str; 11] = ["Plane", "Box", "Sphere", "Cylinder", "Torus", "Water", "Capsule", "Cone", "Hexagonal prism", "Triangular prism", "Seabed"];

    /// The primitives with sensible start values (in menu order).
    pub fn defaults() -> [Primitive; 11] {
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
            Primitive::seabed(),
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
            Primitive::Seabed { .. } => SEABED_KIND,
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

    /// An ocean floor with good start values.
    pub fn seabed() -> Primitive {
        Primitive::Seabed {
            hills: 0.25,
            hill_size: 2.5,
            ripples: 0.015,
            ripple_length: 0.12,
            angle: 0.0,
            rocks: 0.35,
            rock_size: seabed_rock_size(),
            pebbles: 0.5,
            seaweed: 0.3,
            weed_height: seabed_weed_height(),
            sway: seabed_sway(),
            sway_speed: seabed_sway_speed(),
            phase: 0.0,
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
            Primitive::Seabed { hills, hill_size, ripples, ripple_length, angle, rocks, rock_size, pebbles, seaweed, weed_height, sway, phase, .. } => [hills, hill_size, ripples, ripple_length, angle, rocks, rock_size, pebbles, seaweed, weed_height, sway, phase],
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
    /// Strength of the caustics: light focused by the waves on everything under the water
    /// (0 = none, 1 = natural).
    pub caustics: f64,
    /// Sharpness of the caustics (0 = soft, 1 = fine lines).
    pub caustic_sharpness: f64,
    /// Strength of the light rays (god rays) in the water: sunlight scattered by the water,
    /// broken into shafts by the waves and by the shadows of objects (0 = none).
    pub light_rays: f64,
    /// Air bubbles rising in the water: how many (0 = none, 1 = many), their size (scene
    /// units) and how fast they rise (units per second).
    pub bubbles: f64,
    pub bubble_size: f64,
    pub bubble_speed: f64,
    /// Where the bubbles rise: spread through the water, or in plumes from vents.
    pub bubble_source: BubbleSource,
    /// Vents: mean distance between the random vents on the ground (scene units; 0 = none,
    /// only the objects marked as bubble sources).
    pub vent_spacing: f64,
    /// Vents: width of a bubble plume (scene units).
    pub plume_width: f64,
    /// Tiny floating particles (marine snow): how many (0 = none, 1 = many) and their size.
    pub particles: f64,
    pub particle_size: f64,
    /// Clock of the bubbles and particles (seconds; animations advance it).
    pub time: f64,
    /// Size of the camera's front glass (scene units): where the water surface crosses it,
    /// the picture splits into the view above and below the water, with a dark line where
    /// the water clings to the glass (0 = a point: no split).
    pub lens: f64,
    /// The camera's front glass is flat: rays bend where they leave it into the water, so
    /// everything under the water looks 1.33 times larger (like most underwater cameras);
    /// off: a dome (no bending).
    pub flat_lens: bool,
    /// Water drops on the part of the glass above the water after the camera left the water
    /// (0 = none, 1 = many); they run down the glass with the clock and dry up within
    /// `dry_time`. Spray near a shore leaves a few as well.
    pub lens_drops: f64,
    /// How wet the glass is (0 = dry, 1 = just out of the water): the share of the lens drops
    /// shown in stills. Animations and the preview raise it from the camera's path (diving
    /// out, spray).
    pub lens_wet: f64,
    /// Seconds the lens drops take to dry up after the camera left the water.
    pub dry_time: f64,
    /// Wet trails the drops leave on the glass as they run down (0 = none, 1 = strong).
    pub drop_trails: f64,
    /// Focus of the lens drops: 0 = soft, blurred blobs (drops on a camera lens are out of
    /// focus), 1 = sharp drops with dark rims (like a window in focus).
    pub drop_focus: f64,
}

/// Where the bubbles of a water medium rise.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum BubbleSource {
    /// Spread evenly through the water.
    #[default]
    Everywhere,
    /// In plumes from random vents on the ground and from objects marked as bubble sources.
    Vents,
}

impl BubbleSource {
    pub const ALL: [BubbleSource; 2] = [BubbleSource::Everywhere, BubbleSource::Vents];

    pub fn label(self) -> &'static str {
        match self {
            BubbleSource::Everywhere => "Everywhere",
            BubbleSource::Vents => "Vents and sources",
        }
    }
}

impl Default for WaterMedium {
    fn default() -> Self {
        WaterMedium { see_through: false, ior: 1.33, color: [0.03, 0.22, 0.3], filter: [0.12, 0.6, 0.78], visibility: 3.0, murk: 0.25, depth_dark: 1.0, caustics: 1.0, caustic_sharpness: 0.5, light_rays: 0.4, bubbles: 0.2, bubble_size: 0.012, bubble_speed: 0.25, bubble_source: BubbleSource::Everywhere, vent_spacing: 1.0, plume_width: 0.08, particles: 0.15, particle_size: 0.0025, time: 0.0, lens: 0.2, flat_lens: true, lens_drops: 0.4, lens_wet: 0.0, dry_time: 5.0, drop_trails: 0.6, drop_focus: 0.0 }
    }
}

impl WaterMedium {
    /// Fading per scene unit of the view through the water, per channel: absorption from the
    /// filter color plus the murk (the same for all channels).
    pub fn extinction(&self) -> [f64; 3] {
        let vis = self.visibility.max(1e-6);
        self.filter.map(|f| -(f as f64).clamp(1e-4, 1.0).ln() / vis + self.murk.max(0.0) * 3.0 / vis)
    }

    /// Light scattered per scene unit (all colors alike): the murk plus a little that even
    /// clear water scatters (the light rays come from it).
    pub fn scattering(&self) -> f64 {
        (self.murk.max(0.0) * 3.0 + 0.3) / self.visibility.max(1e-6)
    }

    /// Fading per unit of depth of the light from above.
    pub fn depth_fade(&self) -> [f64; 3] {
        self.extinction().map(|e| e * self.depth_dark.max(0.0))
    }

    /// Distance (local units of the water) between the surface points whose light the
    /// caustics compare: from 6 % of the wave length (soft) down to 0.8 % (fine lines).
    pub fn caustic_step(&self, wavelength: f64) -> f64 {
        wavelength.max(1e-6) * (0.06 - 0.052 * self.caustic_sharpness.clamp(0.0, 1.0))
    }
}

/// Most a caustic brightens the light (march.wgsl `CAUSTIC_MAX`).
pub const CAUSTIC_MAX: f64 = 6.0;

/// Refraction of the unit direction `i` at the unit normal `n` (facing against `i`) with
/// `eta` = index before / index after (None at total internal reflection; march.wgsl
/// `refract_dir`).
pub fn refract_dir(i: V3, n: V3, eta: f64) -> Option<V3> {
    let ci = -i.dot(n);
    let k = 1.0 - eta * eta * (1.0 - ci * ci);
    (k > 0.0).then(|| (i * eta + n * (eta * ci - k.sqrt())).normalize())
}

/// Brightness of the light the waves focus at the local point `q` below the open sea
/// (caustics; mirrors march.wgsl `water_caustic`), 1 for flat water: light from the local
/// direction `l` (unit, toward the light) refracts into the water at the waves; three
/// surface points `e` apart send their light to three points at `q`'s depth, and the
/// brightness is the ratio of the areas between them (light spread or focused). The surface
/// points sit where a flat surface would refract the light toward `q`, moved by
/// (`jx`, `jy`) × `e` (jitter against aliasing). The contrast fades with depth (over four
/// wave lengths) and by `strength`. With `shaft` > 0 the light lands at that depth below the
/// surface instead of at `q`'s: the pattern stays the same along the light (light shafts).
#[allow(clippy::too_many_arguments)]
pub fn water_caustic(a: &[f64; 12], q: V3, l: V3, ior: f64, e: f64, jx: f64, jy: f64, strength: f64, shaft: f64) -> f64 {
    if l.z <= 0.0 || strength <= 0.0 {
        return 1.0;
    }
    let i = -l;
    let eta = 1.0 / ior.max(1.0);
    let Some(t0) = refract_dir(i, v3(0.0, 0.0, 1.0), eta) else { return 1.0 };
    // Ten heights from one loop (one call site on the GPU): the depth, then per surface
    // point its own and two neighbors' (normal by differences).
    let mut hs = [0.0; 10];
    let (mut sx, mut sy, mut depth) = (0.0, 0.0, 0.0);
    for (k, hk) in hs.iter_mut().enumerate() {
        let (x, y) = if k == 0 {
            (q.x, q.y)
        } else {
            let c = (k - 1) / 3;
            let n = (k - 1) % 3;
            (sx + if c == 1 { e } else { 0.0 } + if n == 1 { e } else { 0.0 }, sy + if c == 2 { e } else { 0.0 } + if n == 2 { e } else { 0.0 })
        };
        *hk = water_height(a, x, y, NO_SHORE);
        if k == 0 {
            depth = *hk - q.z;
            if depth <= 0.0 {
                return 1.0;
            }
            sx = q.x - t0.x * depth / -t0.z + jx * e;
            sy = q.y - t0.y * depth / -t0.z + jy * e;
        }
    }
    let floor = if shaft > 0.0 { hs[1] - shaft } else { q.z };
    let mut land = [(0.0, 0.0); 3];
    for (c, lc) in land.iter_mut().enumerate() {
        let (x, y) = (sx + if c == 1 { e } else { 0.0 }, sy + if c == 2 { e } else { 0.0 });
        let h0 = hs[1 + 3 * c];
        let n = v3(-(hs[2 + 3 * c] - h0) / e, -(hs[3 + 3 * c] - h0) / e, 1.0).normalize();
        let Some(t) = refract_dir(i, n, eta) else { return 1.0 };
        let k = (h0 - floor).max(0.0) / (-t.z).max(1e-6);
        *lc = (x + t.x * k, y + t.y * k);
    }
    let det = ((land[1].0 - land[0].0) * (land[2].1 - land[0].1) - (land[1].1 - land[0].1) * (land[2].0 - land[0].0)) / (e * e);
    let bright = (1.0 / det.abs().max(1e-6)).min(CAUSTIC_MAX);
    let fade = (-depth / (4.0 * a[1].max(1e-6))).exp();
    (1.0 + (bright - 1.0) * strength * fade).max(0.0)
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
        SEABED_KIND => seabed_distance(&a, q),
        _ => water_distance(&a, q, NO_SHORE),
    }
}

/// `Primitive::index` of the seabed.
pub const SEABED_KIND: u32 = 10;

/// Height of the seabed at (`x`, `y`) (mirrors march.wgsl `seabed_height`): four octaves of
/// value noise (`a[0]` high, the largest `a[1]` across) and sand ripples (`a[2]` high,
/// `a[3]` apart, across the direction `a[4]`), whose crests bend with slow noise.
pub fn seabed_height(a: &[f64; 12], x: f64, y: f64) -> f64 {
    let size = a[1].abs().max(1e-6);
    let (mut sum, mut amp, mut f) = (0.0, 1.0, 1.0 / size);
    for o in 0..4 {
        let off = o as f64 * 17.31;
        sum += amp * (water_noise(x * f + off, y * f - off) * 2.0 - 1.0);
        amp *= 0.5;
        f *= 2.0;
    }
    let len = a[3].abs().max(1e-6);
    let ang = a[4].to_radians();
    let u = x * ang.cos() + y * ang.sin();
    let warp = (water_noise(x / (len * 6.0) + 3.1, y / (len * 6.0) + 8.7) * 2.0 - 1.0) * len * 1.5;
    let ripple = 0.5 + 0.5 * ((u + warp) * std::f64::consts::TAU / len).sin();
    a[0] * sum / 1.875 + a[2] * ripple
}

/// Steepest slope of the seabed (mirrors march.wgsl `seabed_distance`): per noise octave
/// 3 √2 × its height × its frequency (value noise with smoothstep), the ripples π × height /
/// length × (1 + the warp's rate); 1.25 for safety (test `seabed_stays_a_distance_bound`).
fn seabed_steepness(a: &[f64; 12]) -> f64 {
    let hills = a[0].abs() * 3.0 * std::f64::consts::SQRT_2 / 1.875 * 4.0 / a[1].abs().max(1e-6);
    let ripples = a[2].abs() * std::f64::consts::PI / a[3].abs().max(1e-6) * (1.0 + 0.75 * std::f64::consts::SQRT_2);
    (hills + ripples) * 1.25
}

/// Distance bound of the seabed (negative below its surface). Rocks, pebbles and seaweed
/// sit in the ground-following frame (x, y, height above the ground): that shear stretches
/// distances by at most 1 + the steepest slope.
pub fn seabed_distance(a: &[f64; 12], q: V3) -> f64 {
    let parts = seabed_feature_top(a);
    let top = a[0].abs() + a[2].abs() + parts;
    let above = q.z - top;
    if above > top.max(1e-6) {
        return above;
    }
    let steep = seabed_steepness(a);
    let z = q.z - seabed_height(a, q.x, q.y);
    let mut d = z / (1.0 + steep * steep).sqrt();
    if parts > 0.0 {
        let near = if z > parts * 1.5 { z - parts } else { seabed_parts(a, q.x, q.y, z).iter().fold(NO_SHORE, |m, p| m.min(p.0)) };
        d = d.min(near / (1.0 + steep));
    }
    if above > 0.0 { d.max(above) } else { d }
}

fn seabed_rock_size() -> f64 {
    0.25
}

fn seabed_weed_height() -> f64 {
    0.4
}

fn seabed_sway() -> f64 {
    0.5
}

fn seabed_sway_speed() -> f64 {
    1.0
}

/// Rock grid cell per rock size (march.wgsl `ROCK_CELL`).
const ROCK_CELL: f64 = 3.0;
/// Pebble size per rock size and pebble grid cell per pebble size.
const PEBBLE_SIZE: f64 = 0.12;
const PEBBLE_CELL: f64 = 4.0;
/// Seaweed grid cell per seaweed height (two blades per cell).
const WEED_CELL: f64 = 0.45;
/// Seaweed blade half width at the root and thickness, per seaweed height.
const WEED_WIDTH: f64 = 0.035;
const WEED_THICK: f64 = 0.004;
/// Bumps on the rocks (share of the rock size).
const ROCK_LUMPS: f64 = 0.1;

/// Stones on the ground (march.wgsl `seabed_stones`): at (`x`, `y`) and `z` above the ground,
/// the nearest of the 3 × 3 grid cells around (cells `cell` × `size`, `share` of them with a
/// stone): a turned ellipsoid `size` (largest), `flat` of it high, partly buried, with bumps
/// (`lumps`). Returns a distance bound (in the ground-following frame) and the stone's hash.
#[allow(clippy::too_many_arguments)]
fn seabed_stones(x: f64, y: f64, z: f64, size: f64, share: f64, cell: f64, flat: f64, lumps: f64, salt: i32) -> (f64, f64) {
    let c = size * cell;
    let rmax = size * 1.2 * (1.0 + lumps);
    // Stones of farther cells: one cell between, each reaching at most rmax - c/4 out of its
    // own cell.
    if share <= 0.0 || size <= 0.0 {
        return (NO_SHORE, 0.0);
    }
    // Above the highest stone: at least that far (soft shadows read the bound as the distance
    // to the nearest surface, so it must not stay at the cell bound high above the stones).
    let above = z - size * (0.7 + lumps);
    let mut best = ((1.25 * c - rmax).max(c * 0.05).max(above), 0.0);
    let (ci, cj) = ((x / c).floor() as i32, (y / c).floor() as i32);
    for k in 0..9 {
        let (i, j) = (ci + k % 3 - 1, cj + k / 3 - 1);
        let h = |n: i32| crate::render::hash3(i.wrapping_add(n.wrapping_mul(613)), j, salt) as f64;
        if h(0) >= share {
            continue;
        }
        let r = size * (0.5 + 0.5 * h(1));
        let (cx, cy) = ((i as f64 + 0.25 + 0.5 * h(2)) * c, (j as f64 + 0.25 + 0.5 * h(3)) * c);
        let (sn, cs) = (h(4) * std::f64::consts::TAU).sin_cos();
        let (dx, dy) = (x - cx, y - cy);
        let (u, v) = (dx * cs + dy * sn, dy * cs - dx * sn);
        let rr = [r * (0.8 + 0.4 * h(5)), r * (0.7 + 0.3 * h(6)), r * flat * (0.7 + 0.3 * h(7))];
        let w = z + rr[2] * 0.3;
        let e = ((u / rr[0]).powi(2) + (v / rr[1]).powi(2) + (w / rr[2]).powi(2)).sqrt();
        let mut d = (e - 1.0) * rr[0].min(rr[1]).min(rr[2]);
        if lumps > 0.0 {
            let f = 3.0 / r;
            let bump = (u * f + h(8) * 6.0).sin() * (v * f + h(9) * 6.0).sin() * (w * f + h(10) * 6.0).sin();
            d = (d + lumps * r * bump) / (1.0 + lumps * 3.0 * 3f64.sqrt());
        }
        if d < best.0 {
            best = (d.max(above), h(11));
        }
    }
    best
}

/// Seaweed (march.wgsl `seabed_weed`): two blades per grid cell (`WEED_CELL` × `height`) in
/// meadows (noise), each up to `height` tall: a flat ribbon (half width `WEED_WIDTH` ×
/// `height` at the root, tapering to a point; `WEED_THICK` thick) that leans and sways by
/// `sway` at the time `phase` and twists along its length. Returns a distance bound
/// (ground-following frame) and how far up the nearest blade the point is (0..1).
fn seabed_weed(x: f64, y: f64, z: f64, height: f64, share: f64, sway: f64, phase: f64) -> (f64, f64) {
    if share <= 0.0 || height <= 0.0 {
        return (NO_SHORE, 0.0);
    }
    let c = height * WEED_CELL;
    let (w0, th) = (height * WEED_WIDTH, height * WEED_THICK);
    let sway = sway.clamp(0.0, 1.0);
    let reach = height * (0.25 + 0.2 * sway) + w0 + th;
    // Rate of change along the height: the axis's sideways offset (lean 0.5, sway 0.9 ×
    // sway), the taper (0.8 of the half width per blade height) and the twist (up to 1.5
    // radians per blade height moves the edge by the half width × that); the shortest blades
    // are half the height.
    let k = 0.5 + 0.9 * sway + 2.0 * (0.8 + 1.5) * WEED_WIDTH;
    let norm = (1.0 + k * k).sqrt();
    // Blades of cells outside the 5 × 5 block reach at most `reach` - 0.2 cells into it;
    // above the tallest blade the height gap bounds them all.
    let (ci, cj) = ((x / c).floor() as i32, (y / c).floor() as i32);
    let edge = (x - (ci - 2) as f64 * c).min((ci + 3) as f64 * c - x).min(y - (cj - 2) as f64 * c).min((cj + 3) as f64 * c - y);
    let above = z - height - th;
    let mut best = (((edge - (reach - 0.2 * c)) / norm).max(above), 0.0);
    for n in 0..25 {
        let (i, j) = (ci + n % 5 - 2, cj + n / 5 - 2);
        let meadow = (2.5 * water_noise(i as f64 * WEED_CELL / 5.0 + 5.3, j as f64 * WEED_CELL / 5.0 + 1.7) - 1.0).clamp(0.0, 1.0);
        if meadow <= 0.0 {
            continue;
        }
        for b in 0..2 {
            let h = |m: i32| crate::render::hash3(i.wrapping_add(m.wrapping_mul(613)), j, 120 + b) as f64;
            if h(0) >= share * meadow {
                continue;
            }
            let hh = height * (0.5 + 0.5 * h(1));
            let (rx, ry) = ((i as f64 + 0.2 + 0.6 * h(2)) * c, (j as f64 + 0.2 + 0.6 * h(3)) * c);
            // Bounding cylinder of the blade (lean, sway, width).
            let lean = 0.25 * h(5) + 0.2 * sway;
            if (((x - rx).powi(2) + (y - ry).powi(2)).sqrt() - hh * lean - w0 - th) / norm >= best.0 {
                continue;
            }
            let (sn, cs) = (h(4) * std::f64::consts::TAU).sin_cos();
            let t = (z / hh).clamp(0.0, 1.0);
            let off = hh * t * t * (0.25 * h(5) + 0.2 * sway * (phase + h(6) * std::f64::consts::TAU - 2.5 * t).sin());
            let (dx, dy) = (x - rx - cs * off, y - ry - sn * off);
            // The ribbon's wide side faces the bend, turning slowly up the blade.
            let (ts, tc) = (h(4) * std::f64::consts::TAU + 1.5 * t * (h(7) - 0.5) * 2.0).sin_cos();
            let (along, across) = (dx * tc + dy * ts, dy * tc - dx * ts);
            let half = w0 * (1.0 - 0.8 * t);
            let wide = (across.abs() - half).max(0.0);
            let mut d = (along * along + wide * wide).sqrt() - th;
            if z > hh {
                d = (along * along + wide * wide + (z - hh) * (z - hh)).sqrt() - th;
            }
            d /= norm;
            if d < best.0 {
                best = (d, t);
            }
        }
    }
    best
}

/// Height of the tallest seabed part above the ground (rocks, pebbles, seaweed).
fn seabed_feature_top(a: &[f64; 12]) -> f64 {
    let mut top: f64 = 0.0;
    if a[5] > 0.0 {
        top = top.max(a[6].abs() * (0.7 + ROCK_LUMPS));
    }
    if a[7] > 0.0 {
        top = top.max(a[6].abs() * PEBBLE_SIZE * 0.7 * 0.6);
    }
    if a[8] > 0.0 {
        top = top.max(a[9].abs() * (1.0 + WEED_THICK));
    }
    top
}

/// The parts of the seabed at local (`x`, `y`) and `z` above the ground (mirrors march.wgsl
/// `seabed_parts`): distance bounds in the ground-following frame of rocks, pebbles and
/// seaweed, each with its color value (see [`seabed_color`]).
pub fn seabed_parts(a: &[f64; 12], x: f64, y: f64, z: f64) -> [(f64, f64); 3] {
    let size = a[6].abs();
    let (rd, rh) = seabed_stones(x, y, z, size, a[5].clamp(0.0, 1.0), ROCK_CELL, 1.0, ROCK_LUMPS, 101);
    let (pd, ph) = seabed_stones(x, y, z, size * PEBBLE_SIZE, a[7].clamp(0.0, 1.0), PEBBLE_CELL, 0.6, 0.0, 102);
    let (wd, wt) = seabed_weed(x, y, z, a[9].abs(), a[8].clamp(0.0, 1.0), a[10], a[11]);
    [(rd, 0.5 + 0.25 * rh), (pd, 0.58 + 0.12 * ph), (wd, 0.8 + 0.2 * wt)]
}

/// A gradient for `Pattern::Seabed`: sand from dark to light (0..0.45), stones from brown to
/// gray (0.5..0.75), seaweed from dark green at the root to yellow green at the tip (0.8..1).
pub fn seabed_gradient() -> Vec<ColorStop> {
    [
        (0.0, [0.4, 0.34, 0.24]),
        (0.45, [0.86, 0.8, 0.66]),
        (0.48, [0.34, 0.3, 0.26]),
        (0.62, [0.5, 0.47, 0.42]),
        (0.75, [0.32, 0.33, 0.33]),
        (0.78, [0.05, 0.16, 0.06]),
        (0.9, [0.16, 0.36, 0.1]),
        (1.0, [0.45, 0.55, 0.18]),
    ]
    .into_iter()
    .map(|(p, c)| ColorStop::new(p, c))
    .collect()
}

/// Color value of the seabed's own pattern (`Pattern::Seabed`, march.wgsl `seabed_color`) at
/// the local point `q`: sand 0..0.45 by height (`size` per range), rocks 0.5..0.75, pebbles
/// 0.58..0.7, seaweed 0.8 (root) ..1 (tip).
pub fn seabed_color(a: &[f64; 12], q: V3, size: f64) -> f64 {
    let sand = 0.45 * (0.5 + q.z / size.abs().max(1e-9)).clamp(0.0, 1.0);
    let top = seabed_feature_top(a);
    if top <= 0.0 {
        return sand;
    }
    let z = q.z - seabed_height(a, q.x, q.y);
    if z > top * 1.5 {
        return sand;
    }
    let mut best = (z.max(0.0), sand);
    for (d, v) in seabed_parts(a, q.x, q.y, z) {
        if d < best.0 {
            best = (d, v);
        }
    }
    best.1
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

/// How strongly spray reaches the local point `q` of a water `s` away from the shore (0..1):
/// 1 at the coast within the height the drops fly, falling to 0 at the edge of their zone
/// (the zone of [`water_spray`]).
pub fn water_spray_zone(a: &[f64; 12], q: V3, s: f64) -> f64 {
    let (amp, len, amount) = (a[0].abs().max(1e-9), a[1].max(1e-6), a[9].clamp(0.0, 2.0));
    if amount <= 0.0 || a[8] <= 0.0 || s >= NO_SHORE {
        return 0.0;
    }
    let top = amp * 3.0 * amount;
    let range = len * SPRAY_RANGE + top * 0.6;
    let along = (1.0 - s.max(0.0) / range).clamp(0.0, 1.0);
    let up = (1.0 - (q.z - amp) / (top * 1.5)).clamp(0.0, 1.0);
    along * up * amount.min(1.0)
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
pub(crate) fn water_noise(x: f64, y: f64) -> f64 {
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
    /// Seabed: sand, stones and seaweed from their own parts of the gradient
    /// ([`seabed_color`]).
    Seabed,
}

impl Pattern {
    pub const ALL: [Pattern; 4] = [Pattern::Solid, Pattern::Checker, Pattern::Height, Pattern::Radial];
    /// The patterns of 3D models.
    pub const FOR_MODELS: [Pattern; 5] = [Pattern::Model, Pattern::Solid, Pattern::Checker, Pattern::Height, Pattern::Radial];
    /// The patterns of the seabed.
    pub const FOR_SEABED: [Pattern; 5] = [Pattern::Seabed, Pattern::Solid, Pattern::Checker, Pattern::Height, Pattern::Radial];

    pub fn label(self) -> &'static str {
        match self {
            Pattern::Solid => "Solid",
            Pattern::Checker => "Checker",
            Pattern::Height => "By height",
            Pattern::Radial => "From the center",
            Pattern::Model => "Model colors",
            Pattern::Seabed => "Sand, stones, seaweed",
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
    fn caustics_keep_the_light_and_shafts_follow_it() {
        let a = Primitive::water().params();
        let l = v3(0.3, 0.2, 0.9).normalize();
        // Flat water (no waves): no caustics.
        let mut flat = a;
        flat[0] = 0.0;
        assert!((water_caustic(&flat, v3(0.3, 0.4, -1.0), l, 1.33, 0.07, 0.0, 0.0, 1.0, 0.0) - 1.0).abs() < 1e-9);
        // Waves focus and spread the light, about as much as they take away.
        let (mut sum, mut lo, mut hi) = (0.0, f64::MAX, 0.0f64);
        let n = 3000;
        for i in 0..n {
            let q = v3((i % 61) as f64 * 0.0731, (i / 61) as f64 * 0.0517, -0.5);
            let c = water_caustic(&a, q, l, 1.33, 0.07, 0.0, 0.0, 1.0, 0.0);
            sum += c;
            lo = lo.min(c);
            hi = hi.max(c);
        }
        let mean = sum / n as f64;
        assert!((0.9..1.2).contains(&mean) && lo < 0.8 && hi > 1.5, "mean {mean} range {lo}..{hi}");
        // Shafts: the same pattern along the light (refracted) below the surface.
        let t = refract_dir(-l, v3(0.0, 0.0, 1.0), 1.0 / 1.33).unwrap();
        let q = v3(0.4, 0.3, -0.4);
        let c1 = water_caustic(&a, q, l, 1.33, 0.07, 0.0, 0.0, 1.0, 1.0);
        let c2 = water_caustic(&a, q + t * 0.5, l, 1.33, 0.07, 0.0, 0.0, 1.0, 1.0);
        let fade = (-(0.5 * -t.z) / (4.0 * a[1])).exp();
        assert!(((c2 - 1.0) - (c1 - 1.0) * fade).abs() < 0.03, "{c1} {c2}");
        // Another point beside it shows another part of the pattern.
        let c3 = water_caustic(&a, q + v3(0.13, 0.05, 0.0), l, 1.33, 0.07, 0.0, 0.0, 1.0, 1.0);
        assert!((c3 - c1).abs() > 0.03, "{c1} {c3}");
    }

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
    fn seabed_stays_a_distance_bound() {
        // The bound divides by the steepest slope: measure the real slopes (and how close the
        // bound comes, so it is not needlessly slow).
        for (hills, size, ripples, len, angle) in [(0.25, 2.5, 0.015, 0.12, 0.0), (1.0, 1.0, 0.0, 0.1, 30.0), (0.0, 1.0, 0.05, 0.05, 70.0), (0.5, 4.0, 0.03, 0.3, -20.0)] {
            let a = [hills, size, ripples, len, angle, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
            let steep = seabed_steepness(&a);
            let e = 1e-6;
            let mut worst: f64 = 0.0;
            for i in 0..40000 {
                let (x, y) = ((i as f64 * 0.6180339) % 1.0 * 20.0, (i as f64 * 0.7548776) % 1.0 * 20.0);
                let gx = (seabed_height(&a, x + e, y) - seabed_height(&a, x - e, y)) / (2.0 * e);
                let gy = (seabed_height(&a, x, y + e) - seabed_height(&a, x, y - e)) / (2.0 * e);
                worst = worst.max((gx * gx + gy * gy).sqrt());
            }
            assert!(worst < steep && worst > steep * 0.3, "slope {worst}, bound {steep}");
            let top = hills + ripples;
            let range = (0..4000).map(|i| seabed_height(&a, i as f64 * 0.0137, i as f64 * 0.0071)).fold((f64::MAX, f64::MIN), |r, h| (r.0.min(h), r.1.max(h)));
            assert!(range.0 >= -top - 1e-9 && range.1 <= top + 1e-9, "{range:?}");
        }
        // Below the surface negative, above positive.
        let a = Primitive::seabed().params();
        assert!(seabed_distance(&a, v3(0.3, 0.2, -1.0)) < 0.0 && seabed_distance(&a, v3(0.3, 0.2, 1.0)) > 0.0);
    }

    #[test]
    fn seabed_parts_stay_a_distance_bound() {
        // Rocks, pebbles and swaying seaweed on hills: no point within the bound of a point
        // has the other sign (checked on rings of points around random points near the ground).
        let a = Primitive::seabed().params();
        let mut a2 = a;
        (a2[5], a2[7], a2[8], a2[10], a2[11]) = (0.8, 1.0, 1.0, 1.0, 2.3);
        for a in [a, a2] {
            let mut checked = 0;
            for i in 0..6000 {
                let (x, y) = ((i as f64 * 0.6180339) % 1.0 * 6.0, (i as f64 * 0.7548776) % 1.0 * 6.0);
                let z = seabed_height(&a, x, y) + ((i as f64 * 0.5698403) % 1.0) * 0.5 - 0.05;
                let p = v3(x, y, z);
                let d = seabed_distance(&a, p);
                if d <= 1e-4 {
                    continue;
                }
                checked += 1;
                for k in 0..26 {
                    let dir = v3(((k % 3) as f64) - 1.0, ((k / 3 % 3) as f64) - 1.0, ((k / 9) as f64) - 1.0 + if k == 13 { 1.0 } else { 0.0 });
                    let q = p + dir.normalize() * (d * 0.98);
                    assert!(seabed_distance(&a, q) > -1e-9, "bound {d} at {p:?} reaches inside at {q:?}");
                }
            }
            assert!(checked > 3000);
        }
        // Parts show where they are: rock, pebble and seaweed colors occur, and sand.
        let colors: Vec<f64> = (0..20000).filter_map(|i| {
            let (x, y) = ((i as f64 * 0.6180339) % 1.0 * 8.0, (i as f64 * 0.7548776) % 1.0 * 8.0);
            let z = seabed_height(&a2, x, y) + ((i as f64 * 0.5698403) % 1.0) * 0.3;
            let q = v3(x, y, z);
            (seabed_distance(&a2, q).abs() < 0.003).then(|| seabed_color(&a2, q, 0.6))
        }).collect();
        for (lo, hi) in [(0.0, 0.45), (0.5, 0.76), (0.8, 1.0)] {
            assert!(colors.iter().any(|c| (lo..=hi).contains(c)), "no color in {lo}..{hi}");
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



