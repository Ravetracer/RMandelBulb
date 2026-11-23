// SPDX-License-Identifier: LGPL-2.1-or-later

//! Palette tools: a random palette generator and edits of whole gradients (hue, saturation,
//! lightness, contrast, reverse, mirror, repeat …).
//!
//! Colors are made and changed in OKLCh (lightness, chroma, hue of the OKLab color space):
//! equal steps there look equally large, so generated palettes keep even contrast and hue
//! changes do not make some colors jump in brightness. Colors outside the sRGB range are
//! brought back by lowering their chroma.

use serde::{Deserialize, Serialize};

use crate::scene::ColorStop;

/// How the hues of a generated palette relate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Harmony {
    /// Any hues.
    Random,
    /// Neighboring hues within a narrow arc.
    Analogous,
    /// The base hue and its opposite.
    Complementary,
    /// The base hue and the two neighbors of its opposite.
    SplitComplementary,
    /// Three hues a third of the circle apart.
    Triadic,
    /// Four hues a quarter of the circle apart.
    Tetradic,
    /// One hue in different lightness and chroma.
    Monochrome,
    /// Mostly very dark, desaturated colors with a few bright, saturated (and emissive) ones:
    /// dark structures with neon lines.
    DarkNeon,
}

impl Harmony {
    pub const ALL: [Harmony; 8] = [
        Harmony::Random,
        Harmony::Analogous,
        Harmony::Complementary,
        Harmony::SplitComplementary,
        Harmony::Triadic,
        Harmony::Tetradic,
        Harmony::Monochrome,
        Harmony::DarkNeon,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Harmony::Random => "Random",
            Harmony::Analogous => "Analogous",
            Harmony::Complementary => "Complementary",
            Harmony::SplitComplementary => "Split complementary",
            Harmony::Triadic => "Triadic",
            Harmony::Tetradic => "Tetradic",
            Harmony::Monochrome => "Monochrome",
            Harmony::DarkNeon => "Dark + neon",
        }
    }

    /// The hue offsets (degrees) the colors cycle through.
    fn offsets(self) -> &'static [f64] {
        match self {
            Harmony::Random | Harmony::Monochrome => &[0.0],
            Harmony::Analogous => &[-30.0, -15.0, 0.0, 15.0, 30.0],
            Harmony::Complementary => &[0.0, 180.0],
            Harmony::SplitComplementary => &[0.0, 150.0, 210.0],
            Harmony::Triadic => &[0.0, 120.0, 240.0],
            Harmony::Tetradic => &[0.0, 90.0, 180.0, 270.0],
            Harmony::DarkNeon => &[0.0, 120.0, 240.0, 60.0, 180.0, 300.0],
        }
    }
}

/// Order of the generated colors along the gradient.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Order {
    /// As the harmony gives them.
    Harmony,
    /// Dark and light colors take turns (strong contrast between neighbors).
    Alternate,
    /// From dark to light.
    Lightness,
    /// Around the hue circle.
    Hue,
}

impl Order {
    pub const ALL: [Order; 4] = [Order::Harmony, Order::Alternate, Order::Lightness, Order::Hue];

    pub fn label(self) -> &'static str {
        match self {
            Order::Harmony => "As generated",
            Order::Alternate => "Dark / light",
            Order::Lightness => "Dark to light",
            Order::Hue => "By hue",
        }
    }
}

/// Settings of the random palette generator.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PaletteOptions {
    /// Number of colors (2..=16).
    pub colors: usize,
    pub harmony: Harmony,
    /// Base hue in degrees (ignored with `random_hue`).
    pub hue: f64,
    /// Pick the base hue at random.
    pub random_hue: bool,
    /// Random deviation of every hue, in degrees.
    pub hue_jitter: f64,
    /// Range of the chroma (0 gray .. 1 the most saturated colors).
    pub saturation: [f64; 2],
    /// Range of the lightness (0 black .. 1 white).
    pub lightness: [f64; 2],
    pub order: Order,
    /// 0: smooth blends between the colors, 1: hard steps (bands of solid color).
    pub hardness: f64,
    /// The gradient ends with its first color, so it repeats without a seam.
    pub cyclic: bool,
    /// Share of the colors (the brightest first) that emit light.
    pub emissive_share: f64,
    /// Emission strength of those colors.
    pub emission: f64,
    pub seed: u64,
}

impl Default for PaletteOptions {
    fn default() -> Self {
        PaletteOptions {
            colors: 5,
            harmony: Harmony::Analogous,
            hue: 210.0,
            random_hue: true,
            hue_jitter: 10.0,
            saturation: [0.3, 0.8],
            lightness: [0.2, 0.85],
            order: Order::Alternate,
            hardness: 0.0,
            cyclic: true,
            emissive_share: 0.0,
            emission: 2.0,
            seed: 1,
        }
    }
}

/// Small deterministic random stream (splitmix64).
struct Rand(u64);

impl Rand {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^= z >> 31;
        (z >> 11) as f64 / (1u64 << 53) as f64
    }

    fn range(&mut self, r: [f64; 2]) -> f64 {
        let (a, b) = (r[0].min(r[1]), r[0].max(r[1]));
        a + (b - a) * self.next()
    }
}

/// Largest chroma of OKLCh colors that the saturation range maps to (about the most
/// saturated sRGB colors).
const MAX_CHROMA: f64 = 0.32;

/// A random palette.
pub fn generate(o: &PaletteOptions) -> Vec<ColorStop> {
    let n = o.colors.clamp(2, 16);
    let mut rnd = Rand(o.seed.wrapping_mul(0x2545_f491_4f6c_dd1d) ^ 0x5851_f42d);
    let base = if o.random_hue { rnd.next() * 360.0 } else { o.hue };
    let offsets = o.harmony.offsets();
    // Colors as (lightness, chroma, hue).
    let mut lch: Vec<[f64; 3]> = (0..n)
        .map(|i| {
            let jitter = (rnd.next() * 2.0 - 1.0) * o.hue_jitter;
            let h = match o.harmony {
                Harmony::Random => rnd.next() * 360.0,
                _ => base + offsets[i % offsets.len()] + jitter,
            };
            let (l, c) = match o.harmony {
                // About a third bright and saturated, the rest dark and dull.
                Harmony::DarkNeon if i % 3 == 1 => (rnd.range([0.72, 0.9]), rnd.range([0.75, 1.0])),
                Harmony::DarkNeon => (rnd.range([0.05, 0.2]), rnd.range([0.05, 0.3])),
                _ => (rnd.range(o.lightness), rnd.range(o.saturation)),
            };
            [l.clamp(0.0, 1.0), c.clamp(0.0, 1.0) * MAX_CHROMA, h.rem_euclid(360.0)]
        })
        .collect();
    match o.order {
        Order::Harmony => {}
        Order::Lightness => lch.sort_by(|a, b| a[0].total_cmp(&b[0])),
        Order::Hue => lch.sort_by(|a, b| a[2].total_cmp(&b[2])),
        Order::Alternate => {
            lch.sort_by(|a, b| a[0].total_cmp(&b[0]));
            // Dark, light, dark, light …: the darkest half interleaved with the lightest half.
            let (dark, light) = lch.split_at(n.div_ceil(2));
            let mut out = Vec::with_capacity(n);
            for i in 0..n.div_ceil(2) {
                out.push(dark[i]);
                if i < light.len() {
                    out.push(light[light.len() - 1 - i]);
                }
            }
            lch = out;
        }
    }
    // The brightest colors emit.
    let emitting = ((o.emissive_share.clamp(0.0, 1.0) * n as f64).round() as usize).min(n);
    let mut by_light: Vec<usize> = (0..n).collect();
    by_light.sort_by(|a, b| lch[*b][0].total_cmp(&lch[*a][0]));
    let mut stops: Vec<ColorStop> = lch
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let mut s = ColorStop::new(0.0, from_oklch(*c));
            if by_light[..emitting].contains(&i) {
                s.emission = o.emission as f32;
            }
            s
        })
        .collect();
    if o.cyclic {
        stops.push(stops[0].clone());
    }
    space_evenly(&mut stops);
    set_hardness(&stops, o.hardness)
}

/// Linear sRGB to OKLab.
pub fn to_oklab(c: [f32; 3]) -> [f64; 3] {
    let [r, g, b] = c.map(|v| v.max(0.0) as f64);
    let l = (0.412_221_470_8 * r + 0.536_332_536_3 * g + 0.051_445_992_9 * b).cbrt();
    let m = (0.211_903_498_2 * r + 0.680_699_545_1 * g + 0.107_396_956_6 * b).cbrt();
    let s = (0.088_302_461_9 * r + 0.281_718_837_6 * g + 0.629_978_700_5 * b).cbrt();
    [
        0.210_454_255_3 * l + 0.793_617_785 * m - 0.004_072_046_8 * s,
        1.977_998_495_1 * l - 2.428_592_205 * m + 0.450_593_709_9 * s,
        0.025_904_037_1 * l + 0.782_771_766_2 * m - 0.808_675_766 * s,
    ]
}

/// OKLab to linear sRGB (may leave 0..1).
pub fn from_oklab(c: [f64; 3]) -> [f64; 3] {
    let l = c[0] + 0.396_337_777_4 * c[1] + 0.215_803_757_3 * c[2];
    let m = c[0] - 0.105_561_345_8 * c[1] - 0.063_854_172_8 * c[2];
    let s = c[0] - 0.089_484_177_5 * c[1] - 1.291_485_548 * c[2];
    let (l, m, s) = (l * l * l, m * m * m, s * s * s);
    [
        4.076_741_662_1 * l - 3.307_711_591_3 * m + 0.230_969_929_2 * s,
        -1.268_438_004_6 * l + 2.609_757_401_1 * m - 0.341_319_396_5 * s,
        -0.004_196_086_3 * l - 0.703_418_614_7 * m + 1.707_614_701 * s,
    ]
}

/// Linear sRGB to OKLCh (lightness, chroma, hue in degrees).
pub fn to_oklch(c: [f32; 3]) -> [f64; 3] {
    let [l, a, b] = to_oklab(c);
    [l, a.hypot(b), b.atan2(a).to_degrees().rem_euclid(360.0)]
}

/// OKLCh to linear sRGB, lowering the chroma until the color fits into sRGB.
pub fn from_oklch(c: [f64; 3]) -> [f32; 3] {
    let l = c[0].clamp(0.0, 1.0);
    let h = c[2].to_radians();
    let rgb = |ch: f64| from_oklab([l, ch * h.cos(), ch * h.sin()]);
    let inside = |v: [f64; 3]| v.iter().all(|x| (-1e-6..=1.0 + 1e-6).contains(x));
    let mut ch = c[1].max(0.0);
    if !inside(rgb(ch)) {
        // Bisect the largest chroma that fits.
        let (mut lo, mut hi) = (0.0, ch);
        for _ in 0..24 {
            let mid = 0.5 * (lo + hi);
            if inside(rgb(mid)) { lo = mid } else { hi = mid }
        }
        ch = lo;
    }
    rgb(ch).map(|v| v.clamp(0.0, 1.0) as f32)
}

/// Sorted by position (stable).
fn sorted(stops: &[ColorStop]) -> Vec<ColorStop> {
    let mut s = stops.to_vec();
    s.sort_by(|a, b| a.pos.total_cmp(&b.pos));
    s
}

/// The colors in order, one stop each: neighbors with the same color, emission and
/// transparency (the two ends of a hard band) fold into one.
fn distinct(stops: &[ColorStop]) -> Vec<ColorStop> {
    let mut out: Vec<ColorStop> = Vec::new();
    for s in sorted(stops) {
        if let Some(last) = out.last()
            && last.color == s.color
            && last.emission == s.emission
            && last.transparency == s.transparency
        {
            continue;
        }
        out.push(s);
    }
    out
}

/// Spreads the stops evenly over 0..1 in their current order.
pub fn space_evenly(stops: &mut [ColorStop]) {
    let n = stops.len();
    for (i, s) in stops.iter_mut().enumerate() {
        s.pos = if n > 1 { i as f64 / (n - 1) as f64 } else { 0.0 };
    }
}

/// The colors (in order, see [`distinct`]) spaced evenly with blends of the given hardness:
/// 0 blends smoothly from color to color, 1 holds each color and switches hard (each color
/// gets two stops; in between, the blend zone shrinks).
pub fn set_hardness(stops: &[ColorStop], hardness: f64) -> Vec<ColorStop> {
    let mut colors = distinct(stops);
    let k = hardness.clamp(0.0, 1.0);
    if k <= 0.0 || colors.len() < 2 {
        space_evenly(&mut colors);
        return colors;
    }
    // Hard: n colors in n equal bands; the blend between neighbors takes (1 - k) of a band.
    let n = colors.len();
    let band = 1.0 / n as f64;
    let blend = (1.0 - k) * band;
    let mut out = Vec::with_capacity(2 * n);
    for (i, c) in colors.iter().enumerate() {
        let (a, b) = (i as f64 * band, (i + 1) as f64 * band);
        let start = if i == 0 { 0.0 } else { a + 0.5 * blend };
        let end = if i + 1 == n { 1.0 } else { b - 0.5 * blend };
        out.push(ColorStop { pos: start, ..c.clone() });
        out.push(ColorStop { pos: end.max(start), ..c.clone() });
    }
    out
}

/// Relative edits of all colors in OKLCh.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Adjust {
    /// Degrees added to every hue.
    pub hue: f64,
    /// Saturation change (-1 gray .. +1 double chroma).
    pub saturation: f64,
    /// Added to the lightness (-1 .. 1).
    pub lightness: f64,
    /// Lightness contrast around the middle (-1 flat .. +1 doubled).
    pub contrast: f64,
}

/// `stops` with every surface color changed by `a` (positions, highlights and emission stay).
pub fn adjust(stops: &[ColorStop], a: Adjust) -> Vec<ColorStop> {
    stops
        .iter()
        .map(|s| {
            let [l, c, h] = to_oklch(s.color);
            let l = (0.5 + (l - 0.5) * (1.0 + a.contrast) + a.lightness).clamp(0.0, 1.0);
            let c = c * (1.0 + a.saturation).max(0.0);
            ColorStop { color: from_oklch([l, c, h + a.hue]), ..s.clone() }
        })
        .collect()
}

/// The gradient running the other way.
pub fn reverse(stops: &[ColorStop]) -> Vec<ColorStop> {
    sorted(&stops.iter().map(|s| ColorStop { pos: 1.0 - s.pos, ..s.clone() }).collect::<Vec<_>>())
}

/// The gradient shifted by `offset` (wrapping around; the colors at both ends are kept so the
/// shift does not cut a blend).
pub fn rotate(stops: &[ColorStop], offset: f64) -> Vec<ColorStop> {
    let s = sorted(stops);
    if s.len() < 2 {
        return s;
    }
    let mut out: Vec<ColorStop> = s.iter().map(|c| ColorStop { pos: (c.pos + offset).rem_euclid(1.0), ..c.clone() }).collect();
    // Colors at the seam (0 and 1) from the original gradient.
    let seam = |t: f64| {
        let u = (t - offset).rem_euclid(1.0);
        let mut c = s[0].clone();
        c.pos = t;
        c.color = crate::gradient::sample(&s, u);
        c.specular = crate::gradient::sample_specular(&s, u);
        c.emission = crate::gradient::sample_emission(&s, u);
        c.transparency = crate::gradient::sample_transparency(&s, u);
        c
    };
    out.push(seam(0.0));
    out.push(seam(1.0));
    sorted(&out)
}

/// The gradient squeezed into the first half and mirrored into the second (seamless).
pub fn mirror(stops: &[ColorStop]) -> Vec<ColorStop> {
    let s = sorted(stops);
    let mut out: Vec<ColorStop> = s.iter().map(|c| ColorStop { pos: c.pos * 0.5, ..c.clone() }).collect();
    out.extend(s.iter().rev().map(|c| ColorStop { pos: 1.0 - c.pos * 0.5, ..c.clone() }));
    sorted(&out)
}

/// The gradient `n` times over 0..1.
pub fn repeat(stops: &[ColorStop], n: usize) -> Vec<ColorStop> {
    let s = sorted(stops);
    let n = n.max(1);
    (0..n).flat_map(|k| s.iter().map(move |c| ColorStop { pos: (k as f64 + c.pos) / n as f64, ..c.clone() })).collect()
}

/// The colors sorted from dark to light, spaced evenly.
pub fn sort_by_lightness(stops: &[ColorStop]) -> Vec<ColorStop> {
    let mut s = distinct(stops);
    s.sort_by(|a, b| to_oklch(a.color)[0].total_cmp(&to_oklch(b.color)[0]));
    space_evenly(&mut s);
    s
}

/// The colors inverted in lightness (dark becomes light), hue and chroma kept.
pub fn invert(stops: &[ColorStop]) -> Vec<ColorStop> {
    stops
        .iter()
        .map(|s| {
            let [l, c, h] = to_oklch(s.color);
            ColorStop { color: from_oklch([1.0 - l, c, h]), ..s.clone() }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oklab_round_trips() {
        for c in [[0.2f32, 0.5, 0.9], [1.0, 0.0, 0.0], [0.0, 0.0, 0.0], [1.0, 1.0, 1.0], [0.01, 0.3, 0.02]] {
            let back = from_oklch(to_oklch(c));
            for k in 0..3 {
                assert!((back[k] - c[k]).abs() < 1e-4, "{c:?} -> {back:?}");
            }
        }
        // White has lightness 1, black 0.
        assert!((to_oklch([1.0; 3])[0] - 1.0).abs() < 1e-4);
    }

    #[test]
    fn generated_palettes_are_deterministic_in_gamut_and_seamless() {
        for harmony in Harmony::ALL {
            for order in Order::ALL {
                let o = PaletteOptions { harmony, order, colors: 6, seed: 7, ..Default::default() };
                let a = generate(&o);
                assert_eq!(a, generate(&o), "{harmony:?} deterministic");
                // 6 colors plus the first again.
                assert_eq!(a.len(), 7, "{harmony:?}");
                assert!(a.iter().all(|s| s.color.iter().all(|v| (0.0..=1.0).contains(v))));
                assert_eq!(a[0].color, a[6].color, "cyclic");
                assert!(a.windows(2).all(|w| w[0].pos <= w[1].pos));
                assert_ne!(a, generate(&PaletteOptions { seed: 8, ..o }), "{harmony:?}: other seed");
            }
        }
        let neon = generate(&PaletteOptions { harmony: Harmony::DarkNeon, colors: 9, emissive_share: 0.34, ..Default::default() });
        assert_eq!(neon.iter().take(9).filter(|s| s.emission > 0.0).count(), 3);
    }

    #[test]
    fn hardness_makes_bands_and_back() {
        let o = PaletteOptions { colors: 4, cyclic: false, ..Default::default() };
        let smooth = generate(&o);
        let hard = set_hardness(&smooth, 1.0);
        assert_eq!(hard.len(), 8);
        // Each color holds over its quarter.
        assert_eq!(crate::gradient::sample(&hard, 0.1), smooth[0].color);
        assert_eq!(crate::gradient::sample(&hard, 0.6), smooth[2].color);
        assert_eq!(set_hardness(&hard, 0.0), smooth, "back to one stop per color");
        assert_eq!(set_hardness(&smooth, 0.0), smooth);
    }

    #[test]
    fn edits_keep_the_gradient_consistent() {
        let g = generate(&PaletteOptions { colors: 5, cyclic: false, ..Default::default() });
        let r = reverse(&g);
        assert_eq!(r[0].color, g[4].color);
        assert_eq!(reverse(&r), g);
        assert_eq!(g.len(), 5);
        let close = |a: [f32; 3], b: [f32; 3]| (0..3).all(|k| (a[k] - b[k]).abs() < 1e-5);
        let m = mirror(&g);
        assert!(close(crate::gradient::sample(&m, 0.25), crate::gradient::sample(&g, 0.5)));
        assert!(close(crate::gradient::sample(&m, 0.75), crate::gradient::sample(&g, 0.5)));
        let rep = repeat(&g, 3);
        assert_eq!(rep.len(), 15);
        let c = crate::gradient::sample(&rep, 0.5 / 3.0);
        let want = crate::gradient::sample(&g, 0.5);
        assert!((0..3).all(|k| (c[k] - want[k]).abs() < 1e-5));
        let rot = rotate(&g, 0.25);
        let c = crate::gradient::sample(&rot, 0.5);
        let want = crate::gradient::sample(&g, 0.25);
        assert!((0..3).all(|k| (c[k] - want[k]).abs() < 1e-5), "{c:?} vs {want:?}");
        let same = adjust(&g, Adjust::default());
        assert!(same.iter().zip(&g).all(|(a, b)| (0..3).all(|k| (a.color[k] - b.color[k]).abs() < 1e-4)));
        let gray = adjust(&g, Adjust { saturation: -1.0, ..Default::default() });
        assert!(gray.iter().all(|s| to_oklch(s.color)[1] < 1e-3));
        let sorted = sort_by_lightness(&g);
        assert!(sorted.windows(2).all(|w| to_oklch(w[0].color)[0] <= to_oklch(w[1].color)[0] + 1e-9));
    }
}
