// SPDX-License-Identifier: LGPL-2.1-or-later

//! Finished images: the averaged radiance of a render with its helper images (albedo and
//! normals of the first hit), the AI denoiser (Intel Open Image Denoise, see [`super::oidn`])
//! and the post processing into 8-bit RGBA.

use rayon::prelude::*;

use super::oidn::{self, DeviceType};
use super::post_process;
use crate::scene::Post;

/// Averaged render result before post processing; RGB triples, top row first.
#[derive(Clone, Debug, Default)]
pub struct HdrImage {
    pub width: u32,
    pub height: u32,
    /// Radiance (linear, or display values with `Post::display_space`).
    pub color: Vec<f32>,
    /// Surface color of the first hit (0..1; the background color for misses).
    pub albedo: Vec<f32>,
    /// World normal of the first hit (zero for misses).
    pub normal: Vec<f32>,
    /// Share of the samples that hit a surface (0..1, one value per pixel; empty if unknown).
    pub alpha: Vec<f32>,
    /// Mean distance of the hits from the camera (infinity without hits; empty if unknown).
    pub depth: Vec<f32>,
}

/// What happened to the denoiser when finishing an image.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// Not asked for (or blend 0).
    Off,
    /// Denoised on this device.
    Denoised(DeviceType),
    /// Asked for but not done.
    Failed(String),
}

impl Outcome {
    pub fn describe(&self) -> String {
        match self {
            Outcome::Off => "not denoised".into(),
            Outcome::Denoised(d) => format!("denoised on the {}", d.label()),
            Outcome::Failed(e) => format!("not denoised: {e}"),
        }
    }
}

impl HdrImage {
    /// Denoises the color in place and mixes `blend` (0..1) of the result with the original.
    pub fn denoise(&mut self, blend: f64) -> Result<DeviceType, String> {
        let (w, h) = (self.width as usize, self.height as usize);
        let clean = |v: &[f32], lo: f32, hi: f32| v.iter().map(|x| if x.is_finite() { x.clamp(lo, hi) } else { 0.0 }).collect::<Vec<f32>>();
        let mut out = clean(&self.color, 0.0, 65504.0);
        let have_aux = self.albedo.len() == out.len() && self.normal.len() == out.len();
        let albedo = have_aux.then(|| clean(&self.albedo, 0.0, 1.0));
        let normal = have_aux.then(|| clean(&self.normal, -1.0, 1.0));
        let used = oidn::denoise(&mut out, w, h, true, false, DeviceType::Default, albedo.as_deref(), normal.as_deref())?;
        let b = blend.clamp(0.0, 1.0) as f32;
        for (c, d) in self.color.iter_mut().zip(&out) {
            *c = if c.is_finite() { *c + (d - *c) * b } else { *d };
        }
        Ok(used)
    }

    /// Post processing into tightly packed 16-bit RGBA.
    pub fn to_rgba16(&self, post: &Post) -> Vec<u16> {
        let (w, h) = (self.width as usize, self.height as usize);
        let mut out = vec![0u16; w * h * 4];
        out.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
            for x in 0..w {
                let i = (y * w + x) * 3;
                let c = [self.color[i] as f64, self.color[i + 1] as f64, self.color[i + 2] as f64].map(|v| if v.is_finite() { v } else { 0.0 });
                let c = post_process(c, post, (x as f64 + 0.5) / w as f64, (y as f64 + 0.5) / h as f64);
                let q = |v: f64| (v.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16;
                row[x * 4..x * 4 + 4].copy_from_slice(&[q(c[0]), q(c[1]), q(c[2]), 65535]);
            }
        });
        out
    }

    /// Post processing into tightly packed RGBA8 (same as the GPU display pass).
    pub fn to_rgba8(&self, post: &Post) -> Vec<u8> {
        let (w, h) = (self.width as usize, self.height as usize);
        let mut out = vec![0u8; w * h * 4];
        out.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
            for x in 0..w {
                let i = (y * w + x) * 3;
                let c = [self.color[i] as f64, self.color[i + 1] as f64, self.color[i + 2] as f64].map(|v| if v.is_finite() { v } else { 0.0 });
                let c = post_process(c, post, (x as f64 + 0.5) / w as f64, (y as f64 + 0.5) / h as f64);
                let o = &mut row[x * 4..x * 4 + 4];
                o.copy_from_slice(&[(c[0] * 255.0 + 0.5) as u8, (c[1] * 255.0 + 0.5) as u8, (c[2] * 255.0 + 0.5) as u8, 255]);
            }
        });
        out
    }

    /// Denoises if `post` asks for it, then post processes into RGBA8.
    /// Adds the lens flares of `scene`'s lights (after denoising: they are clean already).
    pub fn add_flares(&mut self, scene: &crate::Scene) {
        if let Some(fl) = crate::flare::layer(scene, self.width, self.height) {
            for (c, f) in self.color.as_chunks_mut::<3>().0.iter_mut().zip(&fl) {
                for i in 0..3 {
                    c[i] += f[i];
                }
            }
        }
    }

    /// Like [`HdrImage::finish`] with the scene's post processing and its lens flares.
    pub fn finish_scene(self, scene: &crate::Scene) -> (Vec<u8>, Outcome) {
        let (img, outcome) = self.finish_scene_hdr(scene);
        (img.to_rgba8(&scene.post), outcome)
    }

    /// [`finish_scene`](Self::finish_scene) into 16-bit RGBA (16-bit PNG).
    pub fn finish_scene16(self, scene: &crate::Scene) -> (Vec<u16>, Outcome) {
        let (img, outcome) = self.finish_scene_hdr(scene);
        (img.to_rgba16(&scene.post), outcome)
    }

    /// Denoising (as the scene says), lens flares, bloom and chromatic aberration: the image
    /// before the per-pixel post processing.
    pub fn finish_scene_hdr(mut self, scene: &crate::Scene) -> (HdrImage, Outcome) {
        let post = &scene.post;
        let outcome = self.denoise_as(post);
        self.add_flares(scene);
        super::bloom::apply(&mut self.color, self.width, self.height, post);
        super::chromatic_aberration(&mut self.color, self.width, self.height, post.chromatic);
        (self, outcome)
    }

    /// The image for an EXR file: denoised as the scene says, in linear light (display values
    /// of `Post::display_space` scenes decoded), without lens flares and post processing.
    pub fn finish_linear(mut self, scene: &crate::Scene) -> (HdrImage, Outcome) {
        let outcome = self.denoise_as(&scene.post);
        if scene.post.display_space {
            for c in self.color.iter_mut() {
                *c = super::srgb_decode(c.max(0.0) as f64) as f32;
            }
            for c in self.albedo.iter_mut() {
                *c = super::srgb_decode(c.max(0.0) as f64) as f32;
            }
        }
        (self, outcome)
    }

    fn denoise_as(&mut self, post: &Post) -> Outcome {
        if !post.denoise || post.denoise_blend <= 0.0 {
            Outcome::Off
        } else {
            match self.denoise(post.denoise_blend) {
                Ok(d) => Outcome::Denoised(d),
                Err(e) => Outcome::Failed(e),
            }
        }
    }

    pub fn finish(mut self, post: &Post) -> (Vec<u8>, Outcome) {
        let outcome = if !post.denoise || post.denoise_blend <= 0.0 {
            Outcome::Off
        } else {
            match self.denoise(post.denoise_blend) {
                Ok(d) => Outcome::Denoised(d),
                Err(e) => Outcome::Failed(e),
            }
        };
        super::bloom::apply(&mut self.color, self.width, self.height, post);
        super::chromatic_aberration(&mut self.color, self.width, self.height, post.chromatic);
        (self.to_rgba8(post), outcome)
    }
}

/// Path of the denoiser library, or why it is not available.
pub fn available() -> Result<std::path::PathBuf, String> {
    oidn::library()
}

#[cfg(test)]
mod tests {
    use crate::Scene;
    use crate::render::cpu::CpuRenderer;

    fn small_render(samples: u32) -> (CpuRenderer, Scene) {
        let mut scene = Scene::default();
        scene.lighting.ao_mode = crate::scene::AoMode::Raytraced;
        let mut r = CpuRenderer::new(scene.clone(), 64, 48);
        for _ in 0..samples {
            r.render_pass();
        }
        (r, scene)
    }

    #[test]
    fn hdr_image_post_processes_like_rgba8() {
        let (r, scene) = small_render(1);
        assert_eq!(r.hdr_image().to_rgba8(&scene.post), r.rgba8());
    }

    #[test]
    fn denoiser_smooths_a_noisy_render() {
        if let Err(e) = super::available() {
            eprintln!("OIDN not available, skipped ({e})");
            return;
        }
        let (noisy, scene) = small_render(2);
        let (clean, _) = small_render(64);
        let rms = |a: &[f32], b: &[f32]| (a.iter().zip(b).map(|(x, y)| (x - y) * (x - y)).sum::<f32>() / a.len() as f32).sqrt();
        let reference = clean.hdr_image().color;
        let mut img = noisy.hdr_image();
        let before = rms(&img.color, &reference);
        let used = img.denoise(1.0).expect("denoise");
        let after = rms(&img.color, &reference);
        eprintln!("{used:?}: rms {before:.4} -> {after:.4}");
        assert!(after < before, "no improvement ({before} -> {after})");
        let _ = scene;
    }
}
