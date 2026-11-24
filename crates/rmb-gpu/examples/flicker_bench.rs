// SPDX-License-Identifier: LGPL-2.1-or-later
//! Measures the noise of animations: renders an animation's first frames at a sample count, raw
//! and denoised, and at four times the samples, and compares them with a reference at 512
//! samples. Error: mean difference to the reference; flicker: mean difference of the
//! frame-to-frame changes (what shimmers when the movie plays). Used to judge temporal
//! smoothing (measured and dropped, see TODO.md › Performance).
//!
//! `cargo run --release -p rmb-gpu --example flicker_bench -- scene.rmb [frames] [samples]`

use std::sync::atomic::AtomicBool;

use rmb_core::animation::AnimationPlan;

fn frames(scene: &rmb_core::Scene, n: usize, w: u32, h: u32, samples: u32, denoise: bool) -> Vec<Vec<f32>> {
    let anim = scene.animation.clone().expect("the scene needs an animation");
    let plan = AnimationPlan::new(&anim).expect("plan");
    let fs = |t: f64| {
        let mut s = plan.scene_at_time(t);
        s.post.denoise = denoise;
        s.post.denoise_blend = 1.0;
        s
    };
    let ctx = rmb_gpu::GpuContext::new_headless().expect("GPU");
    let mut r = rmb_gpu::FrameRenderer::new(Some(ctx), 0..n, w, h, samples, 0.0, &fs);
    let cancel = AtomicBool::new(false);
    (0..n).map(|f| r.render(f, &fs, &cancel, &mut |_| {}).expect("render").iter().map(|v| *v as f32 / 255.0).collect()).collect()
}

/// Mean difference to the reference and mean difference of the frame-to-frame changes.
fn score(a: &[Vec<f32>], r: &[Vec<f32>]) -> (f64, f64) {
    let mut err = 0.0;
    let mut flicker = 0.0;
    let px = r[0].len() as f64;
    for f in 0..r.len() {
        err += a[f].iter().zip(&r[f]).map(|(x, y)| (x - y).abs() as f64).sum::<f64>() / px;
        if f > 0 {
            flicker += (0..r[f].len()).map(|i| ((a[f][i] - a[f - 1][i]) - (r[f][i] - r[f - 1][i])).abs() as f64).sum::<f64>() / px;
        }
    }
    (err / r.len() as f64 * 255.0, flicker / (r.len() - 1) as f64 * 255.0)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let scene = rmb_core::io::load(std::path::Path::new(&args[0])).expect("scene");
    let n: usize = args.get(1).and_then(|v| v.parse().ok()).unwrap_or(12);
    let samples: u32 = args.get(2).and_then(|v| v.parse().ok()).unwrap_or(16);
    let (w, h) = (640, 360);
    let reference = frames(&scene, n, w, h, 512, false);
    println!("{n} frames, {w} x {h}; error and flicker in levels of 255 against 512 samples");
    for (label, s, dn) in [("raw", samples, false), ("denoised", samples, true), ("raw x4 samples", samples * 4, false), ("denoised x4 samples", samples * 4, true)] {
        let (e, fl) = score(&frames(&scene, n, w, h, s, dn), &reference);
        println!("{label:<26} {s:>4} samples   error {e:6.2}   flicker {fl:6.2}");
    }
}
