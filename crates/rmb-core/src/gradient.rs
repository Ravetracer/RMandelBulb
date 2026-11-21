// SPDX-License-Identifier: LGPL-2.1-or-later

//! Surface color gradients.

use crate::scene::ColorStop;

/// Entries of the sampled gradient table (CPU and GPU use the same table).
pub const LUT_SIZE: usize = 256;

type Stops = &'static [(f64, [u8; 3])];

const PRESETS: &[(&str, Stops)] = &[
    ("Copper", &[(0.0, [40, 18, 10]), (0.25, [184, 92, 48]), (0.5, [250, 196, 140]), (0.75, [150, 70, 40]), (1.0, [40, 18, 10])]),
    ("Ice", &[(0.0, [20, 40, 70]), (0.3, [90, 160, 210]), (0.55, [235, 245, 255]), (0.8, [70, 120, 180]), (1.0, [20, 40, 70])]),
    ("Bone", &[(0.0, [120, 110, 95]), (0.5, [235, 228, 210]), (1.0, [120, 110, 95])]),
    ("Jade", &[(0.0, [10, 50, 40]), (0.35, [60, 160, 120]), (0.6, [200, 240, 210]), (1.0, [10, 50, 40])]),
    ("Lava", &[(0.0, [30, 5, 5]), (0.3, [180, 30, 10]), (0.5, [255, 140, 20]), (0.65, [255, 235, 150]), (1.0, [30, 5, 5])]),
    ("Gold", &[(0.0, [70, 45, 10]), (0.4, [220, 170, 60]), (0.6, [255, 240, 180]), (1.0, [70, 45, 10])]),
    ("Spectrum", &[(0.0, [230, 60, 60]), (0.17, [230, 200, 60]), (0.33, [80, 220, 80]), (0.5, [60, 200, 220]), (0.67, [70, 90, 230]), (0.83, [210, 70, 220]), (1.0, [230, 60, 60])]),
    ("Slate", &[(0.0, [60, 64, 72]), (0.5, [170, 176, 186]), (1.0, [60, 64, 72])]),
    ("White", &[(0.0, [235, 235, 235]), (1.0, [235, 235, 235])]),
];

pub fn preset_names() -> impl Iterator<Item = &'static str> {
    PRESETS.iter().map(|(n, _)| *n)
}

pub fn preset(name: &str) -> Option<Vec<ColorStop>> {
    PRESETS.iter().find(|(n, _)| *n == name).map(|(_, s)| {
        s.iter().map(|(p, c)| ColorStop::new(*p, [srgb_to_linear(c[0]), srgb_to_linear(c[1]), srgb_to_linear(c[2])])).collect()
    })
}

pub fn srgb_to_linear(c: u8) -> f32 {
    let v = c as f32 / 255.0;
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

pub fn linear_to_srgb(v: f32) -> u8 {
    let v = v.clamp(0.0, 1.0);
    let s = if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
    (s * 255.0 + 0.5) as u8
}

/// Linear color at `t` (0..1) of a gradient.
pub fn sample(stops: &[ColorStop], t: f64) -> [f32; 3] {
    sample_by(stops, t, [0.7; 3], |s| s.color)
}

/// Highlight color at `t` (0..1) of a gradient.
pub fn sample_specular(stops: &[ColorStop], t: f64) -> [f32; 3] {
    sample_by(stops, t, [1.0; 3], |s| s.specular)
}

fn sample_by(stops: &[ColorStop], t: f64, empty: [f32; 3], get: impl Fn(&ColorStop) -> [f32; 3]) -> [f32; 3] {
    match stops.len() {
        0 => return empty,
        1 => return get(&stops[0]),
        _ => {}
    }
    let mut sorted: Vec<&ColorStop> = stops.iter().collect();
    sorted.sort_by(|a, b| a.pos.total_cmp(&b.pos));
    if t <= sorted[0].pos {
        return get(sorted[0]);
    }
    for w in sorted.windows(2) {
        if t <= w[1].pos {
            let span = (w[1].pos - w[0].pos).max(1e-9);
            let f = ((t - w[0].pos) / span) as f32;
            let (a, b) = (get(w[0]), get(w[1]));
            return [0, 1, 2].map(|k| a[k] + (b[k] - a[k]) * f);
        }
    }
    get(sorted[sorted.len() - 1])
}

/// The gradient sampled into `LUT_SIZE` linear RGB entries.
pub fn lut(stops: &[ColorStop]) -> Vec<[f32; 3]> {
    (0..LUT_SIZE).map(|i| sample(stops, i as f64 / (LUT_SIZE - 1) as f64)).collect()
}

/// Transparency at `t` (0..1) of a gradient.
pub fn sample_transparency(stops: &[ColorStop], t: f64) -> f32 {
    sample_by(stops, t, [0.0; 3], |s| [s.transparency; 3])[0]
}

/// The transparencies sampled like [`lut`].
pub fn lut_transparency(stops: &[ColorStop]) -> Vec<f32> {
    (0..LUT_SIZE).map(|i| sample_transparency(stops, i as f64 / (LUT_SIZE - 1) as f64)).collect()
}

/// The highlight colors sampled like [`lut`].
pub fn lut_specular(stops: &[ColorStop]) -> Vec<[f32; 3]> {
    (0..LUT_SIZE).map(|i| sample_specular(stops, i as f64 / (LUT_SIZE - 1) as f64)).collect()
}
