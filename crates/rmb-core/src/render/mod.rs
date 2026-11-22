// SPDX-License-Identifier: LGPL-2.1-or-later

//! Rendering shared by the CPU and GPU renderers: frame setup, sampling and post processing.
//!
//! The GPU kernel (`rmb-gpu/src/march.wgsl`) mirrors `cpu.rs` step by step; keep them in sync.

pub mod bloom;
pub mod cpu;
pub mod denoise;
pub mod oidn;

use std::borrow::Cow;

use crate::math::{V3, pcg, unit, v3};
use crate::scene::{LightKind, Scene};

/// Light prepared for shading.
#[derive(Clone, Copy, Debug)]
pub struct FrameLight {
    /// Direction toward a directional light.
    pub dir: V3,
    /// Positional lights: their position (area lights: the center; line lights: one end).
    pub point: Option<V3>,
    pub shape: LightShape,
    pub color: [f64; 3],
    pub specular: f64,
    pub shadows: bool,
    pub softness: f64,
    pub diffuse: crate::scene::DiffuseCurve,
    pub occlusion: f64,
    pub glow: Option<Glow>,
}

/// The extent of a positional light.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LightShape {
    Directional,
    Point,
    /// Shines into the cone around `axis` (unit, away from the light).
    Spot { axis: V3, cos_outer: f64, cos_inner: f64 },
    /// Rectangle around the center with the half extents `u`, `v`, emitting toward `n`.
    Area { n: V3, u: V3, v: V3 },
    /// Tube from `point` to `end`.
    Line { end: V3 },
}

impl FrameLight {
    /// Area and line lights take two random numbers per shading point (soft shadows build up
    /// over the samples).
    pub fn sampled(&self) -> bool {
        matches!(self.shape, LightShape::Area { .. } | LightShape::Line { .. })
    }

    /// From the shading point `p`: direction toward the light (for area and line lights toward
    /// the point picked by `r1`, `r2`), the light's falloff there and the shadow length.
    pub fn sample(&self, p: V3, r1: f64, r2: f64, max_distance: f64) -> (V3, f64, f64) {
        let toward = |q: V3| {
            let v = q - p;
            let d2 = v.dot(v).max(1e-30);
            let dist = d2.sqrt();
            (v * (1.0 / dist), d2, dist)
        };
        let Some(c) = self.point else { return (self.dir, 1.0, max_distance) };
        match self.shape {
            LightShape::Directional | LightShape::Point => {
                let (l, d2, dist) = toward(c);
                (l, 1.0 / d2, dist)
            }
            LightShape::Spot { axis, cos_outer, cos_inner } => {
                let (l, d2, dist) = toward(c);
                let x = ((-l.dot(axis) - cos_outer) / (cos_inner - cos_outer).max(1e-9)).clamp(0.0, 1.0);
                (l, x * x * (3.0 - 2.0 * x) / d2, dist)
            }
            LightShape::Area { n, u, v } => {
                let (l, d2, dist) = toward(c + u * (2.0 * r1 - 1.0) + v * (2.0 * r2 - 1.0));
                ((l), (-l.dot(n)).max(0.0) / d2, dist)
            }
            LightShape::Line { end } => {
                let (l, d2, dist) = toward(c + (end - c) * r1);
                (l, 1.0 / d2, dist)
            }
        }
    }
}

/// A visible light source prepared for shading.
#[derive(Clone, Copy, Debug)]
pub struct Glow {
    pub shape: crate::scene::GlowShape,
    /// Squared radius: world units² for point lights, 1 - cos(angle) for directional ones.
    pub r2: f64,
    pub color: [f64; 3],
    /// Added on top (else it partly covers what is behind).
    pub additive: bool,
}

/// Glow of a visible light on the primary ray (`o`, `d`) with the surface at `hit_t`: the
/// light added, the share of the pixel that stays and the fog amount at the light (what the
/// glow covers turns into that much fog; Mandelbulb3D's visible lights).
#[allow(clippy::too_many_arguments)]
pub fn light_glow(li: &FrameLight, g: &Glow, o: V3, d: V3, hit_t: Option<f64>, fog: &crate::scene::Fog, near: f64, far: f64) -> Option<([f64; 3], f64, f64)> {
    // Area lights show as their glowing rectangle (front side), covering what is behind.
    if let (Some(c), LightShape::Area { n, u, v }) = (li.point, li.shape) {
        let den = d.dot(n);
        if den >= 0.0 {
            return None;
        }
        let t = (c - o).dot(n) / den;
        if t <= 0.0 || hit_t.is_some_and(|h| h < t) {
            return None;
        }
        let q = o + d * t - c;
        if (q.dot(u) / u.dot(u).max(1e-30)).abs() > 1.0 || (q.dot(v) / v.dot(v).max(1e-30)).abs() > 1.0 {
            return None;
        }
        let fl = fog.amount(t, near, far);
        return Some((g.color.map(|c| c * (1.0 - fl.max(0.0))), 0.0, fl.max(0.0)));
    }
    // Line lights glow around the point of the tube nearest to the ray.
    let point = match (li.point, li.shape) {
        (Some(a), LightShape::Line { end }) => Some(nearest_on_segment(a, end, o, d)),
        (p, _) => p,
    };
    let (u, t_light) = match point {
        Some(p) => {
            let v = p - o;
            let along = v.dot(d);
            // Behind the viewer.
            if along < 0.0 {
                return None;
            }
            let d2 = (v - d * along).dot(v - d * along);
            if d2 >= g.r2 {
                return None;
            }
            // The surface is in front of the glowing sphere.
            if hit_t.is_some_and(|t| t < along - (g.r2 - d2).sqrt()) {
                return None;
            }
            (d2 / g.r2, along)
        }
        None => {
            // Directional lights only show on the background.
            if hit_t.is_some() {
                return None;
            }
            let a = (1.0 - li.dir.dot(d)).max(0.0);
            if a >= g.r2 {
                return None;
            }
            (a / g.r2, far)
        }
    };
    let (transp, flux) = g.shape.shape(u, g.r2, point.is_some());
    // Depth fog at the light dims it.
    let fl = fog.amount(t_light, near, far);
    let flux = flux * (1.0 - fl * 0.9);
    let add = g.color.map(|c| c * flux);
    Some((add, if g.additive { 1.0 } else { transp.clamp(0.0, 1.0) }, fl.max(0.0)))
}

/// The point of the segment `a`..`b` nearest to the ray from `o` along the unit `d`.
pub fn nearest_on_segment(a: V3, b: V3, o: V3, d: V3) -> V3 {
    let e = b - a;
    let w = a - o;
    let (ee, ed) = (e.dot(e), e.dot(d));
    let den = ee - ed * ed;
    let s = if den.abs() < 1e-12 * ee.max(1e-30) { 0.5 } else { ((ed * w.dot(d) - w.dot(e)) / den).clamp(0.0, 1.0) };
    a + e * s
}

/// Per-frame values derived from the scene (identical on CPU and GPU).
#[derive(Clone, Debug)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub aspect: f64,
    pub tan_half: f64,
    /// Angle covered by one pixel (radians), the base of the surface threshold.
    pub pixel_angle: f64,
    pub lights: Vec<FrameLight>,
    /// Axis toward the sky color of the ambient light.
    pub ambient_up: V3,
}

impl Frame {
    pub fn new(scene: &Scene, width: u32, height: u32) -> Frame {
        let tan_half = scene.camera.tan_half_fov();
        let lights = scene
            .lighting
            .lights
            .iter()
            .filter(|l| l.enabled && (l.intensity > 0.0 || l.visible))
            .take(crate::scene::MAX_LIGHTS)
            .map(|l| FrameLight {
                dir: l.direction(&scene.camera),
                point: l.kind.positional().then_some(l.position),
                shape: light_shape(l),
                color: [0, 1, 2].map(|k| l.color[k] as f64 * l.intensity),
                specular: l.specular,
                shadows: l.shadows,
                // Area and line lights get their soft shadows from sampling.
                softness: if matches!(l.kind, LightKind::Area | LightKind::Line) { 0.0 } else { l.softness },
                diffuse: l.diffuse,
                occlusion: l.occlusion.clamp(0.0, 1.0),
                glow: l.visible.then(|| Glow {
                    shape: l.glow_shape,
                    r2: if l.kind.positional() {
                        l.glow_size * l.glow_size
                    } else {
                        1.0 - l.glow_size.to_radians().cos()
                    },
                    color: l.color.map(|c| c as f64 * l.glow_intensity),
                    additive: matches!(l.glow_shape, crate::scene::GlowShape::Soft | crate::scene::GlowShape::Sharp),
                }),
            })
            .collect();
        Frame {
            width,
            height,
            aspect: width.max(1) as f64 / height.max(1) as f64,
            tan_half,
            pixel_angle: 2.0 * tan_half / height.max(1) as f64,
            lights,
            ambient_up: if scene.lighting.ambient_camera_relative { scene.camera.up } else { v3(0.0, 0.0, 1.0) },
        }
    }
}

/// An area light's facing direction and its half-width and half-height axes.
pub fn area_axes(l: &crate::scene::Light) -> (V3, V3, V3) {
    let aim = (l.target - l.position).normalize();
    let side = if aim.z.abs() < 0.99 { aim.cross(v3(0.0, 0.0, 1.0)) } else { aim.cross(v3(1.0, 0.0, 0.0)) }.normalize();
    let up = side.cross(aim).normalize();
    (aim, side * (0.5 * l.width.max(0.0)), up * (0.5 * l.height.max(0.0)))
}

/// The shape of a light for shading.
fn light_shape(l: &crate::scene::Light) -> LightShape {
    let aim = (l.target - l.position).normalize();
    match l.kind {
        LightKind::Directional => LightShape::Directional,
        LightKind::Point => LightShape::Point,
        LightKind::Spot => {
            let outer = l.cone_angle.clamp(0.1, 179.0).to_radians();
            let inner = outer * (1.0 - l.cone_softness.clamp(0.0, 1.0));
            LightShape::Spot { axis: aim, cos_outer: outer.cos(), cos_inner: inner.cos() }
        }
        LightKind::Area => {
            let (n, u, v) = area_axes(l);
            LightShape::Area { n, u, v }
        }
        LightKind::Line => LightShape::Line { end: l.target },
    }
}

/// Mandelbulb3D's diffuse tables for rough surfaces (LightAdjust.SetCosTabFunction): per diffuse
/// curve, 128 entries over the angle (index 62 - 60 cos).
pub fn rough_tables() -> &'static [[f64; 128]; 4] {
    static T: std::sync::OnceLock<[[f64; 128]; 4]> = std::sync::OnceLock::new();
    T.get_or_init(|| {
        let base: [[f64; 128]; 4] = std::array::from_fn(|k| {
            std::array::from_fn(|i| {
                let d = 1.0 - (i as f64 - 2.0) / 60.0;
                match k {
                    0 if d > 0.15 => (d - 0.08) * 1.0869565,
                    0 if d <= 0.0 => 0.0,
                    0 => d.powf(((0.505 - d) * 3.8).max(1.0)),
                    1 => d.max(0.0).powi(2),
                    2 => d * 0.5 + 0.5,
                    _ => (d * 0.5 + 0.5).powi(2),
                }
            })
        });
        let mut out = [[0.0; 128]; 4];
        for k in 0..4 {
            let tmp: Vec<f64> = base[k].iter().map(|v| v.max(0.0).sqrt()).collect();
            for (j, o) in out[k].iter_mut().enumerate() {
                let e: f64 = (0..=60).map(|i| (j as i64 + i - 30).unsigned_abs() as usize).filter(|l| *l < 128).map(|l| tmp[l]).sum();
                *o = (e * 0.011 + (e * 0.007).powi(2)).powi(2);
            }
        }
        out
    })
}

/// Rough diffuse factor of curve `k` for `ndl` (GetCosTabVal: B-spline over the table).
pub fn rough_diffuse(k: usize, ndl: f64) -> f64 {
    let tab = &rough_tables()[k.min(3)];
    let t = 62.0 - 60.0 * ndl;
    let (ip, f) = if t.trunc() - 1.0 < 0.0 {
        (0usize, 0.0)
    } else if t.trunc() - 1.0 > 124.0 {
        (124usize, 1.0)
    } else {
        ((t.trunc() - 1.0) as usize, t - t.trunc())
    };
    let w = crate::maps::spline_weights(f);
    (0..4).map(|i| tab[ip + i] * w[i]).sum()
}

/// Linear color to its display (sRGB encoded) value, kept as a float above 1.
pub fn to_display(v: f32) -> f32 {
    if v <= 0.003_130_8 { v.max(0.0) * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
}

/// The scene as the shading sees it: with `post.display_space` all colors are converted to
/// their display values, so lighting works on them like Mandelbulb3D does.
pub fn shading_scene(scene: &Scene) -> Cow<'_, Scene> {
    if !scene.post.display_space {
        return Cow::Borrowed(scene);
    }
    let mut s = scene.clone();
    let c = |x: &mut [f32; 3]| *x = x.map(to_display);
    let objects = s.objects.iter_mut().flat_map(|o| o.material.gradient.iter_mut());
    for st in s.material.gradient.iter_mut().chain(s.material.interior.iter_mut()).chain(objects) {
        c(&mut st.color);
        c(&mut st.specular);
    }
    for l in &mut s.lighting.lights {
        c(&mut l.color);
    }
    c(&mut s.lighting.sky_color);
    c(&mut s.lighting.ground_color);
    c(&mut s.background.top);
    c(&mut s.background.horizon);
    c(&mut s.background.bottom);
    c(&mut s.fog.color);
    c(&mut s.fog.glow_color);
    c(&mut s.fog.glow_color2);
    Cow::Owned(s)
}

/// The background image of `scene` (loaded on first use) and its rotation matrix.
pub fn background_image(scene: &Scene) -> Option<(std::sync::Arc<crate::maps::Map>, [[f64; 3]; 3])> {
    background_maps(scene).map(|(m, _, r)| (m, r))
}

/// The background image as prepared for rendering, its 16 × 8 ambient copy and its rotation.
pub type BackgroundMaps = (std::sync::Arc<crate::maps::Map>, std::sync::Arc<crate::maps::Map>, [[f64; 3]; 3]);

pub fn background_maps(scene: &Scene) -> Option<BackgroundMaps> {
    let b = &scene.background;
    if b.image.trim().is_empty() {
        return None;
    }
    crate::maps::background(b.image.trim(), b.image_spherical, b.image_fit_border).map(|(m, s)| (m, s, crate::maps::rotation_matrix(b.image_rotation)))
}

/// sRGB value (0..1) to linear light.
pub fn srgb_decode(v: f64) -> f64 {
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

/// Linear light (clamped to 0..1) to an sRGB value.
pub fn srgb_encode(v: f64) -> f64 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.0031308 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
}

/// Distance from the camera where primary rays start: the near distance; inside rendering
/// needs a positive one (the step size grows with the distance).
pub fn start_distance(scene: &Scene) -> f64 {
    let n = scene.march.near.max(0.0);
    if scene.fractal.inside { n.max(scene.march.max_distance * 1.0e-4) } else { n }
}

/// Random number stream of one pixel sample (same sequence on CPU and GPU).
pub struct Rng(pub u32);

impl Rng {
    pub fn new(px: u32, py: u32, sample: u32) -> Rng {
        Rng(pcg(px ^ pcg(py ^ pcg(sample))))
    }

    pub fn next_f64(&mut self) -> f64 {
        self.0 = pcg(self.0);
        unit(self.0)
    }
}

/// Filmic curve (Narkowicz ACES fit).
pub fn aces(x: f64) -> f64 {
    ((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14)).clamp(0.0, 1.0)
}

/// Linear radiance to display RGB (0..1): exposure, tone map, saturation, contrast, vignette, gamma.
/// `u`, `v` are the pixel center in 0..1.
pub fn post_process(c: [f64; 3], post: &crate::scene::Post, u: f64, v: f64) -> [f64; 3] {
    let e = 2f64.powf(post.exposure);
    let mut c = c.map(|x| (x * e).max(0.0));
    if post.tone_map {
        c = c.map(aces);
    }
    let luma = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
    c = c.map(|x| luma + (x - luma) * post.saturation);
    // Display space: the values are already gamma encoded.
    let g = if post.display_space { 2.2 / post.gamma.max(0.1) } else { 1.0 / post.gamma.max(0.1) };
    let (du, dv) = (u - 0.5, v - 0.5);
    let vig = (1.0 - post.vignette * (du * du + dv * dv) * 2.0).max(0.0);
    c.map(|x| {
        let d = x.max(0.0).powf(g);
        (((d - 0.5) * post.contrast + 0.5) * vig).clamp(0.0, 1.0)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{Fog, GlowShape};

    fn frame_light(kind: crate::scene::LightKind, edit: impl Fn(&mut crate::scene::Light)) -> FrameLight {
        let mut s = Scene::default();
        let mut l = crate::scene::Light { kind, position: v3(0.0, 0.0, 4.0), target: V3::ZERO, ..Default::default() };
        edit(&mut l);
        s.lighting.lights = vec![l];
        Frame::new(&s, 64, 64).lights[0]
    }

    #[test]
    fn spot_area_and_line_lights_fall_off_correctly() {
        use crate::scene::LightKind;
        // Spot: full inside the cone, nothing outside.
        let spot = frame_light(LightKind::Spot, |l| {
            l.cone_angle = 20.0;
            l.cone_softness = 0.2;
        });
        let (dir, a, dist) = spot.sample(V3::ZERO, 0.5, 0.5, 100.0);
        assert!((dir - v3(0.0, 0.0, 1.0)).length() < 1e-12 && (a - 1.0 / 16.0).abs() < 1e-12 && (dist - 4.0).abs() < 1e-12);
        assert_eq!(spot.sample(v3(3.0, 0.0, 0.0), 0.5, 0.5, 100.0).1, 0.0);
        // Area: lights the side it faces only; samples spread over the rectangle.
        let area = frame_light(LightKind::Area, |l| {
            l.width = 2.0;
            l.height = 1.0;
        });
        assert!(area.sample(V3::ZERO, 0.5, 0.5, 100.0).1 > 0.0);
        assert_eq!(area.sample(v3(0.0, 0.0, 8.0), 0.5, 0.5, 100.0).1, 0.0);
        let (a, b) = (area.sample(V3::ZERO, 0.0, 0.5, 100.0).0, area.sample(V3::ZERO, 1.0, 0.5, 100.0).0);
        assert!((a - b).length() > 0.4, "corners in different directions");
        // Line: from one end to the other.
        let line = frame_light(LightKind::Line, |l| l.target = v3(4.0, 0.0, 4.0));
        let (d0, _, _) = line.sample(v3(0.0, 0.0, 0.0), 0.0, 0.0, 100.0);
        let (d1, _, _) = line.sample(v3(0.0, 0.0, 0.0), 1.0, 0.0, 100.0);
        assert!((d0 - v3(0.0, 0.0, 1.0)).length() < 1e-12 && (d1 - v3(1.0, 0.0, 1.0).normalize()).length() < 1e-12);
        // The tube glows where the ray passes closest.
        assert!((nearest_on_segment(v3(0.0, 0.0, 4.0), v3(4.0, 0.0, 4.0), v3(2.0, -5.0, 4.0), v3(0.0, 1.0, 0.0)) - v3(2.0, 0.0, 4.0)).length() < 1e-12);
    }

    #[test]
    fn point_light_glows_unless_hidden() {
        let li = FrameLight {
            dir: v3(0.0, 0.0, 1.0),
            point: Some(v3(0.0, 5.0, 0.0)),
            shape: LightShape::Point,
            color: [1.0; 3],
            specular: 0.0,
            shadows: false,
            softness: 0.0,
            diffuse: crate::scene::DiffuseCurve::Lambert,
            occlusion: 0.0,
            glow: None,
        };
        let g = Glow { shape: GlowShape::Soft, r2: 0.25, color: [1.0; 3], additive: true };
        let fog = Fog::default();
        let (o, d) = (V3::ZERO, v3(0.0, 1.0, 0.0));
        let (add, keep, _) = light_glow(&li, &g, o, d, None, &fog, 0.0, 100.0).expect("glow on the axis");
        assert!(add[0] > 1.0 && keep == 1.0);
        // A surface in front of the light hides it; one behind does not.
        assert!(light_glow(&li, &g, o, d, Some(2.0), &fog, 0.0, 100.0).is_none());
        assert!(light_glow(&li, &g, o, d, Some(9.0), &fog, 0.0, 100.0).is_some());
        // Off to the side or behind the viewer: nothing.
        assert!(light_glow(&li, &g, o, v3(1.0, 0.0, 0.0), None, &fog, 0.0, 100.0).is_none());
        assert!(light_glow(&li, &g, o, v3(0.0, -1.0, 0.0), None, &fog, 0.0, 100.0).is_none());
    }
}
