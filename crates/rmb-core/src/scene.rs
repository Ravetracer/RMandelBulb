// SPDX-License-Identifier: LGPL-2.1-or-later

//! The scene: everything needed to render one image.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::math::{V3, v3};

pub const FORMAT_VERSION: u32 = 1;
/// Number of formula slots of a hybrid (as in Mandelbulb3D).
pub const MAX_SLOTS: usize = 6;
pub const MAX_LIGHTS: usize = 8;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Scene {
    pub version: u32,
    pub width: u32,
    pub height: u32,
    pub camera: Camera,
    pub fractal: Fractal,
    pub march: March,
    pub lighting: Lighting,
    pub material: Material,
    pub background: Background,
    pub fog: Fog,
    pub dof: Dof,
    pub post: Post,
    pub cut: Cutting,
    /// Free text (author, notes, where it was imported from).
    pub notes: String,
    /// Objects combined with the main fractal (scene composition).
    pub objects: Vec<crate::compose::SceneObject>,
    /// Keyframe animation (keyframe scenes have none).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub animation: Option<crate::animation::Animation>,
    /// Motion blur: the camera at the end of the exposure. Every sample picks a random time
    /// between `camera` and this one (set per pass by the animation renderer, never saved).
    #[serde(skip)]
    pub motion_end: Option<Camera>,
}

impl Default for Scene {
    fn default() -> Self {
        Scene {
            version: FORMAT_VERSION,
            width: 1280,
            height: 720,
            camera: Camera::default(),
            fractal: Fractal::default(),
            march: March::default(),
            lighting: Lighting::default(),
            material: Material::default(),
            background: Background::default(),
            fog: Fog::default(),
            dof: Dof::default(),
            post: Post::default(),
            cut: Cutting::default(),
            notes: String::new(),
            objects: Vec::new(),
            animation: None,
            motion_end: None,
        }
    }
}

impl Scene {
    pub fn aspect(&self) -> f64 {
        self.width.max(1) as f64 / self.height.max(1) as f64
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    pub fn from_json(s: &str) -> Result<Scene, String> {
        serde_json::from_str(s).map_err(|e| e.to_string())
    }
}

/// Perspective camera. `right`, `up` and `forward` form an orthonormal basis.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Camera {
    pub position: V3,
    pub right: V3,
    pub up: V3,
    pub forward: V3,
    /// Vertical field of view in degrees.
    pub fov: f64,
}

impl Default for Camera {
    fn default() -> Self {
        let mut c = Camera { position: v3(1.9, -2.2, 1.25), right: v3(1.0, 0.0, 0.0), up: v3(0.0, 0.0, 1.0), forward: v3(0.0, 1.0, 0.0), fov: 40.0 };
        c.look_at(V3::ZERO, v3(0.0, 0.0, 1.0));
        c
    }
}

impl Camera {
    /// Points the camera at `target` keeping `world_up` up.
    pub fn look_at(&mut self, target: V3, world_up: V3) {
        let f = (target - self.position).normalize();
        let mut r = f.cross(world_up);
        if r.length() < 1e-9 {
            r = f.cross(v3(0.0, 1.0, 0.0));
        }
        let r = r.normalize();
        self.forward = f;
        self.right = r;
        self.up = r.cross(f).normalize();
    }

    /// Orientation as a quaternion (x → right, y → forward, z → up).
    pub fn orientation(&self) -> crate::math::Quat {
        crate::math::Quat::from_basis(self.right, self.forward, self.up)
    }

    /// Sets the orientation from a quaternion (see [`Camera::orientation`]).
    pub fn set_orientation(&mut self, q: crate::math::Quat) {
        let (r, f, u) = q.normalize().basis();
        self.right = r;
        self.forward = f;
        self.up = u;
        self.orthonormalize();
    }

    /// Re-orthonormalizes the basis (after many incremental rotations).
    pub fn orthonormalize(&mut self) {
        let f = self.forward.normalize();
        let r = f.cross(self.up).normalize();
        self.forward = f;
        self.right = r;
        self.up = r.cross(f).normalize();
    }

    pub fn tan_half_fov(&self) -> f64 {
        (self.fov.clamp(1.0, 170.0).to_radians() * 0.5).tan()
    }

    /// Ray direction through normalized screen coordinates (`sx`, `sy` in -1..1, y up).
    /// The camera a fraction `t` of the way to `b` (motion blur within one exposure: position
    /// and field of view linear, the axes linear and normalized; same on the GPU).
    pub fn lerp(&self, b: &Camera, t: f64) -> Camera {
        let mix = |x: V3, y: V3| x + (y - x) * t;
        let ha = (self.fov.clamp(1.0, 170.0) * 0.5).to_radians();
        let hb = (b.fov.clamp(1.0, 170.0) * 0.5).to_radians();
        Camera {
            position: mix(self.position, b.position),
            right: mix(self.right, b.right).normalize(),
            up: mix(self.up, b.up).normalize(),
            forward: mix(self.forward, b.forward).normalize(),
            fov: (ha + (hb - ha) * t).to_degrees() * 2.0,
        }
    }

    pub fn ray_dir(&self, sx: f64, sy: f64, aspect: f64) -> V3 {
        let t = self.tan_half_fov();
        (self.forward + self.right * (sx * t * aspect) + self.up * (sy * t)).normalize()
    }

    /// Rotates the camera about its own axes (radians): yaw about up, pitch about right, roll about forward.
    pub fn rotate_local(&mut self, yaw: f64, pitch: f64, roll: f64) {
        let (r, u, f) = (self.right, self.up, self.forward);
        let f2 = f.rotate(u, -yaw);
        let r2 = r.rotate(u, -yaw);
        let f3 = f2.rotate(r2, pitch);
        let u3 = u.rotate(r2, pitch);
        self.forward = f3;
        self.up = u3.rotate(f3, roll);
        self.right = r2.rotate(f3, roll);
        self.orthonormalize();
    }

    /// Orbits the camera about `pivot`: yaw about the world axis `axis`, pitch about the camera's right axis.
    pub fn orbit(&mut self, pivot: V3, axis: V3, yaw: f64, pitch: f64) {
        let rel = self.position - pivot;
        let rel = rel.rotate(axis, -yaw);
        let (r, u, f) = (self.right.rotate(axis, -yaw), self.up.rotate(axis, -yaw), self.forward.rotate(axis, -yaw));
        let rel = rel.rotate(r, pitch);
        self.position = pivot + rel;
        self.right = r;
        self.up = u.rotate(r, pitch);
        self.forward = f.rotate(r, pitch);
        self.orthonormalize();
    }
}

/// One formula of the hybrid chain.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Slot {
    pub formula: String,
    /// Iterations in a row before the next slot; 0 disables the slot.
    pub iterations: u32,
    /// Parameter values by parameter identifier; missing ones use the formula default.
    pub params: BTreeMap<String, f64>,
}

impl Default for Slot {
    fn default() -> Self {
        Slot { formula: "Mandelbulb".into(), iterations: 1, params: BTreeMap::new() }
    }
}

impl Slot {
    pub fn new(formula: &str) -> Slot {
        let iterations = crate::formula::find(formula).map(|f| f.def.iterations).unwrap_or(1);
        Slot { formula: formula.into(), iterations, params: BTreeMap::new() }
    }

    /// Value of a parameter, falling back to the formula's default.
    pub fn value(&self, p: &crate::formula::ParamDef) -> f64 {
        self.params.get(&p.ident).copied().unwrap_or(p.default)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum DeMode {
    /// Analytic, with the formula's own estimate (log or linear).
    #[default]
    Auto,
    /// Analytic `0.5 r ln r / dr`.
    Log,
    /// Analytic `r / dr`.
    Linear,
    /// Numerical gradient, log kind (bulbs in hybrids).
    NumericLog,
    /// Numerical gradient, linear kind (boxes in hybrids).
    NumericLinear,
}

impl DeMode {
    pub const ALL: [DeMode; 5] = [DeMode::Auto, DeMode::Log, DeMode::Linear, DeMode::NumericLog, DeMode::NumericLinear];

    pub fn label(self) -> &'static str {
        match self {
            DeMode::Auto => "Auto",
            DeMode::Log => "Analytic (log)",
            DeMode::Linear => "Analytic (linear)",
            DeMode::NumericLog => "Numeric (log)",
            DeMode::NumericLinear => "Numeric (linear)",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Fractal {
    pub slots: Vec<Slot>,
    /// After the last slot the chain continues at this slot (0-based).
    pub repeat_from: usize,
    pub max_iterations: u32,
    /// Escape radius.
    pub bailout: f64,
    pub julia: bool,
    pub julia_c: [f64; 4],
    /// Start value of the fourth dimension (4D formulas).
    pub w_slice: f64,
    pub de_mode: DeMode,
    /// Multiplies the distance estimate (below 1 for formulas that overestimate).
    pub de_scale: f64,
    /// Rotation of the start point into 4D (XW, YW, ZW planes, degrees; 4D formulas).
    pub rotation_4d: [f64; 3],
    /// Render the inside of the set: points that escape are solid, the set is empty space
    /// (Mandelbulb3D's inside rendering, for views from within the fractal).
    pub inside: bool,
    /// Two formula chains whose distance estimates are combined (Mandelbulb3D's DE
    /// combination): slots before `start` form the first chain, the rest the second.
    pub combine: Option<DeCombine>,
    /// Interpolation of the first two slots (Mandelbulb3D's interpolation hybrid): every
    /// iteration runs both on the same point and blends the results with these weights.
    pub blend: Option<[f64; 2]>,
    /// Placement of the main fractal in the scene (position, rotation, scale, stretch; object
    /// fractals use their object's placement instead).
    pub transform: crate::compose::Transform,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CombineMode {
    /// Union.
    #[default]
    Min,
    /// Intersection.
    Max,
    /// The first chain with the second cut away.
    Subtract,
    /// Union with a linear blend over the smoothing distance.
    SmoothLinear,
    /// Union with a smooth blend over the smoothing distance.
    Smooth,
    /// The first chain transforms the point, the second (a dIFS) measures the distance there
    /// (Mandelbulb3D's "mix").
    Mix,
}

impl CombineMode {
    pub const ALL: [CombineMode; 6] = [CombineMode::Min, CombineMode::Max, CombineMode::Subtract, CombineMode::SmoothLinear, CombineMode::Smooth, CombineMode::Mix];

    pub fn label(self) -> &'static str {
        match self {
            CombineMode::Min => "Union (min)",
            CombineMode::Max => "Intersection (max)",
            CombineMode::Subtract => "Subtract second",
            CombineMode::SmoothLinear => "Union, linear blend",
            CombineMode::Smooth => "Union, smooth blend",
            CombineMode::Mix => "Mix (chain 1, then dIFS)",
        }
    }

    pub fn index(self) -> u32 {
        self as u32
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DeCombine {
    pub mode: CombineMode,
    /// First slot of the second chain.
    pub start: usize,
    /// The second chain repeats from this slot (absolute slot index).
    pub repeat_from: usize,
    pub max_iterations: u32,
    /// Blend distance of the smooth modes (world units).
    pub smooth: f64,
    /// Mix mode with a first chain without a linear estimate: the distance is divided by this
    /// power per iteration of the first chain (Mandelbulb3D's FmixPow).
    pub mix_pow: f64,
}

impl Default for DeCombine {
    fn default() -> Self {
        DeCombine { mode: CombineMode::Min, start: 1, repeat_from: 1, max_iterations: 12, smooth: 0.05, mix_pow: 2.0 }
    }
}

impl Default for Fractal {
    fn default() -> Self {
        Fractal {
            slots: vec![Slot::new("Mandelbulb")],
            repeat_from: 0,
            max_iterations: 12,
            bailout: 4.0,
            julia: false,
            julia_c: [0.0; 4],
            w_slice: 0.0,
            de_mode: DeMode::Auto,
            de_scale: 1.0,
            rotation_4d: [0.0; 3],
            inside: false,
            combine: None,
            blend: None,
            transform: crate::compose::Transform::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct March {
    /// Surface threshold in pixels: smaller values give finer detail.
    pub detail: f64,
    /// Ray step multiplier (below 1 against overstepping).
    pub step: f64,
    pub max_steps: u32,
    /// Far clip distance.
    pub max_distance: f64,
    /// Binary search refinement steps at the surface.
    pub refine_steps: u32,
    /// Rays start at this distance from the camera (near clipping).
    pub near: f64,
    /// Smoothed normals (Mandelbulb3D): the estimate is averaged over this many steps to each
    /// side along the surface; also measures the roughness (0 = off, 1..8).
    pub smooth_normals: u32,
    /// Number precision of the GPU renderer (deep zooms need double precision).
    pub precision: Precision,
}

impl Default for March {
    fn default() -> Self {
        March { detail: 1.0, step: 0.8, max_steps: 400, max_distance: 40.0, refine_steps: 4, near: 0.0, smooth_normals: 0, precision: Precision::Auto }
    }
}

/// Number precision of the GPU renderer. Single precision (f32) holds up to a zoom of about
/// 1e5; double precision (two f32 per value, about 14 digits) reaches about 1e11 but renders
/// several times slower. The CPU renderer always uses f64.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Precision {
    /// Double precision when the zoom needs it.
    #[default]
    Auto,
    Single,
    Double,
}

impl Precision {
    pub const ALL: [Precision; 3] = [Precision::Auto, Precision::Single, Precision::Double];

    pub fn label(self) -> &'static str {
        match self {
            Precision::Auto => "Auto",
            Precision::Single => "Single",
            Precision::Double => "Double",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum LightKind {
    /// Parallel light from a direction (azimuth, elevation).
    #[default]
    Directional,
    /// Light from a point (`position`); falls off with the squared distance.
    Point,
    /// A point light that shines into a cone toward `target`.
    Spot,
    /// A glowing rectangle (`width` × `height`) at `position` facing `target`; soft shadows.
    Area,
    /// A glowing tube from `position` to `target`; soft shadows.
    Line,
}

impl LightKind {
    pub const ALL: [LightKind; 5] = [LightKind::Directional, LightKind::Point, LightKind::Spot, LightKind::Area, LightKind::Line];

    pub fn label(self) -> &'static str {
        match self {
            LightKind::Directional => "Directional",
            LightKind::Point => "Point",
            LightKind::Spot => "Spot",
            LightKind::Area => "Area",
            LightKind::Line => "Line",
        }
    }

    /// The light has a position in the scene.
    pub fn positional(self) -> bool {
        self != LightKind::Directional
    }

    /// Index shared with the GPU kernel.
    pub fn index(self) -> u32 {
        self as u32
    }
}

/// How the diffuse light depends on the angle between the surface and the light
/// (Mandelbulb3D's diffuse functions).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum DiffuseCurve {
    /// cos (Lambert).
    #[default]
    Lambert,
    /// cos²: harder.
    Squared,
    /// cos / 2 + 1/2: wraps around to the back side.
    Wrap,
    /// (cos / 2 + 1/2)²: wraps, softer.
    WrapSquared,
}

impl DiffuseCurve {
    pub const ALL: [DiffuseCurve; 4] = [DiffuseCurve::Lambert, DiffuseCurve::Squared, DiffuseCurve::Wrap, DiffuseCurve::WrapSquared];

    pub fn label(self) -> &'static str {
        match self {
            DiffuseCurve::Lambert => "cos",
            DiffuseCurve::Squared => "cos²",
            DiffuseCurve::Wrap => "Wrap",
            DiffuseCurve::WrapSquared => "Wrap²",
        }
    }

    /// Diffuse factor for `ndl` (cos of the angle).
    pub fn apply(self, ndl: f64) -> f64 {
        match self {
            DiffuseCurve::Lambert => ndl.max(0.0),
            DiffuseCurve::Squared => ndl.max(0.0).powi(2),
            DiffuseCurve::Wrap => ndl * 0.5 + 0.5,
            DiffuseCurve::WrapSquared => (ndl * 0.5 + 0.5).powi(2),
        }
    }

    pub fn index(self) -> u32 {
        self as u32
    }
}

/// Look of a visible light source (Mandelbulb3D's visible light functions).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum GlowShape {
    /// Bright core with a soft falloff.
    #[default]
    Soft,
    /// Small sharp spot.
    Sharp,
    /// Glowing ball that partly covers what is behind it.
    Ball,
    /// Transparent halo, brightest in the middle.
    Halo,
}

impl GlowShape {
    pub const ALL: [GlowShape; 4] = [GlowShape::Soft, GlowShape::Sharp, GlowShape::Ball, GlowShape::Halo];

    pub fn label(self) -> &'static str {
        match self {
            GlowShape::Soft => "Soft",
            GlowShape::Sharp => "Sharp",
            GlowShape::Ball => "Ball",
            GlowShape::Halo => "Halo",
        }
    }

    pub fn index(self) -> u32 {
        self as u32
    }

    /// Light added (as a share of the light color) and how much of what lies behind stays
    /// visible, for `u` = squared distance from the light center / squared glow radius (0..1).
    /// `r2` is the squared radius (brighter for small glows, like Mandelbulb3D).
    pub fn shape(self, u: f64, r2: f64, point: bool) -> (f64, f64) {
        let r2 = r2.max(1e-30);
        match self {
            GlowShape::Soft => (1.0, (1.0 - u) * 0.1 / ((u + 0.0004) * 25.0).powi(2) / r2),
            GlowShape::Sharp => (1.0, 0.05 * (1.0 - u.sqrt()).max(0.0).powi(50) / r2 * if point { 1.5 } else { 1.0 }),
            GlowShape::Ball => {
                let f = u * 1.05;
                if f < 1.0 {
                    let t = if point { (f - 0.95).max(0.0) * 10.0 } else { 0.7 };
                    (t, 4e-3 * (1.05 - f * f).sqrt() / r2 * if point { 1.0 } else { 0.7 })
                } else if point {
                    ((f - 0.95) * 10.0, 17.8e-3 * (1.05 - f) / r2)
                } else {
                    ((f - 1.0) * 6.0 + 0.7, 12.5e-3 * (1.05 - f) / r2)
                }
            }
            GlowShape::Halo => (u.powi(4), 7e-3 * (1.0 - u) / r2),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Light {
    pub enabled: bool,
    pub kind: LightKind,
    /// Positional lights: position in the world (line lights: one end).
    pub position: V3,
    /// Spot and area lights: the point they face; line lights: the other end.
    pub target: V3,
    /// Spot lights: half angle of the cone in degrees.
    pub cone_angle: f64,
    /// Spot lights: share of the cone (from its edge inward) where the light fades (0..1).
    pub cone_softness: f64,
    /// Area lights: size of the rectangle in world units.
    pub width: f64,
    pub height: f64,
    /// Degrees; with `camera_relative` around the view direction, else world azimuth (from +x).
    pub azimuth: f64,
    /// Degrees above the horizon (or above the view plane).
    pub elevation: f64,
    pub camera_relative: bool,
    pub color: [f32; 3],
    pub intensity: f64,
    pub specular: f64,
    pub shadows: bool,
    /// Soft shadow width (0 = hard).
    pub softness: f64,
    pub diffuse: DiffuseCurve,
    /// How much the ambient occlusion darkens this light (0..1).
    pub occlusion: f64,
    /// The light source itself shows in the image as a glow.
    pub visible: bool,
    pub glow_shape: GlowShape,
    /// Glow radius: world units for point lights, degrees for directional lights.
    pub glow_size: f64,
    pub glow_intensity: f64,
    /// Lens flare of this light.
    pub flare: crate::flare::LensFlare,
}

impl Default for Light {
    fn default() -> Self {
        Light {
            enabled: true,
            kind: LightKind::Directional,
            position: v3(2.0, -2.0, 2.0),
            target: V3::ZERO,
            cone_angle: 25.0,
            cone_softness: 0.3,
            width: 0.5,
            height: 0.5,
            azimuth: -35.0,
            elevation: 40.0,
            camera_relative: true,
            color: [1.0, 0.96, 0.9],
            intensity: 1.0,
            specular: 0.5,
            shadows: true,
            softness: 0.08,
            diffuse: DiffuseCurve::Lambert,
            occlusion: 0.0,
            visible: false,
            glow_shape: GlowShape::Soft,
            glow_size: 0.1,
            glow_intensity: 1.0,
            flare: crate::flare::LensFlare::default(),
        }
    }
}

impl Light {
    /// Unit direction toward the light.
    pub fn direction(&self, cam: &Camera) -> V3 {
        let (az, el) = (self.azimuth.to_radians(), self.elevation.to_radians());
        if self.camera_relative {
            // Azimuth 0 / elevation 0: the light shines along the view direction (from behind the camera).
            (-cam.forward * (el.cos() * az.cos()) + cam.right * (el.cos() * az.sin()) + cam.up * el.sin()).normalize()
        } else {
            v3(el.cos() * az.cos(), el.cos() * az.sin(), el.sin()).normalize()
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum AoMode {
    Off,
    /// Five distance samples along the normal: fast, no noise.
    Fast,
    /// Random hemisphere rays, converges with the sample count.
    #[default]
    Raytraced,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Lighting {
    pub lights: Vec<Light>,
    pub ambient: f64,
    pub sky_color: [f32; 3],
    pub ground_color: [f32; 3],
    pub ao_mode: AoMode,
    pub ao_strength: f64,
    /// Ambient occlusion radius relative to the view distance.
    pub ao_radius: f64,
    pub shininess: f64,
    /// How much the surface roughness (measured with smoothed normals) dulls the highlights and
    /// softens the diffuse light (Mandelbulb3D's roughness factor, 0..1).
    pub roughness: f64,
    /// The sky and ground colors of the ambient light follow the camera's up axis
    /// (Mandelbulb3D) instead of the world z axis.
    pub ambient_camera_relative: bool,
    /// The ambient light comes from a blurred copy of the background image (around the surface
    /// normal) instead of the sky and ground colors.
    pub ambient_from_background: bool,
    /// How light is computed: classic (Mandelbulb3D style) or path traced (Monte Carlo).
    pub render_mode: RenderMode,
    /// Path tracing: light bounces per path (1..16).
    pub path_bounces: u32,
    /// Path tracing: the light one bounce adds is limited to this luminance (0 = no limit);
    /// removes single bright pixels ("fireflies") at the cost of a little energy.
    pub path_clamp: f64,
}

/// How the renderers compute the light.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum RenderMode {
    /// Lights with shadows, ambient light with occlusion, emission light rays, reflections as
    /// set (Mandelbulb3D style).
    #[default]
    Classic,
    /// Monte Carlo path tracing: rays bounce between surfaces (diffuse and reflective), every
    /// surface they meet adds the light of the lights and its emission; the ambient colors
    /// become the light of the sky.
    PathTraced,
}

impl RenderMode {
    pub const ALL: [RenderMode; 2] = [RenderMode::Classic, RenderMode::PathTraced];

    pub fn label(self) -> &'static str {
        match self {
            RenderMode::Classic => "Classic",
            RenderMode::PathTraced => "Path tracing",
        }
    }
}

impl Default for Lighting {
    fn default() -> Self {
        Lighting {
            lights: vec![
                Light::default(),
                Light {
                    azimuth: 120.0,
                    elevation: 10.0,
                    color: [0.55, 0.65, 0.9],
                    intensity: 0.35,
                    specular: 0.1,
                    shadows: false,
                    ..Light::default()
                },
            ],
            ambient: 0.35,
            sky_color: [0.75, 0.82, 1.0],
            ground_color: [0.35, 0.3, 0.28],
            ao_mode: AoMode::Raytraced,
            ao_strength: 0.9,
            ao_radius: 0.08,
            shininess: 40.0,
            roughness: 0.0,
            ambient_camera_relative: false,
            ambient_from_background: false,
            render_mode: RenderMode::Classic,
            path_bounces: 4,
            path_clamp: 10.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ColorMode {
    /// Smooth iteration count.
    #[default]
    Iterations,
    /// Closest approach of the orbit to the origin.
    OrbitTrap,
    /// Distance of the surface point from the origin.
    Radius,
    /// Surface normal direction.
    Normal,
    /// Smooth iteration count interpolated between the last two radii (Mandelbulb3D).
    IterationsRatio,
    /// Distance along `Material::color_direction` from `Material::color_center` (world units):
    /// the gradient runs through space in layers; one glow band is one sheet of light.
    Plane,
    /// Distance from `Material::color_center` (world units): the gradient runs outward in
    /// shells; one glow band is one sphere of light.
    Sphere,
    /// Mandelbulb3D's second color choice (`Material::second_choice`), 0..1.
    SecondChoice,
}

/// Mandelbulb3D's "mode for 2. color choice": a value from the end of the orbit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SecondChoice {
    /// Smallest squared length along the orbit.
    #[default]
    OrbitTrap,
    /// Logarithm of how much the last iteration grew the length (times `lli_multiplier`).
    LastLengthIncrease,
    /// Angle of the final vector relative to the constant in the x-y, x-z or y-z plane.
    AngleXY,
    AngleXZ,
    AngleYZ,
    /// Angle of the final vector around z (MB3D's "map on output vector").
    OutputVector,
}

impl SecondChoice {
    pub const ALL: [SecondChoice; 6] = [SecondChoice::OrbitTrap, SecondChoice::LastLengthIncrease, SecondChoice::AngleXY, SecondChoice::AngleXZ, SecondChoice::AngleYZ, SecondChoice::OutputVector];

    pub fn label(self) -> &'static str {
        match self {
            SecondChoice::OrbitTrap => "Orbit trap (min. distance to 0)",
            SecondChoice::LastLengthIncrease => "Last length increase",
            SecondChoice::AngleXY => "Rout angle of X, Y",
            SecondChoice::AngleXZ => "Rout angle of X, Z",
            SecondChoice::AngleYZ => "Rout angle of Y, Z",
            SecondChoice::OutputVector => "Map on output vector",
        }
    }

    pub fn index(self) -> u32 {
        self as u32
    }

    /// From Mandelbulb3D's `byColor2Option`.
    pub fn from_mb3d(v: u8) -> SecondChoice {
        SecondChoice::ALL[(v as usize).min(5)]
    }
}

impl ColorMode {
    pub const ALL: [ColorMode; 8] = [ColorMode::Iterations, ColorMode::IterationsRatio, ColorMode::OrbitTrap, ColorMode::SecondChoice, ColorMode::Radius, ColorMode::Normal, ColorMode::Plane, ColorMode::Sphere];

    pub fn label(self) -> &'static str {
        match self {
            ColorMode::Iterations => "Iterations",
            ColorMode::OrbitTrap => "Orbit trap",
            ColorMode::Radius => "Radius",
            ColorMode::Normal => "Normal",
            ColorMode::IterationsRatio => "Iterations (MB3D)",
            ColorMode::Plane => "Position: plane",
            ColorMode::Sphere => "Position: sphere",
            ColorMode::SecondChoice => "MB3D 2nd choice",
        }
    }

    pub fn index(self) -> u32 {
        self as u32
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ColorStop {
    pub pos: f64,
    pub color: [f32; 3],
    /// Color of the highlights at this stop (multiplies the specular light).
    #[serde(default = "white")]
    pub specular: [f32; 3],
    /// Share of light that passes through the surface at this stop (0..1; needs
    /// `Material::transmission`).
    #[serde(default)]
    pub transparency: f32,
    /// The surface shines in its color by itself with this strength (0 = not), lights its
    /// surroundings and glows (see `Material::emission_*`).
    #[serde(default)]
    pub emission: f32,
}

fn white() -> [f32; 3] {
    [1.0; 3]
}

impl ColorStop {
    pub fn new(pos: f64, color: [f32; 3]) -> ColorStop {
        ColorStop { pos, color, specular: white(), transparency: 0.0, emission: 0.0 }
    }
}

/// How a color map is put on the surface (Mandelbulb3D's color map modes).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum MapMode {
    /// 2D: orbit trap and iteration value as the two coordinates.
    #[default]
    Orbit,
    /// By the surface normal (panorama lookup).
    Normal,
    /// 3D position through sine waves.
    Wrap,
    /// 3D position, three planar projections blended by the normal.
    Triplanar,
}

impl MapMode {
    pub const ALL: [MapMode; 4] = [MapMode::Orbit, MapMode::Normal, MapMode::Wrap, MapMode::Triplanar];

    pub fn label(self) -> &'static str {
        match self {
            MapMode::Orbit => "Orbit trap × iterations",
            MapMode::Normal => "Surface normal",
            MapMode::Wrap => "3D waves",
            MapMode::Triplanar => "3D projection",
        }
    }

    pub fn index(self) -> u32 {
        self as u32
    }
}

/// An image that gives the surface color instead of the gradient.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ColorMap {
    /// File (path, or name in the map folders) or a Mandelbulb3D map number. Empty: off.
    pub image: String,
    pub mode: MapMode,
    /// Repetitions (orbit mode) or map tiles per world unit (3D modes).
    pub scale: f64,
    /// Degrees: the third is the 2D rotation of the orbit mode; normal mode uses all three.
    pub rotation: [f64; 3],
    pub offset: [f64; 2],
    /// Only the brightness of the map; the hue comes from the gradient.
    pub luminance_only: bool,
    /// Orbit mode: the iteration coordinate is value · scale + offset (0..1).
    pub iter_scale: f64,
    pub iter_offset: f64,
    /// The bright parts of the map shine by themselves (strength; 0 = off).
    pub emission: f64,
}

impl Default for ColorMap {
    fn default() -> Self {
        ColorMap { image: String::new(), mode: MapMode::Orbit, scale: 1.0, rotation: [0.0; 3], offset: [0.0; 2], luminance_only: false, iter_scale: 0.05, iter_offset: 0.0, emission: 0.0 }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Material {
    pub gradient: Vec<ColorStop>,
    /// Center of the position color modes (also used by the objects' materials).
    pub color_center: [f64; 3],
    /// Direction of the plane color mode (normalized when used).
    pub color_direction: [f64; 3],
    /// Mandelbulb3D's second color choice (color mode "MB3D 2nd choice", all materials).
    pub second_choice: SecondChoice,
    /// Multiplier of the "last length increase" choice.
    pub lli_multiplier: f64,
    /// Mandelbulb3D's "color on iteration": the colors come from the orbit after this many
    /// iterations (0: the start point) instead of the bailout; -1 = off. Main fractal only.
    pub color_on_iteration: i32,
    /// Glowing stripes over the gradient.
    pub bands: Vec<crate::gradient::GlowBand>,
    /// Animation: the gradient turns by this many lengths per second (moves `color_offset`).
    pub rotation: f64,
    pub color_mode: ColorMode,
    pub color_scale: f64,
    pub color_offset: f64,
    /// Repeat the gradient (else clamp at its ends).
    pub repeat: bool,
    pub specular: f64,
    /// Amount of the gradient (0 = all surfaces in the first gradient color).
    pub saturation: f64,
    /// Adds this times the natural log of the surface threshold (world units) to the
    /// iteration color value; keeps colors steady while zooming (Mandelbulb3D).
    pub detail_shift: f64,
    /// Shifts the gradient position by this much per world unit of distance beyond
    /// `depth_reference` (Mandelbulb3D's color variation with depth; 0 = off).
    pub depth_shift: f64,
    /// Distance from the camera where the depth shift is 0.
    pub depth_reference: f64,
    pub color_map: ColorMap,
    /// Texture layers over the gradient colors, bottom first.
    pub layers: Vec<crate::texture::TextureLayer>,
    /// Colors where a ray meets the inside of the fractal (cut planes, camera inside the solid);
    /// empty: the normal gradient. Indexed by the final radius (inside rendering: iterations).
    pub interior: Vec<ColorStop>,
    pub interior_scale: f64,
    pub interior_offset: f64,
    /// Mirror reflections: amount (0 = off) times the highlight color of the gradient.
    pub reflection: f64,
    /// Reflections of reflections.
    pub reflection_bounces: u32,
    /// Blur of reflections and refractions (rough metal, frosted glass): roughness of the
    /// surface's micro facets (GGX), 0 = mirror sharp.
    pub gloss: f64,
    /// Glass: light passes through surfaces by the transparency of the gradient stops, refracted
    /// and absorbed inside (Mandelbulb3D's transmission). Shares `reflection` (light amount) and
    /// `reflection_bounces` (path length) with the reflections.
    pub transmission: bool,
    /// Index of refraction of the glass (1 = no bending).
    pub refraction_index: f64,
    /// Absorption per world unit inside the glass; tints with the surface color where the ray
    /// entered.
    pub absorption: f64,
    /// Light scattered inside the glass (Mandelbulb3D's scattering, 0..330 and more).
    pub scattering: f64,
    /// Emissive colors (`ColorStop::emission`): how much they light their surroundings (0 =
    /// not; costs a ray per sample).
    pub emission_light: f64,
    /// How far that light reaches, relative to the distance from the camera.
    pub emission_reach: f64,
    /// Glow around emissive parts, collected along the rays.
    pub emission_glow: f64,
    /// Size of that glow, relative to the distance from the camera.
    pub emission_glow_size: f64,
    /// Rays per sample that gather the light of emissive colors (1..8; more: less noise).
    pub emission_rays: u32,
    /// Steam: glowing haze around emissive parts in their color, shaped by 3D noise (0 = off).
    pub steam: f64,
    /// How far the haze reaches from the surface, relative to the distance from the camera.
    pub steam_size: f64,
    /// Size of the noise blobs (world units).
    pub steam_grain: f64,
    /// Share of the space the haze fills (0..1).
    pub steam_cover: f64,
    /// Shift of the noise pattern (world units: the pattern moves by minus this); animations
    /// move it by `steam_drift`.
    pub steam_offset: [f64; 3],
    /// Animation: the pattern drifts by this much per second (world units; z is up).
    pub steam_drift: [f64; 3],
    /// Shimmer: the emitted light flickers by up to this share (0 = off) over patches of
    /// `shimmer_size` world units.
    pub shimmer: f64,
    pub shimmer_size: f64,
    /// Animation: new flicker patterns per second.
    pub shimmer_speed: f64,
    /// Where the flicker is (patterns; animations advance it by `shimmer_speed`).
    pub shimmer_phase: f64,
}

impl Default for Material {
    fn default() -> Self {
        Material {
            gradient: crate::gradient::preset("Copper").unwrap_or_default(),
            color_center: [0.0; 3],
            color_direction: [0.0, 0.0, 1.0],
            second_choice: SecondChoice::OrbitTrap,
            lli_multiplier: 1.0,
            color_on_iteration: -1,
            bands: Vec::new(),
            rotation: 0.0,
            color_mode: ColorMode::Iterations,
            color_scale: 0.15,
            color_offset: 0.0,
            repeat: true,
            specular: 1.0,
            saturation: 1.0,
            detail_shift: 0.0,
            depth_shift: 0.0,
            depth_reference: 0.0,
            color_map: ColorMap::default(),
            layers: Vec::new(),
            interior: Vec::new(),
            interior_scale: 1.0,
            interior_offset: 0.0,
            reflection: 0.0,
            reflection_bounces: 2,
            gloss: 0.0,
            transmission: false,
            refraction_index: 1.5,
            absorption: 1.0,
            scattering: 0.0,
            emission_light: 1.0,
            emission_reach: 0.5,
            emission_glow: 1.0,
            emission_glow_size: 0.02,
            emission_rays: 1,
            steam: 0.0,
            steam_size: 0.08,
            steam_grain: 0.1,
            steam_cover: 0.5,
            steam_offset: [0.0; 3],
            steam_drift: [0.0, 0.0, 0.05],
            shimmer: 0.0,
            shimmer_size: 0.05,
            shimmer_speed: 3.0,
            shimmer_phase: 0.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum BackgroundMode {
    Solid,
    #[default]
    Gradient,
    /// Top to bottom over the screen height (Mandelbulb3D).
    Vertical,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Background {
    pub mode: BackgroundMode,
    pub top: [f32; 3],
    pub horizon: [f32; 3],
    pub bottom: [f32; 3],
    /// Vertical mode: exponent of the screen position (1 linear, 2 more top color, 0.5 more bottom color).
    pub curve: f64,
    /// Panorama image (latitude/longitude) shown behind the object instead of the colors; the
    /// fog keeps using the colors. A path, or a file name in the map folders. Empty: none.
    pub image: String,
    /// Rotation of the image (degrees about x, y, z; Mandelbulb3D's picture offsets).
    pub image_rotation: [f64; 3],
    /// Brightness of the image (also of the ambient light taken from it).
    pub image_intensity: f64,
    /// Squeeze a flat image into a panorama on load (Mandelbulb3D's "convert to spherical").
    pub image_spherical: bool,
    /// Blend the left and right borders on load, so the panorama has no seam.
    pub image_fit_border: bool,
    /// The image fills the screen instead of surrounding the scene (reflections see it by angle).
    pub image_screen: bool,
    /// The fog is added on top of the image instead of covering it.
    pub image_fog_add: bool,
}

impl Default for Background {
    fn default() -> Self {
        Background {
            mode: BackgroundMode::Gradient,
            top: [0.08, 0.1, 0.16],
            horizon: [0.32, 0.34, 0.4],
            bottom: [0.06, 0.06, 0.07],
            curve: 1.0,
            image: String::new(),
            image_rotation: [0.0; 3],
            image_intensity: 1.0,
            image_spherical: false,
            image_fit_border: false,
            image_screen: false,
            image_fog_add: false,
        }
    }
}

/// What shapes the fractal haze.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum HazeShape {
    /// Thickest at the surfaces, fading with the distance from them: hugs every structure,
    /// fills narrow gaps.
    #[default]
    Distance,
    /// Where the fractal's orbits take long to escape (Mandelbulber's iteration fog): shells
    /// around the fractal, depending a lot on the formula.
    Iterations,
}

impl HazeShape {
    pub const ALL: [HazeShape; 2] = [HazeShape::Distance, HazeShape::Iterations];

    pub fn label(self) -> &'static str {
        match self {
            HazeShape::Distance => "Distance",
            HazeShape::Iterations => "Iterations",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FogMode {
    /// Exponential with the distance (`density`).
    #[default]
    Exponential,
    /// Grows over the view range from the near to the far distance (Mandelbulb3D's depth fog).
    Range,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Fog {
    pub mode: FogMode,
    /// Depth fog density per world unit (exponential mode; 0 = off).
    pub density: f64,
    /// Range mode: fog amount at the far end, relative to `start` (0 = off).
    pub strength: f64,
    /// Range mode: position in the range (0..1, after the curve) where the fog starts.
    pub start: f64,
    /// Range mode: bends the range like Mandelbulb3D's depth buffer (0 = linear).
    pub curve: f64,
    /// Range mode: square the fog amount (less fog near, Mandelbulb3D's "far fog").
    pub squared: bool,
    pub color: [f32; 3],
    /// The fog takes the background color behind each pixel instead of `color`.
    pub from_background: bool,
    /// Glow of near misses and long marches (0 = off, below 0 darkens).
    pub glow: f64,
    pub glow_color: [f32; 3],
    /// Ray steps that give no glow yet.
    pub glow_offset: f64,
    /// Second glow color, reached after `glow_mix_steps` ray steps.
    pub glow_color2: [f32; 3],
    /// Ray steps until the glow is fully the second color (0 = only the first color).
    pub glow_mix_steps: f64,
    /// The glow replaces the color instead of adding to it.
    pub glow_blend: bool,
    /// Only ray steps where the fractal escaped after this many iterations glow (0 = all steps;
    /// Mandelbulb3D's "dynamic fog on iteration").
    pub glow_iteration: u32,
    /// Light scattered by the fog toward the camera (0 = off): beams of light through gaps,
    /// lit haze around lights. One shadow ray per sample from a point in the fog.
    pub scatter: f64,
    /// Henyey-Greenstein anisotropy of the scattering: above 0 the fog glows most when looking
    /// toward a light, 0 scatters evenly.
    pub scatter_forward: f64,
    /// Haze shaped by the main fractal (0 = off): thickness of a glowing haze near its
    /// surfaces or where its orbits take long to escape (see `haze_shape`).
    pub haze: f64,
    pub haze_shape: HazeShape,
    /// Distance shape: how far the haze reaches from the surfaces, relative to the distance
    /// from the camera.
    pub haze_size: f64,
    /// Iterations shape: share of the maximum iterations (0..1) from which on a point lies in
    /// the haze; higher values keep it closer to the fractal.
    pub haze_start: f64,
    /// Iterations shape: share of the maximum iterations above which there is no haze again
    /// (1 = none cut): keeps it out of regions that never escape.
    pub haze_end: f64,
    /// Brightness of the haze's own light (0: a dark, absorbing smoke).
    pub haze_glow: f64,
    pub haze_color: [f32; 3],
    /// The haze takes its colors from the main gradient (by the smooth iteration count, like
    /// the "Iterations" color mode) instead of `haze_color`.
    pub haze_gradient: bool,
}

impl Default for Fog {
    fn default() -> Self {
        Fog {
            mode: FogMode::Exponential,
            density: 0.0,
            strength: 0.0,
            start: 0.0,
            curve: 0.0,
            squared: false,
            color: [0.32, 0.34, 0.4],
            from_background: false,
            glow: 0.0,
            glow_color: [0.5, 0.7, 1.0],
            glow_offset: 0.0,
            glow_color2: [0.5, 0.7, 1.0],
            glow_mix_steps: 0.0,
            glow_blend: false,
            glow_iteration: 0,
            scatter: 0.0,
            scatter_forward: 0.3,
            haze: 0.0,
            haze_shape: HazeShape::Distance,
            haze_size: 0.02,
            haze_start: 0.2,
            haze_end: 1.0,
            haze_glow: 1.0,
            haze_color: [0.25, 0.5, 1.0],
            haze_gradient: false,
        }
    }
}

impl Fog {
    /// Smooth iteration count of an orbit that ran `iterations` (squared radius `r2`, before the
    /// last iteration `r2_old`, squared bailout `bailout2`): the fraction of the last iteration
    /// comes from how fast the radius grew, which fits any power (march.wgsl `haze_iterations`).
    pub fn haze_iterations(iterations: u32, r2: f64, r2_old: f64, escaped: bool, bailout2: f64) -> f64 {
        let mut n = iterations as f64;
        if escaped && r2 > 1.0 {
            let lb = bailout2.max(4.0).sqrt().ln().ln();
            let d = (0.5 * r2.ln()).ln();
            let dd = if r2_old > 1.0 { d - (0.5 * r2_old.ln()).ln() } else { std::f64::consts::LN_2 };
            n += ((lb - d) / dd.max(1e-6)).clamp(-1.0, 0.0);
        }
        n
    }

    /// Density of the fractal haze (0..1) at a point with distance estimate `de` and smooth
    /// iteration count `n` of at most `max_iterations`, `dist` from the camera (march.wgsl
    /// `haze_density`). Distance shape: falls off from the surfaces over `haze_size` × `dist`.
    /// Iterations shape: the iteration share from `haze_start` to `haze_end`, squared for a
    /// soft onset.
    pub fn haze_density(&self, de: f64, n: f64, max_iterations: f64, dist: f64) -> f64 {
        match self.haze_shape {
            HazeShape::Distance => (-de.max(0.0) / (self.haze_size * dist).max(1e-30)).exp(),
            HazeShape::Iterations => {
                let share = n / max_iterations.max(1.0);
                if share > self.haze_end {
                    return 0.0;
                }
                let x = ((share - self.haze_start) / (self.haze_end - self.haze_start).max(1e-6)).clamp(0.0, 1.0);
                x * x
            }
        }
    }

    /// Fog amount at ray distance `t` (0 = none, 1 = only fog; below 0 brightens like
    /// Mandelbulb3D's depth fog in front of its start).
    pub fn amount(&self, t: f64, near: f64, far: f64) -> f64 {
        match self.mode {
            FogMode::Exponential => 1.0 - (-self.density * t).exp(),
            FogMode::Range => {
                if self.strength == 0.0 {
                    return 0.0;
                }
                let x = ((t - near) / (far - near).max(1e-30)).clamp(0.0, 1.0);
                let c = self.curve.max(0.0);
                let u = if c < 1e-6 { x } else { ((1.0 + c * x).sqrt() - 1.0) / ((1.0 + c).sqrt() - 1.0) };
                let f = (self.strength * (u - self.start)).min(1.0);
                if self.squared && f > 0.0 { f * f } else { f }
            }
        }
    }

    /// The fog distance in `0..=end` where the fog amount (clamped to 0..1) reaches `target`
    /// (light scattering samples the fog in proportion to how much it adds; same on the GPU).
    pub fn distance_for(&self, target: f64, near: f64, far: f64, end: f64) -> f64 {
        match self.mode {
            FogMode::Exponential => (-(1.0 - target).max(1e-30).ln() / self.density.max(1e-30)).clamp(0.0, end),
            FogMode::Range => {
                let (mut lo, mut hi) = (0.0, end);
                for _ in 0..24 {
                    let mid = 0.5 * (lo + hi);
                    if self.amount(mid, near, far).clamp(0.0, 1.0) < target {
                        lo = mid;
                    } else {
                        hi = mid;
                    }
                }
                0.5 * (lo + hi)
            }
        }
    }

    /// How fast the fog amount (clamped to 0..1) grows at distance `t`: the light scattered per
    /// unit of length that reaches the camera (same on the GPU).
    pub fn density_at(&self, t: f64, near: f64, far: f64) -> f64 {
        match self.mode {
            FogMode::Exponential => self.density * (-self.density * t).exp(),
            FogMode::Range => {
                let h = 1e-3 * (far - near).max(1e-9);
                let lo = (t - h).max(0.0);
                (self.amount(t + h, near, far).clamp(0.0, 1.0) - self.amount(lo, near, far).clamp(0.0, 1.0)) / (t + h - lo)
            }
        }
    }

    /// Henyey-Greenstein phase of the light scattering for the cosine between the light's
    /// direction (from the point toward the light) and the ray; 1 when it scatters evenly.
    pub fn scatter_phase(&self, cos: f64) -> f64 {
        let g = self.scatter_forward.clamp(-0.95, 0.95);
        (1.0 - g * g) / (1.0 + g * g - 2.0 * g * cos).max(1e-12).powf(1.5)
    }

    /// Glow amount and the mix toward the second glow color after `steps` ray steps.
    pub fn glow_at(&self, steps: f64, max_steps: f64) -> (f64, f64) {
        let g = self.glow * (steps - self.glow_offset).max(0.0) / max_steps.max(1.0);
        let mix = if self.glow_mix_steps > 0.0 { (steps / self.glow_mix_steps).min(1.0) } else { 0.0 };
        (g, mix)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Dof {
    pub enabled: bool,
    /// Lens radius relative to the focus distance.
    pub aperture: f64,
    pub focus_distance: f64,
}

impl Default for Dof {
    fn default() -> Self {
        Dof { enabled: false, aperture: 0.02, focus_distance: 2.5 }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Post {
    /// Exposure in stops.
    pub exposure: f64,
    pub gamma: f64,
    pub contrast: f64,
    pub saturation: f64,
    /// Filmic tone mapping (else clamp).
    pub tone_map: bool,
    pub vignette: f64,
    /// Shade with the colors as shown (gamma encoded) instead of linear light, like
    /// Mandelbulb3D. The output gamma then is relative to 2.2.
    pub display_space: bool,
    /// Run the AI denoiser (Intel Open Image Denoise) on finished images.
    pub denoise: bool,
    /// Share of the denoised image (1 = only denoised, 0 = original).
    pub denoise_blend: f64,
    /// Bloom: strength of the glow around bright parts (0 = off), the brightness above which
    /// it starts (after exposure, 1 = white) and its size (0 small .. 1 wide).
    pub bloom: f64,
    pub bloom_threshold: f64,
    pub bloom_size: f64,
    /// Streaks: lines of light through very bright parts (0 = off), the brightness above
    /// which they start, their length (share of the image width), angle (degrees), number of
    /// directions (1: a line, 2: a cross, 3 and 4: stars) and tint.
    pub streak: f64,
    pub streak_threshold: f64,
    pub streak_length: f64,
    pub streak_angle: f64,
    pub streak_count: u32,
    pub streak_tint: [f32; 3],
    /// Chromatic aberration: red drawn outward, blue inward from the image center, by this many
    /// percent of the distance from the center (0 = off).
    pub chromatic: f64,
    /// White balance: warmer (above 0) or cooler colors (-1..1).
    pub temperature: f64,
    /// White balance: toward magenta (above 0) or green (-1..1).
    pub tint: f64,
    /// Split toning: the dark parts take this color by `shadow_amount` (0..1), the bright parts
    /// `highlight_color` by `highlight_amount`; brightness stays.
    pub shadow_color: [f32; 3],
    pub shadow_amount: f64,
    pub highlight_color: [f32; 3],
    pub highlight_amount: f64,
}

impl Default for Post {
    fn default() -> Self {
        Post {
            exposure: 0.0,
            gamma: 2.2,
            contrast: 1.0,
            saturation: 1.0,
            tone_map: true,
            vignette: 0.15,
            display_space: false,
            denoise: false,
            denoise_blend: 1.0,
            bloom: 0.0,
            bloom_threshold: 1.0,
            bloom_size: 0.6,
            streak: 0.0,
            streak_threshold: 2.0,
            streak_length: 0.3,
            streak_angle: 0.0,
            streak_count: 1,
            streak_tint: [0.6, 0.8, 1.0],
            chromatic: 0.0,
            temperature: 0.0,
            tint: 0.0,
            shadow_color: [0.1, 0.45, 0.55],
            shadow_amount: 0.0,
            highlight_color: [1.0, 0.6, 0.3],
            highlight_amount: 0.0,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CutPlane {
    pub enabled: bool,
    pub position: f64,
    /// Keep the side above the position (else below).
    pub keep_above: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Cutting {
    pub x: CutPlane,
    pub y: CutPlane,
    pub z: CutPlane,
}

impl Cutting {
    pub fn planes(&self) -> [&CutPlane; 3] {
        [&self.x, &self.y, &self.z]
    }
}
