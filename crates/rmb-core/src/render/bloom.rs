// SPDX-License-Identifier: LGPL-2.1-or-later

//! Bloom: bright parts of the image spread a soft glow over their surroundings, like light
//! scattering in a lens or the eye.
//!
//! The light above the threshold (luminance after exposure) is averaged down into a pyramid
//! of up to `LEVELS` images (a quarter of the image size, then halving), each level is blurred
//! with a 5-tap binomial filter, and the levels are added back with weights that shift toward
//! the coarse ones as the size grows. The glow is added to the radiance before exposure and
//! tone mapping. The GPU preview (rmb-gpu bloom.wgsl, display.wgsl) runs the same steps.
//!
//! Streaks: the light above the streak threshold, averaged like the first level, is smeared
//! along each direction and its opposite with Kawase's streak filter: three passes of four taps
//! `k · step` apart (k = 0..3, the step growing 4× per pass, so the last tap reaches 63
//! steps), weighted by `a^distance` with `a` falling to 1 % over the streak length.

use rayon::prelude::*;

use crate::scene::Post;

/// Levels of the pyramid at most.
pub const LEVELS: usize = 6;

/// Sizes of the pyramid levels for a `w` × `h` image.
pub fn level_sizes(w: u32, h: u32) -> Vec<(u32, u32)> {
    let mut out = vec![(w.div_ceil(4).max(1), h.div_ceil(4).max(1))];
    while out.len() < LEVELS {
        let (pw, ph) = out[out.len() - 1];
        if pw == 1 && ph == 1 {
            break;
        }
        out.push((pw.div_ceil(2).max(1), ph.div_ceil(2).max(1)));
    }
    out
}

/// Weights of the levels: `size^k`, normalized.
pub fn level_weights(size: f64, levels: usize) -> Vec<f32> {
    let s = size.clamp(0.0, 1.0);
    let w: Vec<f64> = (0..levels).map(|k| s.powi(k as i32)).collect();
    let sum: f64 = w.iter().sum::<f64>().max(1e-12);
    w.iter().map(|v| (v / sum) as f32).collect()
}

/// Share of a pixel's light that blooms: the part of its luminance (after `exposure`, a
/// factor) above `threshold`.
pub fn bright_factor(c: [f32; 3], exposure: f32, threshold: f32) -> f32 {
    let y = (0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]) * exposure;
    (y - threshold).max(0.0) / y.max(1e-6)
}

struct Level {
    w: usize,
    h: usize,
    px: Vec<[f32; 3]>,
}

/// Average of `src` (`sw` × `sh`) over the block of `f` × `f` pixels at block (`i`, `j`), the
/// part inside the image.
fn block_mean(src: &[[f32; 3]], sw: usize, sh: usize, i: usize, j: usize, f: usize) -> [f32; 3] {
    let mut s = [0.0f32; 3];
    let mut n = 0.0f32;
    for y in j * f..((j + 1) * f).min(sh) {
        for x in i * f..((i + 1) * f).min(sw) {
            let c = src[y * sw + x];
            for k in 0..3 {
                s[k] += c[k];
            }
            n += 1.0;
        }
    }
    s.map(|v| v / n.max(1.0))
}

/// 5-tap binomial blur along x (`dx`) or y, edges clamped.
fn blur(l: &Level, horizontal: bool) -> Vec<[f32; 3]> {
    const W: [f32; 5] = [1.0 / 16.0, 4.0 / 16.0, 6.0 / 16.0, 4.0 / 16.0, 1.0 / 16.0];
    let mut out = vec![[0.0f32; 3]; l.w * l.h];
    out.par_chunks_mut(l.w).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let mut s = [0.0f32; 3];
            for (t, w) in W.iter().enumerate() {
                let (sx, sy) = if horizontal { ((x as i64 + t as i64 - 2).clamp(0, l.w as i64 - 1) as usize, y) } else { (x, (y as i64 + t as i64 - 2).clamp(0, l.h as i64 - 1) as usize) };
                let c = l.px[sy * l.w + sx];
                for k in 0..3 {
                    s[k] += c[k] * w;
                }
            }
            *o = s;
        }
    });
    out
}

/// Bilinear sample of level `l` at the image position (`u`, `v`) in 0..1 (edges clamped).
fn sample(l: &Level, u: f32, v: f32) -> [f32; 3] {
    let fx = (u * l.w as f32 - 0.5).clamp(0.0, l.w as f32 - 1.0);
    let fy = (v * l.h as f32 - 0.5).clamp(0.0, l.h as f32 - 1.0);
    let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(l.w - 1), (y0 + 1).min(l.h - 1));
    let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
    let p = |x: usize, y: usize| l.px[y * l.w + x];
    let (a, b, c, d) = (p(x0, y0), p(x1, y0), p(x0, y1), p(x1, y1));
    std::array::from_fn(|k| {
        let top = a[k] + (b[k] - a[k]) * tx;
        let bottom = c[k] + (d[k] - c[k]) * tx;
        top + (bottom - top) * ty
    })
}

/// The streak directions as screen vectors (x right, y down): `count` directions spread over
/// half a turn from `angle` (degrees, counterclockwise), each both ways.
pub fn streak_dirs(angle: f64, count: u32) -> Vec<[f32; 2]> {
    let n = count.clamp(1, 4);
    let mut out = Vec::new();
    for i in 0..n {
        let a = (angle + i as f64 * 180.0 / n as f64).to_radians();
        let d = [a.cos() as f32, -a.sin() as f32];
        out.push(d);
        out.push([-d[0], -d[1]]);
    }
    out
}

/// Step of the first streak pass and the falloff per pixel for a streak of `length` (share of
/// the width) on a level `lw` pixels wide.
pub fn streak_params(length: f64, lw: u32) -> (f32, f32) {
    let px = (length * lw as f64).max(1.0);
    ((px / 63.0) as f32, 0.01f64.powf(1.0 / px) as f32)
}

/// Bilinear sample of `l` at texel position (`fx`, `fy`) (texel centers at integers); taps
/// outside the level count as black.
fn sample_zero(l: &Level, fx: f32, fy: f32) -> [f32; 3] {
    let (x0, y0) = (fx.floor(), fy.floor());
    let (tx, ty) = (fx - x0, fy - y0);
    let p = |x: f32, y: f32| if x < 0.0 || y < 0.0 || x >= l.w as f32 || y >= l.h as f32 { [0.0; 3] } else { l.px[y as usize * l.w + x as usize] };
    let (a, b, c, d) = (p(x0, y0), p(x0 + 1.0, y0), p(x0, y0 + 1.0), p(x0 + 1.0, y0 + 1.0));
    std::array::from_fn(|k| {
        let top = a[k] + (b[k] - a[k]) * tx;
        let bottom = c[k] + (d[k] - c[k]) * tx;
        top + (bottom - top) * ty
    })
}

/// Weights of the four taps of a streak pass (normalized).
pub fn streak_weights(step: f32, a: f32) -> [f32; 4] {
    let w: [f32; 4] = std::array::from_fn(|k| a.powf(k as f32 * step));
    let sum: f32 = w.iter().sum();
    w.map(|v| v / sum)
}

/// One streak pass: per texel the weighted sum of four taps `k · step` along `dir`.
fn streak_pass(l: &Level, dir: [f32; 2], step: f32, a: f32) -> Vec<[f32; 3]> {
    let w = streak_weights(step, a);
    let mut out = vec![[0.0f32; 3]; l.w * l.h];
    out.par_chunks_mut(l.w).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let mut s = [0.0f32; 3];
            for (k, wk) in w.iter().enumerate() {
                let off = k as f32 * step;
                let c = sample_zero(l, x as f32 + dir[0] * off, y as f32 + dir[1] * off);
                for i in 0..3 {
                    s[i] += c[i] * wk;
                }
            }
            *o = s;
        }
    });
    out
}

/// The light of `color` above `threshold` (after the `exposure` factor), averaged into the
/// first pyramid level.
fn bright_level(color: &[f32], w: usize, h: usize, exposure: f32, threshold: f32) -> Level {
    let bright: Vec<[f32; 3]> = color
        .as_chunks::<3>()
        .0
        .iter()
        .map(|c| {
            let c = c.map(|v| if v.is_finite() { v.max(0.0) } else { 0.0 });
            let f = bright_factor(c, exposure, threshold);
            c.map(|v| v * f)
        })
        .collect();
    let (lw, lh) = level_sizes(w as u32, h as u32)[0];
    let (lw, lh) = (lw as usize, lh as usize);
    Level { w: lw, h: lh, px: (0..lw * lh).into_par_iter().map(|i| block_mean(&bright, w, h, i % lw, i / lw, 4)).collect() }
}

/// The streaks of `post` on the first pyramid level (None when they are off).
fn streaks(color: &[f32], w: usize, h: usize, post: &Post) -> Option<Level> {
    if post.streak <= 0.0 {
        return None;
    }
    let src = bright_level(color, w, h, 2f32.powf(post.exposure as f32), post.streak_threshold as f32);
    let (step0, a) = streak_params(post.streak_length, src.w as u32);
    let mut acc = vec![[0.0f32; 3]; src.w * src.h];
    for dir in streak_dirs(post.streak_angle, post.streak_count) {
        let mut l = Level { w: src.w, h: src.h, px: src.px.clone() };
        for p in 0..3 {
            l.px = streak_pass(&l, dir, step0 * 4f32.powi(p), a);
        }
        for (o, c) in acc.iter_mut().zip(&l.px) {
            for k in 0..3 {
                o[k] += c[k];
            }
        }
    }
    Some(Level { w: src.w, h: src.h, px: acc })
}

/// Adds the bloom and the streaks of `post` to `color` (RGB per pixel, `w` × `h`). Nothing
/// when both are off.
pub fn apply(color: &mut [f32], w: u32, h: u32, post: &Post) {
    if (post.bloom <= 0.0 && post.streak <= 0.0) || w == 0 || h == 0 {
        return;
    }
    let (w, h) = (w as usize, h as usize);
    let streak = streaks(color, w, h, post);
    let bloom = if post.bloom > 0.0 { bloom_levels(color, w, h, post) } else { Vec::new() };
    let weights = level_weights(post.bloom_size, bloom.len());
    let strength = post.bloom as f32;
    let tint = post.streak_tint.map(|t| t * post.streak as f32);
    color.par_chunks_mut(w * 3).enumerate().for_each(|(y, row)| {
        let v = (y as f32 + 0.5) / h as f32;
        for x in 0..w {
            let u = (x as f32 + 0.5) / w as f32;
            let mut s = [0.0f32; 3];
            for (l, wt) in bloom.iter().zip(&weights) {
                let c = sample(l, u, v);
                for k in 0..3 {
                    s[k] += c[k] * wt * strength;
                }
            }
            if let Some(l) = &streak {
                let c = sample(l, u, v);
                for k in 0..3 {
                    s[k] += c[k] * tint[k];
                }
            }
            for k in 0..3 {
                row[x * 3 + k] += s[k];
            }
        }
    });
}

/// The blurred bloom pyramid of `color`.
fn bloom_levels(color: &[f32], w: usize, h: usize, post: &Post) -> Vec<Level> {
    let first = bright_level(color, w, h, 2f32.powf(post.exposure as f32), post.bloom_threshold as f32);
    let sizes = level_sizes(w as u32, h as u32);
    let mut levels: Vec<Level> = vec![first];
    for &(lw, lh) in &sizes[1..] {
        let (lw, lh) = (lw as usize, lh as usize);
        let p = &levels[levels.len() - 1];
        let px: Vec<[f32; 3]> = (0..lw * lh).into_par_iter().map(|i| block_mean(&p.px, p.w, p.h, i % lw, i / lw, 2)).collect();
        levels.push(Level { w: lw, h: lh, px });
    }
    // Downsampling uses the levels before blurring; then blur each.
    for l in &mut levels {
        let t = Level { w: l.w, h: l.h, px: blur(l, true) };
        l.px = blur(&t, false);
    }
    levels
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bright_dot_spreads() {
        let (w, h) = (64u32, 48u32);
        let mut img = vec![0.0f32; (w * h * 3) as usize];
        let at = |x: u32, y: u32| ((y * w + x) * 3) as usize;
        for k in 0..3 {
            img[at(32, 24) + k] = 50.0;
            img[at(5, 5) + k] = 0.5;
        }
        let post = Post { bloom: 1.0, bloom_threshold: 1.0, bloom_size: 0.6, ..Post::default() };
        let before = img.clone();
        apply(&mut img, w, h, &post);
        // Light around the bright dot, falling off; the dim one (below the threshold) only gets
        // a trace of the bright dot's widest glow.
        assert!(img[at(36, 24)] > 0.05);
        assert!(img[at(36, 24)] > img[at(44, 24)]);
        assert!(img[at(5, 5)] - before[at(5, 5)] < 0.01);
        let mut off = before.clone();
        apply(&mut off, w, h, &Post::default());
        assert_eq!(off, before);
        assert_eq!(level_sizes(64, 48), vec![(16, 12), (8, 6), (4, 3), (2, 2), (1, 1)]);
    }

    #[test]
    fn streaks_run_along_their_direction() {
        let (w, h) = (128u32, 64u32);
        let mut img = vec![0.0f32; (w * h * 3) as usize];
        let at = |x: u32, y: u32| ((y * w + x) * 3) as usize;
        for y in 30..34 {
            for x in 62..66 {
                for k in 0..3 {
                    img[at(x, y) + k] = 200.0;
                }
            }
        }
        let post = Post { streak: 1.0, streak_threshold: 1.0, streak_length: 0.4, streak_count: 1, streak_tint: [1.0; 3], ..Post::default() };
        apply(&mut img, w, h, &post);
        // Light along the horizontal line on both sides, far more than above or below.
        let (right, left, up) = (img[at(90, 32)], img[at(38, 32)], img[at(64, 10)]);
        assert!(right > 0.05 && left > 0.05, "{right} {left}");
        assert!(up < right * 0.05, "{up} {right}");
        assert_eq!(streak_dirs(0.0, 2).len(), 4);
    }
}
