//! Plants on the seabed grown by L-systems.
//!
//! A [`PlantSpecies`] holds an L-system (start string, rewriting rules with weights, number of
//! generations) and how a turtle draws its result: `F` draws a branch segment, `f` moves
//! without drawing, `+`/`-` turn left/right, `&`/`^` pitch down/up, `\`/`/` roll, `|` turns
//! around, `[`/`]` start and end a side branch (shorter and thinner by the level factors),
//! `!` thins the branch, `L` adds a leaf. The drawn plant ([`PlantTemplate`]) is normalized to
//! height 1 and kept with a bounding volume hierarchy over its segments; the seabed places
//! copies of it on a grid (turned, sized and swaying per copy, [`PlantField`]). The GPU mirrors
//! the distance and the placement in march.wgsl (`plant_de`), with the templates in the
//! `vols` buffer.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use serde::{Deserialize, Serialize};

use crate::math::{V3, v3};

/// One rewriting rule: `symbol` becomes `replacement`. Several rules for one symbol are picked
/// at random by their weights (stochastic L-system).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlantRule {
    pub symbol: char,
    pub replacement: String,
    pub weight: f64,
}

impl Default for PlantRule {
    fn default() -> Self {
        PlantRule { symbol: 'F', replacement: "F".into(), weight: 1.0 }
    }
}

/// A kind of plant: its L-system, how the turtle draws it, where and how large the seabed
/// grows it, and its colors.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlantSpecies {
    pub name: String,
    pub enabled: bool,
    /// Start string of the L-system.
    pub axiom: String,
    pub rules: Vec<PlantRule>,
    /// Generations of rewriting.
    pub iterations: u32,
    /// Turning angle of `+ - & ^ \ /` (degrees) and its random variation (degrees).
    pub angle: f64,
    pub angle_variation: f64,
    /// Length of side branches relative to their parent branch (per `[`).
    pub length_scale: f64,
    /// Radius of the stem relative to a segment's length.
    pub thickness: f64,
    /// Radius of side branches relative to their parent (per `[` and per `!`).
    pub thinning: f64,
    /// Leaves: length relative to a segment's length, width relative to their length.
    pub leaf_length: f64,
    pub leaf_width: f64,
    /// Bending of every segment toward up (positive, like kelp held up by floats) or down
    /// (negative, drooping).
    pub tropism: f64,
    /// Random variant of the growth (stochastic rules, angle variation).
    pub seed: u32,
    /// Share of the grid cells with a plant (0 = none, 1 = all).
    pub density: f64,
    /// How much the plants cluster in patches (0 = spread evenly, 1 = only in patches).
    pub clumping: f64,
    /// Distance between the plants (local units of the seabed; raised so neighbours fit).
    pub spacing: f64,
    /// Height of the tallest plants (local units) and how much smaller some are (0..1).
    pub height: f64,
    pub size_variation: f64,
    /// Swaying in the current (0 = still, 1 = strongly), timed by the seabed's sway.
    pub sway: f64,
    /// Plants on other surfaces ([`crate::plant_cover`]): direction of growth between the
    /// surface normal (0) and straight up (1, the host's own z axis), and the steepest surface
    /// they grow on (degrees from up; 180 = anywhere).
    pub upright: f64,
    pub max_slope: f64,
    /// Colors of the branches and of the leaves, at the root and at the top.
    pub branch_colors: [[f32; 3]; 2],
    pub leaf_colors: [[f32; 3]; 2],
}

impl Default for PlantSpecies {
    fn default() -> Self {
        PlantSpecies::preset("Kelp").unwrap_or_else(|| PlantSpecies {
            name: "Plant".into(),
            enabled: true,
            axiom: "F".into(),
            rules: Vec::new(),
            iterations: 3,
            angle: 25.0,
            angle_variation: 5.0,
            length_scale: 0.7,
            thickness: 0.06,
            thinning: 0.7,
            leaf_length: 1.0,
            leaf_width: 0.3,
            tropism: 0.0,
            seed: 1,
            density: 0.5,
            clumping: 0.5,
            spacing: 0.5,
            height: 0.5,
            size_variation: 0.4,
            sway: 0.5,
            upright: 0.6,
            max_slope: 180.0,
            branch_colors: [[0.2, 0.25, 0.1], [0.3, 0.4, 0.15]],
            leaf_colors: [[0.2, 0.35, 0.1], [0.45, 0.55, 0.2]],
        })
    }
}

/// The preset plants: name, start string, rules (symbol, replacement, weight), generations,
/// angle, angle variation, length scale, thickness, thinning, leaf length, leaf width,
/// tropism, height, spacing, density, clumping, sway, branch colors, leaf colors.
type PresetRow = (&'static str, &'static str, &'static [(char, &'static str, f64)], u32, [f64; 9], [f64; 5], [[f32; 3]; 4]);

const PRESETS: &[PresetRow] = &[
    (
        "Kelp",
        "FFFA",
        &[('A', "F[&&L]///F[^^L]///A", 0.6), ('A', "F[&&L]//F[&L]//A", 0.4)],
        7,
        [22.0, 8.0, 0.8, 0.03, 0.8, 3.0, 0.22, 0.1, 0.0],
        [0.9, 0.7, 0.45, 0.7, 0.8],
        [[0.25, 0.2, 0.06], [0.4, 0.33, 0.1], [0.3, 0.26, 0.06], [0.6, 0.5, 0.15]],
    ),
    (
        "Sea fan",
        "FA",
        &[('A', "F[+A][-A]", 0.6), ('A', "F[+A]A", 0.2), ('A', "F[-A]A", 0.2)],
        8,
        [22.0, 8.0, 0.88, 0.06, 0.75, 0.0, 0.3, 0.04, 0.0],
        [0.5, 0.9, 0.3, 0.4, 0.15],
        [[0.45, 0.08, 0.12], [0.85, 0.3, 0.35], [0.85, 0.3, 0.35], [0.95, 0.5, 0.5]],
    ),
    (
        "Staghorn coral",
        "[&A]///[&A]///[&A]///[A]",
        &[('A', "F[&+A]F[&-A]A", 0.5), ('A', "F[^/A]FA", 0.3), ('A', "FA", 0.2)],
        5,
        [30.0, 10.0, 0.72, 0.1, 0.85, 0.0, 0.3, 0.1, 0.0],
        [0.35, 0.6, 0.4, 0.6, 0.0],
        [[0.75, 0.62, 0.45], [0.95, 0.85, 0.7], [0.95, 0.85, 0.7], [1.0, 0.95, 0.85]],
    ),
    (
        "Algae tuft",
        "[A]//[A]//[A]//[A]//[A]",
        &[('A', "F&A", 0.5), ('A', "F^A", 0.3), ('A', "FL", 0.2)],
        6,
        [14.0, 10.0, 0.9, 0.035, 0.9, 1.4, 0.35, -0.04, 0.0],
        [0.25, 0.25, 0.5, 0.8, 1.0],
        [[0.12, 0.28, 0.08], [0.3, 0.5, 0.12], [0.2, 0.45, 0.1], [0.5, 0.7, 0.2]],
    ),
    (
        "Fern weed",
        "A",
        &[('A', "F[+L][-L]F[+L][-L]A", 0.7), ('A', "F[+A][-A]", 0.3)],
        5,
        [55.0, 8.0, 0.65, 0.025, 0.7, 1.6, 0.3, 0.05, 0.0],
        [0.3, 0.35, 0.6, 0.6, 0.7],
        [[0.15, 0.2, 0.05], [0.25, 0.35, 0.08], [0.15, 0.32, 0.1], [0.4, 0.55, 0.15]],
    ),
];

impl PlantSpecies {
    /// Names of the presets.
    pub fn preset_names() -> Vec<&'static str> {
        PRESETS.iter().map(|p| p.0).collect()
    }

    /// A preset plant by name.
    pub fn preset(name: &str) -> Option<PlantSpecies> {
        let (n, axiom, rules, iterations, g, p, c) = PRESETS.iter().find(|p| p.0 == name)?;
        Some(PlantSpecies {
            name: n.to_string(),
            enabled: true,
            axiom: axiom.to_string(),
            rules: rules.iter().map(|(s, r, w)| PlantRule { symbol: *s, replacement: r.to_string(), weight: *w }).collect(),
            iterations: *iterations,
            angle: g[0],
            angle_variation: g[1],
            length_scale: g[2],
            thickness: g[3],
            thinning: g[4],
            leaf_length: g[5],
            leaf_width: g[6],
            tropism: g[7],
            seed: 1,
            height: p[0],
            spacing: p[1],
            density: p[2],
            clumping: p[3],
            sway: p[4],
            upright: 0.6,
            max_slope: 180.0,
            size_variation: 0.4,
            branch_colors: [c[0], c[1]],
            leaf_colors: [c[2], c[3]],
        })
    }

    /// The fields that shape the template (not placement or colors), as a cache key.
    fn growth_key(&self) -> String {
        let rules: Vec<String> = self.rules.iter().map(|r| format!("{}>{}:{}", r.symbol, r.replacement, r.weight)).collect();
        format!(
            "{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}",
            self.axiom,
            rules.join(";"),
            self.iterations,
            self.angle,
            self.angle_variation,
            self.length_scale,
            self.thickness,
            self.thinning,
            self.leaf_length,
            self.leaf_width,
            self.tropism,
            self.seed
        )
    }
}

/// Longest L-system string expanded (later generations are cut there).
pub const MAX_SYMBOLS: usize = 200_000;
/// Most segments of a plant (the turtle stops there).
pub const MAX_SEGMENTS: usize = 4000;
/// Segments per BVH leaf.
const LEAF_SIZE: usize = 4;
/// Most plant species per seabed (march.wgsl `PLANT_SPECIES`).
pub const MAX_SPECIES: usize = 4;

/// Small deterministic random numbers (PCG) for the rules and the angle variation.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let x = (((self.0 >> 18) ^ self.0) >> 27) as u32;
        let rot = (self.0 >> 59) as u32;
        x.rotate_right(rot) as f64 / 4_294_967_296.0
    }
}

/// The L-system string after `species.iterations` generations (at most `MAX_SYMBOLS`).
pub fn expand(species: &PlantSpecies) -> String {
    let mut rng = Rng(species.seed as u64 * 0x9e37_79b9 + 17);
    let mut by_symbol: HashMap<char, Vec<(&str, f64)>> = HashMap::new();
    for r in &species.rules {
        by_symbol.entry(r.symbol).or_default().push((r.replacement.as_str(), r.weight.max(0.0)));
    }
    let mut s = species.axiom.clone();
    for _ in 0..species.iterations.min(12) {
        let mut out = String::with_capacity(s.len() * 2);
        for c in s.chars() {
            match by_symbol.get(&c) {
                Some(options) => {
                    let total: f64 = options.iter().map(|o| o.1).sum();
                    let mut pick = rng.next() * total;
                    let mut chosen = options[0].0;
                    for o in options {
                        if pick < o.1 {
                            chosen = o.0;
                            break;
                        }
                        pick -= o.1;
                    }
                    out.push_str(chosen);
                }
                None => out.push(c),
            }
            if out.len() > MAX_SYMBOLS {
                break;
            }
        }
        s = out;
        if s.len() > MAX_SYMBOLS {
            s.truncate(MAX_SYMBOLS);
            break;
        }
    }
    s
}

/// A drawn piece of a plant: a branch (round cone from `a` with radius `ra` to `b` with
/// `rb`) or a leaf (flat blade from `a` to `b`, widest in the middle with half width `ra`,
/// thickness `rb`, its width along the unit vector `side`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlantSeg {
    pub a: [f32; 3],
    pub b: [f32; 3],
    pub ra: f32,
    pub rb: f32,
    pub side: [f32; 3],
    pub leaf: bool,
}

/// Bounding box node (as `mesh::Node`): a leaf holds `count` segments from `start`, an inner
/// node has `count` 0 and its children at `start` and `start + 1`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlantNode {
    pub lo: [f32; 3],
    pub hi: [f32; 3],
    pub start: u32,
    pub count: u32,
}

/// A drawn plant, normalized to height 1 with its root at the origin.
#[derive(Clone, Debug, PartialEq)]
pub struct PlantTemplate {
    pub segs: Vec<PlantSeg>,
    pub nodes: Vec<PlantNode>,
    /// Farthest reach from the vertical axis through the root, and the top (≥ 1 by the
    /// radii).
    pub radius: f64,
    pub top: f64,
    /// Farthest reach from the root in any direction (with the radii).
    pub extent: f64,
    /// Largest divisor of the segments' distance bounds (see [`seg_norm`]): box distances
    /// divided by it never exceed a segment's bound, so the search may skip boxes by them.
    pub norm: f64,
}

/// Draws the expanded string with the turtle (see the module documentation). Heading +z.
pub fn draw(species: &PlantSpecies, s: &str) -> Vec<PlantSeg> {
    #[derive(Clone, Copy)]
    struct Turtle {
        pos: V3,
        h: V3,
        l: V3,
        u: V3,
        len: f64,
        radius: f64,
    }
    let mut rng = Rng(species.seed as u64 * 0x2545_f491 + 3);
    let base = species.angle.to_radians();
    let vary = species.angle_variation.to_radians();
    let mut t = Turtle { pos: V3::ZERO, h: v3(0.0, 0.0, 1.0), l: v3(-1.0, 0.0, 0.0), u: v3(0.0, -1.0, 0.0), len: 1.0, radius: species.thickness.max(0.0) };
    let mut stack: Vec<Turtle> = Vec::new();
    let mut segs = Vec::new();
    let up = v3(0.0, 0.0, 1.0);
    for c in s.chars() {
        if segs.len() >= MAX_SEGMENTS {
            break;
        }
        let mut angle = || base + (rng.next() * 2.0 - 1.0) * vary;
        match c {
            'F' | 'f' => {
                let end = t.pos + t.h * t.len;
                if c == 'F' {
                    // Slight taper along every branch.
                    let rb = t.radius * 0.96;
                    segs.push(PlantSeg { a: t.pos.to_f32(), b: end.to_f32(), ra: t.radius as f32, rb: rb as f32, side: [0.0; 3], leaf: false });
                    t.radius = rb;
                }
                t.pos = end;
                // Tropism: the heading bends toward up (or down).
                if species.tropism != 0.0 {
                    let bent = t.h + up * species.tropism;
                    if bent.length() > 1e-9 {
                        let nh = bent.normalize();
                        let axis = t.h.cross(nh);
                        if axis.length() > 1e-12 {
                            let ang = t.h.dot(nh).clamp(-1.0, 1.0).acos();
                            let k = axis.normalize();
                            t.l = t.l.rotate(k, ang);
                            t.u = t.u.rotate(k, ang);
                        }
                        t.h = nh;
                    }
                }
            }
            '+' | '-' => {
                let a = if c == '+' { angle() } else { -angle() };
                t.h = t.h.rotate(t.u, a);
                t.l = t.l.rotate(t.u, a);
            }
            '&' | '^' => {
                let a = if c == '&' { angle() } else { -angle() };
                t.h = t.h.rotate(t.l, a);
                t.u = t.u.rotate(t.l, a);
            }
            '\\' | '/' => {
                let a = if c == '\\' { angle() } else { -angle() };
                t.l = t.l.rotate(t.h, a);
                t.u = t.u.rotate(t.h, a);
            }
            '|' => {
                t.h = t.h * -1.0;
                t.l = t.l * -1.0;
            }
            '[' => {
                stack.push(t);
                t.len *= species.length_scale.max(0.0);
                t.radius *= species.thinning.max(0.0);
            }
            ']' => {
                if let Some(o) = stack.pop() {
                    t = o;
                }
            }
            '!' => t.radius *= species.thinning.max(0.0),
            'L' => {
                let len = t.len * species.leaf_length.max(0.0);
                if len > 0.0 {
                    let end = t.pos + t.h * len;
                    let half = len * species.leaf_width.max(0.0) * 0.5;
                    segs.push(PlantSeg { a: t.pos.to_f32(), b: end.to_f32(), ra: half as f32, rb: (len * 0.01).max(t.radius * 0.2) as f32, side: t.l.normalize().to_f32(), leaf: true });
                }
            }
            _ => {}
        }
    }
    segs
}

impl PlantTemplate {
    /// Grows, draws and normalizes `species` (see [`template`] for the cached version).
    pub fn grow(species: &PlantSpecies) -> PlantTemplate {
        let mut segs = draw(species, &expand(species));
        // Height 1 from the root at the origin (or the largest extent for plants that hang).
        let (mut top, mut ext) = (0.0f64, 0.0f64);
        for s in &segs {
            for p in [s.a, s.b] {
                top = top.max(p[2] as f64);
                ext = ext.max((p[0] as f64).abs()).max((p[1] as f64).abs()).max((p[2] as f64).abs());
            }
        }
        let scale = if top > ext * 0.05 { 1.0 / top } else if ext > 0.0 { 1.0 / ext } else { 1.0 };
        for s in &mut segs {
            s.a = s.a.map(|v| v * scale as f32);
            s.b = s.b.map(|v| v * scale as f32);
            s.ra *= scale as f32;
            s.rb *= scale as f32;
        }
        let (mut radius, mut top, mut extent) = (0.0f64, 0.0f64, 0.0f64);
        for s in &segs {
            let r = s.ra.max(s.rb) as f64;
            for p in [s.a, s.b] {
                radius = radius.max(((p[0] * p[0] + p[1] * p[1]) as f64).sqrt() + r);
                top = top.max(p[2] as f64 + r);
                extent = extent.max(f3(p).length() + r);
            }
        }
        let nodes = build_bvh(&mut segs);
        let norm = segs.iter().map(seg_norm).fold(1.0, f64::max);
        PlantTemplate { segs, nodes, radius, top: top.max(1e-3), extent: extent.max(1e-3), norm }
    }

    /// Distance from `p` (template units) to the plant, if below `limit`: distance and the
    /// segment.
    pub fn distance(&self, p: V3, limit: f64) -> Option<(f64, usize)> {
        let root = self.nodes.first()?;
        let mut best = (limit, usize::MAX);
        let box_d = |n: &PlantNode| box_distance(p, n) / self.norm;
        let mut stack: Vec<(u32, f64)> = vec![(0, box_d(root))];
        while let Some((ni, nd)) = stack.pop() {
            if nd >= best.0 {
                continue;
            }
            let n = &self.nodes[ni as usize];
            if n.count > 0 {
                for i in n.start..n.start + n.count {
                    let d = seg_distance(&self.segs[i as usize], p);
                    if d < best.0 {
                        best = (d, i as usize);
                    }
                }
                continue;
            }
            let (l, r) = (n.start, n.start + 1);
            let (dl, dr) = (box_d(&self.nodes[l as usize]), box_d(&self.nodes[r as usize]));
            let (near, far) = if dl <= dr { ((l, dl), (r, dr)) } else { ((r, dr), (l, dl)) };
            if far.1 < best.0 {
                stack.push(far);
            }
            if near.1 < best.0 {
                stack.push(near);
            }
        }
        (best.1 != usize::MAX).then_some(best)
    }
}

fn f3(a: [f32; 3]) -> V3 {
    v3(a[0] as f64, a[1] as f64, a[2] as f64)
}

/// Distance from `p` to the box of node `n` (0 inside).
fn box_distance(p: V3, n: &PlantNode) -> f64 {
    let d = |k: usize, v: f64| (n.lo[k] as f64 - v).max(v - n.hi[k] as f64).max(0.0);
    let (x, y, z) = (d(0, p.x), d(1, p.y), d(2, p.z));
    (x * x + y * y + z * z).sqrt()
}

/// Divisor of a segment's distance bound: the rate of its radius (branches) or width (leaves)
/// change along it.
fn seg_norm(s: &PlantSeg) -> f64 {
    let len = (f3(s.b) - f3(s.a)).length().max(1e-10);
    let k = if s.leaf { 4.0 * s.ra as f64 / len } else { (s.ra as f64 - s.rb as f64) / len };
    (1.0 + k * k).sqrt()
}

/// Distance bound of one segment (mirrors march.wgsl `plant_seg`): branches as round cones
/// (divided by the rate of their radius change), leaves as flat blades widest in the middle
/// (divided by the rate of their width change).
pub fn seg_distance(s: &PlantSeg, p: V3) -> f64 {
    let (a, b) = (f3(s.a), f3(s.b));
    let ba = b - a;
    let pa = p - a;
    let len2 = ba.dot(ba).max(1e-20);
    let len = len2.sqrt();
    if !s.leaf {
        let h = (pa.dot(ba) / len2).clamp(0.0, 1.0);
        let r = s.ra as f64 + (s.rb as f64 - s.ra as f64) * h;
        let k = (s.ra as f64 - s.rb as f64) / len;
        return ((pa - ba * h).length() - r) / (1.0 + k * k).sqrt();
    }
    let u = ba * (1.0 / len);
    let side = f3(s.side);
    let n = u.cross(side);
    let along = pa.dot(u);
    let t = (along / len).clamp(0.0, 1.0);
    let w = s.ra as f64 * 4.0 * t * (1.0 - t);
    let out = along - along.clamp(0.0, len);
    let wide = (pa.dot(side).abs() - w).max(0.0);
    let nrm = pa.dot(n);
    let k = 4.0 * s.ra as f64 / len;
    ((out * out + wide * wide + nrm * nrm).sqrt() - s.rb as f64) / (1.0 + k * k).sqrt()
}

/// Bounding box of a segment (with its radius or half width).
fn seg_box(s: &PlantSeg) -> ([f32; 3], [f32; 3]) {
    let r = s.ra.max(s.rb);
    (std::array::from_fn(|k| s.a[k].min(s.b[k]) - r), std::array::from_fn(|k| s.a[k].max(s.b[k]) + r))
}

/// Builds the BVH (median split along the longest centroid axis) and puts `segs` in leaf
/// order.
fn build_bvh(segs: &mut [PlantSeg]) -> Vec<PlantNode> {
    let mut nodes = Vec::new();
    if segs.is_empty() {
        return nodes;
    }
    let centroid = |s: &PlantSeg| -> [f32; 3] { std::array::from_fn(|k| (s.a[k] + s.b[k]) * 0.5) };
    nodes.push(PlantNode { lo: [0.0; 3], hi: [0.0; 3], start: 0, count: 0 });
    let mut stack = vec![(0usize, 0usize, segs.len())];
    while let Some((ni, s, e)) = stack.pop() {
        let (mut lo, mut hi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
        let (mut clo, mut chi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
        for sg in &segs[s..e] {
            let (a, b) = seg_box(sg);
            let c = centroid(sg);
            for k in 0..3 {
                lo[k] = lo[k].min(a[k]);
                hi[k] = hi[k].max(b[k]);
                clo[k] = clo[k].min(c[k]);
                chi[k] = chi[k].max(c[k]);
            }
        }
        nodes[ni].lo = lo;
        nodes[ni].hi = hi;
        let ext = [0, 1, 2].map(|k| chi[k] - clo[k]);
        let axis = if ext[0] >= ext[1] && ext[0] >= ext[2] { 0 } else if ext[1] >= ext[2] { 1 } else { 2 };
        if e - s <= LEAF_SIZE || ext[axis] <= 0.0 {
            nodes[ni].start = s as u32;
            nodes[ni].count = (e - s) as u32;
            continue;
        }
        let mid = (s + e) / 2;
        segs[s..e].select_nth_unstable_by(mid - s, |a, b| centroid(a)[axis].total_cmp(&centroid(b)[axis]));
        let l = nodes.len();
        nodes.push(nodes[ni]);
        nodes.push(nodes[ni]);
        nodes[ni].start = l as u32;
        nodes[ni].count = 0;
        stack.push((l, s, mid));
        stack.push((l + 1, mid, e));
    }
    nodes
}

/// The template of `species`, grown once per growth setting (cached for the process; the
/// placement and colors do not change it).
pub fn template(species: &PlantSpecies) -> Arc<PlantTemplate> {
    static CACHE: OnceLock<Mutex<HashMap<String, Arc<PlantTemplate>>>> = OnceLock::new();
    let key = species.growth_key();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(t) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return t.clone();
    }
    let t = Arc::new(PlantTemplate::grow(species));
    let mut c = cache.lock().unwrap_or_else(|e| e.into_inner());
    if c.len() > 64 {
        c.clear();
    }
    c.insert(key, t.clone());
    t
}

/// The 5 × 5 cells around a point, nearest first (march.wgsl `CELL_ORDER`).
pub const CELL_ORDER: [(i32, i32); 25] = [
    (0, 0),
    (-1, 0),
    (1, 0),
    (0, -1),
    (0, 1),
    (-1, -1),
    (1, -1),
    (-1, 1),
    (1, 1),
    (-2, 0),
    (2, 0),
    (0, -2),
    (0, 2),
    (-2, -1),
    (2, -1),
    (-2, 1),
    (2, 1),
    (-1, -2),
    (1, -2),
    (-1, 2),
    (1, 2),
    (-2, -2),
    (2, -2),
    (-2, 2),
    (2, 2),
];

/// Swaying sideways offset per unit of plant height at most, per unit of sway (march.wgsl
/// `PLANT_SWAY`).
pub const PLANT_SWAY: f64 = 0.15;

/// One species placed on a seabed (march.wgsl: a species block in `vols`).
#[derive(Clone, Debug)]
pub struct PlacedSpecies {
    pub template: Arc<PlantTemplate>,
    /// Grid cell (raised so a plant never reaches beyond the neighbouring cells).
    pub cell: f64,
    pub share: f64,
    pub clumping: f64,
    pub height: f64,
    pub variation: f64,
    pub sway: f64,
    pub salt: i32,
    pub branch_colors: [[f32; 3]; 2],
    pub leaf_colors: [[f32; 3]; 2],
}

impl PlacedSpecies {
    pub fn new(s: &PlantSpecies, index: usize) -> PlacedSpecies {
        let template = template(s);
        let height = s.height.abs();
        let sway = s.sway.clamp(0.0, 1.0);
        let reach = height * (template.radius + PLANT_SWAY * sway * template.top * template.top);
        PlacedSpecies {
            cell: s.spacing.abs().max(reach / 0.45).max(1e-4),
            share: s.density.clamp(0.0, 1.0),
            clumping: s.clumping.clamp(0.0, 1.0),
            height,
            variation: s.size_variation.clamp(0.0, 0.95),
            sway,
            salt: 200 + index as i32,
            branch_colors: s.branch_colors,
            leaf_colors: s.leaf_colors,
            template,
        }
    }
}

/// The plants of one seabed object.
#[derive(Clone, Debug, Default)]
pub struct PlantField {
    pub species: Vec<PlacedSpecies>,
}

/// Where the nearest plant is: its distance, which species, whether a leaf, and how high up
/// the plant (0..1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlantHit {
    pub d: f64,
    pub species: usize,
    pub leaf: bool,
    pub height: f64,
}

/// Color value that stands for a plant color (`Shader::gradient` and march.wgsl `gradient`
/// take values from here on as plant colors, in scenes with plants): `PLANT_COLOR + 2 ×
/// (object × MAX_SPECIES + species) + leaf + height` ([`decode_color`]). Far above the color
/// values of fractals (iteration counts) and patterns.
pub const PLANT_COLOR: f64 = 100_000.0;
/// Largest height part of a plant color value (march.wgsl `PLANT_HEIGHT_MAX`); f32 steps are
/// 1/128 there.
pub const PLANT_HEIGHT_MAX: f64 = 0.98;

impl PlantHit {
    /// The color value of this plant on the `object`-th composed object (0-based). The height
    /// stays below `PLANT_HEIGHT_MAX`: in f32 near 1e5 a value closer to the next integer rounds
    /// up to it (the GPU then decoded the next species).
    pub fn color_value(&self, object: usize) -> f64 {
        PLANT_COLOR + 2.0 * (object * MAX_SPECIES + self.species) as f64 + if self.leaf { 1.0 } else { 0.0 } + self.height.clamp(0.0, PLANT_HEIGHT_MAX)
    }
}

/// Object, species, leaf and height of a plant color value (see [`PLANT_COLOR`]).
pub fn decode_color(v: f64) -> (usize, usize, bool, f64) {
    let x = v - PLANT_COLOR;
    let k = x.floor().max(0.0) as usize;
    (k / 2 / MAX_SPECIES, (k / 2) % MAX_SPECIES, k % 2 == 1, x - k as f64)
}

impl PlantField {
    /// The enabled species of a seabed (at most `MAX_SPECIES`).
    pub fn new(species: &[PlantSpecies]) -> Option<PlantField> {
        let list: Vec<PlacedSpecies> = species.iter().filter(|s| s.enabled && s.density > 0.0 && s.height > 0.0).take(MAX_SPECIES).enumerate().map(|(i, s)| PlacedSpecies::new(s, i)).collect();
        let field = PlantField { species: list };
        field.species.iter().any(|s| !s.template.segs.is_empty()).then_some(field)
    }

    /// The color of a species' branches or leaves at `height` (0 root .. 1 top).
    pub fn color(&self, species: usize, leaf: bool, height: f64) -> [f64; 3] {
        let Some(s) = self.species.get(species) else { return [0.5; 3] };
        let c = if leaf { &s.leaf_colors } else { &s.branch_colors };
        [0, 1, 2].map(|i| c[0][i] as f64 + (c[1][i] as f64 - c[0][i] as f64) * height)
    }

    /// Nearest plant to the seabed's local point `q` within `limit` (mirrors march.wgsl
    /// `plant_de`): per species the 5 × 5 grid cells around, a plant in a cell by the share
    /// (and the patches), standing on the ground below its root, turned, sized and swaying at
    /// the time `phase`; plants whose bounding cylinder is farther than the best so far are
    /// skipped. The result is a distance bound; plants of cells farther away reach at most
    /// 0.2 cells into the block, so the distance to the block's edge minus that covers them
    /// (a tight bound matters: soft shadows and occlusion read it as the distance to the
    /// nearest surface).
    pub fn nearest(&self, a: &[f64; 12], q: V3, phase: f64, limit: f64) -> Option<PlantHit> {
        let mut best = PlantHit { d: limit, species: 0, leaf: false, height: 0.0 };
        let ground_top = a[0].abs() + a[2].abs();
        for (k, s) in self.species.iter().enumerate() {
            let t = &s.template;
            if t.segs.is_empty() {
                continue;
            }
            // Height above the tallest plant on the highest ground: a lower bound for every
            // plant, combined with the sideways gap (alone it would be a flat plane that soft
            // shadows take for a surface).
            let above = q.z - ground_top - s.height * t.top;
            if above >= best.d {
                continue;
            }
            let c = s.cell;
            let (ci, cj) = ((q.x / c).floor() as i32, (q.y / c).floor() as i32);
            let edge = (q.x - (ci - 2) as f64 * c).min((ci + 3) as f64 * c - q.x).min(q.y - (cj - 2) as f64 * c).min((cj + 3) as f64 * c - q.y);
            let far = (edge - 0.2 * c).max(above);
            if far < best.d {
                best = PlantHit { d: far, species: k, leaf: false, height: 0.0 };
            }
            // Farthest sideways reach of the largest plant, and how far above all plants.
            let reach_max = s.height * (t.radius + PLANT_SWAY * s.sway * t.top * t.top);
            let up = q.z - ground_top - s.height * t.top;
            for &(di, dj) in &CELL_ORDER {
                let (i, j) = (ci + di, cj + dj);
                // Roots stay in the middle half of their cell: skip cells too far to beat the
                // best before any hashing (nearest cells first, so the best shrinks early).
                let gx = ((q.x - (i as f64 + 0.5) * c).abs() - 0.25 * c).max(0.0);
                let gy = ((q.y - (j as f64 + 0.5) * c).abs() - 0.25 * c).max(0.0);
                if ((gx * gx + gy * gy).sqrt() - reach_max).max(up) >= best.d {
                    continue;
                }
                let h = |m: i32| crate::render::hash3(i.wrapping_add(m.wrapping_mul(613)), j, s.salt) as f64;
                // The patches only thin the plants out: test the share first (cheap).
                let h0 = h(0);
                if h0 >= s.share || (s.clumping > 0.0 && h0 >= s.share * (1.0 + ((2.5 * crate::compose::water_noise(i as f64 * c / 1.5 + 3.7, j as f64 * c / 1.5 + 9.1) - 1.0).clamp(0.0, 1.0) - 1.0) * s.clumping)) {
                    continue;
                }
                let size = s.height * (1.0 - s.variation * h(1));
                let (rx, ry) = ((i as f64 + 0.25 + 0.5 * h(2)) * c, (j as f64 + 0.25 + 0.5 * h(3)) * c);
                let (dx, dy) = (q.x - rx, q.y - ry);
                // Too far sideways to beat the best.
                let reach = size * (t.radius + PLANT_SWAY * s.sway * t.top * t.top);
                if ((dx * dx + dy * dy).sqrt() - reach).max(q.z - ground_top - size * t.top) >= best.d {
                    continue;
                }
                let gz = crate::compose::seabed_height(a, rx, ry);
                let (sn, cs) = (h(4) * std::f64::consts::TAU).sin_cos();
                let mut p = v3((dx * cs + dy * sn) / size, (dy * cs - dx * sn) / size, (q.z - gz) / size);
                let z = p.z.clamp(0.0, t.top);
                p.x -= PLANT_SWAY * s.sway * z * z * (phase + h(5) * std::f64::consts::TAU - 2.0 * z).sin();
                // The sway's warp stretches distances by at most 1 + its rate along the height
                // (growing with the height): the rate at the highest point a nearer result
                // can reach (most steps happen low, near the ground).
                let zt = (p.z + best.d / size).clamp(0.0, t.top);
                let warp = 1.0 + PLANT_SWAY * s.sway * (2.0 * zt + 2.0 * zt * zt);
                if let Some((d, seg)) = t.distance(p, best.d / size * warp) {
                    let d = d * size / warp;
                    if d < best.d {
                        best = PlantHit { d, species: k, leaf: t.segs[seg].leaf, height: (p.z / t.top).clamp(0.0, 1.0) };
                    }
                }
            }
        }
        (best.d < limit).then_some(best)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_grow_into_plants_of_height_one() {
        for name in PlantSpecies::preset_names() {
            let s = PlantSpecies::preset(name).unwrap();
            let t = PlantTemplate::grow(&s);
            assert!(t.segs.len() > 10 && t.segs.len() <= MAX_SEGMENTS, "{name}: {} segments", t.segs.len());
            assert!(t.top >= 1.0 && t.top < 1.3, "{name}: top {}", t.top);
            assert!(t.radius > 0.05 && t.radius < 3.0, "{name}: radius {}", t.radius);
            // The BVH finds what a brute-force search finds.
            for i in 0..200 {
                let p = v3((i as f64 * 0.618) % 1.0 - 0.5, (i as f64 * 0.755) % 1.0 - 0.5, (i as f64 * 0.569) % 1.0 * 1.2);
                let brute = t.segs.iter().map(|sg| seg_distance(sg, p)).fold(f64::INFINITY, f64::min);
                let found = t.distance(p, 1e9).map_or(f64::INFINITY, |h| h.0);
                assert!((brute - found).abs() < 1e-9, "{name}: {brute} vs {found}");
            }
        }
    }

    #[test]
    fn rules_rewrite_by_weight_and_the_turtle_branches() {
        let mut s = PlantSpecies { axiom: "A".into(), rules: vec![PlantRule { symbol: 'A', replacement: "F[+A][-A]".into(), weight: 1.0 }], iterations: 3, ..PlantSpecies::default() };
        let e = expand(&s);
        assert_eq!(e.matches('F').count(), 7);
        let segs = draw(&s, &e);
        assert_eq!(segs.len(), 7);
        // Side branches are shorter and thinner.
        let len = |g: &PlantSeg| (f3(g.b) - f3(g.a)).length();
        assert!(len(&segs[1]) < len(&segs[0]) && segs[1].ra < segs[0].ra);
        // Two rules for one symbol: both are used.
        s.rules.push(PlantRule { symbol: 'A', replacement: "FL".into(), weight: 1.0 });
        s.iterations = 6;
        let e = expand(&s);
        assert!(e.contains('L') && e.contains('['));
        // Leaves are flat: thin across, wide along their side.
        let leaf = draw(&s, &e).into_iter().find(|g| g.leaf).unwrap();
        let mid = (f3(leaf.a) + f3(leaf.b)) * 0.5;
        assert!(seg_distance(&leaf, mid + f3(leaf.side) * (leaf.ra as f64 * 0.9)) < 0.0);
        assert!(seg_distance(&leaf, mid + f3(leaf.side).cross((f3(leaf.b) - f3(leaf.a)).normalize()) * (leaf.ra as f64 * 0.9)) > 0.0);
    }

    #[test]
    fn placed_plants_stay_a_distance_bound() {
        let a = crate::compose::Primitive::seabed().params();
        let species: Vec<PlantSpecies> = PlantSpecies::preset_names().iter().map(|n| PlantSpecies::preset(n).unwrap()).collect();
        let field = PlantField::new(&species).unwrap();
        let mut checked = 0;
        for i in 0..4000 {
            let (x, y) = ((i as f64 * 0.6180339) % 1.0 * 4.0, (i as f64 * 0.7548776) % 1.0 * 4.0);
            let z = crate::compose::seabed_height(&a, x, y) + ((i as f64 * 0.5698403) % 1.0) * 1.0;
            let p = v3(x, y, z);
            let Some(h) = field.nearest(&a, p, 1.3, 1e9) else { continue };
            if h.d <= 1e-4 {
                continue;
            }
            checked += 1;
            for k in 0..26 {
                let dir = v3(((k % 3) as f64) - 1.0, ((k / 3 % 3) as f64) - 1.0, ((k / 9) as f64) - 1.0 + if k == 13 { 1.0 } else { 0.0 });
                let q = p + dir.normalize() * (h.d * 0.98);
                let inside = field.nearest(&a, q, 1.3, 1e9).is_some_and(|g| g.d < -1e-9);
                assert!(!inside, "bound {} at {p:?} reaches into a plant at {q:?}", h.d);
            }
        }
        assert!(checked > 1000, "{checked}");
    }
}

