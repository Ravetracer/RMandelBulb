// SPDX-License-Identifier: LGPL-2.1-or-later

//! Thunderstorms (`Sky::storm`): lightning bolts from the cloud base down to the ground or
//! the sea, and flashes inside the clouds.
//!
//! Strikes come by the storm's clock (`Storm::time`): time slots of 60 / rate seconds, one
//! strike per slot at a hashed offset, place, kind and shape, so any frame time gives the same
//! strikes (animations, tiles and network renders agree). A strike flickers with 2–4 return
//! strokes ([`envelope`]). A bolt is a jagged main channel by recursive midpoint
//! displacement plus thinner, dimmer branches that fade first; its segments are grouped in
//! chunks of 16 with bounding spheres. While a strike lasts it adds a line light (bolt) or a
//! point light (cloud flash) to the frame's lights (`render::Frame::new`), lights the clouds
//! from inside (`clouds::march`) and glows along the rays that pass it ([`glow`]).
//! march.wgsl `bolt_*` mirrors this file.

use serde::{Deserialize, Serialize};

use crate::math::{V3, v3};

/// How strikes are placed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum StrikeMode {
    /// By the storm's clock: random times, places and shapes.
    #[default]
    Random,
    /// One bolt where the settings say (stills).
    Fixed,
}

impl StrikeMode {
    pub fn label(self) -> &'static str {
        match self {
            StrikeMode::Random => "Random",
            StrikeMode::Fixed => "Fixed",
        }
    }
}

/// The storm's settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Storm {
    pub enabled: bool,
    pub mode: StrikeMode,
    /// Random: strikes per minute, the seed of their sequence and the distance range around
    /// the scene's center (world units).
    pub rate: f64,
    pub seed: u32,
    pub distance_min: f64,
    pub distance_max: f64,
    /// Fixed: direction (degrees from +x), distance, shape variant and the moment of the
    /// flash (0 = the first, brightest stroke .. 1 = its end).
    pub direction: f64,
    pub distance: f64,
    pub variant: u32,
    pub stage: f64,
    /// Share of strikes that only light the clouds from inside (no bolt).
    pub cloud_flashes: f64,
    pub brightness: f64,
    pub color: [f32; 3],
    /// How much the bolts branch (0..1) and their width (world units).
    pub branches: f64,
    pub width: f64,
    /// Where bolts start without clouds (with clouds: the cloud base) and where they end
    /// (the ground or the sea level).
    pub top: f64,
    pub ground: f64,
    /// Strength of the light a strike throws on the scene.
    pub scene_light: f64,
    /// The storm's clock (seconds; advanced in animations).
    pub time: f64,
    /// World position of the scene's origin (deep zoom shift, set by
    /// `render::shifted_scene`; not saved).
    #[serde(skip)]
    pub origin: [f64; 3],
}

impl Default for Storm {
    fn default() -> Self {
        Storm {
            enabled: false,
            mode: StrikeMode::Random,
            rate: 8.0,
            seed: 1,
            distance_min: 8.0,
            distance_max: 40.0,
            direction: 40.0,
            distance: 18.0,
            variant: 1,
            stage: 0.0,
            cloud_flashes: 0.4,
            brightness: 1.0,
            color: [0.78, 0.84, 1.0],
            branches: 0.6,
            width: 0.03,
            top: 6.0,
            ground: -1.0,
            scene_light: 1.0,
            time: 0.0,
            origin: [0.0; 3],
        }
    }
}

/// How long a strike lasts (seconds).
pub const DURATION: f64 = 0.6;
/// At most this many strikes at once, segments per bolt, segments per chunk.
pub const MAX_STRIKES: usize = 2;
pub const MAX_SEGMENTS: usize = 256;
pub const CHUNK: usize = 16;
/// Radiance of a bolt's core and its halo (relative), and the halo's width in bolt widths.
const CORE: f64 = 60.0;
const HALO: f64 = 0.06;
const HALO_WIDTHS: f64 = 25.0;
/// Light thrown on the scene (line light) and into the clouds per unit of brightness.
const SCENE_K: f64 = 600.0;
const CLOUD_K: f64 = 6.0;
/// The flashing cloud deck's diffuse light on the scene (ambient fill) per unit of the
/// light inside the clouds.
const FILL_K: f64 = 0.015;

fn hash(seed: i32, a: i32, b: i32) -> f64 {
    crate::render::hash3(seed.wrapping_mul(7919).wrapping_add(a), b, 0x5eed) as f64
}

/// One strike at a frame time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Strike {
    /// Shape seed, ground point (x, y), seconds since it began, a bolt (else a cloud flash).
    pub seed: i32,
    pub x: f64,
    pub y: f64,
    pub age: f64,
    pub bolt: bool,
}

/// The strikes active at the storm's time (at most [`MAX_STRIKES`]).
pub fn strikes(s: &Storm) -> Vec<Strike> {
    match s.mode {
        StrikeMode::Fixed => {
            let a = s.direction.to_radians();
            vec![Strike { seed: s.variant as i32, x: a.cos() * s.distance, y: a.sin() * s.distance, age: s.stage.clamp(0.0, 1.0) * DURATION, bolt: true }]
        }
        StrikeMode::Random => {
            if s.rate <= 0.0 {
                return Vec::new();
            }
            let slot = 60.0 / s.rate;
            let seed = s.seed as i32;
            let first = ((s.time - DURATION) / slot).floor() as i64;
            let last = (s.time / slot).floor() as i64;
            let mut out = Vec::new();
            for k in first..=last {
                let k32 = k as i32;
                let start = k as f64 * slot + hash(seed, k32, 0) * slot;
                let age = s.time - start;
                if !(0.0..DURATION).contains(&age) {
                    continue;
                }
                let ang = hash(seed, k32, 1) * std::f64::consts::TAU;
                let dist = s.distance_min + (s.distance_max - s.distance_min).max(0.0) * hash(seed, k32, 2).sqrt();
                let bolt = hash(seed, k32, 3) >= s.cloud_flashes.clamp(0.0, 1.0);
                out.push(Strike { seed: (hash(seed, k32, 4) * 1e6) as i32, x: ang.cos() * dist, y: ang.sin() * dist, age, bolt });
                if out.len() == MAX_STRIKES {
                    break;
                }
            }
            out
        }
    }
}

/// Brightness of a strike `age` seconds after it began (2–4 return strokes, fast decay, a
/// weak continuing current): (main channel, branches).
pub fn envelope(seed: i32, age: f64) -> (f64, f64) {
    if !(0.0..DURATION).contains(&age) {
        return (0.0, 0.0);
    }
    let strokes = 2 + (hash(seed, 100, 0) * 3.0) as i32;
    let mut t = 0.0;
    let mut main = 0.0;
    let mut first = 0.0;
    for i in 0..strokes {
        let b = if i == 0 { 1.0 } else { 0.45 + 0.45 * hash(seed, 101, i) };
        if age >= t {
            let v = b * (-(age - t) / 0.03).exp();
            main += v;
            if i == 0 {
                first = v;
            }
        }
        t += 0.04 + 0.11 * hash(seed, 102, i);
    }
    main += 0.12 * (-age / 0.25).exp();
    // Fading out at the end, so a strike never stops abruptly.
    let tail = ((DURATION - age) / 0.1).clamp(0.0, 1.0);
    ((main * tail).min(1.5), first * tail)
}

/// A bolt segment: ends, radius, brightness weight and whether it is part of a branch.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Segment {
    pub a: V3,
    pub b: V3,
    pub r: f64,
    pub w: f64,
    pub branch: bool,
}

/// Midpoint displacement from `a` to `b` in `levels` levels (2^levels segments).
fn jagged(seed: i32, salt: i32, a: V3, b: V3, levels: u32, rough: f64) -> Vec<V3> {
    let mut pts = vec![a, b];
    for level in 0..levels {
        let mut next = Vec::with_capacity(pts.len() * 2);
        for (i, w) in pts.windows(2).enumerate() {
            let (p, q) = (w[0], w[1]);
            let len = (q - p).length();
            let h = |k: i32| hash(seed, salt.wrapping_mul(1000).wrapping_add(level as i32 * 97 + k), i as i32) * 2.0 - 1.0;
            // Mostly sideways: the channel zigzags, it does not bunch up along itself.
            let off = v3(h(0), h(1), h(2) * 0.3) * (len * rough);
            next.push(p);
            next.push((p + q) * 0.5 + off);
        }
        next.push(*pts.last().unwrap());
        pts = next;
    }
    pts
}

/// The bolt of a strike: from `top` (cloud base) down to the strike height at (x, y).
pub fn bolt(s: &Storm, strike: &Strike, top: f64) -> Vec<Segment> {
    let seed = strike.seed;
    let height = (top - s.ground).max(0.1);
    // The channel leans a little: it starts somewhat away from where it lands.
    let lean = v3(hash(seed, 1, 0) * 2.0 - 1.0, hash(seed, 1, 1) * 2.0 - 1.0, 0.0) * (height * 0.25);
    let a = v3(strike.x, strike.y, top) + lean;
    let b = v3(strike.x, strike.y, s.ground);
    let r = s.width.max(1e-4) * 0.5;
    let main = jagged(seed, 0, a, b, 6, 0.28);
    let mut segs: Vec<Segment> = main.windows(2).map(|w| Segment { a: w[0], b: w[1], r, w: 1.0, branch: false }).collect();
    let count = (s.branches.clamp(0.0, 1.0) * 10.0).round() as i32;
    for k in 0..count {
        if segs.len() + 16 > MAX_SEGMENTS {
            break;
        }
        // From the upper two thirds of the channel, downward and outward.
        let i = (5.0 + hash(seed, 2, k) * (main.len() as f64 * 0.6)) as usize;
        let start = main[i.min(main.len() - 2)];
        let below = start.z - s.ground;
        let len = below * (0.25 + 0.35 * hash(seed, 3, k));
        let ang = hash(seed, 4, k) * std::f64::consts::TAU;
        let dir = v3(ang.cos() * 0.8, ang.sin() * 0.8, -1.0).normalize();
        let end = start + dir * len;
        let pts = jagged(seed, 10 + k, start, end, 4, 0.3);
        let n = pts.len() - 1;
        for (j, w) in pts.windows(2).enumerate() {
            // Thinner and dimmer toward the tip.
            let f = 1.0 - j as f64 / n as f64;
            segs.push(Segment { a: w[0], b: w[1], r: r * (0.35 + 0.3 * f), w: 0.25 + 0.35 * f, branch: true });
        }
    }
    segs
}

/// A group of up to [`CHUNK`] segments with a sphere around them and their halo.
#[derive(Clone, Debug, PartialEq)]
pub struct Chunk {
    pub center: V3,
    pub radius: f64,
    /// Parts above this height glow behind the clouds (the cloud base at the strike).
    pub split: f64,
    /// Segments with the strike's brightness folded into `w`.
    pub segs: Vec<Segment>,
}

/// A light inside the clouds (bolt top or cloud flash): position and color × strength.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CloudLight {
    pub pos: V3,
    pub color: [f64; 3],
}

/// The storm prepared for a frame.
#[derive(Clone, Debug, PartialEq)]
pub struct StormFrame {
    pub chunks: Vec<Chunk>,
    pub color: [f64; 3],
    /// Bolt parts above this height glow behind the clouds (the cloud base; scene
    /// coordinates like everything here: world minus `Storm::origin`).
    pub split: f64,
    pub halo_width: f64,
    pub cloud_lights: Vec<CloudLight>,
    /// The brightest strike's light on the scene: line from `a` to `b` (bolt) or point `a`
    /// (`b` = `a`), color × strength.
    pub scene_light: Option<(V3, V3, [f64; 3])>,
}

impl StormFrame {
    /// `None` without lightning, outside the sky background or when no strike is active.
    pub fn for_scene(scene: &crate::Scene) -> Option<StormFrame> {
        let s = &scene.sky.storm;
        if scene.background.mode != crate::scene::BackgroundMode::Sky || !s.enabled {
            return None;
        }
        let c = &scene.sky.clouds;
        let (top, thick) = if c.enabled { (c.base, c.thickness.max(0.1)) } else { (s.top, 1.0) };
        // The cloud base curves down with the planet (clouds::height).
        let curve = if c.enabled && c.planet_radius > 0.0 { 0.5 / c.planet_radius } else { 0.0 };
        let origin = V3::from_array(s.origin);
        let color = s.color.map(|v| v as f64 * s.brightness.max(0.0));
        let mut f = StormFrame { chunks: Vec::new(), color, split: top - origin.z, halo_width: s.width.max(1e-4) * HALO_WIDTHS, cloud_lights: Vec::new(), scene_light: None };
        let mut best = 0.0;
        for st in strikes(s) {
            let (main, branch) = envelope(st.seed, st.age);
            if main <= 1e-4 {
                continue;
            }
            let glow = |k: f64| color.map(|v| v * main * k);
            let top = top - curve * (st.x * st.x + st.y * st.y);
            if st.bolt {
                let segs: Vec<Segment> = bolt(s, &st, top).iter().map(|g| Segment { a: g.a - origin, b: g.b - origin, ..*g }).collect();
                let lit: Vec<Segment> = segs.iter().map(|g| Segment { w: g.w * if g.branch { branch } else { main }, ..*g }).collect();
                for part in lit.chunks(CHUNK) {
                    let center = part.iter().fold(V3::ZERO, |acc, g| acc + g.a + g.b) * (0.5 / part.len() as f64);
                    let radius = part.iter().map(|g| (g.a - center).length().max((g.b - center).length())).fold(0.0, f64::max) + f.halo_width * 4.0;
                    f.chunks.push(Chunk { center, radius, split: top - origin.z, segs: part.to_vec() });
                }
                let (a, b) = (segs[0].a, v3(st.x, st.y, s.ground) - origin);
                f.cloud_lights.push(CloudLight { pos: a + v3(0.0, 0.0, thick * 0.3), color: glow(CLOUD_K) });
                if main > best {
                    best = main;
                    f.scene_light = Some((a, b, glow(SCENE_K * s.scene_light.max(0.0))));
                }
            } else {
                let p = v3(st.x, st.y, top + thick * 0.5) - origin;
                f.cloud_lights.push(CloudLight { pos: p, color: glow(CLOUD_K * 1.5) });
                if main > best {
                    best = main;
                    // Diffused by the clouds: weaker than a bolt.
                    f.scene_light = Some((p, p, glow(SCENE_K * 0.4 * s.scene_light.max(0.0))));
                }
            }
        }
        (!f.chunks.is_empty() || !f.cloud_lights.is_empty()).then_some(f)
    }
}

impl StormFrame {
    /// Diffuse light of the flashing clouds on the scene (added to the ambient colors by
    /// `sky::sync_sun`): the brightest light inside the clouds × [`FILL_K`].
    pub fn fill(&self) -> [f64; 3] {
        let best = self.cloud_lights.iter().map(|l| l.color).fold([0.0; 3], |a, c| if c[0] + c[1] + c[2] > a[0] + a[1] + a[2] { c } else { a });
        best.map(|c| c * FILL_K)
    }
}

/// Closest points of the ray part `o + d s` (s in 0..`t_end`) and the segment `a`..`b`:
/// (distance, s).
fn ray_segment(o: V3, d: V3, t_end: f64, a: V3, b: V3) -> (f64, f64) {
    let e = b - a;
    let w = o - a;
    let (dd, de, ee) = (d.dot(d), d.dot(e), e.dot(e));
    let (dw, ew) = (d.dot(w), e.dot(w));
    let den = dd * ee - de * de;
    let mut s = if den > 1e-12 { ((de * ew - ee * dw) / den).clamp(0.0, t_end) } else { 0.0 };
    let mut u = ((de * s + ew) / ee.max(1e-12)).clamp(0.0, 1.0);
    // Re-project once after clamping the segment parameter.
    s = ((e.dot(d) * u - dw) / dd.max(1e-12)).clamp(0.0, t_end);
    u = ((de * s + ew) / ee.max(1e-12)).clamp(0.0, 1.0);
    ((o + d * s - (a + e * u)).length(), s)
}

/// The bolts' light along the ray from `o` (scene coordinates) in direction `d` up to
/// `t_end`: (below the cloud base, above it). `pixel` = pixel angle (radians): the core is at
/// least about a pixel wide, its brightness spread accordingly.
pub fn glow(f: &StormFrame, o: V3, d: V3, t_end: f64, pixel: f64) -> ([f64; 3], [f64; 3]) {
    let (mut below, mut above) = (0.0f64, 0.0f64);
    for c in &f.chunks {
        let s = (c.center - o).dot(d).clamp(0.0, t_end);
        if (o + d * s - c.center).length() > c.radius {
            continue;
        }
        for g in &c.segs {
            let (dist, s) = ray_segment(o, d, t_end, g.a, g.b);
            let wc = g.r.max(pixel * s * 0.75);
            let core = g.r / wc * (-(dist / wc) * (dist / wc)).exp() * CORE;
            let halo = HALO / (1.0 + (dist / f.halo_width) * (dist / f.halo_width));
            let v = (core + halo) * g.w;
            if o.z + d.z * s > c.split {
                above = above.max(v);
            } else {
                below = below.max(v);
            }
        }
    }
    (f.color.map(|c| c * below), f.color.map(|c| c * above))
}

/// Light of the strikes inside the clouds at `p` (scene coordinates; `clouds::march`), with
/// `r0` the falloff radius (the layer's thickness) and `sigma` the clouds' extinction.
pub fn cloud_light(lights: &[CloudLight], p: V3, r0: f64, sigma: f64) -> [f64; 3] {
    let mut out = [0.0; 3];
    for l in lights {
        let dist = (p - l.pos).length();
        let k = 1.0 / (1.0 + (dist / r0) * (dist / r0)) * (-dist * sigma * 0.02).exp();
        for (o, c) in out.iter_mut().zip(l.color) {
            *o += c * k;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn storm() -> Storm {
        Storm { enabled: true, ..Storm::default() }
    }

    #[test]
    fn strikes_follow_the_clock_and_the_rate() {
        let mut s = storm();
        s.rate = 12.0;
        let mut started = std::collections::BTreeSet::new();
        let mut t = 0.0;
        while t < 600.0 {
            s.time = t;
            let now = strikes(&s);
            s.time = t;
            assert_eq!(now, strikes(&s), "deterministic");
            for st in now {
                started.insert(((t - st.age) * 1000.0).round() as i64);
            }
            t += 0.05;
        }
        // About 12 per minute over 10 minutes.
        assert!((100..=140).contains(&started.len()), "{} strikes", started.len());
    }

    #[test]
    fn a_strike_flickers_and_ends() {
        let (peak, _) = envelope(7, 0.0);
        assert!(peak > 0.9);
        let mut lows = 0;
        let mut prev = peak;
        let mut rises = 0;
        for i in 1..120 {
            let (v, _) = envelope(7, i as f64 * 0.005);
            if v > prev + 1e-3 {
                rises += 1;
            }
            if v < 0.3 {
                lows += 1;
            }
            prev = v;
        }
        assert!(rises >= 1 && lows > 5, "rises {rises} lows {lows}");
        assert_eq!(envelope(7, DURATION + 0.01), (0.0, 0.0));
        // Branches only follow the first stroke.
        assert!(envelope(7, 0.2).1 < envelope(7, 0.0).1 * 0.01);
    }

    #[test]
    fn bolts_run_from_the_clouds_to_the_ground() {
        let s = storm();
        let st = Strike { seed: 3, x: 10.0, y: -4.0, age: 0.0, bolt: true };
        let segs = bolt(&s, &st, 6.0);
        assert!(segs.len() <= MAX_SEGMENTS && segs.len() > 64);
        let main: Vec<_> = segs.iter().filter(|g| !g.branch).collect();
        assert!((main[0].a.z - 6.0).abs() < 1.0);
        let end = main.last().unwrap().b;
        assert!((end - v3(10.0, -4.0, s.ground)).length() < 1e-9);
        for w in main.windows(2) {
            assert!((w[0].b - w[1].a).length() < 1e-12, "connected");
        }
        assert!(segs.iter().filter(|g| g.branch).all(|g| g.r < s.width * 0.5 && g.w < 1.0));
    }

    #[test]
    fn a_fixed_bolt_glows_where_it_stands() {
        let mut scene = crate::Scene::default();
        scene.background.mode = crate::scene::BackgroundMode::Sky;
        scene.sky.storm = Storm { enabled: true, mode: StrikeMode::Fixed, direction: 0.0, distance: 20.0, ..Storm::default() };
        let f = StormFrame::for_scene(&scene).expect("a bolt");
        assert!(f.scene_light.is_some());
        let o = v3(0.0, 0.0, 0.0);
        // A point of the main channel halfway down.
        let toward = f.chunks[2].segs[0].a - o;
        let (b0, b1) = glow(&f, o, toward.normalize(), 1e9, 0.001);
        let b = [b0[0] + b1[0], b0[1] + b1[1], b0[2] + b1[2]];
        let (miss, miss_above) = glow(&f, o, v3(-1.0, 0.0, 0.0), 1e9, 0.001);
        assert!(b[2] > 1.0, "{b:?}");
        assert_eq!(miss, [0.0; 3]);
        assert_eq!(miss_above, [0.0; 3]);
        // Behind a surface nearer than the bolt: nothing.
        let (hidden, hidden_above) = glow(&f, o, toward.normalize(), 5.0, 0.001);
        assert!(hidden[2] + hidden_above[2] < (b[2] + glow(&f, o, toward.normalize(), 1e9, 0.001).1[2]) * 0.01);
    }
}
