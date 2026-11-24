// SPDX-License-Identifier: LGPL-2.1-or-later
//! Renders random explore scenes as thumbnails and prints their quality numbers
//! (`explore::thumb_quality`), to tune the explore lane's filter.
//! `cargo run --release -p rmb-gpu --example explore_quality -- <out folder> [count] [seed]`

use rmb_core::explore::{RandomKind, random_framed, thumb_quality};
use rmb_gpu::{GpuContext, GpuRenderer};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = std::path::PathBuf::from(args.first().map_or("explore-quality", |s| s.as_str()));
    let count: u64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(48);
    let seed0: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(1);
    std::fs::create_dir_all(&out).expect("folder");
    let ctx = GpuContext::new_headless().expect("GPU");
    let mut r = GpuRenderer::new(ctx);
    let (w, h) = (192u32, 108u32);
    for k in 0..count {
        let seed = seed0.wrapping_mul(1_000_003).wrapping_add(k * 7919);
        let mut s = random_framed(RandomKind::Any, seed, 6);
        s.march.detail *= h as f64 / s.height.max(1) as f64;
        s.post.denoise = false;
        r.set_scene(&s, w, h, false);
        r.wait_for_pipeline().expect("shader");
        while r.samples() < 3 {
            r.work(std::time::Duration::from_millis(200), 3);
        }
        let half = r.read_rgba8().expect("image");
        while r.samples() < 6 {
            r.work(std::time::Duration::from_millis(200), 6);
        }
        let img = r.read_rgba8().expect("image");
        let (c, n) = thumb_quality(&half, &img);
        println!("{k:3} contrast {c:.3} noise {n:.3}");
        image::save_buffer(out.join(format!("{k:03}_c{:03}_n{:03}.png", (c * 1000.0) as u32, (n * 1000.0) as u32)), &img, w, h, image::ColorType::Rgba8).expect("png");
    }
}
