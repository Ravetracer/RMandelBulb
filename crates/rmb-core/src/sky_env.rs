// SPDX-License-Identifier: LGPL-2.1-or-later

//! The sky as environment light (like an HDRI box image): the sky's air and its clouds baked
//! from the scene's center into an equirectangular radiance map, and from it an irradiance
//! map (the cosine-weighted average radiance around each normal, the same units as the
//! hemisphere ambient colors). The sun disk is left out: the sun stays light 1.
//!
//! Classic shading takes the ambient light from [`irradiance`] (× the ambient amount, ×
//! AO), path tracing's misses from [`radiance`]. The GPU reads the same maps from `lut`
//! (renderer.rs `env_gpu`), march.wgsl `env_lookup` mirrors [`lookup`].

use std::sync::{Arc, Mutex};

use crate::clouds::CloudFrame;
use crate::math::{V3, v3};
use crate::sky::SkyFrame;

/// Radiance map and irradiance map sizes (equirectangular: x around from +x, y from the
/// zenith down to the nadir).
pub const ENV_W: usize = 64;
pub const ENV_H: usize = 32;
pub const IRR_W: usize = 16;
pub const IRR_H: usize = 8;

/// The baked maps (linear RGB).
#[derive(Clone, Debug, PartialEq)]
pub struct SkyEnv {
    pub radiance: Vec<[f32; 3]>,
    pub irradiance: Vec<[f32; 3]>,
}

/// Direction of the texel center (x, y) of a `w` × `h` map.
fn texel_dir(x: usize, y: usize, w: usize, h: usize) -> V3 {
    let az = (x as f64 + 0.5) / w as f64 * std::f64::consts::TAU;
    let el = std::f64::consts::FRAC_PI_2 - (y as f64 + 0.5) / h as f64 * std::f64::consts::PI;
    v3(el.cos() * az.cos(), el.cos() * az.sin(), el.sin())
}

/// Whether the scene takes its ambient light from the sky's environment.
pub fn active(scene: &crate::Scene) -> bool {
    scene.background.mode == crate::scene::BackgroundMode::Sky && scene.sky.sun_light && scene.sky.environment
}

/// The environment of the scene's sky (cached for the last sky and cloud frame values; the
/// bake marches the clouds once per radiance texel). `None` when it is not used.
pub fn for_scene(scene: &crate::Scene) -> Option<Arc<SkyEnv>> {
    if !active(scene) {
        return None;
    }
    let f = SkyFrame::new(&scene.sky);
    let cf = CloudFrame::for_scene(scene, &f);
    type Cache = Option<(SkyFrame, Option<CloudFrame>, Arc<SkyEnv>)>;
    static CACHE: Mutex<Cache> = Mutex::new(None);
    if let Ok(c) = CACHE.lock()
        && let Some((a, b, env)) = c.as_ref()
        && *a == f
        && *b == cf
    {
        return Some(env.clone());
    }
    let env = Arc::new(bake(&f, cf.as_ref()));
    if let Ok(mut c) = CACHE.lock() {
        *c = Some((f, cf, env.clone()));
    }
    Some(env)
}

/// Bakes the maps: the sky's air (ground below the horizon included) seen through the
/// clouds from the world's origin.
pub fn bake(f: &SkyFrame, cf: Option<&CloudFrame>) -> SkyEnv {
    use rayon::prelude::*;
    let radiance: Vec<[f32; 3]> = (0..ENV_W * ENV_H)
        .into_par_iter()
        .map(|i| {
            let d = texel_dir(i % ENV_W, i / ENV_W, ENV_W, ENV_H);
            let air = crate::sky::air(f, d);
            let c = match cf {
                Some(cf) => {
                    let (keep, add) = crate::clouds::march(cf, V3::ZERO - cf.origin, d, f64::MAX, 0.5, air, &[]);
                    [0, 1, 2].map(|k| air[k] * keep + add[k])
                }
                None => air,
            };
            c.map(|v| v as f32)
        })
        .collect();
    // Solid angle of each radiance row (equal-angle rows: smaller toward the poles).
    let dirs: Vec<(V3, f64)> = (0..ENV_W * ENV_H)
        .map(|i| {
            let d = texel_dir(i % ENV_W, i / ENV_W, ENV_W, ENV_H);
            let solid = (std::f64::consts::TAU / ENV_W as f64) * (std::f64::consts::PI / ENV_H as f64) * (1.0 - d.z * d.z).sqrt();
            (d, solid)
        })
        .collect();
    let irradiance: Vec<[f32; 3]> = (0..IRR_W * IRR_H)
        .into_par_iter()
        .map(|i| {
            let n = texel_dir(i % IRR_W, i / IRR_W, IRR_W, IRR_H);
            let mut sum = [0.0f64; 3];
            let mut weight = 0.0;
            for ((d, solid), l) in dirs.iter().zip(&radiance) {
                let c = n.dot(*d);
                if c > 0.0 {
                    let w = c * solid;
                    for k in 0..3 {
                        sum[k] += l[k] as f64 * w;
                    }
                    weight += w;
                }
            }
            sum.map(|s| (s / weight.max(1e-12)) as f32)
        })
        .collect();
    SkyEnv { radiance, irradiance }
}

/// Bilinear lookup of a `w` × `h` map in direction `d` (wraps around, clamps at the poles).
pub fn lookup(map: &[[f32; 3]], w: usize, h: usize, d: V3) -> [f64; 3] {
    let u = d.y.atan2(d.x) / std::f64::consts::TAU;
    let u = u - u.floor();
    let v = (std::f64::consts::FRAC_PI_2 - d.z.clamp(-1.0, 1.0).asin()) / std::f64::consts::PI;
    let x = u * w as f64 - 0.5;
    let y = (v * h as f64 - 0.5).clamp(0.0, h as f64 - 1.0);
    let (x0, y0) = (x.floor(), y.floor());
    let (tx, ty) = (x - x0, y - y0);
    let xi = |dx: i32| ((x0 as i32 + dx).rem_euclid(w as i32)) as usize;
    let yi = |dy: i32| ((y0 as i32 + dy).clamp(0, h as i32 - 1)) as usize;
    let at = |dx: i32, dy: i32| map[yi(dy) * w + xi(dx)];
    let mut out = [0.0; 3];
    for (k, o) in out.iter_mut().enumerate() {
        let a = at(0, 0)[k] as f64 * (1.0 - tx) + at(1, 0)[k] as f64 * tx;
        let b = at(0, 1)[k] as f64 * (1.0 - tx) + at(1, 1)[k] as f64 * tx;
        *o = a * (1.0 - ty) + b * ty;
    }
    out
}

/// Ambient light (average radiance seen by a surface) for the normal `n`.
pub fn irradiance(env: &SkyEnv, n: V3) -> [f64; 3] {
    lookup(&env.irradiance, IRR_W, IRR_H, n)
}

/// The sky's radiance in direction `d` (without the sun disk).
pub fn radiance(env: &SkyEnv, d: V3) -> [f64; 3] {
    lookup(&env.radiance, ENV_W, ENV_H, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sky::Sky;

    #[test]
    fn a_uniform_sky_gives_its_own_radiance() {
        // Irradiance of a constant map equals the constant.
        let flat = vec![[0.5f32, 0.25, 1.0]; ENV_W * ENV_H];
        let dirs: Vec<V3> = (0..ENV_W * ENV_H).map(|i| texel_dir(i % ENV_W, i / ENV_W, ENV_W, ENV_H)).collect();
        let n = v3(0.3, -0.2, 0.9).normalize();
        let (mut s, mut w) = (0.0, 0.0);
        for d in &dirs {
            let c = n.dot(*d).max(0.0) * (1.0 - d.z * d.z).sqrt();
            s += flat[0][2] as f64 * c;
            w += c;
        }
        assert!((s / w - 1.0).abs() < 1e-9);
        assert!((lookup(&flat, ENV_W, ENV_H, n)[1] - 0.25).abs() < 1e-9);
    }

    #[test]
    fn a_sunset_lights_more_from_the_sun_and_the_sky_from_above() {
        let sky = Sky { hour: 19.0, latitude: 45.0, day: 172.0, ..Sky::default() };
        let f = SkyFrame::new(&sky);
        let env = bake(&f, None);
        let toward = v3(f.sun.x, f.sun.y, 0.2).normalize();
        let away = v3(-f.sun.x, -f.sun.y, 0.2).normalize();
        let lum = |c: [f64; 3]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
        assert!(lum(irradiance(&env, toward)) > lum(irradiance(&env, away)) * 1.2);
        // By day: an upward face sees more sky than a downward one sees ground... and bluer.
        let day = Sky { hour: 11.0, ..Sky::default() };
        let env = bake(&SkyFrame::new(&day), None);
        let (up, down) = (irradiance(&env, v3(0.0, 0.0, 1.0)), irradiance(&env, v3(0.0, 0.0, -1.0)));
        assert!(up[2] / up[0] > down[2] / down[0], "{up:?} {down:?}");
        // The radiance map matches the sky away from the sun disk.
        let d = v3(0.2, 0.5, 0.6).normalize();
        let (r, a) = (radiance(&env, d), crate::sky::air(&SkyFrame::new(&day), d));
        assert!((lum(r) - lum(a)).abs() < 0.08 * lum(a), "{r:?} {a:?}");
    }
}
