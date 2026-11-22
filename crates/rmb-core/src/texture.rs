// SPDX-License-Identifier: LGPL-2.1-or-later

//! Texture layers of a material: images (or a solid color) put on the surface by a projection,
//! blended over the gradient colors with a blend mode, optionally through a mask image. A layer
//! can also bump the surface normal or add emission. Mirrored in march.wgsl (`apply_layers`,
//! `bump_layers`).

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::maps::Map;
use crate::math::{V3, v3};

/// Layers per material (the GPU reserves this many).
pub const MAX_LAYERS: usize = 4;

/// How the image is projected onto the surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Mapping {
    /// Three planar projections blended by the surface normal (no seams; good for fractals).
    #[default]
    Triplanar,
    PlanarXY,
    PlanarXZ,
    PlanarYZ,
    /// Longitude and latitude around the center.
    Spherical,
    /// Around the z axis, height along z.
    Cylindrical,
    /// By the fractal's color value and orbit trap (like Mandelbulb3D's orbit color maps).
    Orbit,
}

impl Mapping {
    pub const ALL: [Mapping; 7] = [Mapping::Triplanar, Mapping::PlanarXY, Mapping::PlanarXZ, Mapping::PlanarYZ, Mapping::Spherical, Mapping::Cylindrical, Mapping::Orbit];

    pub fn label(self) -> &'static str {
        match self {
            Mapping::Triplanar => "Triplanar",
            Mapping::PlanarXY => "Planar XY",
            Mapping::PlanarXZ => "Planar XZ",
            Mapping::PlanarYZ => "Planar YZ",
            Mapping::Spherical => "Spherical",
            Mapping::Cylindrical => "Cylindrical",
            Mapping::Orbit => "Orbit",
        }
    }

    pub fn index(self) -> u32 {
        self as u32
    }
}

/// Coordinates the projection works in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Space {
    /// The object's own coordinates: the texture moves and turns with the object.
    #[default]
    Object,
    World,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum BlendMode {
    #[default]
    Normal,
    Multiply,
    Screen,
    Overlay,
    SoftLight,
    Add,
    Subtract,
    Darken,
    Lighten,
    Difference,
    /// The layer's hue and saturation with the brightness below.
    Color,
}

impl BlendMode {
    pub const ALL: [BlendMode; 11] = [
        BlendMode::Normal,
        BlendMode::Multiply,
        BlendMode::Screen,
        BlendMode::Overlay,
        BlendMode::SoftLight,
        BlendMode::Add,
        BlendMode::Subtract,
        BlendMode::Darken,
        BlendMode::Lighten,
        BlendMode::Difference,
        BlendMode::Color,
    ];

    pub fn label(self) -> &'static str {
        match self {
            BlendMode::Normal => "Normal",
            BlendMode::Multiply => "Multiply",
            BlendMode::Screen => "Screen",
            BlendMode::Overlay => "Overlay",
            BlendMode::SoftLight => "Soft light",
            BlendMode::Add => "Add",
            BlendMode::Subtract => "Subtract",
            BlendMode::Darken => "Darken",
            BlendMode::Lighten => "Lighten",
            BlendMode::Difference => "Difference",
            BlendMode::Color => "Color",
        }
    }

    pub fn index(self) -> u32 {
        self as u32
    }
}

/// What a layer changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Target {
    /// The surface color.
    #[default]
    Color,
    /// The surface relief: the brightness is a height that bends the normal.
    Bump,
    /// Glow: bright parts emit light (adds to the gradient's emission).
    Emission,
    /// Shine: the brightness scales the highlights (white: unchanged, black: none).
    Specular,
    /// Roughness: the brightness replaces the surface roughness (dull highlights, softer
    /// diffuse light where bright).
    Roughness,
    /// Normal map: tangent-space normals (red right, green up, blue out) bend the surface.
    Normal,
}

impl Target {
    pub const ALL: [Target; 6] = [Target::Color, Target::Bump, Target::Normal, Target::Emission, Target::Specular, Target::Roughness];

    pub fn label(self) -> &'static str {
        match self {
            Target::Color => "Color",
            Target::Bump => "Bump",
            Target::Emission => "Emission",
            Target::Specular => "Specular",
            Target::Roughness => "Roughness",
            Target::Normal => "Normal map",
        }
    }

    /// The layer bends the normal (bump or normal map).
    pub fn bends(self) -> bool {
        matches!(self, Target::Bump | Target::Normal)
    }

    pub fn index(self) -> u32 {
        self as u32
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TextureLayer {
    pub enabled: bool,
    /// Image file (path, or name in the map folders, or a Mandelbulb3D map number). Empty: the
    /// solid `color`.
    pub image: String,
    pub color: [f32; 3],
    /// Image whose brightness is the layer's opacity per point (empty: none).
    pub mask: String,
    pub mask_invert: bool,
    /// Sharpens (> 1) or softens (< 1) the mask around its middle gray.
    pub mask_contrast: f64,
    pub mapping: Mapping,
    pub space: Space,
    /// Image tiles per unit (spherical, cylindrical, orbit: repetitions).
    pub scale: f64,
    pub offset: [f64; 3],
    /// Degrees around x, y, z.
    pub rotation: [f64; 3],
    pub blend: BlendMode,
    pub opacity: f64,
    pub target: Target,
    /// Bump: height per brightness step of one texel; emission: brightness of the glow;
    /// normal map: how far its normals bend the surface (1: as drawn).
    pub strength: f64,
    /// The image's alpha channel (if it has one) multiplies the opacity.
    pub use_alpha: bool,
    /// Animations: the offset moves this far per second (texture scrolling).
    pub drift: [f64; 3],
}

impl Default for TextureLayer {
    fn default() -> Self {
        TextureLayer {
            enabled: true,
            image: String::new(),
            color: [1.0, 1.0, 1.0],
            mask: String::new(),
            mask_invert: false,
            mask_contrast: 1.0,
            mapping: Mapping::Triplanar,
            space: Space::Object,
            scale: 1.0,
            offset: [0.0; 3],
            rotation: [0.0; 3],
            blend: BlendMode::Normal,
            opacity: 1.0,
            target: Target::Color,
            strength: 4.0,
            use_alpha: true,
            drift: [0.0; 3],
        }
    }
}

/// A layer prepared for shading: its images and rotation.
#[derive(Clone)]
pub struct Prepared {
    pub layer: TextureLayer,
    pub image: Option<Arc<Map>>,
    pub mask: Option<Arc<Map>>,
    /// Rotation applied to the coordinates (rows).
    pub rot: [[f64; 3]; 3],
}

/// The enabled layers of a list that can be drawn (an image layer whose file is missing is
/// left out), at most [`MAX_LAYERS`].
pub fn prepare(layers: &[TextureLayer]) -> Vec<Prepared> {
    layers
        .iter()
        .filter(|l| l.enabled)
        .filter_map(|l| {
            let image = crate::maps::get_any(&l.image);
            if image.is_none() && !l.image.trim().is_empty() {
                return None;
            }
            Some(Prepared { layer: l.clone(), image, mask: crate::maps::get_any(&l.mask), rot: crate::maps::rotation_matrix(l.rotation) })
        })
        .take(MAX_LAYERS)
        .collect()
}

/// Where a layer is evaluated.
#[derive(Clone, Copy, Debug)]
pub struct Coord {
    /// Position in the material's object coordinates (world for the main fractal).
    pub obj: V3,
    pub world: V3,
    /// Surface normal in object and world coordinates (zero: unknown).
    pub n_obj: V3,
    pub n_world: V3,
    /// Orbit mapping: color value and orbit trap value.
    pub value: f64,
    pub trap: f64,
}

pub fn rotate(m: &[[f64; 3]; 3], v: V3) -> V3 {
    v3(m[0][0] * v.x + m[0][1] * v.y + m[0][2] * v.z, m[1][0] * v.x + m[1][1] * v.y + m[1][2] * v.z, m[2][0] * v.x + m[2][1] * v.y + m[2][2] * v.z)
}

/// Texture coordinates and weights of a layer at `q` (in the layer's space, not yet rotated) with
/// normal `n` (same space; zero: unknown). Up to three projections.
pub fn uv_sets(p: &Prepared, q: V3, n: V3, value: f64, trap: f64) -> [(f64, f64, f64); 3] {
    let l = &p.layer;
    let d = rotate(&p.rot, q);
    let s = l.scale;
    let o = l.offset;
    let a = d * s + v3(o[0], o[1], o[2]);
    let one = |u: f64, v: f64| [(u, v, 1.0), (0.0, 0.0, 0.0), (0.0, 0.0, 0.0)];
    let tau = std::f64::consts::TAU;
    match l.mapping {
        Mapping::Triplanar => {
            let rn = rotate(&p.rot, n);
            let w = [rn.x, rn.y, rn.z].map(|c| c * c * c * c);
            let sum = w[0] + w[1] + w[2];
            let w = if sum > 1e-12 { w.map(|c| c / sum) } else { [1.0 / 3.0; 3] };
            [(a.y, a.z, w[0]), (a.x, a.z, w[1]), (a.x, a.y, w[2])]
        }
        Mapping::PlanarXY => one(a.x, a.y),
        Mapping::PlanarXZ => one(a.x, a.z),
        Mapping::PlanarYZ => one(a.y, a.z),
        Mapping::Spherical => {
            let len = d.length().max(1e-30);
            one(d.y.atan2(d.x) / tau * s + o[0], (d.z / len).clamp(-1.0, 1.0).acos() / std::f64::consts::PI * s + o[1])
        }
        Mapping::Cylindrical => one(d.y.atan2(d.x) / tau * s + o[0], d.z * s + o[1]),
        Mapping::Orbit => one(value * s + o[0], trap.max(0.0).sqrt() * 4.0 * s + o[1]),
    }
}

/// Bilinear RGB (0..1, display values) of an image at `u`, `v`, tiled in both directions.
pub fn bilinear(m: &Map, u: f64, v: f64) -> [f64; 3] {
    let (w, h) = (m.width as f64, m.height as f64);
    let x = (u - u.floor()) * w - 0.5;
    let y = (v - v.floor()) * h - 0.5;
    let (xf, yf) = (x.floor(), y.floor());
    let (fx, fy) = (x - xf, y - yf);
    let (x0, y0) = (xf as i64, yf as i64);
    let t = |dx: i64, dy: i64| {
        let p = m.pixels[((y0 + dy).rem_euclid(m.height as i64) * m.width as i64 + (x0 + dx).rem_euclid(m.width as i64)) as usize];
        [p[2] as f64, p[1] as f64, p[0] as f64]
    };
    let (a, b, c, d) = (t(0, 0), t(1, 0), t(0, 1), t(1, 1));
    [0, 1, 2].map(|k| ((a[k] * (1.0 - fx) + b[k] * fx) * (1.0 - fy) + (c[k] * (1.0 - fx) + d[k] * fx) * fy) / 255.0)
}

/// Bilinear opacity (0..1) of an image at `u`, `v` like [`bilinear`] (1 without alpha).
pub fn bilinear_alpha(m: &Map, u: f64, v: f64) -> f64 {
    let Some(alpha) = &m.alpha else { return 1.0 };
    let (w, h) = (m.width as f64, m.height as f64);
    let x = (u - u.floor()) * w - 0.5;
    let y = (v - v.floor()) * h - 0.5;
    let (xf, yf) = (x.floor(), y.floor());
    let (fx, fy) = (x - xf, y - yf);
    let (x0, y0) = (xf as i64, yf as i64);
    let t = |dx: i64, dy: i64| alpha[((y0 + dy).rem_euclid(m.height as i64) * m.width as i64 + (x0 + dx).rem_euclid(m.width as i64)) as usize] as f64;
    ((t(0, 0) * (1.0 - fx) + t(1, 0) * fx) * (1.0 - fy) + (t(0, 1) * (1.0 - fx) + t(1, 1) * fx) * fy) / 255.0
}

pub fn luminance(c: [f64; 3]) -> f64 {
    0.3 * c[0] + 0.59 * c[1] + 0.11 * c[2]
}

/// Weighted sum of an image over the projections.
fn sample(m: &Map, sets: &[(f64, f64, f64); 3]) -> [f64; 3] {
    let mut out = [0.0; 3];
    for &(u, v, w) in sets {
        if w > 0.0 {
            let c = bilinear(m, u, v);
            for k in 0..3 {
                out[k] += c[k] * w;
            }
        }
    }
    out
}

/// The layer's color (display values) and opacity at a point.
fn layer_at(p: &Prepared, sets: &[(f64, f64, f64); 3]) -> ([f64; 3], f64) {
    let l = &p.layer;
    let c = match &p.image {
        Some(m) => sample(m, sets),
        None => l.color.map(|v| v as f64),
    };
    let mut a = l.opacity;
    if let Some(m) = p.image.as_ref().filter(|m| l.use_alpha && m.alpha.is_some()) {
        a *= sets.iter().filter(|s| s.2 > 0.0).map(|&(u, v, w)| bilinear_alpha(m, u, v) * w).sum::<f64>();
    }
    if let Some(m) = &p.mask {
        let mut v = luminance(sample(m, sets));
        if l.mask_invert {
            v = 1.0 - v;
        }
        a *= ((v - 0.5) * l.mask_contrast + 0.5).clamp(0.0, 1.0);
    }
    (c, a)
}

/// Blends `s` over `b` by `mode` (full strength; the caller mixes by the opacity).
pub fn blend(mode: BlendMode, b: [f64; 3], s: [f64; 3]) -> [f64; 3] {
    let per = |f: &dyn Fn(f64, f64) -> f64| [0, 1, 2].map(|k| f(b[k], s[k]));
    match mode {
        BlendMode::Normal => s,
        BlendMode::Multiply => per(&|b, s| b * s),
        BlendMode::Screen => per(&|b, s| 1.0 - (1.0 - b) * (1.0 - s)),
        BlendMode::Overlay => per(&|b, s| if b < 0.5 { 2.0 * b * s } else { 1.0 - 2.0 * (1.0 - b) * (1.0 - s) }),
        BlendMode::SoftLight => per(&|b, s| (1.0 - 2.0 * s) * b * b + 2.0 * s * b),
        BlendMode::Add => per(&|b, s| b + s),
        BlendMode::Subtract => per(&|b, s| (b - s).max(0.0)),
        BlendMode::Darken => per(&|b, s| b.min(s)),
        BlendMode::Lighten => per(&|b, s| b.max(s)),
        BlendMode::Difference => per(&|b, s| (b - s).abs()),
        BlendMode::Color => {
            let d = luminance(b) - luminance(s);
            s.map(|v| (v + d).max(0.0))
        }
    }
}

/// Layer coordinates: object or world space.
fn space(p: &Prepared, c: &Coord) -> (V3, V3) {
    match p.layer.space {
        Space::Object => (c.obj, c.n_obj),
        Space::World => (c.world, c.n_world),
    }
}

/// What the color, emission, specular and roughness layers make of a surface.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Applied {
    pub base: [f64; 3],
    pub emis: f64,
    /// Factor of the highlights.
    pub spec: f64,
    /// The roughness becomes `rough * rough_keep + rough_add`.
    pub rough_keep: f64,
    pub rough_add: f64,
}

/// Applies the color, emission, specular and roughness layers to the surface color `base`
/// and emission `emis`. `decode`: turn the image colors into linear values (the gradient is
/// linear).
pub fn apply(layers: &[Prepared], c: &Coord, decode: bool, base: [f64; 3], emis: f64) -> Applied {
    let mut col = base;
    let mut e = emis;
    let (mut spec, mut keep, mut add) = (1.0, 1.0, 0.0);
    for p in layers {
        let l = &p.layer;
        if l.target.bends() {
            continue;
        }
        let (q, n) = space(p, c);
        let sets = uv_sets(p, q, n, c.value, c.trap);
        let (mut s, a) = layer_at(p, &sets);
        if a <= 0.0 {
            continue;
        }
        match l.target {
            Target::Emission => {
                e += l.strength * luminance(s) * a;
                continue;
            }
            Target::Specular => {
                spec *= 1.0 + (luminance(s) - 1.0) * a;
                continue;
            }
            Target::Roughness => {
                keep *= 1.0 - a;
                add = add * (1.0 - a) + luminance(s) * a;
                continue;
            }
            _ => {}
        }
        // Images hold display values; the solid color is linear like the gradient.
        if decode && p.image.is_some() {
            s = s.map(crate::render::srgb_decode);
        }
        let r = blend(l.blend, col, s);
        col = [0, 1, 2].map(|k| col[k] + (r[k] - col[k]) * a);
    }
    Applied { base: col, emis: e, spec, rough_keep: keep, rough_add: add }
}

/// Orthonormal tangents of a unit normal (Duff et al. 2017).
pub fn tangents(n: V3) -> (V3, V3) {
    let sign = if n.z >= 0.0 { 1.0 } else { -1.0 };
    let a = -1.0 / (sign + n.z);
    let b = n.x * n.y * a;
    (v3(1.0 + sign * n.x * n.x * a, sign * b, -sign * n.x), v3(b, sign + n.y * n.y * a, -n.y))
}

/// The world normal `n` bent by the bump and normal map layers. `to_world` turns an
/// object-space direction into world space.
pub fn bump(layers: &[Prepared], c: &Coord, to_world: &dyn Fn(V3) -> V3) -> V3 {
    let mut n_out = c.n_world;
    for p in layers {
        let l = &p.layer;
        if !l.target.bends() || l.mapping == Mapping::Orbit {
            continue;
        }
        let (q, n) = space(p, c);
        if n.length() < 0.5 {
            continue;
        }
        let sets = uv_sets(p, q, n, c.value, c.trap);
        let a = layer_at(p, &sets).1;
        if a <= 0.0 {
            continue;
        }
        // One texel in the layer's coordinates.
        let size = p.image.as_ref().map_or(1.0, |m| m.width.max(m.height) as f64);
        let e = 1.0 / (l.scale.abs().max(1e-9) * size);
        let (t1, t2) = tangents(n);
        if l.target == Target::Normal {
            // Per projection: the directions in which u and v grow on the surface (one texel
            // steps), the image's normal in that frame (rows go down the image: green is -v).
            let Some(m) = &p.image else { continue };
            let s1 = uv_sets(p, q + t1 * e, n, c.value, c.trap);
            let s2 = uv_sets(p, q + t2 * e, n, c.value, c.trap);
            let mut acc = V3::ZERO;
            for i in 0..3 {
                let (u, v, w) = sets[i];
                if w <= 0.0 {
                    continue;
                }
                let tu = t1 * (s1[i].0 - u) + t2 * (s2[i].0 - u);
                let tv = t1 * (s1[i].1 - v) + t2 * (s2[i].1 - v);
                if tu.length() < 1e-30 || tv.length() < 1e-30 {
                    continue;
                }
                let col = bilinear(m, u, v);
                let (x, y, z) = (col[0] * 2.0 - 1.0, col[1] * 2.0 - 1.0, col[2] * 2.0 - 1.0);
                acc += ((tu.normalize() * x - tv.normalize() * y) * l.strength + n * z.max(0.0)).normalize() * w;
            }
            if acc.length() < 1e-30 {
                continue;
            }
            let d = (acc.normalize() - n) * a;
            n_out += if l.space == Space::Object { to_world(d) } else { d };
            continue;
        }
        let h = |dq: V3| luminance(layer_at(p, &uv_sets(p, q + dq, n, c.value, c.trap)).0);
        let g1 = (h(t1 * e) - h(t1 * -e)) * 0.5;
        let g2 = (h(t2 * e) - h(t2 * -e)) * 0.5;
        let g = (t1 * g1 + t2 * g2) * (l.strength * a);
        let gw = if l.space == Space::Object { to_world(g) } else { g };
        n_out = n_out - gw;
    }
    n_out.normalize()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checker() -> Arc<Map> {
        // 2 × 2: black, white / white, black (blue, green, red order).
        Arc::new(Map { number: -1, width: 2, height: 2, pixels: vec![[0; 3], [255; 3], [255; 3], [0; 3]], alpha: None })
    }

    fn prepared(l: TextureLayer, image: Option<Arc<Map>>) -> Prepared {
        Prepared { rot: crate::maps::rotation_matrix(l.rotation), layer: l, image, mask: None }
    }

    #[test]
    fn blend_modes() {
        let b = [0.25, 0.5, 0.75];
        let s = [0.5, 0.5, 0.5];
        assert_eq!(blend(BlendMode::Normal, b, s), s);
        assert_eq!(blend(BlendMode::Multiply, b, s), [0.125, 0.25, 0.375]);
        assert_eq!(blend(BlendMode::Screen, b, s), [0.625, 0.75, 0.875]);
        assert_eq!(blend(BlendMode::Overlay, b, s), [0.25, 0.5, 0.75]);
        assert_eq!(blend(BlendMode::SoftLight, b, s), b);
        assert_eq!(blend(BlendMode::Add, b, s), [0.75, 1.0, 1.25]);
        assert_eq!(blend(BlendMode::Subtract, b, s), [0.0, 0.0, 0.25]);
        assert_eq!(blend(BlendMode::Darken, b, s), [0.25, 0.5, 0.5]);
        assert_eq!(blend(BlendMode::Lighten, b, s), [0.5, 0.5, 0.75]);
        assert_eq!(blend(BlendMode::Difference, b, s), [0.25, 0.0, 0.25]);
        let c = blend(BlendMode::Color, b, [1.0, 0.0, 0.0]);
        assert!((luminance(c) - luminance(b)).abs() < 1e-12);
        assert!(c[0] > c[1] && c[1] == c[2]);
    }

    #[test]
    fn mappings() {
        let n = v3(0.0, 0.0, 1.0);
        let p = prepared(TextureLayer { scale: 2.0, offset: [0.5, 0.0, 0.0], ..Default::default() }, None);
        // Triplanar on a surface facing up: only the xy projection.
        let s = uv_sets(&p, v3(0.25, 0.5, 3.0), n, 0.0, 0.0);
        assert_eq!(s[2], (1.0, 1.0, 1.0));
        assert_eq!(s[0].2, 0.0);
        // Unknown normal: equal weights.
        let s = uv_sets(&p, v3(0.0, 0.0, 0.0), V3::ZERO, 0.0, 0.0);
        assert!((s[0].2 - 1.0 / 3.0).abs() < 1e-12);
        let p = prepared(TextureLayer { mapping: Mapping::Cylindrical, ..Default::default() }, None);
        let s = uv_sets(&p, v3(0.0, 1.0, 0.5), n, 0.0, 0.0);
        assert!((s[0].0 - 0.25).abs() < 1e-12 && (s[0].1 - 0.5).abs() < 1e-12);
        let p = prepared(TextureLayer { mapping: Mapping::Spherical, ..Default::default() }, None);
        let s = uv_sets(&p, v3(0.0, 0.0, -2.0), n, 0.0, 0.0);
        assert!((s[0].1 - 1.0).abs() < 1e-12);
        // Rotation by 90° about z turns x into y.
        let p = prepared(TextureLayer { mapping: Mapping::PlanarXY, rotation: [0.0, 0.0, 90.0], ..Default::default() }, None);
        let s = uv_sets(&p, v3(1.0, 0.0, 0.0), n, 0.0, 0.0);
        assert!(s[0].0.abs() < 1e-12 && (s[0].1 - 1.0).abs() < 1e-12);
    }

    #[test]
    fn bilinear_tiles() {
        let m = checker();
        // Texel centers.
        assert_eq!(bilinear(&m, 0.25, 0.25), [0.0; 3]);
        assert_eq!(bilinear(&m, 0.75, 0.25), [1.0; 3]);
        assert_eq!(bilinear(&m, 1.75, -0.75), [1.0; 3]);
        // Between them.
        assert!((bilinear(&m, 0.5, 0.25)[0] - 0.5).abs() < 1e-12);
    }

    #[test]
    fn color_mask_and_emission_layers() {
        let c = Coord { obj: v3(0.25, 0.25, 0.0), world: v3(0.25, 0.25, 0.0), n_obj: v3(0.0, 0.0, 1.0), n_world: v3(0.0, 0.0, 1.0), value: 0.0, trap: 0.0 };
        let base = [0.5, 0.5, 0.5];
        // A red layer at half opacity.
        let red = prepared(TextureLayer { color: [1.0, 0.0, 0.0], opacity: 0.5, mapping: Mapping::PlanarXY, ..Default::default() }, None);
        let r = apply(std::slice::from_ref(&red), &c, false, base, 0.0);
        assert_eq!(r.base, [0.75, 0.25, 0.25]);
        assert_eq!(r.emis, 0.0);
        // Masked by the checker: black at (0.25, 0.25) keeps the base, white at (0.75, 0.25) paints.
        let mut masked = red.clone();
        masked.layer.opacity = 1.0;
        masked.mask = Some(checker());
        assert_eq!(apply(std::slice::from_ref(&masked), &c, false, base, 0.0).base, base);
        let c2 = Coord { obj: v3(0.75, 0.25, 0.0), ..c };
        let close = |a: [f64; 3], b: [f64; 3]| (0..3).all(|k| (a[k] - b[k]).abs() < 1e-12);
        assert!(close(apply(std::slice::from_ref(&masked), &c2, false, base, 0.0).base, [1.0, 0.0, 0.0]));
        masked.layer.mask_invert = true;
        assert!(close(apply(std::slice::from_ref(&masked), &c, false, base, 0.0).base, [1.0, 0.0, 0.0]));
        // Emission from a white layer.
        let glow = prepared(TextureLayer { target: Target::Emission, strength: 2.0, ..Default::default() }, None);
        let r = apply(&[glow], &c, false, base, 0.5);
        assert_eq!(r.base, base);
        assert!((r.emis - 2.5).abs() < 1e-12);
        // Specular and roughness from a 25 % gray at half opacity.
        let gray = [0.25f32; 3];
        let spec = prepared(TextureLayer { target: Target::Specular, color: gray, opacity: 0.5, ..Default::default() }, None);
        let rough = prepared(TextureLayer { target: Target::Roughness, color: gray, opacity: 0.5, ..Default::default() }, None);
        let r = apply(&[spec, rough], &c, false, base, 0.0);
        assert!((r.spec - 0.625).abs() < 1e-6 && (r.rough_keep - 0.5).abs() < 1e-12 && (r.rough_add - 0.125).abs() < 1e-6, "{r:?}");
        // The image's alpha channel: transparent texels keep the base.
        let mut m = (*checker()).clone();
        m.alpha = Some(vec![0, 255, 255, 0]);
        let img = prepared(TextureLayer { mapping: Mapping::PlanarXY, ..Default::default() }, Some(Arc::new(m)));
        assert_eq!(apply(std::slice::from_ref(&img), &c, false, base, 0.0).base, base);
        assert!(close(apply(std::slice::from_ref(&img), &c2, false, base, 0.0).base, [1.0, 1.0, 1.0]));
        let mut no_alpha = img;
        no_alpha.layer.use_alpha = false;
        assert!(close(apply(&[no_alpha], &c, false, base, 0.0).base, [0.0, 0.0, 0.0]));
    }

    #[test]
    fn normal_map_bends_the_normal() {
        // A normal map tilted toward +u everywhere (red 230, green 128, blue 255; stored blue,
        // green, red): on a surface facing up with planar xy mapping, +u is +x.
        let w = 4;
        let m = Arc::new(Map { number: -1, width: w, height: w, pixels: vec![[255, 128, 230]; 16], alpha: None });
        let l = TextureLayer { image: "n".into(), target: Target::Normal, mapping: Mapping::PlanarXY, strength: 1.0, ..Default::default() };
        let n = v3(0.0, 0.0, 1.0);
        let c = Coord { obj: v3(0.3, 0.3, 0.0), world: v3(0.3, 0.3, 0.0), n_obj: n, n_world: n, value: 0.0, trap: 0.0 };
        let b = bump(&[prepared(l, Some(m))], &c, &|v| v);
        assert!(b.x > 0.5 && b.y.abs() < 0.05 && b.z > 0.3, "{b:?}");
    }

    #[test]
    fn bump_bends_the_normal() {
        // A ramp along x: the normal tilts toward -x (uphill is +x).
        let w = 16;
        let pixels = (0..w * w).map(|i| [((i % w) * 16) as u8; 3]).collect();
        let m = Arc::new(Map { number: -1, width: w as u32, height: w as u32, pixels, alpha: None });
        let l = TextureLayer { image: "ramp".into(), target: Target::Bump, mapping: Mapping::PlanarXY, strength: 8.0, ..Default::default() };
        let p = prepared(l, Some(m));
        let n = v3(0.0, 0.0, 1.0);
        let c = Coord { obj: v3(0.5, 0.5, 0.0), world: v3(0.5, 0.5, 0.0), n_obj: n, n_world: n, value: 0.0, trap: 0.0 };
        let b = bump(&[p], &c, &|v| v);
        assert!(b.x < -0.1 && b.y.abs() < 1e-9 && b.z > 0.5, "{b:?}");
        assert!((b.length() - 1.0).abs() < 1e-12);
    }
}
