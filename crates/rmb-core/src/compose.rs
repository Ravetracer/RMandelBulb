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
    /// A water surface at z = 0 with waves (three octaves at turning directions).
    Water { amplitude: f64, wavelength: f64, phase: f64, angle: f64 },
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
            Primitive::Water { amplitude: 0.02, wavelength: 0.3, phase: 0.0, angle: 0.0 },
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

    /// The parameters in a fixed order (GPU buffer).
    pub fn params(&self) -> [f64; 4] {
        match *self {
            Primitive::Plane => [0.0; 4],
            Primitive::Box { size, rounding } => [size[0], size[1], size[2], rounding],
            Primitive::Sphere { radius } => [radius, 0.0, 0.0, 0.0],
            Primitive::Cylinder { radius, height, rounding } => [radius, height, rounding, 0.0],
            Primitive::Torus { major, minor } => [major, minor, 0.0, 0.0],
            Primitive::Water { amplitude, wavelength, phase, angle } => [amplitude, wavelength, phase, angle],
            Primitive::Capsule { radius, height } => [radius, height, 0.0, 0.0],
            Primitive::Cone { bottom, top, height } => [bottom, top, height, 0.0],
            Primitive::HexPrism { radius, height } => [radius, height, 0.0, 0.0],
            Primitive::TriPrism { side, height } => [side, height, 0.0, 0.0],
        }
    }

    /// Signed distance from the local point `q` (negative inside).
    pub fn distance(&self, q: V3) -> f64 {
        primitive_distance(self.index(), self.params(), q)
    }
}

/// Signed distance of primitive `kind` with parameters `a` at the local point `q`.
pub fn primitive_distance(kind: u32, a: [f64; 4], q: V3) -> f64 {
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
        _ => {
            let (h, slope) = water_height(a, q.x, q.y);
            (q.z - h) / (1.0 + slope * slope).sqrt()
        }
    }
}

/// Height of the water waves at (`x`, `y`) and the largest slope they can have.
fn water_height(a: [f64; 4], x: f64, y: f64) -> (f64, f64) {
    let (amp, len, phase, angle) = (a[0], a[1].max(1e-6), a[2], a[3].to_radians());
    let mut h = 0.0;
    let mut slope = 0.0;
    for i in 0..3 {
        let f = 2f64.powi(i);
        let dir = angle + i as f64 * 1.1;
        let k = std::f64::consts::TAU * f / len;
        let amp_i = amp / f;
        h += amp_i * (k * (x * dir.cos() + y * dir.sin()) + phase * (1.0 + 0.3 * i as f64)).sin();
        slope += amp_i * k;
    }
    (h, slope)
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
        let w = Primitive::Water { amplitude: 0.05, wavelength: 0.3, phase: 0.4, angle: 20.0 };
        for i in 0..200 {
            let (x, y) = (i as f64 * 0.013, i as f64 * 0.007);
            let (h, _) = water_height(w.params(), x, y);
            let d = w.distance(v3(x, y, 0.5));
            assert!(d <= 0.5 - h + 1e-12 && d > 0.0);
        }
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
