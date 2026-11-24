// SPDX-License-Identifier: LGPL-2.1-or-later
//! Voxel export: the grid of the mesh export (`mesh_export::Grid`, `Field`) as voxels, written
//! as a stack of PNG images (one per z layer, like Mandelbulb3D's voxel export) or as one
//! MagicaVoxel `.vox` model (at most 256 voxels per side, colors in a 255-entry palette).
//!
//! A voxel is filled where the distance estimate at its center is below the mesh export's
//! thickness (the same surface as the mesh). Hollow exports keep only the filled voxels next
//! to an empty one (the visible shell).

use std::path::{Path, PathBuf};

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::Program;
use crate::math::{V3, v3};
use crate::mesh_export::{CpuField, Field, Grid, MeshSettings};
use crate::scene::Scene;

/// Largest model side of a MagicaVoxel file.
pub const VOX_MAX: u32 = 256;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum VoxelFormat {
    /// A folder of PNG images, one per z layer (bottom first): white on black, or colors with
    /// transparency.
    #[default]
    PngStack,
    /// One MagicaVoxel model (`.vox`).
    MagicaVoxel,
}

impl VoxelFormat {
    pub const ALL: [VoxelFormat; 2] = [VoxelFormat::PngStack, VoxelFormat::MagicaVoxel];

    pub fn label(self) -> &'static str {
        match self {
            VoxelFormat::PngStack => "PNG slices (folder)",
            VoxelFormat::MagicaVoxel => "MagicaVoxel (.vox)",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VoxelSettings {
    pub format: VoxelFormat,
    /// Colors from the scene's coloring (else white).
    pub colors: bool,
    /// Only the voxels next to empty space.
    pub hollow: bool,
}

impl Default for VoxelSettings {
    fn default() -> Self {
        VoxelSettings { format: VoxelFormat::PngStack, colors: true, hollow: false }
    }
}

/// The mesh settings with the resolution limited for the format (MagicaVoxel: 256).
pub fn limited(mesh: &MeshSettings, vox: &VoxelSettings) -> MeshSettings {
    let mut m = mesh.clone();
    if vox.format == VoxelFormat::MagicaVoxel {
        // Grid samples are the voxels: one more sample than cells per side.
        m.resolution = m.resolution.min(VOX_MAX - 1);
    }
    m
}

/// One z layer of the result: filled voxels (x fastest) and their colors (sRGB).
struct Layer {
    filled: Vec<bool>,
    colors: Vec<[u8; 3]>,
}

/// Exports the voxels of `scene` in the box and grid of `mesh`. PNG stacks go into the folder
/// `path` (created), MagicaVoxel models into the file `path`. Returns the number of voxels
/// written and the files.
pub fn export(scene: &Scene, mesh: &MeshSettings, vox: &VoxelSettings, field: Option<&mut dyn Field>, path: &Path, progress: &mut dyn FnMut(f32) -> bool) -> Result<(usize, Vec<PathBuf>), String> {
    let mesh = limited(mesh, vox);
    let driven = crate::animation::still_scene(scene);
    let scene = driven.as_ref().unwrap_or(scene);
    let prog = Program::for_scene(scene);
    let grid = Grid::new(&mesh);
    let iso = (mesh.thickness.max(0.0) * grid.step) as f32;
    let mut cpu = CpuField { prog: &prog };
    let field: &mut dyn Field = match field {
        Some(f) => f,
        None => &mut cpu,
    };
    let [nx, ny, nz] = grid.n;
    let sc = crate::render::shading_scene(scene);
    let shader = crate::render::cpu::Shader::new(&sc, &prog, scene.width.max(1), scene.height.max(1));
    let display = scene.post.display_space;
    let mut raw = vec![0f32; nx * ny];
    let mut slice = |z: usize, raw: &mut Vec<f32>| -> Result<Vec<bool>, String> {
        if z >= nz {
            return Ok(vec![false; nx * ny]);
        }
        field.slice(&grid, z, raw)?;
        Ok(raw.iter().map(|v| *v - iso < 0.0).collect())
    };
    // Rolling layers below, at and above z (hollow needs the neighbors).
    let mut below = vec![false; nx * ny];
    let mut here = slice(0, &mut raw)?;
    let mut writer = Writer::new(vox, path, nx, ny)?;
    let mut count = 0;
    for z in 0..nz {
        let above = slice(z + 1, &mut raw)?;
        // Filled voxels next to an empty one (or the box) form the shell.
        let shell: Vec<bool> = (0..nx * ny)
            .map(|i| {
                if !here[i] {
                    return false;
                }
                let (x, y) = (i % nx, i / nx);
                let empty = |x: isize, y: isize| x < 0 || y < 0 || x >= nx as isize || y >= ny as isize || !here[y as usize * nx + x as usize];
                let (xi, yi) = (x as isize, y as isize);
                empty(xi - 1, yi) || empty(xi + 1, yi) || empty(xi, yi - 1) || empty(xi, yi + 1) || !below[i] || !above[i]
            })
            .collect();
        let keep = if vox.hollow { shell.clone() } else { here.clone() };
        let colors: Vec<[u8; 3]> = if vox.colors {
            let mut colors: Vec<[u8; 3]> = shell
                .par_iter()
                .enumerate()
                .map_init(
                    || prog.evaluator(),
                    |ev, (i, sh)| {
                        if !*sh {
                            return [0; 3];
                        }
                        let p = grid.origin + v3((i % nx) as f64, (i / nx) as f64, z as f64) * grid.step;
                        let n = gradient(ev, p, grid.step);
                        let c = shader.surface_base(ev, p, n);
                        c.map(|v| {
                            let v = if display { v } else { crate::render::srgb_encode(v) };
                            (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
                        })
                    },
                )
                .collect();
            // Interior voxels (never visible from outside) get the layer's mean shell color.
            let (mut sum, mut n) = ([0u64; 3], 0u64);
            for (c, _) in colors.iter().zip(&shell).filter(|(_, s)| **s) {
                for k in 0..3 {
                    sum[k] += c[k] as u64;
                }
                n += 1;
            }
            let mean = if n > 0 { sum.map(|v| ((v + n / 2) / n) as u8) } else { [255; 3] };
            for (c, (k, sh)) in colors.iter_mut().zip(keep.iter().zip(&shell)) {
                if *k && !*sh {
                    *c = mean;
                }
            }
            colors
        } else {
            vec![[255; 3]; nx * ny]
        };
        count += keep.iter().filter(|k| **k).count();
        writer.layer(z, Layer { filled: keep, colors })?;
        below = std::mem::replace(&mut here, above);
        if !progress((z + 1) as f32 / nz as f32 * 0.98) {
            return Err("cancelled".into());
        }
    }
    let files = writer.finish(nz)?;
    progress(1.0);
    Ok((count, files))
}

/// Unit gradient of the distance estimate at `p` (for the color modes that use the normal).
fn gradient(ev: &mut crate::fractal::Evaluator, p: V3, step: f64) -> V3 {
    let h = step * 0.5;
    let k = [v3(1.0, -1.0, -1.0), v3(-1.0, -1.0, 1.0), v3(-1.0, 1.0, -1.0), v3(1.0, 1.0, 1.0)];
    let mut g = V3::ZERO;
    for kk in k {
        g += kk * ev.de(p + kk * h, h);
    }
    if g.length() > 1e-30 { g.normalize() } else { v3(0.0, 0.0, 1.0) }
}

struct Writer {
    format: VoxelFormat,
    path: PathBuf,
    nx: usize,
    ny: usize,
    colors: bool,
    files: Vec<PathBuf>,
    /// MagicaVoxel: position and color of every voxel.
    voxels: Vec<([u8; 3], [u8; 3])>,
}

impl Writer {
    fn new(vox: &VoxelSettings, path: &Path, nx: usize, ny: usize) -> Result<Writer, String> {
        if vox.format == VoxelFormat::PngStack {
            std::fs::create_dir_all(path).map_err(|e| format!("{}: {e}", path.display()))?;
        }
        Ok(Writer { format: vox.format, path: path.to_path_buf(), nx, ny, colors: vox.colors, files: Vec::new(), voxels: Vec::new() })
    }

    fn layer(&mut self, z: usize, l: Layer) -> Result<(), String> {
        let (nx, ny) = (self.nx, self.ny);
        match self.format {
            VoxelFormat::PngStack => {
                // Seen from above: x to the right, y up (the image's first row is the largest y).
                let file = self.path.join(format!("slice_{z:04}.png"));
                let res = if self.colors {
                    let mut px = vec![0u8; nx * ny * 4];
                    for y in 0..ny {
                        for x in 0..nx {
                            let i = y * nx + x;
                            if l.filled[i] {
                                let o = ((ny - 1 - y) * nx + x) * 4;
                                px[o..o + 3].copy_from_slice(&l.colors[i]);
                                px[o + 3] = 255;
                            }
                        }
                    }
                    image::save_buffer(&file, &px, nx as u32, ny as u32, image::ExtendedColorType::Rgba8)
                } else {
                    let mut px = vec![0u8; nx * ny];
                    for y in 0..ny {
                        for x in 0..nx {
                            if l.filled[y * nx + x] {
                                px[(ny - 1 - y) * nx + x] = 255;
                            }
                        }
                    }
                    image::save_buffer(&file, &px, nx as u32, ny as u32, image::ExtendedColorType::L8)
                };
                res.map_err(|e| format!("{}: {e}", file.display()))?;
                self.files.push(file);
            }
            VoxelFormat::MagicaVoxel => {
                for (i, f) in l.filled.iter().enumerate() {
                    if *f {
                        self.voxels.push(([(i % nx) as u8, (i / nx) as u8, z as u8], l.colors[i]));
                    }
                }
            }
        }
        Ok(())
    }

    fn finish(self, nz: usize) -> Result<Vec<PathBuf>, String> {
        match self.format {
            VoxelFormat::PngStack => Ok(self.files),
            VoxelFormat::MagicaVoxel => {
                write_vox(&self.path, [self.nx, self.ny, nz], &self.voxels).map_err(|e| format!("{}: {e}", self.path.display()))?;
                Ok(vec![self.path])
            }
        }
    }
}

/// Up to 255 colors for `colors` (median cut on the colors weighted by their counts) and the
/// palette index (0-based) of every color.
pub fn palette(colors: &[[u8; 3]]) -> (Vec<[u8; 3]>, Vec<u8>) {
    let mut counts: std::collections::HashMap<[u8; 3], u32> = std::collections::HashMap::new();
    for c in colors {
        *counts.entry(*c).or_insert(0) += 1;
    }
    let all: Vec<([u8; 3], u32)> = counts.into_iter().collect();
    // Boxes of color entries; split the one with the widest channel range (weighted by count)
    // at its weighted median until there are 255.
    let mut boxes: Vec<Vec<([u8; 3], u32)>> = vec![all];
    while boxes.len() < 255 {
        let range = |b: &Vec<([u8; 3], u32)>| -> (u32, usize) {
            let mut best = (0, 0);
            for ch in 0..3 {
                let lo = b.iter().map(|e| e.0[ch]).min().unwrap_or(0);
                let hi = b.iter().map(|e| e.0[ch]).max().unwrap_or(0);
                let r = (hi - lo) as u32;
                if r > best.0 {
                    best = (r, ch);
                }
            }
            best
        };
        let Some((k, (_, ch))) = boxes.iter().enumerate().filter(|(_, b)| b.len() > 1).map(|(k, b)| (k, range(b))).max_by_key(|(k, (r, _))| (*r as u64) * boxes[*k].iter().map(|e| e.1 as u64).sum::<u64>().max(1).ilog2() as u64) else { break };
        let mut b = boxes.swap_remove(k);
        b.sort_by_key(|e| e.0[ch]);
        let total: u64 = b.iter().map(|e| e.1 as u64).sum();
        let mut acc = 0;
        let mut cut = 1;
        for (i, e) in b.iter().enumerate() {
            acc += e.1 as u64;
            if acc * 2 >= total {
                cut = (i + 1).clamp(1, b.len() - 1);
                break;
            }
        }
        let rest = b.split_off(cut);
        boxes.push(b);
        boxes.push(rest);
    }
    let mut pal = Vec::with_capacity(boxes.len());
    let mut index: std::collections::HashMap<[u8; 3], u8> = std::collections::HashMap::new();
    for (k, b) in boxes.iter().enumerate() {
        let total: u64 = b.iter().map(|e| e.1 as u64).sum::<u64>().max(1);
        let mean = [0, 1, 2].map(|ch| ((b.iter().map(|e| e.0[ch] as u64 * e.1 as u64).sum::<u64>() + total / 2) / total) as u8);
        pal.push(mean);
        for e in b {
            index.insert(e.0, k as u8);
        }
    }
    let idx = colors.iter().map(|c| index[c]).collect();
    (pal, idx)
}

/// Writes a MagicaVoxel file (version 150: SIZE, XYZI, RGBA chunks; z up like the scene).
fn write_vox(path: &Path, n: [usize; 3], voxels: &[([u8; 3], [u8; 3])]) -> std::io::Result<()> {
    let colors: Vec<[u8; 3]> = voxels.iter().map(|v| v.1).collect();
    let (pal, idx) = palette(&colors);
    let mut out = Vec::with_capacity(voxels.len() * 4 + 2048);
    let chunk = |out: &mut Vec<u8>, id: &[u8; 4], content: &[u8], children: &[u8]| {
        out.extend_from_slice(id);
        out.extend_from_slice(&(content.len() as u32).to_le_bytes());
        out.extend_from_slice(&(children.len() as u32).to_le_bytes());
        out.extend_from_slice(content);
        out.extend_from_slice(children);
    };
    let mut size = Vec::new();
    for v in n {
        size.extend_from_slice(&(v.min(VOX_MAX as usize) as u32).to_le_bytes());
    }
    let mut xyzi = Vec::with_capacity(4 + voxels.len() * 4);
    xyzi.extend_from_slice(&(voxels.len() as u32).to_le_bytes());
    for (v, i) in voxels.iter().zip(&idx) {
        // Palette index 0 is empty: colors are 1..=255.
        xyzi.extend_from_slice(&[v.0[0], v.0[1], v.0[2], i + 1]);
    }
    let mut rgba = Vec::with_capacity(1024);
    for k in 0..256 {
        let c = pal.get(k).copied().unwrap_or([0; 3]);
        rgba.extend_from_slice(&[c[0], c[1], c[2], 255]);
    }
    let mut children = Vec::new();
    chunk(&mut children, b"SIZE", &size, &[]);
    chunk(&mut children, b"XYZI", &xyzi, &[]);
    chunk(&mut children, b"RGBA", &rgba, &[]);
    out.extend_from_slice(b"VOX ");
    out.extend_from_slice(&150u32.to_le_bytes());
    chunk(&mut out, b"MAIN", &[], &children);
    std::fs::write(path, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_keeps_few_colors_exact_and_reduces_many() {
        let few = vec![[255, 0, 0], [0, 255, 0], [255, 0, 0], [0, 0, 255]];
        let (pal, idx) = palette(&few);
        assert_eq!(pal.len(), 3);
        for (c, i) in few.iter().zip(&idx) {
            assert_eq!(pal[*i as usize], *c);
        }
        let many: Vec<[u8; 3]> = (0..20000u32).map(|i| [(i * 7) as u8, (i * 13 / 3) as u8, (i / 80) as u8]).collect();
        let (pal, idx) = palette(&many);
        assert!(pal.len() <= 255);
        let err: f64 = many.iter().zip(&idx).map(|(c, i)| (0..3).map(|k| (c[k] as f64 - pal[*i as usize][k] as f64).abs()).sum::<f64>()).sum::<f64>() / many.len() as f64;
        assert!(err < 40.0, "mean error {err}");
    }

    #[test]
    fn mandelbulb_voxels_as_png_stack_and_vox() {
        let scene = Scene::default();
        let dir = std::env::temp_dir().join(format!("rmb-vox-{}", std::process::id()));
        let mesh = MeshSettings { resolution: 32, ..MeshSettings::default() };
        let full = VoxelSettings { format: VoxelFormat::PngStack, colors: false, hollow: false };
        let (n_full, files) = export(&scene, &mesh, &full, None, &dir.join("slices"), &mut |_| true).unwrap();
        assert_eq!(files.len(), Grid::new(&mesh).n[2]);
        let img = image::open(&files[files.len() / 2]).unwrap().to_luma8();
        assert!(img.pixels().any(|p| p.0[0] == 255) && img.pixels().any(|p| p.0[0] == 0));
        let hollow = VoxelSettings { format: VoxelFormat::MagicaVoxel, colors: true, hollow: true };
        let path = dir.join("bulb.vox");
        let (n_hollow, _) = export(&scene, &mesh, &hollow, None, &path, &mut |_| true).unwrap();
        assert!(n_hollow > 1000 && n_hollow < n_full, "{n_hollow} of {n_full}");
        let data = std::fs::read(&path).unwrap();
        assert_eq!(&data[..4], b"VOX ");
        // Header, MAIN, SIZE, XYZI (count + 4 bytes per voxel), RGBA.
        assert_eq!(data.len(), 8 + 12 + (12 + 12) + (12 + 4 + 4 * n_hollow) + (12 + 1024));
        std::fs::remove_dir_all(&dir).ok();
    }
}
