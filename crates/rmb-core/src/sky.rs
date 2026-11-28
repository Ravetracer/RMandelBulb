// SPDX-License-Identifier: LGPL-2.1-or-later

//! The sky: an atmosphere around the scene, lit by a sun placed by the time of day.
//!
//! Analytic single scattering per color channel (Rayleigh and Mie scattering as RGB
//! coefficients, so alien skies are a matter of other colors): the light scattered toward the
//! eye along a view direction is the sun's light, dimmed by the air it crossed (its air mass),
//! times the phase functions, times the share the view path scatters (its own air mass). The
//! horizon whitens because the view path is long; the sun reddens near the horizon because its
//! path is. A small isotropic term stands for multiple scattering, twilight fades the sky out
//! over the sun's last 18° below the horizon, the ground below the horizon is lit by the sun
//! and hazed toward the horizon. The sun disk is seen through the atmosphere; stars show where
//! the sky is dark. march.wgsl `sky_radiance` mirrors [`radiance`].

use serde::{Deserialize, Serialize};

use crate::math::{V3, v3};

/// How the sun is placed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SunPlacement {
    /// By the time of day, latitude and day of the year (real solar geometry).
    #[default]
    TimeOfDay,
    /// By azimuth and elevation.
    Manual,
}

impl SunPlacement {
    pub const ALL: [SunPlacement; 2] = [SunPlacement::TimeOfDay, SunPlacement::Manual];

    pub fn label(self) -> &'static str {
        match self {
            SunPlacement::TimeOfDay => "Time of day",
            SunPlacement::Manual => "Manual",
        }
    }
}

/// The sky's settings (`Scene::sky`; shown when the background mode is Sky).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Sky {
    pub placement: SunPlacement,
    /// Time of day in hours (0..24), latitude in degrees (north positive), day of the year
    /// (1..365) and the direction of north in the world (degrees from +x, like light azimuths).
    pub hour: f64,
    pub latitude: f64,
    pub day: f64,
    pub north: f64,
    /// Manual placement: degrees from +x and above the horizon.
    pub azimuth: f64,
    pub elevation: f64,
    /// Rayleigh scattering (the color of the clear sky): its color and strength.
    pub rayleigh_color: [f32; 3],
    pub rayleigh: f64,
    /// Mie scattering (haze, dust): its color, amount and how much it scatters forward per
    /// channel (0 evenly .. 0.95 a tight halo around the sun).
    pub mie_color: [f32; 3],
    pub mie: f64,
    pub mie_direction: [f64; 3],
    /// Thickness of the atmosphere (multiplies both).
    pub density: f64,
    /// Ozone (absorbs orange and green high above the air: keeps the twilight sky blue;
    /// Earth 1).
    pub ozone: f64,
    /// The ground below the horizon.
    pub ground_color: [f32; 3],
    /// The sun: its color, angular diameter (degrees) and the brightness of its disk.
    pub sun_color: [f32; 3],
    pub sun_size: f64,
    pub sun_brightness: f64,
    /// Overall brightness of the sky.
    pub brightness: f64,
    /// Stars at night: how many and how bright.
    pub stars: f64,
    pub star_brightness: f64,
    /// The sun lights the scene: light 1 follows it (direction, color, intensity × strength),
    /// and the ambient light takes the sky's and the ground's average colors (× skylight).
    pub sun_light: bool,
    pub sun_light_strength: f64,
    pub sky_light_strength: f64,
    /// Time-lapse: hours per second in animations.
    pub day_speed: f64,
}

impl Default for Sky {
    fn default() -> Self {
        let mut s = Sky {
            placement: SunPlacement::TimeOfDay,
            hour: 15.0,
            latitude: 45.0,
            day: 172.0,
            north: 90.0,
            azimuth: 30.0,
            elevation: 35.0,
            rayleigh_color: [0.15, 0.4, 1.0],
            rayleigh: 0.25,
            mie_color: [1.0, 1.0, 1.0],
            mie: 0.025,
            mie_direction: [0.76; 3],
            density: 1.0,
            ozone: 1.0,
            ground_color: [0.3, 0.28, 0.25],
            sun_color: [1.0, 0.98, 0.95],
            sun_size: 0.8,
            sun_brightness: 1.0,
            brightness: 1.0,
            stars: 0.5,
            star_brightness: 1.0,
            sun_light: true,
            sun_light_strength: 1.0,
            sky_light_strength: 1.0,
            day_speed: 0.0,
        };
        s.apply_preset("Earth clear");
        s
    }
}

/// Preset atmospheres: name, Rayleigh color + strength, Mie color + amount, Mie direction
/// per channel, density, ground color, sun color, ozone.
type SkyPreset = (&'static str, [f32; 3], f64, [f32; 3], f64, [f64; 3], f64, [f32; 3], [f32; 3], f64);

const PRESETS: &[SkyPreset] = &[
    ("Earth clear", [0.15, 0.4, 1.0], 0.25, [1.0, 1.0, 1.0], 0.025, [0.76; 3], 1.0, [0.3, 0.28, 0.25], [1.0, 0.98, 0.95], 1.0),
    ("Earth hazy", [0.15, 0.4, 1.0], 0.25, [1.0, 0.98, 0.95], 0.3, [0.7; 3], 1.0, [0.3, 0.28, 0.25], [1.0, 0.97, 0.92], 1.0),
    // Butterscotch dust by day; the dust scatters blue more forward: blue sunsets.
    ("Mars", [0.15, 0.4, 1.0], 0.02, [1.0, 0.68, 0.42], 0.45, [0.55, 0.65, 0.88], 1.0, [0.45, 0.25, 0.14], [1.0, 0.95, 0.9], 0.0),
    ("Titan", [0.3, 0.45, 1.0], 0.05, [1.0, 0.55, 0.18], 1.4, [0.6, 0.62, 0.65], 1.5, [0.3, 0.18, 0.08], [1.0, 0.9, 0.75], 0.0),
    ("Alien green", [0.25, 1.0, 0.35], 0.3, [0.9, 1.0, 0.85], 0.08, [0.75; 3], 1.0, [0.2, 0.25, 0.15], [1.0, 0.95, 0.85], 0.0),
    ("Alien violet", [0.75, 0.2, 1.0], 0.3, [1.0, 0.85, 1.0], 0.1, [0.75; 3], 1.0, [0.22, 0.15, 0.25], [1.0, 0.82, 0.72], 0.0),
];

impl Sky {
    pub fn preset_names() -> Vec<&'static str> {
        PRESETS.iter().map(|p| p.0).collect()
    }

    /// Sets the atmosphere (not the sun's place or the stars) to a preset.
    pub fn apply_preset(&mut self, name: &str) -> bool {
        let Some(p) = PRESETS.iter().find(|p| p.0 == name) else { return false };
        (self.rayleigh_color, self.rayleigh, self.mie_color, self.mie, self.mie_direction, self.density, self.ground_color, self.sun_color, self.ozone) = (p.1, p.2, p.3, p.4, p.5, p.6, p.7, p.8, p.9);
        true
    }

    /// Unit vector toward the sun (world; z up).
    pub fn sun_dir(&self) -> V3 {
        match self.placement {
            SunPlacement::Manual => {
                let (az, el) = (self.azimuth.to_radians(), self.elevation.to_radians());
                v3(el.cos() * az.cos(), el.cos() * az.sin(), el.sin())
            }
            SunPlacement::TimeOfDay => {
                let (e, n, u) = solar_enu(self.latitude, self.day, self.hour);
                let a = self.north.to_radians();
                let north = v3(a.cos(), a.sin(), 0.0);
                let east = v3(a.sin(), -a.cos(), 0.0);
                (east * e + north * n + v3(0.0, 0.0, u)).normalize()
            }
        }
    }
}

/// The sun's direction as east, north, up for `latitude` (degrees), `day` of the year and
/// `hour` (local solar time).
pub fn solar_enu(latitude: f64, day: f64, hour: f64) -> (f64, f64, f64) {
    let decl = (-23.44f64).to_radians() * (std::f64::consts::TAU * (day + 10.0) / 365.0).cos();
    let h = (15.0 * (hour - 12.0)).to_radians();
    let phi = latitude.clamp(-90.0, 90.0).to_radians();
    (-decl.cos() * h.sin(), phi.cos() * decl.sin() - phi.sin() * decl.cos() * h.cos(), phi.sin() * decl.sin() + phi.cos() * decl.cos() * h.cos())
}

/// Relative air mass at `sin_el` (sine of the elevation; Kasten and Young 1989), the horizon's
/// below it.
pub fn air_mass(sin_el: f64) -> f64 {
    let s = sin_el.max(0.0);
    let deg = s.asin().to_degrees();
    1.0 / (s + 0.50572 * (deg + 6.07995).powf(-1.6364))
}

/// Brightness scale of the sky (radiance of the sun's light before the atmosphere).
const SUN_SCALE: f64 = 10.0;
/// Share of the sun's air mass in front of the scattering air (it scatters at some height,
/// so the sky is reddened less than the sun's light at the ground), and its largest value
/// (the low sun still lights the air above through a shorter path than the ground's).
const SUN_PATH_SHARE: f64 = 0.5;
const SUN_PATH_MAX: f64 = 6.0;
/// Ozone's optical depth at the zenith per unit of `Sky::ozone` (Earth's, relative to the
/// default Rayleigh scattering).
const OZONE: [f64; 3] = [0.0085, 0.026, 0.0012];
/// The ground's brightness relative to the sky's scale.
const GROUND_LIGHT: f64 = 0.3;
/// Multiple-scattering share (isotropic fill).
const MULTI: f64 = 0.15;
/// Mie extinction per unit of scattering (some absorption).
const MIE_EXT: f64 = 1.1;
/// Below the horizon the ground fades in over this rate of the downward direction.
const GROUND_FADE: f64 = 8.0;
/// Cells of the star grid per unit of direction.
const STAR_CELLS: f64 = 140.0;

/// The sky prepared for a frame (CPU; the GPU gets the same values, renderer.rs `sky_gpu`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SkyFrame {
    pub sun: V3,
    /// Optical depth at the zenith: Rayleigh scattering, Mie scattering (density included).
    pub beta_r: [f64; 3],
    pub beta_m: [f64; 3],
    pub mie_g: [f64; 3],
    /// The sun's light at the ground (color × scale × transmittance × twilight fade) and at
    /// the scattering air (through `SUN_PATH_SHARE` of the air mass).
    pub sun_light: [f64; 3],
    pub sun_air: [f64; 3],
    /// Cosine of the sun's angular radius, and its disk brightness (× color × scale).
    pub sun_cos: f64,
    pub sun_disk: [f64; 3],
    pub ground: [f64; 3],
    /// Stars: share of grid cells with a star, brightness.
    pub stars: f64,
    pub star_brightness: f64,
}

impl SkyFrame {
    pub fn new(sky: &Sky) -> SkyFrame {
        let sun = sky.sun_dir();
        let dens = sky.density.max(0.0);
        let beta_r = [0, 1, 2].map(|k| sky.rayleigh_color[k] as f64 * sky.rayleigh.max(0.0) * dens);
        let beta_m = [0, 1, 2].map(|k| sky.mie_color[k] as f64 * sky.mie.max(0.0) * dens);
        // The sun just below the horizon still lights the upper sky (twilight, fading over 18°).
        let el = sun.z.clamp(-1.0, 1.0).asin().to_degrees();
        let fade = smoothstep(-18.0, 0.5, el);
        let m_sun = air_mass(sun.z.max(0.005));
        let scale = SUN_SCALE * sky.brightness.max(0.0);
        // Ozone lies above the scattering air: the full air mass for both.
        let oz = [0, 1, 2].map(|k| OZONE[k] * sky.ozone.max(0.0) * dens * m_sun);
        let through = |m: f64| [0, 1, 2].map(|k| sky.sun_color[k] as f64 * scale * (-(beta_r[k] + MIE_EXT * beta_m[k]) * m - oz[k]).exp() * fade);
        let (sun_light, sun_air) = (through(m_sun), through((m_sun * SUN_PATH_SHARE).min(SUN_PATH_MAX)));
        let half = (sky.sun_size.max(0.01) * 0.5).to_radians();
        SkyFrame {
            sun,
            beta_r,
            beta_m,
            mie_g: sky.mie_direction.map(|g| g.clamp(-0.95, 0.95)),
            sun_light,
            sun_air,
            sun_cos: half.cos(),
            sun_disk: [0, 1, 2].map(|k| sky.sun_color[k] as f64 * scale * 50.0 * sky.sun_brightness.max(0.0) * smoothstep(-1.0, 0.5, el)),
            ground: sky.ground_color.map(|c| c as f64),
            stars: sky.stars.clamp(0.0, 1.0),
            star_brightness: sky.star_brightness.max(0.0) * sky.brightness.max(0.0),
        }
    }

    /// The sun's light reaching the ground (for the light it drives), before the strength.
    pub fn sun_ground_light(&self) -> [f64; 3] {
        self.sun_light.map(|v| v / SUN_SCALE)
    }
}

fn smoothstep(a: f64, b: f64, x: f64) -> f64 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Scattered light toward the eye along the direction `d` above the horizon (`d.z ≥ 0`), and
/// the transmittance of that path.
fn atmosphere(f: &SkyFrame, d: V3) -> ([f64; 3], [f64; 3]) {
    let m = air_mass(d.z);
    let mu = d.dot(f.sun).clamp(-1.0, 1.0);
    let rayleigh = 3.0 / (16.0 * std::f64::consts::PI) * (1.0 + mu * mu);
    let mut light = [0.0; 3];
    let mut trans = [0.0; 3];
    for k in 0..3 {
        let g = f.mie_g[k];
        let mie = (1.0 - g * g) / (4.0 * std::f64::consts::PI * (1.0 + g * g - 2.0 * g * mu).max(1e-9).powf(1.5));
        let ext = f.beta_r[k] + MIE_EXT * f.beta_m[k];
        let t = (-ext * m).exp();
        let scattered = if ext > 1e-12 { (f.beta_r[k] * rayleigh + f.beta_m[k] * mie) / ext } else { 0.0 };
        let fill = if ext > 1e-12 { f.beta_r[k] / ext * MULTI / (4.0 * std::f64::consts::PI) } else { 0.0 };
        light[k] = f.sun_air[k] * (scattered + fill) * (1.0 - t);
        trans[k] = t;
    }
    (light, trans)
}

/// Sky radiance in direction `d` (unit; world, z up): atmosphere, sun disk, ground below the
/// horizon, stars (mirrors march.wgsl `sky_radiance`).
pub fn radiance(f: &SkyFrame, d: V3) -> [f64; 3] {
    radiance_ex(f, d, true)
}

/// The air's light only (no sun disk, no stars): the color of the haze in front of distant
/// things (the fog's color with *from background*).
pub fn air(f: &SkyFrame, d: V3) -> [f64; 3] {
    radiance_ex(f, d, false)
}

fn radiance_ex(f: &SkyFrame, d: V3, sources: bool) -> [f64; 3] {
    if d.z < 0.0 {
        // The ground, lit by the sun and the sky above, hazed toward the horizon's color.
        let h = v3(d.x, d.y, 0.0);
        let h = if h.length() > 1e-9 { h.normalize() } else { v3(1.0, 0.0, 0.0) };
        let (haze, _) = atmosphere(f, h);
        let lit = (f.sun.z.max(0.0) / std::f64::consts::PI + 0.05) * GROUND_LIGHT;
        let w = (d.z * GROUND_FADE).exp();
        return [0, 1, 2].map(|k| haze[k] * w + f.ground[k] * f.sun_light[k] * lit * (1.0 - w));
    }
    let (mut c, t) = atmosphere(f, d);
    if !sources {
        return c;
    }
    // The sun disk, through the atmosphere, darker toward its rim (limb darkening).
    let mu = d.dot(f.sun);
    if mu > f.sun_cos {
        let r = ((1.0 - mu) / (1.0 - f.sun_cos)).clamp(0.0, 1.0);
        let edge = smoothstep(1.0, 0.8, r);
        let limb = 1.0 - 0.6 * (1.0 - (1.0 - r).sqrt());
        for k in 0..3 {
            c[k] += f.sun_disk[k] * t[k] * limb * edge;
        }
    }
    // Stars where the sky is dark.
    if f.stars > 0.0 && f.star_brightness > 0.0 {
        let lum = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
        let night = (1.0 - lum * 20.0).clamp(0.0, 1.0);
        if night > 0.0 {
            let s = star(f, d).map(|v| v * night * f.star_brightness);
            for k in 0..3 {
                c[k] += s[k] * t[k];
            }
        }
    }
    c
}

/// A star in the grid cell of `d`, if any (mirrors march.wgsl `sky_star`).
fn star(f: &SkyFrame, d: V3) -> [f64; 3] {
    let p = d * STAR_CELLS;
    let (cx, cy, cz) = (p.x.floor(), p.y.floor(), p.z.floor());
    let (ix, iy, iz) = (cx as i32, cy as i32, cz as i32);
    let h = |m: i32| crate::render::hash3(ix.wrapping_add(m.wrapping_mul(7919)), iy, iz) as f64;
    if h(0) >= f.stars * 0.25 {
        return [0.0; 3];
    }
    // The star in the middle part of its cell, on the sphere's surface direction.
    let s = v3(cx + 0.2 + 0.6 * h(1), cy + 0.2 + 0.6 * h(2), cz + 0.2 + 0.6 * h(3));
    let dist = (p - s.normalize() * p.length()).length();
    let r = 0.06 + 0.05 * h(4);
    if dist >= r {
        return [0.0; 3];
    }
    let b = (1.0 - dist / r).powi(2) * 8.0 * h(5).powi(3);
    // From bluish to orange.
    let tint = h(6);
    [b * (0.8 + 0.3 * tint), b * 0.9, b * (1.15 - 0.4 * tint)]
}

/// Average light of the sky above and of the ground below the horizon, cosine-weighted around
/// up and down (the air only: no sun disk, no stars), for the ambient light.
pub fn hemispheres(f: &SkyFrame) -> ([f64; 3], [f64; 3]) {
    const N: usize = 48;
    let (mut up, mut down) = ([0.0; 3], [0.0; 3]);
    for i in 0..N {
        // Cosine-distributed points (Fibonacci spiral, z = sqrt(u)).
        let z = ((i as f64 + 0.5) / N as f64).sqrt();
        let a = i as f64 * 2.399_963_229_728_653;
        let r = (1.0 - z * z).sqrt();
        let (x, y) = (r * a.cos(), r * a.sin());
        let (u, d) = (air(f, v3(x, y, z)), air(f, v3(x, y, -z)));
        for k in 0..3 {
            up[k] += u[k] / N as f64;
            down[k] += d[k] / N as f64;
        }
    }
    (up, down)
}

/// Writes the sun into light 1 (directional, in the world, its color and intensity) and the
/// sky's average colors into the ambient light, when the background is the sky and the sky
/// drives the light. Returns whether it changed anything.
pub fn sync_sun(scene: &mut crate::Scene) -> bool {
    if scene.background.mode != crate::scene::BackgroundMode::Sky || !scene.sky.sun_light {
        return false;
    }
    let f = SkyFrame::new(&scene.sky);
    let light = f.sun_ground_light();
    let peak = light.iter().fold(0.0f64, |a, b| a.max(*b));
    let color = if peak > 1e-9 { light.map(|v| (v / peak) as f32) } else { scene.sky.sun_color };
    let intensity = peak * scene.sky.sun_light_strength.max(0.0) * 2.5;
    let (az, el) = (f.sun.y.atan2(f.sun.x).to_degrees(), f.sun.z.clamp(-1.0, 1.0).asin().to_degrees());
    // Without any light the sky adds light 1 (the sun must light the scene).
    let added = scene.lighting.lights.is_empty();
    if added {
        scene.lighting.lights.push(crate::scene::Light { shadows: true, ..Default::default() });
    }
    let l = &mut scene.lighting.lights[0];
    let before = l.clone();
    l.kind = crate::scene::LightKind::Directional;
    l.camera_relative = false;
    l.azimuth = az;
    l.elevation = el;
    l.color = color;
    l.intensity = intensity;
    l.enabled = true;
    let mut changed = added || *l != before;
    // Ambient: the hemisphere colors scaled to the brightest channel of the sky.
    let (up, down) = hemispheres(&f);
    let amb = up.iter().fold(0.0f64, |a, b| a.max(*b));
    let lt = &mut scene.lighting;
    let before = (lt.ambient, lt.sky_color, lt.ground_color, lt.ambient_camera_relative);
    lt.ambient = amb * scene.sky.sky_light_strength.max(0.0);
    if amb > 1e-9 {
        lt.sky_color = up.map(|v| (v / amb) as f32);
        lt.ground_color = down.map(|v| (v / amb).min(1.0) as f32);
    }
    lt.ambient_camera_relative = false;
    changed |= before != (lt.ambient, lt.sky_color, lt.ground_color, lt.ambient_camera_relative);
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lum(c: [f64; 3]) -> f64 {
        0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
    }

    fn at_hour(h: f64) -> Sky {
        Sky { hour: h, latitude: 0.0, day: 80.0, stars: 0.0, ..Sky::default() }
    }

    #[test]
    fn the_sun_follows_the_day() {
        // Equinox at the equator: up at 6 h, overhead at noon, down at 18 h.
        let (e, _, u) = solar_enu(0.0, 80.0, 12.0);
        assert!(u > 0.999 && e.abs() < 1e-3, "{u} {e}");
        assert!(solar_enu(0.0, 80.0, 6.0).2.abs() < 0.02);
        assert!(solar_enu(0.0, 80.0, 18.0).2.abs() < 0.02);
        // Rises in the east, sets in the west.
        assert!(solar_enu(0.0, 80.0, 7.0).0 > 0.9 && solar_enu(0.0, 80.0, 17.0).0 < -0.9);
        // Noon height at 45° north in June: 90 - 45 + 23.44.
        let u = solar_enu(45.0, 172.0, 12.0).2;
        assert!((u.asin().to_degrees() - 68.44).abs() < 0.3, "{}", u.asin().to_degrees());
        // North at +y: the noon sun of the north is in the south (-y).
        let s = Sky { latitude: 45.0, day: 172.0, hour: 12.0, ..Sky::default() };
        assert!(s.sun_dir().y < -0.3, "{:?}", s.sun_dir());
    }

    #[test]
    fn blue_by_day_red_at_sunset_dark_at_night() {
        // Morning sun (45° high), the zenith away from it.
        let noon = SkyFrame::new(&at_hour(9.0));
        let zenith = radiance(&noon, v3(0.0, 0.0, 1.0));
        assert!(zenith[2] > 2.0 * zenith[0], "{zenith:?}");
        // The horizon is paler (less blue against red) than the zenith.
        let horizon = radiance(&noon, v3(1.0, 0.0, 0.02).normalize());
        assert!(horizon[2] / horizon[0] < zenith[2] / zenith[0], "{horizon:?} {zenith:?}");
        // At sunset the sky near the sun is red-orange.
        let sunset = at_hour(17.9);
        let f = SkyFrame::new(&sunset);
        let toward = radiance(&f, (v3(f.sun.x, f.sun.y, 0.0).normalize() + v3(0.0, 0.0, 0.05)).normalize());
        assert!(toward[0] > toward[2] * 1.5, "{toward:?}");
        // At night: dark, but the stars show.
        let night = Sky { stars: 1.0, ..at_hour(0.5) };
        let f = SkyFrame::new(&night);
        assert!(lum(radiance(&f, v3(0.3, 0.1, 0.9).normalize())) < 1e-3);
        let mut lit = 0;
        for i in 0..20000 {
            let a = i as f64 * 0.61803 * std::f64::consts::TAU;
            let z = (i as f64 + 0.5) / 20000.0;
            let r = (1.0 - z * z).sqrt();
            if lum(radiance(&f, v3(r * a.cos(), r * a.sin(), z))) > 0.05 {
                lit += 1;
            }
        }
        assert!(lit > 20, "{lit} stars");
    }

    #[test]
    fn mars_has_blue_sunsets_and_the_horizon_is_continuous() {
        let mut s = at_hour(17.8);
        s.apply_preset("Mars");
        let f = SkyFrame::new(&s);
        let near = radiance(&f, (f.sun * 0.97 + v3(0.0, 0.0, 0.25)).normalize());
        assert!(near[2] > near[0] * 0.8, "near the sun: {near:?}");
        let s = at_hour(12.0);
        let mut mars = s.clone();
        mars.apply_preset("Mars");
        let day = radiance(&SkyFrame::new(&mars), v3(0.0, 1.0, 0.4).normalize());
        assert!(day[0] > day[2], "butterscotch by day: {day:?}");
        // Just above and below the horizon: about the same color.
        let f = SkyFrame::new(&s);
        let (a, b) = (radiance(&f, v3(1.0, 0.0, 1e-4).normalize()), radiance(&f, v3(1.0, 0.0, -1e-4).normalize()));
        for k in 0..3 {
            assert!((a[k] - b[k]).abs() < 0.02 * a[k].max(0.01), "{a:?} {b:?}");
        }
    }

    #[test]
    fn ozone_keeps_the_twilight_sky_blue() {
        let zenith = |ozone: f64| {
            let s = Sky { ozone, ..at_hour(18.05) };
            radiance(&SkyFrame::new(&s), v3(0.0, 0.0, 1.0))
        };
        let (with, without) = (zenith(1.0), zenith(0.0));
        assert!(with[2] > with[1] * 1.5 && with[2] / with[1] > without[2] / without[1], "{with:?} {without:?}");
    }

    #[test]
    fn the_sun_drives_light_one() {
        let mut scene = crate::Scene::default();
        scene.background.mode = crate::scene::BackgroundMode::Sky;
        scene.sky = at_hour(9.0);
        assert!(sync_sun(&mut scene));
        assert!(!sync_sun(&mut scene), "idempotent");
        let l = scene.lighting.lights[0].clone();
        let d = l.direction(&scene.camera);
        assert!((d - scene.sky.sun_dir()).length() < 1e-9);
        assert!(l.intensity > 0.5);
        // The ambient light is the sky's: bluish by day.
        let lt = &scene.lighting;
        assert!(lt.ambient > 0.05 && lt.sky_color[2] > lt.sky_color[0], "{} {:?}", lt.ambient, lt.sky_color);
        // Without lights the sky adds light 1.
        let mut bare = scene.clone();
        bare.lighting.lights.clear();
        assert!(sync_sun(&mut bare) && bare.lighting.lights.len() == 1 && bare.lighting.lights[0].intensity > 0.5);
        // Low sun: redder and weaker.
        scene.sky.hour = 6.3;
        sync_sun(&mut scene);
        let low = &scene.lighting.lights[0];
        assert!(low.color[2] < low.color[0] * 0.8 && low.intensity < l.intensity, "{:?} {}", low.color, low.intensity);
    }
}
