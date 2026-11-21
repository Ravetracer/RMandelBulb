// SPDX-License-Identifier: LGPL-2.1-or-later

//! The scene: everything needed to render one image.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::math::{V3, v3};

pub const FORMAT_VERSION: u32 = 1;
/// Number of formula slots of a hybrid (as in Mandelbulb3D).
pub const MAX_SLOTS: usize = 6;
pub const MAX_LIGHTS: usize = 6;

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
}

impl CombineMode {
    pub const ALL: [CombineMode; 5] = [CombineMode::Min, CombineMode::Max, CombineMode::Subtract, CombineMode::SmoothLinear, CombineMode::Smooth];

    pub fn label(self) -> &'static str {
        match self {
            CombineMode::Min => "Union (min)",
            CombineMode::Max => "Intersection (max)",
            CombineMode::Subtract => "Subtract second",
            CombineMode::SmoothLinear => "Union, linear blend",
            CombineMode::Smooth => "Union, smooth blend",
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
}

impl Default for DeCombine {
    fn default() -> Self {
        DeCombine { mode: CombineMode::Min, start: 1, repeat_from: 1, max_iterations: 12, smooth: 0.05 }
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
}

impl Default for March {
    fn default() -> Self {
        March { detail: 1.0, step: 0.8, max_steps: 400, max_distance: 40.0, refine_steps: 4, near: 0.0 }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Light {
    pub enabled: bool,
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
}

impl Default for Light {
    fn default() -> Self {
        Light {
            enabled: true,
            azimuth: -35.0,
            elevation: 40.0,
            camera_relative: true,
            color: [1.0, 0.96, 0.9],
            intensity: 1.0,
            specular: 0.5,
            shadows: true,
            softness: 0.08,
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
}

impl ColorMode {
    pub const ALL: [ColorMode; 4] = [ColorMode::Iterations, ColorMode::OrbitTrap, ColorMode::Radius, ColorMode::Normal];

    pub fn label(self) -> &'static str {
        match self {
            ColorMode::Iterations => "Iterations",
            ColorMode::OrbitTrap => "Orbit trap",
            ColorMode::Radius => "Radius",
            ColorMode::Normal => "Normal",
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
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Material {
    pub gradient: Vec<ColorStop>,
    pub color_mode: ColorMode,
    pub color_scale: f64,
    pub color_offset: f64,
    /// Repeat the gradient (else clamp at its ends).
    pub repeat: bool,
    pub specular: f64,
    /// Amount of the gradient (0 = all surfaces in the first gradient color).
    pub saturation: f64,
}

impl Default for Material {
    fn default() -> Self {
        Material {
            gradient: crate::gradient::preset("Copper").unwrap_or_default(),
            color_mode: ColorMode::Iterations,
            color_scale: 0.15,
            color_offset: 0.0,
            repeat: true,
            specular: 1.0,
            saturation: 1.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum BackgroundMode {
    Solid,
    #[default]
    Gradient,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Background {
    pub mode: BackgroundMode,
    pub top: [f32; 3],
    pub horizon: [f32; 3],
    pub bottom: [f32; 3],
}

impl Default for Background {
    fn default() -> Self {
        Background { mode: BackgroundMode::Gradient, top: [0.08, 0.1, 0.16], horizon: [0.32, 0.34, 0.4], bottom: [0.06, 0.06, 0.07] }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Fog {
    /// Depth fog density per world unit (0 = off).
    pub density: f64,
    pub color: [f32; 3],
    /// Glow of near misses and long marches (0 = off).
    pub glow: f64,
    pub glow_color: [f32; 3],
}

impl Default for Fog {
    fn default() -> Self {
        Fog { density: 0.0, color: [0.32, 0.34, 0.4], glow: 0.0, glow_color: [0.5, 0.7, 1.0] }
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
}

impl Default for Post {
    fn default() -> Self {
        Post { exposure: 0.0, gamma: 2.2, contrast: 1.0, saturation: 1.0, tone_map: true, vignette: 0.15 }
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
