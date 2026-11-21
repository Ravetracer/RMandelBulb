// SPDX-License-Identifier: LGPL-2.1-or-later

//! Rendering shared by the CPU and GPU renderers: frame setup, sampling and post processing.
//!
//! The GPU kernel (`rmb-gpu/src/march.wgsl`) mirrors `cpu.rs` step by step; keep them in sync.

pub mod cpu;

use crate::math::{V3, pcg, unit};
use crate::scene::Scene;

/// Light prepared for shading.
#[derive(Clone, Copy, Debug)]
pub struct FrameLight {
    pub dir: V3,
    pub color: [f64; 3],
    pub specular: f64,
    pub shadows: bool,
    pub softness: f64,
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
}

impl Frame {
    pub fn new(scene: &Scene, width: u32, height: u32) -> Frame {
        let tan_half = scene.camera.tan_half_fov();
        let lights = scene
            .lighting
            .lights
            .iter()
            .filter(|l| l.enabled && l.intensity > 0.0)
            .take(crate::scene::MAX_LIGHTS)
            .map(|l| FrameLight {
                dir: l.direction(&scene.camera),
                color: [0, 1, 2].map(|k| l.color[k] as f64 * l.intensity),
                specular: l.specular,
                shadows: l.shadows,
                softness: l.softness,
            })
            .collect();
        Frame {
            width,
            height,
            aspect: width.max(1) as f64 / height.max(1) as f64,
            tan_half,
            pixel_angle: 2.0 * tan_half / height.max(1) as f64,
            lights,
        }
    }
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
    let g = 1.0 / post.gamma.max(0.1);
    let (du, dv) = (u - 0.5, v - 0.5);
    let vig = (1.0 - post.vignette * (du * du + dv * dv) * 2.0).max(0.0);
    c.map(|x| {
        let d = x.max(0.0).powf(g);
        (((d - 0.5) * post.contrast + 0.5) * vig).clamp(0.0, 1.0)
    })
}
