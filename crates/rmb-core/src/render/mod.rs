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
    /// Largest ray step per unit of distance from the camera (`March::step_limit`; 0 = none).
    pub step_cap: f64,
    /// Share of the threshold taken off every step (`March::step_sub`; 0 = off) and the step
    /// factor of the primary march (`March::step`, changed by `step_sub`).
    pub step_sub: f64,
    pub march_step: f64,
    pub lights: Vec<FrameLight>,
    /// Axis toward the sky color of the ambient light.
    pub ambient_up: V3,
    /// Projection and stereo (`aspect` is the aspect of one eye's image).
    pub view: ViewFrame,
}

/// Projection and stereo values of a frame (identical on CPU and GPU).
#[derive(Clone, Copy, Debug)]
pub struct ViewFrame {
    pub projection: crate::scene::Projection,
    /// Fisheye: half the field of view across the image height (radians).
    pub fisheye_half: f64,
    pub stereo: crate::scene::StereoMode,
    /// Half the distance between the eyes (world units).
    pub half_eye: f64,
    /// Distance where both eyes see the same point (world units).
    pub convergence: f64,
    pub swap: bool,
    pub anaglyph_color: bool,
}

impl ViewFrame {
    pub fn new(scene: &Scene) -> ViewFrame {
        let v = &scene.view;
        let convergence = if v.convergence > 0.0 { v.convergence } else { scene.dof.focus_distance.max(1e-30) };
        let stereo = v.stereo != crate::scene::StereoMode::Off;
        ViewFrame {
            projection: v.projection,
            fisheye_half: (v.fisheye_fov.clamp(1.0, 360.0) * 0.5).to_radians(),
            stereo: v.stereo,
            half_eye: if stereo { 0.5 * v.eye_ratio.max(0.0) * convergence } else { 0.0 },
            convergence,
            swap: v.swap_eyes,
            anaglyph_color: v.anaglyph_color,
        }
    }

    /// Images rendered per sample: 2 for anaglyphs (both eyes mixed in every pixel).
    pub fn eye_count(&self) -> u32 {
        if self.stereo == crate::scene::StereoMode::Anaglyph { 2 } else { 1 }
    }

    /// Width and height of one eye's image.
    pub fn eye_size(&self, width: u32, height: u32) -> (u32, u32) {
        use crate::scene::StereoMode;
        match self.stereo {
            StereoMode::SideBySide => ((width / 2).max(1), height.max(1)),
            StereoMode::OverUnder => (width.max(1), (height / 2).max(1)),
            _ => (width.max(1), height.max(1)),
        }
    }

    /// Image position (`fx`, `fy` in pixels from the top left, `k` the anaglyph image) to
    /// the screen coordinates within that eye's image (-1..1, y up) and the eye (-1 left,
    /// 1 right, 0 without stereo).
    pub fn eye_screen(&self, fx: f64, fy: f64, width: u32, height: u32, k: u32) -> (f64, f64, f64) {
        use crate::scene::StereoMode;
        let (ew, eh) = self.eye_size(width, height);
        let (ew, eh) = (ew as f64, eh as f64);
        let (mut x, mut y, mut eye) = (fx, fy, 0.0);
        match self.stereo {
            StereoMode::Off => {}
            StereoMode::Anaglyph => eye = if k == 0 { -1.0 } else { 1.0 },
            StereoMode::SideBySide => {
                eye = if fx < ew { -1.0 } else { 1.0 };
                x = if fx < ew { fx } else { fx - ew };
            }
            StereoMode::OverUnder => {
                eye = if fy < eh { -1.0 } else { 1.0 };
                y = if fy < eh { fy } else { fy - eh };
            }
        }
        if self.swap {
            eye = -eye;
        }
        (x / ew * 2.0 - 1.0, 1.0 - y / eh * 2.0, eye)
    }

    /// Ray origin and unit direction through the screen point (`sx`, `sy`) of one eye's image
    /// (`aspect`: width / height of that image); None outside the image of a fisheye circle.
    pub fn ray(&self, cam: &crate::scene::Camera, sx: f64, sy: f64, aspect: f64, eye: f64) -> Option<(V3, V3)> {
        use crate::scene::Projection;
        use std::f64::consts::{FRAC_PI_2, PI};
        // Direction in camera coordinates (right, up, forward) and the eyes' axis.
        let (l, side) = match self.projection {
            Projection::Perspective => {
                let t = cam.tan_half_fov();
                (v3(sx * t * aspect, sy * t, 1.0).normalize(), v3(1.0, 0.0, 0.0))
            }
            Projection::Fisheye | Projection::FisheyeCircle => {
                let (x, y) = (sx * aspect, sy);
                let r = (x * x + y * y).sqrt();
                let theta = r * self.fisheye_half;
                if (self.projection == Projection::FisheyeCircle && r > 1.0) || theta > PI {
                    return None;
                }
                let s = if r > 1e-12 { theta.sin() / r } else { self.fisheye_half };
                (v3(x * s, y * s, theta.cos()), v3(1.0, 0.0, 0.0))
            }
            Projection::Panorama | Projection::HalfPanorama => {
                let lon = sx * if self.projection == Projection::Panorama { PI } else { FRAC_PI_2 };
                let lat = sy * FRAC_PI_2;
                // The eyes turn with the view direction; their distance fades out toward the
                // poles (from 60° to 80°), where the turning eyes would twist the image.
                let fade = ((lat.cos() - 80f64.to_radians().cos()) / (60f64.to_radians().cos() - 80f64.to_radians().cos())).clamp(0.0, 1.0);
                (v3(lat.cos() * lon.sin(), lat.sin(), lat.cos() * lon.cos()), v3(lon.cos(), 0.0, -lon.sin()) * fade)
            }
        };
        let world = |a: V3| cam.right * a.x + cam.up * a.y + cam.forward * a.z;
        let d = world(l).normalize();
        if eye == 0.0 || self.half_eye <= 0.0 {
            return Some((cam.position, d));
        }
        // Both eyes look at the point the center ray meets at the convergence distance (on a
        // plane for the perspective, a sphere otherwise): no vertical parallax.
        let reach = if self.projection == Projection::Perspective { self.convergence / l.z } else { self.convergence };
        let target = cam.position + d * reach;
        let o = cam.position + world(side) * (eye * self.half_eye);
        Some((o, (target - o).normalize()))
    }

    /// Combines the two anaglyph images of a sample (linear colors).
    pub fn anaglyph(&self, left: [f64; 3], right: [f64; 3]) -> [f64; 3] {
        let r = if self.anaglyph_color { left[0] } else { 0.299 * left[0] + 0.587 * left[1] + 0.114 * left[2] };
        [r, right[1], right[2]]
    }
}

/// Ray through the image position (`fx`, `fy` in pixels from the top left) of the scene's
/// camera with its projection and stereo (picking, navigation); the left eye of an anaglyph.
pub fn view_ray(scene: &Scene, fx: f64, fy: f64, width: u32, height: u32) -> Option<(V3, V3)> {
    let v = ViewFrame::new(scene);
    let (sx, sy, eye) = v.eye_screen(fx, fy, width, height, 0);
    let (ew, eh) = v.eye_size(width, height);
    v.ray(&scene.camera, sx, sy, ew as f64 / eh as f64, eye)
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
        let view = ViewFrame::new(scene);
        let (ew, eh) = view.eye_size(width, height);
        let (ew, eh) = (ew as f64, eh as f64);
        use crate::scene::Projection;
        // Angle of one pixel at the image center (the finer one of the two axes for panoramas).
        let pixel_angle = match view.projection {
            Projection::Perspective => 2.0 * tan_half / eh,
            Projection::Fisheye | Projection::FisheyeCircle => 2.0 * view.fisheye_half / eh,
            Projection::Panorama => (std::f64::consts::TAU / ew).min(std::f64::consts::PI / eh),
            Projection::HalfPanorama => (std::f64::consts::PI / ew).min(std::f64::consts::PI / eh),
        };
        // Mandelbulb3D (HeaderTrafos, StepSubDEstop): s' = s² + 1.2 s (1 - s), minus
        // min(0.9, √s') thresholds.
        let s = scene.march.step;
        let (step_sub, march_step) = if scene.march.step_sub {
            let s2 = s * s + 1.2 * s * (1.0 - s);
            (s2.max(0.0).sqrt().min(0.9), s2)
        } else {
            (0.0, s)
        };
        Frame {
            width,
            height,
            aspect: ew / eh,
            tan_half,
            pixel_angle,
            step_cap: scene.march.step_limit.max(0.0) * pixel_angle * ew.max(eh) * 0.5,
            step_sub,
            march_step,
            lights,
            ambient_up: if scene.lighting.ambient_camera_relative { scene.camera.up } else { v3(0.0, 0.0, 1.0) },
            view,
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
    crate::maps::background(b.image.trim(), b.image_smooth, b.image_spherical, b.image_fit_border).map(|(m, s)| (m, s, crate::maps::rotation_matrix(b.image_rotation)))
}

/// sRGB value (0..1) to linear light.
pub fn srgb_decode(v: f64) -> f64 {
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

/// Hash of an integer lattice point to 0..1 (same bits on the GPU: march.wgsl `hash3`).
pub fn hash3(x: i32, y: i32, z: i32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x8da6_b343) ^ (y as u32).wrapping_mul(0xd816_3841) ^ (z as u32).wrapping_mul(0xcb1a_b31f);
    h ^= h >> 16;
    h = h.wrapping_mul(0x7feb_352d);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846c_a68b);
    h ^= h >> 16;
    (h >> 8) as f32 / 16_777_216.0
}

/// Smooth value noise (0..1) at `p`.
pub fn value_noise(p: [f64; 3]) -> f64 {
    let i = p.map(|v| v.floor());
    let f = [0, 1, 2].map(|k| p[k] - i[k]);
    let u = f.map(|t| t * t * (3.0 - 2.0 * t));
    let (x, y, z) = (i[0] as i32, i[1] as i32, i[2] as i32);
    let h = |dx: i32, dy: i32, dz: i32| hash3(x.wrapping_add(dx), y.wrapping_add(dy), z.wrapping_add(dz)) as f64;
    let lerp = |a: f64, b: f64, t: f64| a + (b - a) * t;
    let x00 = lerp(h(0, 0, 0), h(1, 0, 0), u[0]);
    let x10 = lerp(h(0, 1, 0), h(1, 1, 0), u[0]);
    let x01 = lerp(h(0, 0, 1), h(1, 0, 1), u[0]);
    let x11 = lerp(h(0, 1, 1), h(1, 1, 1), u[0]);
    lerp(lerp(x00, x10, u[1]), lerp(x01, x11, u[1]), u[2])
}

/// Factor of the emitted light at world point `p` (Material::shimmer): two octaves of value
/// noise over patches of `shimmer_size`, a new pattern per phase step, cross-faded smoothly.
pub fn shimmer(m: &crate::scene::Material, p: V3) -> f64 {
    if m.shimmer <= 0.0 {
        return 1.0;
    }
    let g = m.shimmer_size.max(1e-9);
    let (step, f) = (m.shimmer_phase.floor(), m.shimmer_phase - m.shimmer_phase.floor());
    let pattern = |k: f64| {
        let q = [p.x / g + k * 7.31, p.y / g - k * 3.17, p.z / g + k * 5.53];
        (value_noise(q) * 2.0 + value_noise(q.map(|v| v * 2.07)) ) / 3.0
    };
    let t = f * f * (3.0 - 2.0 * f);
    let n = pattern(step) + (pattern(step + 1.0) - pattern(step)) * t;
    (1.0 + m.shimmer.min(1.0) * (2.0 * n - 1.0) * 1.5).max(0.0)
}

/// Density (0..1) of the steam at world point `p`: four octaves of value noise over
/// `grain`-sized blobs, shifted by `offset`, cut so that about `cover` of space is filled.
pub fn steam_density(p: V3, grain: f64, offset: [f64; 3], cover: f64) -> f64 {
    let g = grain.max(1e-9);
    let mut q = [(p.x + offset[0]) / g, (p.y + offset[1]) / g, (p.z + offset[2]) / g];
    let (mut n, mut amp) = (0.0, 0.5);
    for _ in 0..4 {
        n += value_noise(q) * amp;
        q = q.map(|v| v * 2.03);
        amp *= 0.5;
    }
    let n = n / 0.9375;
    let lo = 1.0 - cover.clamp(0.0, 1.0) - 0.15;
    let x = ((n - lo) / 0.3).clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
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

/// Below this surface threshold relative to the size of the coordinates (see [`zoom_ratio`];
/// about 32 f32 steps) `Precision::Auto` uses double-float. The single precision shader gets
/// visibly grainy at about 4 steps (default bulb: zoom 3e4); this switches at about 5e3.
pub const DEEP_ZOOM_RATIO: f64 = 2e-6;

/// The zoom measure for the precision: the surface threshold where the ray through the image
/// center meets the surface, relative to the size of the coordinates there (a short f64 march;
/// a miss uses the distance estimate at the camera).
pub fn zoom_ratio(scene: &Scene, prog: &crate::Program, height: u32) -> f64 {
    let m = &scene.march;
    let pixel = 2.0 * scene.camera.tan_half_fov() / height.max(1) as f64 * m.detail;
    let (o, d) = (scene.camera.position, scene.camera.forward);
    // The see-through water is smooth and never needs double precision; under it the march
    // would stop at the camera.
    let mut ev = prog.evaluator();
    ev.water_mode = crate::fractal::WATER_IGNORED;
    let mut t = start_distance(scene);
    for _ in 0..m.max_steps.min(1000) {
        let p = o + d * t;
        let eps = (pixel * t).max(1e-300);
        let de = ev.de(p, eps * 0.5);
        if !de.is_finite() {
            break;
        }
        if de < eps {
            return eps / p.length().max(1e-300);
        }
        t += (de * m.step).max(eps * 0.1);
        if t > m.max_distance {
            break;
        }
    }
    let de = ev.de(o, 0.0).abs();
    if !de.is_finite() {
        return 1.0;
    }
    pixel * de / (o.length() + de).max(1e-300)
}

/// Whether the GPU renders `scene` in double precision (setting, else the zoom).
pub fn wants_double(scene: &Scene, prog: &crate::Program, height: u32) -> bool {
    use crate::scene::Precision;
    match scene.march.precision {
        Precision::Single => false,
        Precision::Double => true,
        Precision::Auto => zoom_ratio(scene, prog, height) < DEEP_ZOOM_RATIO,
    }
}

/// Deep zoom: `scene` with every world position relative to `r` (minus `r`). The GPU renderer
/// renders that in small local coordinates and adds `r` back only in the fractal iteration
/// (double-float arithmetic). Texture layers in world space keep their look for planar and
/// triplanar mappings (through their offset); spherical and cylindrical ones turn around `r`.
pub fn shifted_scene(scene: &Scene, r: V3) -> Scene {
    use crate::texture::{Mapping, Space, TextureLayer};
    let shift_layers = |layers: &mut Vec<TextureLayer>, world_only: bool| {
        for l in layers.iter_mut() {
            if (world_only && l.space != Space::World) || !matches!(l.mapping, Mapping::Triplanar | Mapping::PlanarXY | Mapping::PlanarXZ | Mapping::PlanarYZ) {
                continue;
            }
            let d = crate::texture::rotate(&crate::maps::rotation_matrix(l.rotation), r) * l.scale;
            l.offset = [l.offset[0] + d.x, l.offset[1] + d.y, l.offset[2] + d.z];
        }
    };
    let mut s = scene.clone();
    s.camera.position = s.camera.position - r;
    if let Some(c) = &mut s.motion_end {
        c.position = c.position - r;
    }
    for l in &mut s.lighting.lights {
        l.position = l.position - r;
        l.target = l.target - r;
    }
    s.fractal.transform.position = s.fractal.transform.position - r;
    for o in &mut s.objects {
        o.transform.position = o.transform.position - r;
        shift_layers(&mut o.material.layers, true);
    }
    s.cut.x.position -= r.x;
    s.cut.y.position -= r.y;
    s.cut.z.position -= r.z;
    let m = &mut s.material;
    m.color_center = [m.color_center[0] - r.x, m.color_center[1] - r.y, m.color_center[2] - r.z];
    // The steam noise is sampled at the world position plus the offset.
    m.steam_offset = [m.steam_offset[0] + r.x, m.steam_offset[1] + r.y, m.steam_offset[2] + r.z];
    shift_layers(&mut m.layers, false);
    s
}

/// Random number stream of one pixel sample (same sequence on CPU and GPU).
pub struct Rng {
    state: u32,
    /// Low-discrepancy part: per-pixel seed, sample index, draws so far.
    pixel: u32,
    sample: u32,
    n: u32,
}

/// Draws of a sample that come from the R2 sequence (pairs: jitter, lens, first directions).
pub const LD_DRAWS: u32 = 6;
/// R2 sequence steps (1/g and 1/g², g the plastic number) in 32-bit fixed point.
const R2: [u32; 2] = [3242174889, 2447445414];

impl Rng {
    pub fn new(px: u32, py: u32, sample: u32) -> Rng {
        Rng { state: pcg(px ^ pcg(py ^ pcg(sample))), pixel: pcg(px ^ pcg(py ^ 0x5bd1_e995)), sample, n: 0 }
    }

    /// The next number in 0..1. The first `LD_DRAWS` of a sample follow the R2 sequence over
    /// the sample index, shifted by a random offset per pixel and pair (successive samples
    /// cover each pair evenly); the rest are independent (march.wgsl `rnd`).
    pub fn next_f64(&mut self) -> f64 {
        if self.n < LD_DRAWS {
            let j = self.n;
            self.n += 1;
            let offset = pcg(self.pixel ^ (j / 2 + 1).wrapping_mul(0x9e37_79b9));
            return unit(offset.wrapping_add(self.sample.wrapping_mul(R2[(j % 2) as usize])));
        }
        self.state = pcg(self.state);
        unit(self.state)
    }
}


/// Chromatic aberration on a linear RGB image (3 floats per pixel): red sampled `amount`
/// percent farther from the center, blue as much closer (bilinear; display.wgsl does the same).
pub fn chromatic_aberration(color: &mut [f32], width: u32, height: u32, amount: f64) {
    if amount == 0.0 || width == 0 || height == 0 {
        return;
    }
    let src = color.to_vec();
    let (w, h) = (width as usize, height as usize);
    let k = amount * 0.01;
    let sample = |fx: f64, fy: f64, ch: usize| -> f32 {
        let fx = fx.clamp(0.0, (w - 1) as f64);
        let fy = fy.clamp(0.0, (h - 1) as f64);
        let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
        let (tx, ty) = ((fx - x0 as f64) as f32, (fy - y0 as f64) as f32);
        let at = |x: usize, y: usize| src[(y * w + x) * 3 + ch];
        let top = at(x0, y0) + (at(x1, y0) - at(x0, y0)) * tx;
        let bottom = at(x0, y1) + (at(x1, y1) - at(x0, y1)) * tx;
        top + (bottom - top) * ty
    };
    let (cx, cy) = (0.5 * (w as f64 - 1.0), 0.5 * (h as f64 - 1.0));
    use rayon::prelude::*;
    color.par_chunks_mut(w * 3).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            let (dx, dy) = (x as f64 - cx, y as f64 - cy);
            row[x * 3] = sample(cx + dx * (1.0 + k), cy + dy * (1.0 + k), 0);
            row[x * 3 + 2] = sample(cx + dx * (1.0 - k), cy + dy * (1.0 - k), 2);
        }
    });
}

/// Filmic curve (Narkowicz ACES fit).
pub fn aces(x: f64) -> f64 {
    ((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14)).clamp(0.0, 1.0)
}

fn luma(c: [f64; 3]) -> f64 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// White balance factors of `post` (temperature: red against blue, tint: green), scaled so
/// that gray keeps its brightness (display.wgsl `white_balance`).
pub fn white_balance(post: &crate::scene::Post) -> [f64; 3] {
    let (t, g) = (post.temperature.clamp(-1.0, 1.0), post.tint.clamp(-1.0, 1.0));
    let f = [1.0 + 0.3 * t, 1.0 - 0.3 * g, 1.0 - 0.3 * t];
    let l = luma(f).max(1e-6);
    f.map(|x| x / l)
}

/// Split toning: `c` (0..1) toward the shadow color in the dark and the highlight color in the
/// bright parts, each color scaled to the brightness of gray (display.wgsl `split_tone`).
fn split_tone(c: [f64; 3], post: &crate::scene::Post) -> [f64; 3] {
    if post.shadow_amount <= 0.0 && post.highlight_amount <= 0.0 {
        return c;
    }
    let l = luma(c).clamp(0.0, 1.0);
    let tone = |col: [f32; 3], w: f64| {
        let col = col.map(|x| x as f64);
        let lc = luma(col).max(1e-3);
        col.map(|x| 1.0 + (x / lc - 1.0) * w)
    };
    let ts = tone(post.shadow_color, post.shadow_amount.clamp(0.0, 1.0) * (1.0 - l) * (1.0 - l));
    let th = tone(post.highlight_color, post.highlight_amount.clamp(0.0, 1.0) * l * l);
    [0, 1, 2].map(|k| c[k] * ts[k] * th[k])
}

/// Linear radiance to display RGB (0..1): exposure and white balance, tone map, saturation,
/// split toning, contrast, vignette, gamma. `u`, `v` are the pixel center in 0..1.
pub fn post_process(c: [f64; 3], post: &crate::scene::Post, u: f64, v: f64) -> [f64; 3] {
    let e = 2f64.powf(post.exposure);
    let wb = white_balance(post);
    let mut c = [0, 1, 2].map(|k| (c[k] * e * wb[k]).max(0.0));
    if post.tone_map {
        c = c.map(aces);
    }
    let luma = luma(c);
    c = c.map(|x| luma + (x - luma) * post.saturation);
    c = split_tone(c, post);
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
    fn view_rays_follow_projection_and_stereo() {
        use crate::scene::{Projection, StereoMode};
        let mut s = Scene::default();
        let cam = s.camera.clone();
        // Plain perspective: the camera's own rays.
        let v = ViewFrame::new(&s);
        let (o, d) = v.ray(&cam, 0.3, -0.2, 1.6, 0.0).unwrap();
        assert!((o - cam.position).length() < 1e-12 && (d - cam.ray_dir(0.3, -0.2, 1.6)).length() < 1e-12);
        // Panorama: the image center looks forward, the left edge backward, the top up.
        s.view.projection = Projection::Panorama;
        let v = ViewFrame::new(&s);
        assert!((v.ray(&cam, 0.0, 0.0, 2.0, 0.0).unwrap().1 - cam.forward).length() < 1e-9);
        assert!((v.ray(&cam, -1.0, 0.0, 2.0, 0.0).unwrap().1 + cam.forward).length() < 1e-9);
        assert!((v.ray(&cam, 0.3, 1.0, 2.0, 0.0).unwrap().1 - cam.up).length() < 1e-9);
        // Fisheye circle at 180°: the circle's edge looks sideways, outside is empty.
        s.view.projection = Projection::FisheyeCircle;
        let v = ViewFrame::new(&s);
        assert!((v.ray(&cam, 1.0, 0.0, 1.0, 0.0).unwrap().1 - cam.right).length() < 1e-9);
        assert!(v.ray(&cam, 0.8, 0.8, 1.0, 0.0).is_none());
        // Side by side: the halves are the eyes; both eyes meet at the convergence distance.
        s.view.projection = Projection::Perspective;
        s.view.stereo = StereoMode::SideBySide;
        s.view.convergence = 3.0;
        let v = ViewFrame::new(&s);
        let (sx, sy, eye) = v.eye_screen(150.0, 50.0, 200, 100, 0);
        assert!((sx - 0.0).abs() < 1e-12 && (sy - 0.0).abs() < 1e-12 && eye == 1.0);
        assert_eq!(v.eye_screen(10.0, 50.0, 200, 100, 0).2, -1.0);
        let (ol, dl) = v.ray(&cam, 0.0, 0.0, 1.0, -1.0).unwrap();
        let (or, dr) = v.ray(&cam, 0.0, 0.0, 1.0, 1.0).unwrap();
        let target = cam.position + cam.forward * 3.0;
        assert!((ol + dl * (target - ol).length() - target).length() < 1e-9);
        assert!((or + dr * (target - or).length() - target).length() < 1e-9);
        assert!(((or - ol).length() - 3.0 / 30.0).abs() < 1e-9);
    }

    fn frame_light(kind: crate::scene::LightKind, edit: impl Fn(&mut crate::scene::Light)) -> FrameLight {
        let mut s = Scene::default();
        let mut l = crate::scene::Light { kind, position: v3(0.0, 0.0, 4.0), target: V3::ZERO, ..Default::default() };
        edit(&mut l);
        s.lighting.lights = vec![l];
        Frame::new(&s, 64, 64).lights[0]
    }

    #[test]
    fn deep_zoom_shift_and_precision() {
        use crate::scene::Precision;
        let mut s = Scene::default();
        s.material.color_center = [0.5, 0.0, 0.0];
        let r = s.camera.position;
        let sh = shifted_scene(&s, r);
        assert!(sh.camera.position.length() < 1e-12);
        assert!((sh.lighting.lights[0].position - (s.lighting.lights[0].position - r)).length() < 1e-12);
        assert!((sh.material.color_center[0] - (0.5 - r.x)).abs() < 1e-12);
        // Auto: single precision for the default view, double close to the surface.
        let prog = crate::Program::for_scene(&s);
        assert!(zoom_ratio(&s, &prog, 720) > DEEP_ZOOM_RATIO);
        assert!(!wants_double(&s, &prog, 720));
        // Zoom 1e5: ten times closer per step (a hit is only accurate to its threshold).
        let mut near = s.clone();
        for _ in 0..5 {
            let (t, _) = cpu::pick(&near, 0.0, 0.0, 1024, 1024).expect("surface");
            near.camera.position += near.camera.forward * (t * 0.9);
        }
        assert!(wants_double(&near, &prog, 720));
        near.march.precision = Precision::Single;
        assert!(!wants_double(&near, &prog, 720));
    }

    #[test]
    fn path_tracing_adds_indirect_light() {
        // No ambient light: in classic mode the shadows stay dark, path tracing brings in the
        // light bounced off the lit surfaces.
        let mut s = Scene::default();
        s.lighting.ambient = 0.0;
        s.lighting.ao_mode = crate::scene::AoMode::Fast;
        s.material.gradient = crate::gradient::preset("White").unwrap();
        let mean = |s: &Scene| {
            let mut r = cpu::CpuRenderer::new(s.clone(), 40, 30);
            for _ in 0..4 {
                r.render_pass();
            }
            let px = r.rgba8();
            px.iter().map(|v| *v as f64).sum::<f64>() / px.len() as f64
        };
        let classic = mean(&s);
        s.lighting.render_mode = crate::scene::RenderMode::PathTraced;
        let path = mean(&s);
        assert!(path > classic + 1.0, "classic {classic}, path traced {path}");
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
