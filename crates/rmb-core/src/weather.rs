// SPDX-License-Identifier: LGPL-2.1-or-later

//! Weather (`Sky::weather`): rain, snow and hail falling through the scene, ground mist, wet
//! surfaces, snow cover, splash rings on the water and rain drops on the camera's glass.
//!
//! Precipitation is a grid in world space moving along the (wind-slanted) fall direction by
//! speed × time, one particle per cell by `hash3`, walked along each ray from its start to
//! its hit or the reach (Amanatides-Woo, like the underwater bubbles): rain as thin streaks
//! along the fall, snow as soft flakes, hail as small opaque pellets, at least about a pixel
//! wide with their light spread accordingly (far ones fade instead of flickering). Beyond the
//! reach the precipitation is a haze up to the cloud base. The mist is an exponential height
//! fog with its optical depth in closed form. march.wgsl `weather_*` mirrors this file.

use serde::{Deserialize, Serialize};

use crate::math::{V3, v3};

/// What falls.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Precipitation {
    #[default]
    None,
    Rain,
    Snow,
    Hail,
}

impl Precipitation {
    pub const ALL: [Precipitation; 4] = [Precipitation::None, Precipitation::Rain, Precipitation::Snow, Precipitation::Hail];

    pub fn label(self) -> &'static str {
        match self {
            Precipitation::None => "None",
            Precipitation::Rain => "Rain",
            Precipitation::Snow => "Snow",
            Precipitation::Hail => "Hail",
        }
    }

    /// Number in the GPU uniform.
    pub fn index(self) -> u32 {
        self as u32
    }

    /// Typical particle radius, fall speed and color of the kind.
    pub fn defaults(self) -> (f64, f64, [f32; 3]) {
        match self {
            Precipitation::None | Precipitation::Rain => (0.004, 6.0, [0.75, 0.8, 0.85]),
            Precipitation::Snow => (0.012, 0.6, [0.96, 0.97, 1.0]),
            Precipitation::Hail => (0.008, 9.0, [0.9, 0.93, 0.96]),
        }
    }
}

/// The weather's settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Weather {
    pub precipitation: Precipitation,
    /// How much falls (0..1).
    pub intensity: f64,
    /// Particle radius (world units) and fall speed (world units per second).
    pub size: f64,
    pub speed: f64,
    /// Wind: direction it blows toward (degrees from +x) and how much it slants the fall.
    pub wind_direction: f64,
    pub wind: f64,
    pub color: [f32; 3],
    /// How far particles are drawn one by one (world units); beyond, a haze.
    pub reach: f64,
    /// Wet surfaces: darker and glossier where they face the sky (0..1).
    pub wetness: f64,
    /// Snow cover on faces pointing up (0..1), its color and how steep it still holds (0
    /// only flat tops .. 1 also steep sides).
    pub snow_cover: f64,
    pub snow_color: [f32; 3],
    pub snow_slope: f64,
    /// Rain rings on the water and splashes on wet ground (0..1).
    pub splashes: f64,
    /// Rain drops on the camera's glass (0..1).
    pub lens_drops: f64,
    /// Ground mist: density at its base, the base height, how fast it thins upward (height
    /// over which it falls to a third) and its color.
    pub mist: f64,
    pub mist_base: f64,
    pub mist_height: f64,
    pub mist_color: [f32; 3],
    /// The weather's clock (seconds; advanced in animations).
    pub time: f64,
    /// World position of the scene's origin (deep zoom shift; not saved).
    #[serde(skip)]
    pub origin: [f64; 3],
}

impl Default for Weather {
    fn default() -> Self {
        let (size, speed, color) = Precipitation::Rain.defaults();
        Weather {
            precipitation: Precipitation::None,
            intensity: 0.5,
            size,
            speed,
            wind_direction: 30.0,
            wind: 0.2,
            color,
            reach: 10.0,
            wetness: 0.0,
            snow_cover: 0.0,
            snow_color: [0.95, 0.97, 1.0],
            snow_slope: 0.4,
            splashes: 0.7,
            lens_drops: 0.0,
            mist: 0.0,
            mist_base: -1.0,
            mist_height: 1.0,
            mist_color: [0.85, 0.87, 0.9],
            time: 0.0,
            origin: [0.0; 3],
        }
    }
}

/// Presets: name, precipitation, intensity, wind, wetness, snow cover, splashes, lens drops,
/// mist.
type WeatherPreset = (&'static str, Precipitation, f64, f64, f64, f64, f64, f64, f64);

const PRESETS: &[WeatherPreset] = &[
    ("Clear", Precipitation::None, 0.5, 0.2, 0.0, 0.0, 0.0, 0.0, 0.0),
    ("Drizzle", Precipitation::Rain, 0.3, 0.1, 0.6, 0.0, 0.4, 0.3, 0.05),
    ("Downpour", Precipitation::Rain, 1.0, 0.35, 1.0, 0.0, 1.0, 0.8, 0.1),
    ("Snowfall", Precipitation::Snow, 0.5, 0.15, 0.0, 0.7, 0.0, 0.0, 0.03),
    ("Blizzard", Precipitation::Snow, 1.0, 1.2, 0.0, 1.0, 0.0, 0.0, 0.15),
    ("Hailstorm", Precipitation::Hail, 0.7, 0.3, 0.7, 0.15, 0.8, 0.4, 0.0),
    ("Morning mist", Precipitation::None, 0.5, 0.0, 0.3, 0.0, 0.0, 0.0, 0.35),
];

impl Weather {
    pub fn preset_names() -> Vec<&'static str> {
        PRESETS.iter().map(|p| p.0).collect()
    }

    /// Sets a preset's precipitation (with its kind's size, speed and color), amounts and mist.
    pub fn apply_preset(&mut self, name: &str) -> bool {
        let Some(p) = PRESETS.iter().find(|p| p.0 == name) else { return false };
        self.set_kind(p.1);
        (self.intensity, self.wind, self.wetness, self.snow_cover, self.splashes, self.lens_drops, self.mist) = (p.2, p.3, p.4, p.5, p.6, p.7, p.8);
        // Mist hugs the ground when it is the main thing; with precipitation it is a light haze.
        self.mist_height = if p.1 == Precipitation::None { 0.5 } else { 1.5 };
        true
    }

    /// Changes what falls, with that kind's size, speed and color.
    pub fn set_kind(&mut self, kind: Precipitation) {
        if kind != self.precipitation && kind != Precipitation::None {
            (self.size, self.speed, self.color) = kind.defaults();
        }
        self.precipitation = kind;
    }

    /// Whether anything of the weather shows (GPU switch `WEATHER`).
    pub fn active(&self) -> bool {
        (self.precipitation != Precipitation::None && self.intensity > 0.0) || self.mist > 0.0 || self.snow_cover > 0.0 || self.wetness > 0.0
    }
}

/// Grid cells per particle radius, opacity of the kinds (rain is mostly see-through).
fn kind_cell(k: Precipitation) -> f64 {
    match k {
        Precipitation::Snow => 20.0,
        Precipitation::Hail => 25.0,
        _ => 35.0,
    }
}

/// Share of grid cells with a particle at intensity 1 (rain is denser in number than it is
/// visible: a third of the cells already makes a downpour).
fn kind_share(k: Precipitation) -> f64 {
    match k {
        Precipitation::Rain => 0.35,
        Precipitation::Snow => 0.6,
        _ => 0.5,
    }
}

fn kind_opacity(k: Precipitation) -> f64 {
    match k {
        Precipitation::Rain => 0.25,
        Precipitation::Snow => 0.9,
        _ => 1.0,
    }
}

/// Haze extinction per world unit at intensity 1 (beyond the reach).
const HAZE: f64 = 0.04;
/// Ripple rings: cell size per particle radius... in world units per unit of the rain's size.
const RIPPLE_CELL: f64 = 40.0;
/// Most cells a ray walks through.
pub const MAX_CELLS: usize = 256;

/// The weather prepared for a frame (CPU; the GPU gets the same values, renderer.rs
/// `weather_gpu`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WeatherFrame {
    pub kind: Precipitation,
    /// Share of cells with a particle, particle radius, cell size.
    pub share: f64,
    pub radius: f64,
    pub cell: f64,
    /// Fall direction (unit) and streak length (rain).
    pub fall: V3,
    pub streak: f64,
    /// The grid's offset (fallen way) and the reach.
    pub offset: V3,
    pub reach: f64,
    /// Particle light (color × the light level) and opacity.
    pub light: [f64; 3],
    pub opacity: f64,
    /// Haze beyond the reach: extinction, top height, color × light.
    pub haze: f64,
    pub haze_top: f64,
    pub haze_light: [f64; 3],
    /// Mist: density at the base, base height, falloff height, color × light.
    pub mist: f64,
    pub mist_base: f64,
    pub mist_height: f64,
    pub mist_light: [f64; 3],
    pub wetness: f64,
    pub snow_cover: f64,
    pub snow_color: [f64; 3],
    pub snow_slope: f64,
    pub splashes: f64,
    /// Rain on the camera's glass (share of the glass, `render::cpu::lens_drop`).
    pub lens_drops: f64,
    pub ripple_cell: f64,
    pub time: f64,
    /// Added to positions to get world positions (deep zoom).
    pub origin: V3,
}

/// The light level the weather is lit by: the ambient light from above (the sky's
/// environment or the ambient colors) and a quarter of the sun (light 1, when the sky drives
/// it). Shared by the CPU and the GPU uniforms.
pub fn light_level(scene: &crate::Scene, lights: &[crate::render::FrameLight]) -> [f64; 3] {
    let l = &scene.lighting;
    let mut out = l.sky_color.map(|c| c as f64 * l.ambient);
    if let Some(env) = crate::sky_env::for_scene(scene) {
        let e = crate::sky_env::irradiance(&env, v3(0.0, 0.0, 1.0));
        for (o, e) in out.iter_mut().zip(e) {
            *o += e * l.ambient;
        }
    }
    if scene.sky.sun_light
        && let Some(sun) = lights.first().filter(|s| s.point.is_none())
    {
        for (o, c) in out.iter_mut().zip(sun.color) {
            *o += c * 0.25;
        }
    }
    out
}

/// How far the rain rings tilt the water's normal.
pub const RIPPLE_TILT: f64 = 0.5;

impl WeatherFrame {
    /// `None` without weather or outside the sky background. `light` = the light level the
    /// weather is lit by (ambient from above + a share of the sun; `render::cpu`).
    pub fn for_scene(scene: &crate::Scene, light: [f64; 3]) -> Option<WeatherFrame> {
        let w = &scene.sky.weather;
        if scene.background.mode != crate::scene::BackgroundMode::Sky || !w.active() {
            return None;
        }
        let kind = w.precipitation;
        let falling = kind != Precipitation::None && w.intensity > 0.0;
        let radius = w.size.max(1e-5);
        let cell = radius * kind_cell(kind);
        let a = w.wind_direction.to_radians();
        let fall = v3(a.cos() * w.wind, a.sin() * w.wind, -1.0).normalize();
        let c = w.color.map(|v| v as f64);
        let haze_top = if scene.sky.clouds.enabled { scene.sky.clouds.base } else { 20.0 };
        let mist_c = w.mist_color.map(|v| v as f64);
        Some(WeatherFrame {
            kind: if falling { kind } else { Precipitation::None },
            share: if falling { w.intensity.clamp(0.0, 1.0) * kind_share(kind) } else { 0.0 },
            radius,
            cell,
            fall,
            // A rain drop moves this far while the shutter is open (1/50 s), but stays in its cell.
            streak: if kind == Precipitation::Rain { (w.speed * 0.02).min(cell * 0.6) } else { 0.0 },
            offset: fall * (w.speed * w.time),
            reach: w.reach.max(0.0),
            light: [0, 1, 2].map(|k| c[k] * light[k]),
            opacity: kind_opacity(kind),
            haze: if falling { HAZE * w.intensity.clamp(0.0, 1.0) * if kind == Precipitation::Snow { 2.0 } else { 1.0 } } else { 0.0 },
            haze_top,
            haze_light: [0, 1, 2].map(|k| c[k] * light[k] * 0.8),
            mist: w.mist.max(0.0),
            mist_base: w.mist_base,
            mist_height: w.mist_height.max(1e-3),
            mist_light: [0, 1, 2].map(|k| mist_c[k] * light[k]),
            wetness: w.wetness.clamp(0.0, 1.0),
            snow_cover: w.snow_cover.clamp(0.0, 1.0),
            snow_color: w.snow_color.map(|v| v as f64),
            snow_slope: w.snow_slope.clamp(0.0, 1.0),
            splashes: if kind == Precipitation::Rain || kind == Precipitation::Hail { w.splashes.clamp(0.0, 1.0) * w.intensity.clamp(0.0, 1.0) } else { 0.0 },
            lens_drops: if kind == Precipitation::Rain || kind == Precipitation::Hail { w.lens_drops.clamp(0.0, 1.0) * w.intensity.clamp(0.0, 1.0) } else { 0.0 },
            ripple_cell: radius * RIPPLE_CELL,
            time: w.time,
            origin: V3::from_array(w.origin),
        })
    }
}

/// The particle of grid cell `c`, if any: its center (grid coordinates, before the fall
/// offset).
fn particle(f: &WeatherFrame, c: [i32; 3]) -> Option<V3> {
    let h = |k: i32| crate::render::hash3(c[0].wrapping_add(k.wrapping_mul(7919)), c[1], c[2]) as f64;
    if h(0) >= f.share {
        return None;
    }
    // Well inside its cell (with its streak), so walking the cells finds it.
    Some(v3(c[0] as f64 + 0.2 + 0.6 * h(1), c[1] as f64 + 0.2 + 0.6 * h(2), c[2] as f64 + 0.2 + 0.6 * h(3)) * f.cell)
}

/// Closest approach of the ray part `o + d s` (0..`t_end`) to the segment a..b: (distance, s).
fn ray_segment(o: V3, d: V3, t_end: f64, a: V3, b: V3) -> (f64, f64) {
    let e = b - a;
    let w = o - a;
    let (dd, de, ee) = (d.dot(d), d.dot(e), e.dot(e));
    let (dw, ew) = (d.dot(w), e.dot(w));
    let den = dd * ee - de * de;
    let mut s = if den > 1e-12 { ((de * ew - ee * dw) / den).clamp(0.0, t_end) } else { (-dw / dd.max(1e-12)).clamp(0.0, t_end) };
    let u = if ee > 1e-12 { ((de * s + ew) / ee).clamp(0.0, 1.0) } else { 0.0 };
    s = ((de * u - dw) / dd.max(1e-12)).clamp(0.0, t_end);
    let u = if ee > 1e-12 { ((de * s + ew) / ee).clamp(0.0, 1.0) } else { 0.0 };
    ((o + d * s - (a + e * u)).length(), s)
}

/// The particles along the ray from `o` (scene coordinates) in direction `d` up to `t_end`:
/// (share that passes, light added), front to back. `pixel` = pixel angle.
pub fn particles(f: &WeatherFrame, o: V3, d: V3, t_end: f64, pixel: f64) -> (f64, [f64; 3]) {
    let (mut keep, mut add) = (1.0, [0.0; 3]);
    let t_max = t_end.min(f.reach);
    if f.share <= 0.0 || t_max <= 0.0 {
        return (keep, add);
    }
    // Grid coordinates: world minus the fallen way.
    let g = o + f.origin - f.offset;
    let inv = 1.0 / f.cell;
    let mut c = [(g.x * inv).floor() as i32, (g.y * inv).floor() as i32, (g.z * inv).floor() as i32];
    let step = [d.x.signum() as i32, d.y.signum() as i32, d.z.signum() as i32];
    let next = |gc: f64, dc: f64, ci: i32| -> f64 {
        if dc.abs() < 1e-12 {
            return f64::MAX;
        }
        let edge = (ci as f64 + if dc > 0.0 { 1.0 } else { 0.0 }) * f.cell;
        (edge - gc) / dc
    };
    let mut tn = [next(g.x, d.x, c[0]), next(g.y, d.y, c[1]), next(g.z, d.z, c[2])];
    let dt = [f.cell / d.x.abs().max(1e-12), f.cell / d.y.abs().max(1e-12), f.cell / d.z.abs().max(1e-12)];
    let mut t = 0.0;
    for _ in 0..MAX_CELLS {
        if t > t_max || keep < 0.01 {
            break;
        }
        if let Some(p) = particle(f, c) {
            let half = f.fall * (f.streak * 0.5);
            let (dist, s) = ray_segment(g, d, t_max, p - half, p + half);
            let w = f.radius.max(pixel * s * 0.7);
            let cover = if f.kind == Precipitation::Rain { f.radius / w } else { (f.radius / w) * (f.radius / w) };
            // Right in front of the lens a particle would be a big blob: it fades in from 20
            // to 60 radii (a camera blurs it away).
            let near = smoothstep(f.radius * 20.0, f.radius * 60.0, s);
            let a = (cover * (-(dist / w) * (dist / w)).exp() * f.opacity * near).min(1.0);
            if a > 1e-4 {
                for (ak, l) in add.iter_mut().zip(f.light) {
                    *ak += keep * a * l;
                }
                keep *= 1.0 - a;
            }
        }
        // Into the next cell.
        let k = if tn[0] < tn[1] { if tn[0] < tn[2] { 0 } else { 2 } } else if tn[1] < tn[2] { 1 } else { 2 };
        t = tn[k];
        c[k] += step[k];
        tn[k] += dt[k];
    }
    (keep, add)
}

/// The precipitation's haze beyond the reach, up to the cloud base: (keep, add).
pub fn haze(f: &WeatherFrame, o: V3, d: V3, t_end: f64) -> (f64, [f64; 3]) {
    if f.haze <= 0.0 || t_end <= f.reach {
        return (1.0, [0.0; 3]);
    }
    let oz = o.z + f.origin.z;
    // The stretch below the top height, from the reach on.
    let mut t1 = t_end;
    if d.z > 1e-9 {
        t1 = t1.min(((f.haze_top - oz) / d.z).max(0.0));
    } else if oz > f.haze_top {
        t1 = if d.z < -1e-9 { t1.min(f64::MAX) } else { 0.0 };
    }
    let t0 = if d.z < -1e-9 && oz > f.haze_top { f.reach.max((f.haze_top - oz) / d.z) } else { f.reach };
    let len = (t1 - t0).max(0.0);
    let keep = if len >= 1e30 { 0.0 } else { (-f.haze * len).exp() };
    (keep, f.haze_light.map(|l| l * (1.0 - keep)))
}

/// Optical depth of the mist along the ray part 0..`t_end` (`f64::MAX`: to infinity).
pub fn mist_depth(f: &WeatherFrame, o: V3, d: V3, t_end: f64) -> f64 {
    if f.mist <= 0.0 {
        return 0.0;
    }
    let h = f.mist_height;
    let e0 = (-(o.z + f.origin.z - f.mist_base) / h).min(60.0);
    let k = d.z / h;
    if k.abs() < 1e-9 {
        return if t_end >= 1e30 { f64::MAX } else { f.mist * e0.exp() * t_end };
    }
    if t_end >= 1e30 {
        return if k > 0.0 { f.mist * e0.exp() / k } else { f64::MAX };
    }
    // ∫ ρ0 exp(e0 - k t) dt = ρ0 e^e0 (1 - e^(-k t_end)) / k, the exponents kept bounded.
    let tail = (-k * t_end).min(60.0);
    f.mist * e0.exp() * (1.0 - tail.exp()) / k
}

/// The mist along the ray: (keep, add).
pub fn mist(f: &WeatherFrame, o: V3, d: V3, t_end: f64) -> (f64, [f64; 3]) {
    let tau = mist_depth(f, o, d, t_end);
    if tau <= 0.0 {
        return (1.0, [0.0; 3]);
    }
    let keep = if tau >= 1e30 { 0.0 } else { (-tau).exp() };
    (keep, f.mist_light.map(|l| l * (1.0 - keep)))
}

fn smoothstep(a: f64, b: f64, x: f64) -> f64 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Snow cover on a surface at the world point `p` with the normal `n`: share 0..1 (faces
/// pointing up, in patches).
pub fn snow(f: &WeatherFrame, p: V3, n: V3) -> f64 {
    if f.snow_cover <= 0.0 {
        return 0.0;
    }
    // Holds on faces tilted less than the slope allows, fully on flatter ones.
    let lo = 0.75 - 0.75 * f.snow_slope;
    let tilt = smoothstep(lo, lo + 0.3, n.z);
    if tilt <= 0.0 {
        return 0.0;
    }
    // Patches: thinner cover leaves bare spots where the noise is low.
    let q = p + f.origin;
    let s = 1.0 / (f.radius * 400.0).max(1e-6);
    let patch = crate::render::value_noise([q.x * s, q.y * s, q.z * s]);
    let cover = smoothstep(0.0, 0.25, f.snow_cover - 0.6 * (1.0 - patch));
    (tilt * cover).clamp(0.0, 1.0)
}

/// Wetness of a surface with the normal `n` (faces open to the sky; 0..1).
pub fn wet(f: &WeatherFrame, n: V3) -> f64 {
    f.wetness * smoothstep(-0.3, 0.5, n.z)
}

/// Rain rings on a horizontal surface at the world point `p`: the slope (x, y) the rings
/// bend the normal by, and a splash glint (0..1) where a drop just hit.
pub fn ripples(f: &WeatherFrame, p: V3) -> (f64, f64, f64) {
    if f.splashes <= 0.0 {
        return (0.0, 0.0, 0.0);
    }
    let q = p + f.origin;
    let rc = f.ripple_cell;
    let (cx, cy) = ((q.x / rc).floor() as i32, (q.y / rc).floor() as i32);
    let (mut gx, mut gy, mut glint) = (0.0, 0.0, 0.0);
    const LIFE: f64 = 0.8;
    for dy in -1..=1 {
        for dx in -1..=1 {
            let (ix, iy) = (cx + dx, cy + dy);
            let h = |k: i32| crate::render::hash3(ix.wrapping_add(k.wrapping_mul(7919)), iy, 0x0a1b) as f64;
            if h(0) >= f.splashes {
                continue;
            }
            // Each cell is hit again and again: age within its own cycle.
            let cycle = f.time / LIFE + h(1);
            let k = cycle.floor() as i32;
            let age = (cycle - cycle.floor()) * LIFE;
            let hk = |j: i32| crate::render::hash3(ix.wrapping_add(j.wrapping_mul(104_729)), iy, k) as f64;
            let center = v3((ix as f64 + 0.2 + 0.6 * hk(1)) * rc, (iy as f64 + 0.2 + 0.6 * hk(2)) * rc, 0.0);
            let (ex, ey) = (q.x - center.x, q.y - center.y);
            let r = (ex * ex + ey * ey).sqrt();
            let ring = age / LIFE * rc * 0.9;
            let width = rc * 0.06;
            let fade = (1.0 - age / LIFE) * (1.0 - age / LIFE);
            let x = (r - ring) / width;
            // A ripple: one crest and trough around the ring radius.
            let slope = -2.0 * x * (-x * x).exp() * fade * 0.6;
            if r > 1e-9 {
                gx += slope * ex / r;
                gy += slope * ey / r;
            }
            if age < 0.06 && r < rc * 0.08 {
                glint += (1.0 - age / 0.06) * (1.0 - r / (rc * 0.08));
            }
        }
    }
    (gx, gy, glint.min(1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(w: Weather) -> WeatherFrame {
        let mut scene = crate::Scene::default();
        scene.background.mode = crate::scene::BackgroundMode::Sky;
        scene.sky.weather = w;
        WeatherFrame::for_scene(&scene, [1.0; 3]).expect("weather")
    }

    fn rain() -> Weather {
        let mut w = Weather::default();
        w.apply_preset("Downpour");
        w
    }

    #[test]
    fn the_walk_finds_what_a_scan_finds() {
        let mut f = frame(rain());
        f.streak = 0.0;
        f.opacity = 1.0;
        f.share = 1.0;
        let o = v3(0.13, -0.27, 0.4);
        let d = v3(0.6, 0.7, -0.2).normalize();
        let pixel = 1e-6;
        // Brute force: every cell near the ray.
        let mut brute = 1.0;
        let mut hits = Vec::new();
        let n = (f.reach / f.cell) as i32 + 2;
        let g = o - f.offset;
        let c0 = [(g.x / f.cell).floor() as i32, (g.y / f.cell).floor() as i32, (g.z / f.cell).floor() as i32];
        for x in -n..=n {
            for y in -n..=n {
                for z in -n..=n {
                    if let Some(p) = particle(&f, [c0[0] + x, c0[1] + y, c0[2] + z]) {
                        let (dist, s) = ray_segment(g, d, f.reach, p, p);
                        if dist < f.radius * 2.0 && s > 0.0 && s < f.reach {
                            hits.push(s);
                            let a = (-(dist / f.radius).powi(2)).exp().min(1.0);
                            brute *= 1.0 - a;
                        }
                    }
                }
            }
        }
        let (keep, _) = particles(&f, o, d, f64::MAX, pixel);
        assert!(!hits.is_empty(), "the test ray meets particles");
        assert!((keep - brute).abs() < 0.02, "walk {keep} vs scan {brute} ({} hits)", hits.len());
    }

    #[test]
    fn particles_fall_with_time_and_the_wind_and_follow_the_intensity() {
        let mut w = rain();
        let a = frame(w.clone());
        w.time = 2.0;
        let b = frame(w.clone());
        assert!((b.offset - a.offset).length() > 1.0);
        assert!(b.offset.z < 0.0 && b.offset.x.abs() + b.offset.y.abs() > 0.0, "slanted by the wind: {:?}", b.offset);
        let count = |share: f64| {
            let mut f = frame(rain());
            f.share = share;
            (0..4000).filter(|i| particle(&f, [i % 20, (i / 20) % 20, i / 400]).is_some()).count()
        };
        assert!(count(1.0) > count(0.5) && count(0.5) > count(0.1));
        assert!((count(0.5) as f64 / 4000.0 - 0.5).abs() < 0.05);
    }

    #[test]
    fn the_mist_depth_matches_numeric_integration() {
        let mut w = Weather { mist: 0.8, mist_base: -1.0, mist_height: 0.7, ..Weather::default() };
        w.precipitation = Precipitation::None;
        let f = frame(w);
        let o = v3(0.0, 0.0, 0.2);
        for d in [v3(1.0, 0.0, 0.3), v3(1.0, 0.0, -0.2), v3(1.0, 0.0, 0.0)] {
            let d = d.normalize();
            let len = 6.0;
            let mut sum = 0.0;
            let n = 20000;
            for i in 0..n {
                let t = (i as f64 + 0.5) / n as f64 * len;
                let z = o.z + d.z * t;
                sum += 0.8 * (-(z + 1.0) / 0.7).exp() * len / n as f64;
            }
            let a = mist_depth(&f, o, d, len);
            assert!((a - sum).abs() < 1e-3 * sum.max(1.0), "{d:?}: {a} vs {sum}");
        }
        // Up to infinity: finite going up, everything going down or level.
        assert!(mist_depth(&f, o, v3(0.0, 0.6, 0.8), f64::MAX) < 10.0);
        assert_eq!(mist(&f, o, v3(1.0, 0.0, -0.1).normalize(), f64::MAX).0, 0.0);
    }

    #[test]
    fn snow_lies_on_what_faces_up_and_rings_spread_and_fade() {
        let mut w = Weather::default();
        w.apply_preset("Snowfall");
        let f = frame(w);
        let cover_up = (0..200).map(|i| snow(&f, v3(i as f64 * 0.137, 0.3, 0.0), v3(0.0, 0.0, 1.0))).sum::<f64>();
        let cover_side = (0..200).map(|i| snow(&f, v3(i as f64 * 0.137, 0.3, 0.0), v3(1.0, 0.0, 0.0))).sum::<f64>();
        let cover_down = (0..200).map(|i| snow(&f, v3(i as f64 * 0.137, 0.3, 0.0), v3(0.0, 0.0, -1.0))).sum::<f64>();
        assert!(cover_up > 50.0 && cover_side < cover_up * 0.2 && cover_down == 0.0, "{cover_up} {cover_side} {cover_down}");
        // Ripples: something moves on the water in the rain, nothing without splashes.
        let mut r = frame(rain());
        let field = |f: &WeatherFrame| (0..400).map(|i| ripples(f, v3((i % 20) as f64 * 0.013, (i / 20) as f64 * 0.013, 0.0)).0.abs()).sum::<f64>();
        assert!(field(&r) > 0.1);
        let early = field(&r);
        r.time += 0.37;
        assert_ne!(early, field(&r));
        r.splashes = 0.0;
        assert_eq!(field(&r), 0.0);
    }
}
