// SPDX-License-Identifier: LGPL-2.1-or-later

//! Rendering shared by the CPU and GPU renderers: frame setup, sampling and post processing.
//!
//! The GPU kernel (`rmb-gpu/src/march.wgsl`) mirrors `cpu.rs` step by step; keep them in sync.

pub mod cpu;
pub mod denoise;
pub mod oidn;

use std::borrow::Cow;

use crate::math::{V3, pcg, unit, v3};
use crate::scene::Scene;

/// Light prepared for shading.
#[derive(Clone, Copy, Debug)]
pub struct FrameLight {
    /// Direction toward a directional light.
    pub dir: V3,
    /// Point light: its position (`dir` is unused).
    pub point: Option<V3>,
    pub color: [f64; 3],
    pub specular: f64,
    pub shadows: bool,
    pub softness: f64,
    pub diffuse: crate::scene::DiffuseCurve,
    pub occlusion: f64,
    pub glow: Option<Glow>,
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
    let (u, t_light) = match li.point {
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
    let (transp, flux) = g.shape.shape(u, g.r2, li.point.is_some());
    // Depth fog at the light dims it.
    let fl = fog.amount(t_light, near, far);
    let flux = flux * (1.0 - fl * 0.9);
    let add = g.color.map(|c| c * flux);
    Some((add, if g.additive { 1.0 } else { transp.clamp(0.0, 1.0) }, fl.max(0.0)))
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
                point: (l.kind == crate::scene::LightKind::Point).then_some(l.position),
                color: [0, 1, 2].map(|k| l.color[k] as f64 * l.intensity),
                specular: l.specular,
                shadows: l.shadows,
                softness: l.softness,
                diffuse: l.diffuse,
                occlusion: l.occlusion.clamp(0.0, 1.0),
                glow: l.visible.then(|| Glow {
                    shape: l.glow_shape,
                    r2: if l.kind == crate::scene::LightKind::Point {
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
    for st in &mut s.material.gradient {
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
    let b = &scene.background;
    if b.image.trim().is_empty() {
        return None;
    }
    crate::maps::get_file(b.image.trim()).map(|m| (m, crate::maps::rotation_matrix(b.image_rotation)))
}

/// sRGB value (0..1) to linear light.
pub fn srgb_decode(v: f64) -> f64 {
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
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

    #[test]
    fn point_light_glows_unless_hidden() {
        let li = FrameLight {
            dir: v3(0.0, 0.0, 1.0),
            point: Some(v3(0.0, 5.0, 0.0)),
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
