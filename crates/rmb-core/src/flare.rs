// SPDX-License-Identifier: LGPL-2.1-or-later

//! Lens flares: the reflections and diffraction a camera lens adds around a bright light.
//!
//! A flare is drawn in screen space from the light's position in the image and added to the
//! radiance before exposure and tone mapping (it is light that went through the lens). Its
//! parts follow what real lenses show:
//! - glow: a bright core with a long soft tail, and a wide haze that lifts the dark parts;
//! - rays: a diffraction star (4, 6, 8 … tapering spikes) and optional fine random spikes;
//! - anamorphic streak: a long thin line (and a softer one around it), typically blue;
//! - ghosts: reflections of the aperture (round or polygonal discs, some hollow) on the line
//!   from the light through the image center, with colored fringes (chromatic aberration);
//! - ring: a halo around the light, optionally rainbow colored;
//! - sparkles: small dots along the streak; lens dirt: soft specks lit near the light.
//!
//! The flare fades when the fractal hides the light (rays toward a small disc around it), with
//! the depth fog in front of it and when the light leaves the image. Sizes are fractions of
//! the image height, so a flare looks the same at every resolution.

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::math::{V3, pcg, unit};
use crate::scene::{LightKind, Scene};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LensFlare {
    pub enabled: bool,
    /// Overall brightness.
    pub intensity: f64,
    /// Multiplies the light's color.
    pub tint: [f32; 3],
    pub glow: f64,
    pub glow_size: f64,
    pub haze: f64,
    pub haze_size: f64,
    /// Diffraction star: number of spikes (0 = none).
    pub rays: u32,
    pub ray_intensity: f64,
    pub ray_length: f64,
    /// Thickness of the spikes (1 = thin).
    pub ray_width: f64,
    /// Degrees.
    pub ray_rotation: f64,
    /// 0..1: uneven spike lengths and brightness.
    pub ray_random: f64,
    /// Fine random spikes around the light.
    pub spikes: u32,
    pub spike_intensity: f64,
    pub spike_length: f64,
    /// Anamorphic streak brightness (0 = none).
    pub streak: f64,
    pub streak_length: f64,
    pub streak_width: f64,
    pub streak_color: [f32; 3],
    /// Degrees (0 = horizontal).
    pub streak_angle: f64,
    pub ghosts: u32,
    pub ghost_intensity: f64,
    pub ghost_size: f64,
    /// How far the ghosts spread along the line through the image center.
    pub ghost_spread: f64,
    /// Aperture blades (0 = round ghosts).
    pub ghost_blades: u32,
    /// Edge softness of the ghosts (0..1).
    pub ghost_softness: f64,
    /// Share of hollow (ring) ghosts.
    pub ghost_rings: f64,
    /// 0: ghosts in the light's color; 1: in varied lens coating tints.
    pub ghost_colors: f64,
    /// Colored fringes of ghosts and ring.
    pub chromatic: f64,
    /// Halo ring around the light.
    pub ring: f64,
    pub ring_size: f64,
    pub ring_width: f64,
    /// 0..1: rainbow colored ring.
    pub rainbow: f64,
    /// Small dots along the streak line.
    pub sparkles: u32,
    pub sparkle_intensity: f64,
    /// Lens dirt lit by the light.
    pub dirt: f64,
    /// How far from the light the dirt lights up.
    pub dirt_reach: f64,
    /// Random seed of spikes, ghosts, sparkles and dirt.
    pub seed: u32,
}

impl Default for LensFlare {
    fn default() -> Self {
        LensFlare {
            enabled: false,
            intensity: 1.0,
            tint: [1.0; 3],
            glow: 1.0,
            glow_size: 0.03,
            haze: 0.05,
            haze_size: 0.45,
            rays: 6,
            ray_intensity: 0.5,
            ray_length: 0.3,
            ray_width: 1.0,
            ray_rotation: 15.0,
            ray_random: 0.3,
            spikes: 0,
            spike_intensity: 0.2,
            spike_length: 0.25,
            streak: 0.0,
            streak_length: 1.2,
            streak_width: 1.0,
            streak_color: [0.35, 0.55, 1.0],
            streak_angle: 0.0,
            ghosts: 7,
            ghost_intensity: 0.35,
            ghost_size: 0.08,
            ghost_spread: 1.0,
            ghost_blades: 6,
            ghost_softness: 0.4,
            ghost_rings: 0.3,
            ghost_colors: 0.6,
            chromatic: 0.5,
            ring: 0.0,
            ring_size: 0.25,
            ring_width: 0.015,
            rainbow: 0.7,
            sparkles: 0,
            sparkle_intensity: 0.3,
            dirt: 0.0,
            dirt_reach: 0.6,
            seed: 1,
        }
    }
}

impl LensFlare {
    /// Ready-made looks (enabled), to start from.
    pub fn presets() -> Vec<(&'static str, LensFlare)> {
        let d = LensFlare { enabled: true, ..Default::default() };
        vec![
            ("Classic", d.clone()),
            ("Anamorphic", LensFlare { rays: 0, streak: 1.0, streak_length: 1.6, ghosts: 5, ghost_blades: 0, ghost_rings: 0.5, sparkles: 14, sparkle_intensity: 0.25, haze: 0.07, ..d.clone() }),
            ("Sun", LensFlare { glow: 1.6, glow_size: 0.06, haze: 0.12, haze_size: 0.7, rays: 12, ray_intensity: 0.35, ray_length: 0.45, spikes: 60, spike_intensity: 0.15, ghosts: 8, ghost_size: 0.07, ring: 0.25, ring_size: 0.3, rainbow: 0.8, dirt: 0.6, tint: [1.0, 0.92, 0.8], ..d.clone() }),
            ("Star", LensFlare { rays: 4, ray_intensity: 1.0, ray_length: 0.45, ray_random: 0.0, ray_rotation: 0.0, ray_width: 0.8, ghosts: 0, haze: 0.08, ..d.clone() }),
            ("Spike ball", LensFlare { rays: 8, ray_intensity: 0.5, spikes: 120, spike_intensity: 0.3, spike_length: 0.35, ghosts: 0, haze: 0.08, ..d.clone() }),
            ("Sci-fi", LensFlare { rays: 0, streak: 1.4, streak_length: 2.5, streak_width: 0.7, ghosts: 3, ghost_blades: 0, ghost_rings: 1.0, ghost_intensity: 0.2, ring: 0.2, rainbow: 0.0, sparkles: 24, sparkle_intensity: 0.4, haze: 0.08, ..d.clone() }),
            ("Subtle", LensFlare { glow: 0.6, haze: 0.03, rays: 0, ghosts: 4, ghost_intensity: 0.12, ghost_blades: 0, ..d.clone() }),
            ("Dirty lens", LensFlare { rays: 6, ray_intensity: 0.3, ghosts: 6, ghost_intensity: 0.15, dirt: 1.0, dirt_reach: 0.7, haze: 0.08, ..d.clone() }),
        ]
    }
}

// ---------------------------------------------------------------------------------------------

/// A flare prepared in pixel space.
struct Prepared {
    /// Light position in pixels.
    lx: f64,
    ly: f64,
    /// Scale: the image height in pixels.
    s: f64,
    color: [f64; 3],
    f: LensFlare,
    rays: Vec<Ray>,
    ghosts: Vec<Ghost>,
    sparkles: Vec<Dot>,
    dirt: Vec<Dot>,
    /// Bounding radius of everything centered on the light.
    reach: f64,
}

struct Ray {
    cos: f64,
    sin: f64,
    len: f64,
    width: f64,
    int: f64,
}

struct Ghost {
    x: f64,
    y: f64,
    r: f64,
    color: [f64; 3],
    rot: f64,
    ring: bool,
}

struct Dot {
    x: f64,
    y: f64,
    r: f64,
    int: f64,
}

/// Random number `k` of stream `seed`.
fn rnd(seed: u32, k: u32) -> f64 {
    unit(pcg(seed.wrapping_mul(0x9E37_79B9) ^ pcg(k)))
}

/// Lens coating tints for ghosts.
const COATINGS: [[f64; 3]; 6] = [[0.25, 1.0, 0.4], [0.75, 0.3, 1.0], [1.0, 0.55, 0.15], [0.2, 0.5, 1.0], [1.0, 0.3, 0.45], [0.3, 1.0, 0.9]];

/// Rainbow color at `t` (0..1, red outside).
fn spectrum(t: f64) -> [f64; 3] {
    // Hue from violet (inside) to red (outside).
    let h = (1.0 - t.clamp(0.0, 1.0)) * 0.75 * 6.0;
    [((h - 3.0).abs() - 1.0).clamp(0.0, 1.0), (2.0 - (h - 2.0).abs()).clamp(0.0, 1.0), (2.0 - (h - 4.0).abs()).clamp(0.0, 1.0)]
}

fn smoothstep(e0: f64, e1: f64, x: f64) -> f64 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl Prepared {
    fn new(f: &LensFlare, lx: f64, ly: f64, w: f64, h: f64, color: [f64; 3]) -> Prepared {
        let s = h;
        let seed = f.seed;
        let mut rays = Vec::new();
        let base_w = 0.0012 * s * f.ray_width.max(0.05);
        for i in 0..f.rays.min(64) {
            let jit = (rnd(seed, i * 3) - 0.5) * f.ray_random * std::f64::consts::PI / f.rays.max(1) as f64;
            let a = f.ray_rotation.to_radians() + std::f64::consts::TAU * i as f64 / f.rays as f64 + jit;
            let r = f.ray_random.clamp(0.0, 1.0);
            rays.push(Ray { cos: a.cos(), sin: a.sin(), len: f.ray_length * s * (1.0 - 0.6 * r * rnd(seed, i * 3 + 1)), width: base_w, int: f.ray_intensity * (1.0 - 0.6 * r * rnd(seed, i * 3 + 2)) });
        }
        for i in 0..f.spikes.min(400) {
            let k = 1000 + i * 3;
            let a = rnd(seed, k) * std::f64::consts::TAU;
            rays.push(Ray { cos: a.cos(), sin: a.sin(), len: f.spike_length * s * (0.2 + 0.8 * rnd(seed, k + 1)), width: base_w * 0.6, int: f.spike_intensity * (0.2 + 0.8 * rnd(seed, k + 2)) });
        }
        // Ghosts on the line from the light through the image center.
        let (cx, cy) = (w * 0.5, h * 0.5);
        let mut ghosts = Vec::new();
        // Coating tints cycle (5 is coprime to their count), starting at a random one.
        let first = (rnd(seed, 1999) * COATINGS.len() as f64) as usize;
        for i in 0..f.ghosts.min(32) {
            let k = 2000 + i * 7;
            let t = 0.25 + (0.4 + 1.6 * rnd(seed, k)) * f.ghost_spread;
            let size = f.ghost_size * s * (0.35 + 1.3 * rnd(seed, k + 1) * rnd(seed, k + 1).sqrt() + 0.3 * (t - 1.0).abs());
            let tint = COATINGS[(first + i as usize * 5) % COATINGS.len()];
            let mix = f.ghost_colors.clamp(0.0, 1.0);
            let bright = f.ghost_intensity * (0.3 + 0.7 * rnd(seed, k + 3)) / (1.0 + 2.0 * size / s);
            let color = [0, 1, 2].map(|c| (color[c] * (1.0 - mix) + tint[c] * mix) * bright);
            ghosts.push(Ghost { x: lx + (cx - lx) * t, y: ly + (cy - ly) * t, r: size, color, rot: rnd(seed, k + 4) * 0.3, ring: rnd(seed, k + 5) < f.ghost_rings });
        }
        let (sa, ca) = f.streak_angle.to_radians().sin_cos();
        let mut sparkles = Vec::new();
        for i in 0..f.sparkles.min(200) {
            let k = 3000 + i * 4;
            let along = (rnd(seed, k) * 2.0 - 1.0) * f.streak_length * s * 0.9;
            let off = (rnd(seed, k + 1) - 0.5) * 0.01 * s;
            sparkles.push(Dot { x: lx + ca * along - sa * off, y: ly - sa * along - ca * off, r: (0.002 + 0.006 * rnd(seed, k + 2)) * s, int: f.sparkle_intensity * (0.3 + 0.7 * rnd(seed, k + 3)) * (1.0 - (along.abs() / (f.streak_length * s).max(1e-9)).min(1.0)) });
        }
        let mut dirt = Vec::new();
        if f.dirt > 0.0 {
            for i in 0..120 {
                let k = 4000 + i * 4;
                let big = i < 12;
                dirt.push(Dot {
                    x: rnd(seed, k) * w,
                    y: rnd(seed, k + 1) * h,
                    r: if big { (0.05 + 0.1 * rnd(seed, k + 2)) * s } else { (0.004 + 0.02 * rnd(seed, k + 2)) * s },
                    int: f.dirt * if big { 0.04 } else { 0.12 * (0.3 + 0.7 * rnd(seed, k + 3)) },
                });
            }
        }
        let reach = [f.glow_size * 8.0, f.ray_length, f.spike_length, f.ring_size + 4.0 * f.ring_width].iter().cloned().fold(0.0, f64::max) * s;
        Prepared { lx, ly, s, color, f: f.clone(), rays, ghosts, sparkles, dirt, reach }
    }

    /// The flare's light at pixel center (`x`, `y`).
    fn at(&self, x: f64, y: f64) -> [f64; 3] {
        let f = &self.f;
        let (dx, dy) = (x - self.lx, y - self.ly);
        let r2 = dx * dx + dy * dy;
        let r = r2.sqrt();
        let s = self.s;
        let mut out = [0.0; 3];
        let add = |o: &mut [f64; 3], c: [f64; 3], k: f64| {
            for i in 0..3 {
                o[i] += c[i] * k;
            }
        };
        // Haze: a wide glow with a brighter inner part, faded out smoothly.
        if f.haze > 0.0 {
            let hz = (f.haze_size * s).max(1e-9);
            let q = r2 / (hz * hz);
            if q < 16.0 {
                let k = f.haze * (0.6 * (-q).exp() + 0.4 * (-8.0 * q).exp()) * (1.0 - smoothstep(9.0, 16.0, q));
                add(&mut out, self.color, k);
            }
        }
        if r < self.reach {
            // Glow: sharp core with a long tail, and the wide haze.
            let g = (f.glow_size * s).max(1e-9);
            let q = r2 / (g * g);
            let mut k = f.glow * ((-4.0 * q).exp() + 0.2 / (1.0 + 4.0 * q) * (1.0 - smoothstep(16.0, 64.0, q)));
            // Diffraction star and fine spikes: tapering, fading lines.
            for ray in &self.rays {
                let along = dx * ray.cos + dy * ray.sin;
                if along <= 0.0 || along >= ray.len {
                    continue;
                }
                let x = along / ray.len;
                let w = ray.width * (1.0 - 0.7 * x) + 0.5;
                let perp = (dy * ray.cos - dx * ray.sin).abs();
                if perp > 3.0 * w {
                    continue;
                }
                k += ray.int * (-(perp * perp) / (w * w)).exp() * (1.0 - x) * (1.0 - x) * (1.0 + 2.0 * (-8.0 * x).exp());
            }
            add(&mut out, self.color, k);
            // Halo ring.
            if f.ring > 0.0 {
                let (rr, rw) = (f.ring_size * s, (f.ring_width * s).max(1e-9));
                let ch = f.chromatic * rw;
                let mut c = [0.0; 3];
                for (i, o) in [-1.0, 0.0, 1.0].iter().enumerate() {
                    let d = (r - rr - o * ch) / rw;
                    c[2 - i] = (-d * d).exp();
                }
                let t = ((r - rr) / (2.0 * rw) * 0.5 + 0.5).clamp(0.0, 1.0);
                let sp = spectrum(t);
                let tint = [0, 1, 2].map(|i| self.color[i] * (1.0 - f.rainbow) + sp[i] * f.rainbow);
                for i in 0..3 {
                    out[i] += f.ring * c[i] * tint[i] * 0.5;
                }
            }
        }
        // Anamorphic streak: a thin bright line inside a softer one.
        if f.streak > 0.0 {
            let (sa, ca) = f.streak_angle.to_radians().sin_cos();
            let along = dx * ca - dy * sa;
            let perp = dx * sa + dy * ca;
            let len = (f.streak_length * s).max(1e-9);
            let w = (0.0015 * s * f.streak_width.max(0.05)).max(0.6);
            if perp.abs() < 12.0 * w && along.abs() < len * 3.0 {
                let fall = (-along.abs() / (0.35 * len)).exp() * (1.0 - smoothstep(0.0, 3.0 * len, along.abs()));
                let k = f.streak * fall * ((-(perp * perp) / (w * w)).exp() + 0.25 * (-(perp * perp) / (16.0 * w * w)).exp());
                let sc = f.streak_color.map(|v| v as f64);
                add(&mut out, [0, 1, 2].map(|i| sc[i] * (0.7 + 0.3 * self.color[i])), k);
            }
        }
        // Ghosts: aperture-shaped discs or rings with colored fringes.
        for gh in &self.ghosts {
            let (gx, gy) = (x - gh.x, y - gh.y);
            let reach = gh.r * 1.3;
            if gx.abs() > reach || gy.abs() > reach {
                continue;
            }
            let gr = (gx * gx + gy * gy).sqrt();
            // Radius of the aperture polygon in this direction.
            let shape = if f.ghost_blades >= 3 {
                let n = f.ghost_blades as f64;
                let seg = std::f64::consts::TAU / n;
                let a = (gy.atan2(gx) - gh.rot).rem_euclid(seg) - seg * 0.5;
                (seg * 0.5).cos() / a.cos()
            } else {
                1.0
            };
            let soft = f.ghost_softness.clamp(0.02, 1.0);
            let mut c = [0.0; 3];
            for (i, scale) in [1.0 + 0.04 * f.chromatic, 1.0, 1.0 - 0.04 * f.chromatic].iter().enumerate() {
                let rho = gr / (gh.r * shape * scale);
                let m = if gh.ring {
                    let d = (rho - 0.85) / (0.06 + 0.2 * soft);
                    (-d * d).exp()
                } else {
                    // Disc with a slightly brighter rim.
                    (1.0 - smoothstep(1.0 - soft, 1.0, rho)) * (0.55 + 0.45 * rho.powi(4).min(1.0))
                };
                c[i] = m;
            }
            for i in 0..3 {
                out[i] += gh.color[i] * c[i];
            }
        }
        for sp in &self.sparkles {
            let (sx, sy) = (x - sp.x, y - sp.y);
            if sx.abs() > sp.r * 3.0 || sy.abs() > sp.r * 3.0 {
                continue;
            }
            let k = sp.int * (-(sx * sx + sy * sy) / (sp.r * sp.r)).exp();
            let sc = f.streak_color.map(|v| v as f64);
            add(&mut out, [0, 1, 2].map(|i| 0.5 * (sc[i] + self.color[i])), k);
        }
        if !self.dirt.is_empty() {
            let reach = (f.dirt_reach * s).max(1e-9);
            let light = (-r2 / (reach * reach)).exp();
            if light > 1e-3 {
                let mut k = 0.0;
                for d in &self.dirt {
                    let (ex, ey) = (x - d.x, y - d.y);
                    if ex.abs() > d.r || ey.abs() > d.r {
                        continue;
                    }
                    let q = (ex * ex + ey * ey) / (d.r * d.r);
                    k += d.int * (1.0 - smoothstep(0.5, 1.0, q.sqrt()));
                }
                add(&mut out, self.color, k * light);
            }
        }
        out
    }
}

/// Screen position (pixels) of a light, or None behind the camera.
fn screen_pos(scene: &Scene, p: V3, directional: bool, w: f64, h: f64) -> Option<(f64, f64)> {
    let cam = &scene.camera;
    let d = if directional { p } else { p - cam.position };
    let z = d.dot(cam.forward);
    if z <= 1e-9 {
        return None;
    }
    let t = cam.tan_half_fov();
    let sx = d.dot(cam.right) / (z * t * (w / h));
    let sy = d.dot(cam.up) / (z * t);
    Some(((sx + 1.0) * 0.5 * w, (1.0 - sy) * 0.5 * h))
}

/// The lens flares of all lights of `scene` at `width` × `height` (RGB per pixel, top row
/// first), or None when no light has a flare in view.
pub fn layer(scene: &Scene, width: u32, height: u32) -> Option<Vec<[f32; 3]>> {
    let (w, h) = (width.max(1) as f64, height.max(1) as f64);
    let mut flares = Vec::new();
    for l in &scene.lighting.lights {
        if !l.enabled || !l.flare.enabled || l.flare.intensity <= 0.0 {
            continue;
        }
        // Where the light is: area lights at their center, line lights at their middle.
        let (p, directional) = match l.kind {
            LightKind::Directional => (l.direction(&scene.camera), true),
            LightKind::Line => ((l.position + l.target) * 0.5, false),
            _ => (l.position, false),
        };
        let Some((lx, ly)) = screen_pos(scene, p, directional, w, h) else { continue };
        // Fades out as the light leaves the image.
        let out = (-lx).max(lx - w).max(-ly).max(ly - h).max(0.0);
        let edge = 1.0 - smoothstep(0.0, 0.35 * h, out);
        if edge <= 0.0 {
            continue;
        }
        let vis = crate::render::cpu::light_visibility(scene, p, directional, 0.012);
        let fog = if directional {
            scene.fog.amount(scene.march.max_distance, crate::render::start_distance(scene), scene.march.max_distance)
        } else {
            scene.fog.amount((p - scene.camera.position).length(), crate::render::start_distance(scene), scene.march.max_distance)
        };
        let k = l.flare.intensity * vis * edge * (1.0 - fog.clamp(0.0, 1.0));
        if k <= 1e-4 {
            continue;
        }
        let peak = l.color.iter().cloned().fold(1e-6, f32::max) as f64;
        let color = [0, 1, 2].map(|i| l.color[i] as f64 / peak * l.flare.tint[i] as f64 * k);
        flares.push(Prepared::new(&l.flare, lx, ly, w, h, color));
    }
    if flares.is_empty() {
        return None;
    }
    let (wi, hi) = (width as usize, height as usize);
    let mut out = vec![[0.0f32; 3]; wi * hi];
    out.par_chunks_mut(wi).enumerate().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let mut c = [0.0; 3];
            for f in &flares {
                let v = f.at(x as f64 + 0.5, y as f64 + 0.5);
                for i in 0..3 {
                    c[i] += v[i];
                }
            }
            *px = c.map(|v| v as f32);
        }
    });
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::v3;

    fn scene_with_flare(f: LensFlare) -> Scene {
        let mut s = Scene::default();
        // A point light in view, in front of the bulb (seen from the default camera).
        let cam = s.camera.clone();
        let p = cam.position + cam.forward * 1.0 + cam.right * 0.2 + cam.up * 0.1;
        s.lighting.lights.push(crate::scene::Light { kind: LightKind::Point, position: p, flare: f, ..Default::default() });
        s
    }

    #[test]
    fn flare_is_brightest_at_the_light_and_absent_when_off() {
        let s = scene_with_flare(LensFlare { enabled: true, ..Default::default() });
        let (w, h) = (160u32, 90u32);
        let img = layer(&s, w, h).expect("flare in view");
        let (lx, ly) = screen_pos(&s, s.lighting.lights.last().unwrap().position, false, w as f64, h as f64).unwrap();
        let at = |x: f64, y: f64| img[(y as usize).min(h as usize - 1) * w as usize + (x as usize).min(w as usize - 1)][1];
        assert!(at(lx, ly) > 0.5, "{}", at(lx, ly));
        assert!(at(lx, ly) > at(lx + 30.0, ly + 25.0) * 5.0);
        let off = scene_with_flare(LensFlare::default());
        assert!(layer(&off, w, h).is_none());
    }

    #[test]
    fn hidden_light_has_no_flare() {
        // A light inside the bulb cannot be seen.
        let mut s = scene_with_flare(LensFlare { enabled: true, ..Default::default() });
        s.lighting.lights.last_mut().unwrap().position = v3(0.0, 0.0, 0.0);
        assert!(layer(&s, 64, 36).is_none());
    }

    #[test]
    fn presets_render() {
        for (name, f) in LensFlare::presets() {
            let s = scene_with_flare(f);
            let img = layer(&s, 96, 54).unwrap_or_else(|| panic!("{name}"));
            assert!(img.iter().all(|c| c.iter().all(|v| v.is_finite() && *v >= 0.0)), "{name}");
        }
    }
}
