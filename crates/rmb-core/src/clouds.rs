// SPDX-License-Identifier: LGPL-2.1-or-later

//! Volumetric clouds in a horizontal slab of the sky (`Sky::clouds`, background mode Sky).
//!
//! The density follows the scheme of Schneider's Nubis (Horizon Zero Dawn): a coverage map
//! (large-scale 2D noise) decides where clouds may form, a height profile per cloud type
//! (stratus, cumulus, cumulonimbus) shapes them vertically, a Perlin-Worley shape noise
//! gives the round billows and three Worley octaves erode their edges. All noise comes from
//! one baked tileable 3D volume ([`volume`], 64³ texels, four 8-bit channels) that the GPU
//! reads from the same bytes (`maps` buffer under [`NOISE_KEY`]).
//!
//! Lighting per sample: the sun's light at the air (`SkyFrame::sun_air`, so sunsets color
//! the clouds) through a short march toward the sun, a dual-lobe Henyey-Greenstein phase,
//! three octaves of approximate multiple scattering, the "powder" darkening at edges seen
//! away from the sun, and the sky's hemisphere colors as ambient light. Distant clouds fade
//! into the horizon's air. march.wgsl `cloud_*` mirrors this file.

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::math::{V3, v3};
use crate::sky::{Sky, SkyFrame};

/// Key of the noise volume in the GPU's `maps` buffer.
pub const NOISE_KEY: u32 = 0xffff_ffe0;
/// Texels per side of the noise volume.
pub const NOISE_N: usize = 64;
/// Samples along a view ray through the slab, toward the sun, and along a shadow ray.
pub const STEPS: usize = 48;
pub const LIGHT_STEPS: usize = 6;
pub const SHADOW_STEPS: usize = 8;

/// The clouds' settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Clouds {
    pub enabled: bool,
    /// Share of the sky the clouds may cover (0..1).
    pub coverage: f64,
    /// How dense (opaque) the clouds are.
    pub density: f64,
    /// Cloud type: 0 stratus (thin flat layer), 1 cumulus (puffy), 2 cumulonimbus (towering).
    pub kind: f64,
    /// Height of the cloud base, the layer's thickness and the typical size of a cloud
    /// (world units).
    pub base: f64,
    pub thickness: f64,
    pub size: f64,
    /// How much the edges are eroded into wisps and billows (0..1).
    pub detail: f64,
    /// Wind: direction (degrees from +x) and speed (world units per second); evolution: how
    /// fast the shapes change.
    pub wind_direction: f64,
    pub wind_speed: f64,
    pub evolution: f64,
    /// Darker clouds (storms): less of the light is scattered (0..1).
    pub darkness: f64,
    pub tint: [f32; 3],
    /// The clouds shade the scene from the sun (when the sky's sun drives light 1).
    pub shadows: bool,
    /// Thin high cirrus (0 = none).
    pub cirrus: f64,
    /// How far the clouds reach (world units); far clouds fade into the horizon.
    pub reach: f64,
    /// The clouds' clock (seconds; advanced in animations).
    pub time: f64,
    /// World position of the scene's origin (set by `render::shifted_scene` for the deep
    /// zoom, whose positions are relative to the camera; not saved).
    #[serde(skip)]
    pub origin: [f64; 3],
}

impl Default for Clouds {
    fn default() -> Self {
        let mut c = Clouds {
            enabled: false,
            coverage: 0.5,
            density: 1.0,
            kind: 1.0,
            base: 6.0,
            thickness: 3.0,
            size: 5.0,
            detail: 1.0,
            wind_direction: 30.0,
            wind_speed: 0.3,
            evolution: 0.2,
            darkness: 0.0,
            tint: [1.0; 3],
            shadows: true,
            cirrus: 0.0,
            reach: 200.0,
            time: 0.0,
            origin: [0.0; 3],
        };
        c.apply_preset("Fair weather");
        c
    }
}

/// Presets: name, coverage, density, type, thickness, darkness, cirrus.
const PRESETS: &[(&str, f64, f64, f64, f64, f64, f64)] = &[
    ("Fair weather", 0.5, 1.0, 1.0, 3.0, 0.0, 0.0),
    ("Cumulus", 0.55, 1.3, 1.25, 5.0, 0.05, 0.25),
    ("Overcast", 1.0, 1.5, 0.3, 3.0, 0.5, 0.0),
    ("Storm", 0.72, 2.0, 1.9, 9.0, 0.65, 0.0),
    ("High cirrus", 0.0, 1.0, 1.0, 3.0, 0.0, 0.8),
];

impl Clouds {
    pub fn preset_names() -> Vec<&'static str> {
        PRESETS.iter().map(|p| p.0).collect()
    }

    /// Sets the look of a preset (not the height, size, wind or reach).
    pub fn apply_preset(&mut self, name: &str) -> bool {
        let Some(p) = PRESETS.iter().find(|p| p.0 == name) else { return false };
        (self.coverage, self.density, self.kind, self.thickness, self.darkness, self.cirrus) = (p.1, p.2, p.3, p.4, p.5, p.6);
        true
    }
}

/// The baked noise volume: 64³ texels, red = Perlin-Worley shape, green/blue/alpha = Worley
/// fBm at rising frequencies; texel (x, y, z) at index (z · 64 + y) · 64 + x, channels
/// packed red | green << 8 | blue << 16 | alpha << 24. Built once (deterministic).
pub fn volume() -> &'static [u32] {
    static V: OnceLock<Vec<u32>> = OnceLock::new();
    V.get_or_init(bake_volume)
}

fn bake_volume() -> Vec<u32> {
    use rayon::prelude::*;
    let n = NOISE_N;
    (0..n * n * n)
        .into_par_iter()
        .map(|i| {
            let (x, y, z) = (i % n, (i / n) % n, i / (n * n));
            let p = [(x as f64 + 0.5) / n as f64, (y as f64 + 0.5) / n as f64, (z as f64 + 0.5) / n as f64];
            let perlin = (0..4).map(|o| value_noise_tiled(p, 4 << o, 11 + o) * 0.5f64.powi(o)).sum::<f64>() / 1.875;
            let wf = |f: i32, seed: i32| worley(p, f, seed) * 0.625 + worley(p, f * 2, seed + 1) * 0.25 + worley(p, f * 4, seed + 2) * 0.125;
            let shape_worley = wf(4, 100);
            // Perlin-Worley: billowy cells with Perlin's continuity.
            let r = (shape_worley + perlin * (1.0 - shape_worley) - 0.35) / 0.65;
            let q = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
            q(r) | q(wf(2, 200)) << 8 | q(wf(4, 300)) << 16 | q(wf(8, 400)) << 24
        })
        .collect()
}

/// Inverted Worley (cellular) noise of period 1 at `freq` cells: 1 at a feature point.
fn worley(p: [f64; 3], freq: i32, seed: i32) -> f64 {
    let q = p.map(|c| c * freq as f64);
    let cell = q.map(|c| c.floor() as i32);
    let mut best = f64::MAX;
    for dz in -1..=1 {
        for dy in -1..=1 {
            for dx in -1..=1 {
                let c = [cell[0] + dx, cell[1] + dy, cell[2] + dz];
                let w = c.map(|v| v.rem_euclid(freq));
                let h = |k: i32| crate::render::hash3(w[0].wrapping_add(seed.wrapping_mul(7919)).wrapping_add(k * 104_729), w[1], w[2]) as f64;
                let f = [c[0] as f64 + h(0), c[1] as f64 + h(1), c[2] as f64 + h(2)];
                let d = (0..3).map(|k| (q[k] - f[k]).powi(2)).sum::<f64>();
                best = best.min(d);
            }
        }
    }
    1.0 - best.sqrt().min(1.0)
}

/// Value noise of period 1 at `freq` lattice cells (quintic fade).
fn value_noise_tiled(p: [f64; 3], freq: i32, seed: i32) -> f64 {
    let q = p.map(|c| c * freq as f64);
    let i = q.map(|c| c.floor());
    let f = [0, 1, 2].map(|k| {
        let t = q[k] - i[k];
        t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
    });
    let h = |dx: i32, dy: i32, dz: i32| {
        let c = [(i[0] as i32 + dx).rem_euclid(freq), (i[1] as i32 + dy).rem_euclid(freq), (i[2] as i32 + dz).rem_euclid(freq)];
        crate::render::hash3(c[0].wrapping_add(seed.wrapping_mul(7919)), c[1], c[2]) as f64
    };
    let lerp = |a: f64, b: f64, t: f64| a + (b - a) * t;
    let x00 = lerp(h(0, 0, 0), h(1, 0, 0), f[0]);
    let x10 = lerp(h(0, 1, 0), h(1, 1, 0), f[0]);
    let x01 = lerp(h(0, 0, 1), h(1, 0, 1), f[0]);
    let x11 = lerp(h(0, 1, 1), h(1, 1, 1), f[0]);
    lerp(lerp(x00, x10, f[1]), lerp(x01, x11, f[1]), f[2])
}

/// The noise volume at texel coordinates `v` (wrapping, trilinear), four channels 0..1.
pub fn noise(v: V3) -> [f64; 4] {
    let vol = volume();
    let n = NOISE_N as i32;
    let (fx, fy, fz) = (v.x - 0.5, v.y - 0.5, v.z - 0.5);
    let (x0, y0, z0) = (fx.floor(), fy.floor(), fz.floor());
    let (tx, ty, tz) = (fx - x0, fy - y0, fz - z0);
    let (x0, y0, z0) = (x0 as i32, y0 as i32, z0 as i32);
    let mut out = [0.0; 4];
    for dz in 0..2 {
        let wz = if dz == 0 { 1.0 - tz } else { tz };
        for dy in 0..2 {
            let wy = if dy == 0 { 1.0 - ty } else { ty };
            for dx in 0..2 {
                let wx = if dx == 0 { 1.0 - tx } else { tx };
                let (x, y, z) = ((x0 + dx).rem_euclid(n), (y0 + dy).rem_euclid(n), (z0 + dz).rem_euclid(n));
                let t = vol[((z * n + y) * n + x) as usize];
                let w = wx * wy * wz;
                for (k, o) in out.iter_mut().enumerate() {
                    *o += ((t >> (8 * k)) & 255) as f64 * w;
                }
            }
        }
    }
    out.map(|o| o / 255.0)
}

fn smoothstep(a: f64, b: f64, x: f64) -> f64 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Henyey-Greenstein phase.
fn hg(mu: f64, g: f64) -> f64 {
    (1.0 - g * g) / (4.0 * std::f64::consts::PI * (1.0 + g * g - 2.0 * g * mu).max(1e-6).powf(1.5))
}

/// Dual-lobe phase (strong forward peak: silver linings; some back scattering), its
/// eccentricity scaled by `c` (multiple-scattering octaves spread the light).
fn phase(mu: f64, c: f64) -> f64 {
    0.7 * hg(mu, 0.8 * c) + 0.3 * hg(mu, -0.25 * c)
}

/// Scattering albedo of bright clouds; the ambient light's share.
const ALBEDO: f64 = 0.9;
const AMBIENT: f64 = 1.0;
/// Extinction per world unit at density 1 for clouds of size 1 (scaled by 1 / size).
const EXTINCTION: f64 = 24.0;

/// The clouds prepared for a frame (CPU; the GPU gets the same values, renderer.rs
/// `clouds_gpu`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CloudFrame {
    pub base: f64,
    pub top: f64,
    pub reach: f64,
    /// Texels per world unit: shape, coverage, detail.
    pub shape_scale: f64,
    pub cover_scale: f64,
    pub detail_scale: f64,
    /// Extinction per world unit at density 1.
    pub sigma: f64,
    /// First step of the light march (it doubles; the march spans about 1.6 cloud sizes or
    /// layer thicknesses, whichever is smaller, so a low sun still reaches the clouds).
    pub light_step: f64,
    /// Wind offset (world units, x and y) and the detail's evolution offset (texels).
    pub wind: [f64; 2],
    pub evolve: f64,
    pub coverage: f64,
    pub kind: f64,
    pub detail: f64,
    pub cirrus: f64,
    pub cirrus_height: f64,
    pub sun: V3,
    pub shadows: bool,
    /// The sun's light at the clouds, ambient light from above and below, tint × albedo.
    pub sun_light: [f64; 3],
    pub amb_up: [f64; 3],
    pub amb_down: [f64; 3],
    pub tint: [f64; 3],
    /// Added to every position to get world positions (deep zoom shift).
    pub origin: V3,
}

impl CloudFrame {
    /// `None` when the clouds are off or the background is not the sky.
    pub fn for_scene(scene: &crate::Scene, f: &SkyFrame) -> Option<CloudFrame> {
        let c = &scene.sky.clouds;
        if scene.background.mode != crate::scene::BackgroundMode::Sky || !c.enabled {
            return None;
        }
        Some(CloudFrame::new(&scene.sky, f))
    }

    pub fn new(sky: &Sky, f: &SkyFrame) -> CloudFrame {
        let c = &sky.clouds;
        let size = c.size.max(1e-3);
        let thick = c.thickness.max(1e-3);
        let n = NOISE_N as f64;
        let wa = c.wind_direction.to_radians();
        let (up, down) = crate::sky::hemispheres(f);
        let albedo = ALBEDO * (1.0 - 0.75 * c.darkness.clamp(0.0, 1.0));
        CloudFrame {
            base: c.base,
            top: c.base + thick,
            reach: c.reach.max(1.0),
            // One shape tile holds about four clouds across.
            shape_scale: n / (size * 4.0),
            cover_scale: n / (size * 12.0),
            detail_scale: n / size,
            sigma: c.density.max(0.0) * EXTINCTION / size,
            light_step: size.min(thick) * 0.025,
            wind: [wa.cos() * c.wind_speed * c.time, wa.sin() * c.wind_speed * c.time],
            evolve: c.evolution * c.time,
            coverage: c.coverage.clamp(0.0, 1.0),
            kind: c.kind.clamp(0.0, 2.0),
            detail: c.detail.clamp(0.0, 1.0),
            cirrus: c.cirrus.max(0.0),
            cirrus_height: c.base + thick * 2.0 + size * 2.0,
            sun: f.sun,
            shadows: c.shadows,
            sun_light: f.sun_air,
            amb_up: up,
            amb_down: down,
            tint: c.tint.map(|t| t as f64 * albedo),
            origin: V3::from_array(c.origin),
        }
    }
}

/// Height profile of the cloud type at the height share `h` (0 base .. 1 top).
fn profile(h: f64, kind: f64) -> f64 {
    let stratus = smoothstep(0.0, 0.08, h) * smoothstep(0.4, 0.15, h);
    let cumulus = smoothstep(0.0, 0.15, h) * smoothstep(1.0, 0.45, h);
    let nimbus = smoothstep(0.0, 0.06, h) * smoothstep(1.0, 0.8, h);
    if kind < 1.0 { stratus + (cumulus - stratus) * kind } else { cumulus + (nimbus - cumulus) * (kind - 1.0) }
}

/// Cloud density (0..) at the world point `p`; `fine` adds the detail erosion (view rays),
/// without it the cheaper shape only (light and shadow rays).
pub fn density(cf: &CloudFrame, p: V3, fine: bool) -> f64 {
    let p = p + cf.origin;
    let h = (p.z - cf.base) / (cf.top - cf.base);
    if h <= 0.0 || h >= 1.0 || cf.coverage <= 0.0 {
        return 0.0;
    }
    let q = v3(p.x - cf.wind[0], p.y - cf.wind[1], p.z);
    // Where clouds may form: the coverage, varied by a large-scale map.
    let cn = noise(v3(q.x * cf.cover_scale, q.y * cf.cover_scale, 7.5))[0];
    // Anvils: towering clouds spread at their top.
    let anvil = (cf.kind - 1.0).max(0.0) * smoothstep(0.65, 0.95, h) * 0.3;
    // The regional variation fades out toward full coverage (no holes in overcast).
    let vary = 0.6 * (2.0 * (1.0 - cf.coverage)).min(1.0);
    let cov = ((cf.coverage + (cn - 0.5) * vary + anvil).clamp(0.0, 1.0)) * smoothstep(0.0, 0.1, cf.coverage);
    if cov <= 0.0 {
        return 0.0;
    }
    let shape = noise(q * cf.shape_scale)[0];
    let lo = 1.0 - cov;
    let mut d = ((shape * profile(h, cf.kind) - lo) / (1.0 - lo).max(1e-6)).max(0.0) * cov;
    if d <= 0.0 {
        return 0.0;
    }
    if fine && cf.detail > 0.0 {
        let dn = noise(v3(q.x * cf.detail_scale, q.y * cf.detail_scale, q.z * cf.detail_scale + cf.evolve));
        let df = dn[1] * 0.625 + dn[2] * 0.25 + dn[3] * 0.125;
        // Wisps at the bottom, billows at the top.
        let m = df + (1.0 - 2.0 * df) * (h * 8.0).clamp(0.0, 1.0);
        let e = m * 0.4 * cf.detail;
        d = ((d - e) / (1.0 - e)).max(0.0);
    }
    d
}

/// Distances along `d` from `o` where the ray is inside the slab between `lo` and `hi`
/// heights, starting at 0 (`None`: never).
fn slab(cf: &CloudFrame, o: V3, d: V3, lo: f64, hi: f64) -> Option<(f64, f64)> {
    let o = o + cf.origin;
    if d.z.abs() < 1e-9 {
        return (o.z > lo && o.z < hi).then_some((0.0, f64::MAX));
    }
    let (a, b) = ((lo - o.z) / d.z, (hi - o.z) / d.z);
    let (t0, t1) = (a.min(b).max(0.0), a.max(b));
    (t1 > t0).then_some((t0, t1))
}

/// Optical depth toward the sun from `p` (growing steps through the slab, shape only).
fn light_depth(cf: &CloudFrame, p: V3) -> f64 {
    let mut ds = cf.light_step;
    let mut t = 0.0;
    let mut depth = 0.0;
    for _ in 0..LIGHT_STEPS {
        let s = t + ds * 0.5;
        depth += density(cf, p + cf.sun * s, false) * ds;
        t += ds;
        ds *= 2.0;
    }
    depth * cf.sigma
}

/// The clouds along a view ray from `o` (world) in direction `d` up to `t_end` (`f64::MAX`
/// for rays that miss the scene): what passes (`keep`, 0..1) and the light they add.
/// `jitter` (0..1) offsets the steps; `air` is the sky's air color in direction `d` (far
/// clouds fade into it); `flash`: lightning inside the clouds (`storm::StormFrame`).
pub fn march(cf: &CloudFrame, o: V3, d: V3, t_end: f64, jitter: f64, air: [f64; 3], flash: &[crate::storm::CloudLight]) -> (f64, [f64; 3]) {
    let mu = d.dot(cf.sun);
    // Cirrus behind everything (only on rays to the open sky).
    let (mut keep, mut add) = (1.0, [0.0; 3]);
    if cf.cirrus > 0.0 && t_end == f64::MAX && d.z > 1e-4 {
        let t = (cf.cirrus_height - o.z - cf.origin.z) / d.z;
        if t > 0.0 {
            let q = o + cf.origin + d * t;
            let s = cf.shape_scale * 0.35;
            let n = noise(v3((q.x - cf.wind[0] * 2.0) * s * 0.4, (q.y - cf.wind[1] * 2.0) * s * 1.6, 3.5 + cf.evolve * 0.3));
            let streak = smoothstep(0.45, 0.8, n[1] * 0.7 + n[2] * 0.3);
            let a = (streak * cf.cirrus * 0.6 * (-t / (cf.reach * 3.0)).exp()).clamp(0.0, 1.0);
            let light = [0, 1, 2].map(|k| (cf.sun_light[k] * phase(mu, 0.6) * 0.5 + cf.amb_up[k] * AMBIENT) * cf.tint[k]);
            keep = 1.0 - a;
            add = light.map(|l| l * a);
        }
    }
    // Beyond the reach (rays to the open sky): a far deck as opaque as the coverage, in
    // the color of distant hazy clouds; below the horizon the ground lies in their shade.
    let open = t_end == f64::MAX;
    let deck = ((cf.coverage - 0.15) / 0.85).clamp(0.0, 1.0);
    // Under a closed deck the air itself lies in the shade.
    let air = air.map(|a| a * (1.0 - 0.7 * deck));
    if open && d.z < 0.0 && o.z + cf.origin.z < cf.base {
        let shade = 1.0 - 0.7 * deck;
        return (keep * shade, add.map(|a| a * shade));
    }
    let Some((t0, t1_full)) = slab(cf, o, d, cf.base, cf.top) else { return (keep, add) };
    if open && deck > 0.0 && t1_full > cf.reach {
        let far = t1_full - t0.max(cf.reach);
        let a = deck * (1.0 - (-far * 0.15 * cf.sigma).exp());
        if a > 0.0 {
            let cloud = [0, 1, 2].map(|k| (cf.sun_light[k] * phase(mu, 0.5) * 0.6 * (-2.0 * cf.coverage).exp() + cf.amb_up[k] * AMBIENT) * cf.tint[k]);
            let color = [0, 1, 2].map(|k| 0.5 * (cloud[k] + air[k]));
            add = [0, 1, 2].map(|k| add[k] * (1.0 - a) + color[k] * a);
            keep *= 1.0 - a;
        }
    }
    let t1 = t1_full.min(t_end).min(cf.reach);
    if t1 <= t0 {
        return (keep, add);
    }
    let dt = (t1 - t0) / STEPS as f64;
    let mut trans = 1.0;
    let mut light = [0.0; 3];
    let (mut wsum, mut tsum) = (0.0, 0.0);
    for i in 0..STEPS {
        let t = t0 + (i as f64 + jitter) * dt;
        let p = o + d * t;
        let dn = density(cf, p, true);
        if dn <= 1e-4 {
            continue;
        }
        let ld = light_depth(cf, p);
        // Multiple scattering: octaves with less extinction, less contribution, wider phase.
        let mut sun = 0.0;
        let mut a = 1.0;
        for _ in 0..3 {
            sun += a * phase(mu, a) * (-ld * a).exp();
            a *= 0.5;
        }
        // Powder: edges seen away from the sun stay darker.
        let powder = 1.0 - (-2.0 * ld).exp();
        let sun = sun * (powder + (1.0 - powder) * (0.5 + 0.5 * mu));
        let h = (p.z - cf.base) / (cf.top - cf.base);
        let a = (-cf.sigma * dn * dt).exp();
        let w = trans * (1.0 - a);
        let fl = if flash.is_empty() { [0.0; 3] } else { crate::storm::cloud_light(flash, p, cf.top - cf.base, cf.sigma) };
        for (k, l) in light.iter_mut().enumerate() {
            let amb = cf.amb_down[k] + (cf.amb_up[k] - cf.amb_down[k]) * h;
            *l += w * (cf.sun_light[k] * sun + amb * AMBIENT + fl[k]) * cf.tint[k];
        }
        wsum += w;
        tsum += w * t;
        trans *= a;
        if trans < 0.01 {
            break;
        }
    }
    if wsum <= 0.0 {
        return (keep, add);
    }
    // Far clouds melt into the horizon's air.
    let fade = (-(tsum / wsum) / (cf.reach * 0.35)).exp();
    let light = [0, 1, 2].map(|k| light[k] * fade + air[k] * (1.0 - trans) * (1.0 - fade));
    // The cumulus layer in front of the cirrus.
    (keep * trans, [0, 1, 2].map(|k| light[k] + add[k] * trans))
}

/// The sky's average light above and below seen through the clouds from the scene's center
/// (the skylight of `sky::sync_sun`): the same 48 cosine-weighted directions as
/// `sky::hemispheres`, each through [`march`]; the ground below is lit as much as the sun
/// passes the clouds. Cached for the last frame values (called every frame by the app).
pub fn hemispheres(cf: &CloudFrame, f: &SkyFrame) -> ([f64; 3], [f64; 3]) {
    type Cache = Option<(CloudFrame, SkyFrame, ([f64; 3], [f64; 3]))>;
    static CACHE: std::sync::Mutex<Cache> = std::sync::Mutex::new(None);
    if let Ok(c) = CACHE.lock()
        && let Some((a, b, r)) = c.as_ref()
        && a == cf
        && b == f
    {
        return *r;
    }
    const N: usize = 48;
    let (_, clear_down) = crate::sky::hemispheres(f);
    let mut up = [0.0; 3];
    for i in 0..N {
        let z = ((i as f64 + 0.5) / N as f64).sqrt();
        let a = i as f64 * 2.399_963_229_728_653;
        let r = (1.0 - z * z).sqrt();
        let d = v3(r * a.cos(), r * a.sin(), z);
        let air = crate::sky::air(f, d);
        let (keep, add) = march(cf, V3::ZERO, d, f64::MAX, 0.5, air, &[]);
        for k in 0..3 {
            up[k] += (air[k] * keep + add[k]) / N as f64;
        }
    }
    let sun = shadow(cf, V3::ZERO, cf.sun);
    let down = clear_down.map(|v| v * (0.3 + 0.7 * sun));
    if let Ok(mut c) = CACHE.lock() {
        *c = Some((*cf, *f, (up, down)));
    }
    (up, down)
}

/// Share of the sun's light that passes the clouds from `p` toward `l` (cloud shadows).
pub fn shadow(cf: &CloudFrame, p: V3, l: V3) -> f64 {
    let Some((t0, t1)) = slab(cf, p, l, cf.base, cf.top) else { return 1.0 };
    let t1 = t1.min(t0 + cf.reach);
    let dt = (t1 - t0) / SHADOW_STEPS as f64;
    let mut depth = 0.0;
    for i in 0..SHADOW_STEPS {
        depth += density(cf, p + l * (t0 + (i as f64 + 0.5) * dt), false) * dt;
    }
    (-depth * cf.sigma).exp()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(coverage: f64) -> CloudFrame {
        let mut sky = Sky { hour: 11.0, ..Sky::default() };
        sky.clouds.enabled = true;
        sky.clouds.coverage = coverage;
        CloudFrame::new(&sky, &SkyFrame::new(&sky))
    }

    #[test]
    fn the_noise_is_deterministic_and_tiles() {
        let a = noise(v3(3.3, 17.8, 40.1));
        let b = noise(v3(3.3 + 64.0, 17.8 - 128.0, 40.1 + 64.0));
        for k in 0..4 {
            assert!((a[k] - b[k]).abs() < 1e-12, "{a:?} {b:?}");
        }
        // Every channel varies.
        let mut lo = [1.0f64; 4];
        let mut hi = [0.0f64; 4];
        for t in volume().iter().step_by(97) {
            for k in 0..4 {
                let v = ((t >> (8 * k)) & 255) as f64 / 255.0;
                lo[k] = lo[k].min(v);
                hi[k] = hi[k].max(v);
            }
        }
        for k in 0..4 {
            assert!(hi[k] - lo[k] > 0.4, "channel {k}: {} .. {}", lo[k], hi[k]);
        }
    }

    #[test]
    fn clouds_live_in_their_slab_and_follow_the_coverage() {
        let cf = frame(0.6);
        let mut inside = 0;
        for i in 0..400 {
            let (x, y) = ((i % 20) as f64 * 1.7, (i / 20) as f64 * 1.3);
            assert_eq!(density(&cf, v3(x, y, cf.base - 0.01), true), 0.0);
            assert_eq!(density(&cf, v3(x, y, cf.top + 0.01), true), 0.0);
            if density(&cf, v3(x, y, cf.base + 0.3 * (cf.top - cf.base)), true) > 0.0 {
                inside += 1;
            }
        }
        assert!(inside > 40, "{inside} of 400 points in clouds");
        let none = frame(0.0);
        assert_eq!(density(&none, v3(1.0, 2.0, none.base + 1.0), true), 0.0);
        // More coverage, more clouds.
        let count = |c: f64| {
            let cf = frame(c);
            (0..400).filter(|i| density(&cf, v3((i % 20) as f64 * 1.7, (i / 20) as f64 * 1.3, cf.base + 0.3 * (cf.top - cf.base)), true) > 0.0).count()
        };
        assert!(count(0.9) > count(0.6) && count(0.6) > count(0.3), "{} {} {}", count(0.9), count(0.6), count(0.3));
    }

    #[test]
    fn the_slab_is_found_from_below_inside_and_above() {
        let (lo, hi) = (6.0, 9.0);
        let cf = frame(0.5);
        let up = v3(0.0, 0.6, 0.8);
        let (a, b) = slab(&cf, v3(0.0, 0.0, 0.0), up, lo, hi).unwrap();
        assert!((a - 7.5).abs() < 1e-9 && (b - 11.25).abs() < 1e-9);
        let (a, _) = slab(&cf, v3(0.0, 0.0, 7.0), up, lo, hi).unwrap();
        assert_eq!(a, 0.0);
        assert!(slab(&cf, v3(0.0, 0.0, 10.0), up, lo, hi).is_none());
        assert!(slab(&cf, v3(0.0, 0.0, 10.0), -up, lo, hi).is_some());
        assert!(slab(&cf, v3(0.0, 0.0, 0.0), v3(1.0, 0.0, 0.0), lo, hi).is_none());
    }

    #[test]
    fn overcast_hides_the_sky_and_shades_the_ground() {
        let clear = frame(0.0);
        let overcast = frame(1.0);
        let up = v3(0.1, 0.2, 0.97).normalize();
        let (k0, _) = march(&clear, V3::ZERO, up, f64::MAX, 0.5, [0.0; 3], &[]);
        let (k1, l1) = march(&overcast, V3::ZERO, up, f64::MAX, 0.5, [0.0; 3], &[]);
        assert_eq!(k0, 1.0);
        assert!(k1 < 0.2, "overcast lets {k1} through");
        assert!(l1.iter().all(|v| *v > 0.0));
        assert!(shadow(&overcast, V3::ZERO, overcast.sun) < 0.5);
        assert_eq!(shadow(&clear, V3::ZERO, clear.sun), 1.0);
        // Denser clouds let less through.
        let mut thick = overcast;
        thick.sigma *= 3.0;
        assert!(march(&thick, V3::ZERO, up, f64::MAX, 0.5, [0.0; 3], &[]).0 <= k1);
    }

    #[test]
    fn the_wind_moves_the_clouds() {
        let mut sky = Sky { hour: 11.0, ..Sky::default() };
        sky.clouds.enabled = true;
        let f = SkyFrame::new(&sky);
        let a = CloudFrame::new(&sky, &f);
        sky.clouds.time = 10.0;
        let b = CloudFrame::new(&sky, &f);
        let at = |cf: &CloudFrame| (0..200).map(|i| density(cf, v3(i as f64 * 0.37, 1.0, cf.base + 1.0), true)).collect::<Vec<_>>();
        assert_ne!(at(&a), at(&b));
        // Moved along the wind: the field at time 10 equals the one at 0, shifted by the
        // wind's way (detail evolution off).
        sky.clouds.evolution = 0.0;
        sky.clouds.time = 0.0;
        let c0 = CloudFrame::new(&sky, &f);
        sky.clouds.time = 10.0;
        let c1 = CloudFrame::new(&sky, &f);
        let w = v3(c1.wind[0], c1.wind[1], 0.0);
        let p = v3(2.0, 3.0, c0.base + 1.2);
        assert!((density(&c0, p, true) - density(&c1, p + w, true)).abs() < 1e-9);
    }

    #[test]
    fn presets_differ() {
        let names = Clouds::preset_names();
        let mut seen = Vec::new();
        for n in &names {
            let mut c = Clouds::default();
            assert!(c.apply_preset(n));
            assert!(!seen.contains(&c), "{n} equals another preset");
            seen.push(c);
        }
    }
}
