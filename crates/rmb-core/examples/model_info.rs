// SPDX-License-Identifier: LGPL-2.1-or-later

//! Loads and bakes a 3D model and prints its materials and bake statistics:
//! `cargo run --release -p rmb-core --example model_info -- model.glb [resolution]`.

use rmb_core::mesh;

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: model_info <model> [resolution]");
        std::process::exit(2);
    };
    let resolution = args.next().and_then(|r| r.parse().ok()).unwrap_or(128);
    let t = std::time::Instant::now();
    let m = mesh::load(std::path::Path::new(&path), mesh::UpAxis::Auto)
        .unwrap_or_else(|e| panic!("{e}"));
    println!(
        "{} triangles, {} materials, {} textures (load {:.2?})",
        m.triangles.len(),
        m.materials.len(),
        m.textures.len(),
        t.elapsed()
    );
    for (i, mat) in m.materials.iter().enumerate() {
        println!(
            "  {i:3} {:30} color {:?} texture {:?} '{}' bump '{}' spec '{}'",
            mat.name, mat.color, mat.texture, mat.texture_file, mat.bump_file, mat.specular_file
        );
    }
    let t = std::time::Instant::now();
    let v = mesh::bake(
        &m,
        &mesh::ModelOptions {
            resolution,
            ..Default::default()
        },
        &|_| {},
    );
    let leaves = v.nodes.iter().filter(|n| n.count > 0).count();
    println!(
        "bake {:.2?}: grid {:?}, voxel {:.5}, {} nodes ({} leaves)",
        t.elapsed(),
        v.res,
        v.voxel,
        v.nodes.len(),
        leaves
    );
    // Search cost: triangles and nodes visited per near-zone query at random surface-near points.
    let t = std::time::Instant::now();
    let mut n = 0;
    let mut rng = 12345u64;
    let mut next = || {
        rng = rng
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (rng >> 11) as f64 / (1u64 << 53) as f64
    };
    let hi = v.grid_max();
    for _ in 0..200_000 {
        let q: [f64; 3] = std::array::from_fn(|k| v.min[k] + next() * (hi[k] - v.min[k]));
        let d = v.distance(q);
        if d.abs() < 3.0 * v.voxel {
            n += 1;
        }
    }
    println!(
        "200k distance queries ({n} near the surface): {:.2?}",
        t.elapsed()
    );
}
