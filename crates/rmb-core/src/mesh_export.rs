// SPDX-License-Identifier: LGPL-2.1-or-later
//! Mesh export: the scene's distance estimate sampled on a grid, turned into a closed triangle
//! mesh by surface nets (one vertex per grid cell the surface passes through, one quad per grid
//! edge it crosses), with vertex normals and colors, written as PLY, OBJ or STL.
//!
//! The grid is processed one slice at a time, so memory grows with the mesh, not with the grid.
//! Samples outside the box count as empty space: where the fractal reaches the box, the mesh is
//! closed by a cap on the box face (watertight for 3D printing).

use std::io::Write;

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::Program;
use crate::math::{V3, v3};
use crate::scene::Scene;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum MeshFormat {
    /// Binary PLY with normals and vertex colors (Blender, MeshLab, most 3D tools).
    #[default]
    Ply,
    /// Wavefront OBJ with normals; vertex colors as the common `v x y z r g b` extension.
    Obj,
    /// Binary STL (3D printing; no colors).
    Stl,
}

impl MeshFormat {
    pub const ALL: [MeshFormat; 3] = [MeshFormat::Ply, MeshFormat::Obj, MeshFormat::Stl];

    pub fn label(self) -> &'static str {
        match self {
            MeshFormat::Ply => "PLY (colors)",
            MeshFormat::Obj => "OBJ",
            MeshFormat::Stl => "STL (3D printing)",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            MeshFormat::Ply => "ply",
            MeshFormat::Obj => "obj",
            MeshFormat::Stl => "stl",
        }
    }

    /// The format of a file name's extension.
    pub fn of_path(path: &std::path::Path) -> Option<MeshFormat> {
        let e = path.extension()?.to_str()?.to_ascii_lowercase();
        MeshFormat::ALL.into_iter().find(|f| f.extension() == e)
    }
}

/// What to export.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MeshSettings {
    /// Corners of the box that is sampled (world units).
    pub min: V3,
    pub max: V3,
    /// Grid cells along the longest side of the box.
    pub resolution: u32,
    /// The surface lies this many grid cells outside the fractal: 0.5 keeps the shape, larger
    /// values thicken thin parts (3D printing).
    pub thickness: f64,
    /// Vertex colors from the scene's coloring.
    pub colors: bool,
    /// Moves the vertices onto the surface after the grid gives them (sharper, slower).
    pub refine: bool,
    /// Writes y up instead of z up (x, z, -y).
    pub y_up: bool,
    pub format: MeshFormat,
    /// Removes loose pieces with fewer triangles than this share of the whole mesh (percent;
    /// 0 keeps everything): the dust around fractal surfaces.
    pub min_piece: f64,
    /// Keeps this share of the triangles (percent; 100 = all) by merging vertices where the
    /// surface is flat (`mesh_simplify::decimate`).
    pub reduce: f64,
}

impl Default for MeshSettings {
    fn default() -> Self {
        MeshSettings { min: v3(-1.5, -1.5, -1.5), max: v3(1.5, 1.5, 1.5), resolution: 256, thickness: 0.5, colors: true, refine: true, y_up: false, format: MeshFormat::Ply, min_piece: 0.0, reduce: 100.0 }
    }
}

/// Sample grid of a box: `n` samples per axis, `step` apart, starting at `origin`.
#[derive(Clone, Copy, Debug)]
pub struct Grid {
    pub origin: V3,
    pub step: f64,
    pub n: [usize; 3],
}

impl Grid {
    pub fn new(s: &MeshSettings) -> Grid {
        let lo = v3(s.min.x.min(s.max.x), s.min.y.min(s.max.y), s.min.z.min(s.max.z));
        let hi = v3(s.min.x.max(s.max.x), s.min.y.max(s.max.y), s.min.z.max(s.max.z));
        let ext = hi - lo;
        let longest = ext.x.max(ext.y).max(ext.z).max(1e-300);
        let step = longest / s.resolution.clamp(4, 4096) as f64;
        let count = |e: f64| ((e / step).ceil() as usize).max(1) + 1;
        let n = [count(ext.x), count(ext.y), count(ext.z)];
        // Centered in the box.
        let used = v3((n[0] - 1) as f64, (n[1] - 1) as f64, (n[2] - 1) as f64) * step;
        Grid { origin: lo + (ext - used) * 0.5, step, n }
    }

    pub fn samples(&self) -> u64 {
        self.n.iter().map(|&v| v as u64).product()
    }
}

/// Where the distances come from (CPU here; the GPU crate has its own).
pub trait Field {
    /// Distance estimates of the grid's slice `z` (`n[0] * n[1]` values, x fastest).
    fn slice(&mut self, grid: &Grid, z: usize, out: &mut [f32]) -> Result<(), String>;
}

/// Distances from the CPU evaluator (all cores).
pub struct CpuField<'a> {
    pub prog: &'a Program,
}

impl Field for CpuField<'_> {
    fn slice(&mut self, g: &Grid, z: usize, out: &mut [f32]) -> Result<(), String> {
        let nx = g.n[0];
        out.par_chunks_mut(nx).enumerate().for_each_init(
            || self.prog.evaluator(),
            |ev, (y, row)| {
                for (x, v) in row.iter_mut().enumerate() {
                    let p = g.origin + v3(x as f64, y as f64, z as f64) * g.step;
                    *v = ev.de(p, g.step * 0.5) as f32;
                }
            },
        );
        Ok(())
    }
}

/// A triangle mesh.
#[derive(Clone, Debug, Default)]
pub struct Mesh {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    /// sRGB, empty without colors.
    pub colors: Vec<[u8; 3]>,
    pub triangles: Vec<[u32; 3]>,
}

/// Builds the mesh of `field` on `grid`; the surface lies where the distance equals `iso`.
/// `progress` gets the share done (0..1) and returns false to cancel (error "cancelled").
pub fn surface_nets(field: &mut dyn Field, grid: &Grid, iso: f32, progress: &mut dyn FnMut(f32) -> bool) -> Result<Mesh, String> {
    let [nx, ny, nz] = grid.n;
    // Padded grid: one empty sample around the box, so every surface is closed.
    let (px, py) = (nx + 2, ny + 2);
    let (cx, cy) = (px - 1, py - 1);
    let mut raw = vec![0f32; nx * ny];
    let empty = vec![f32::INFINITY; px * py];
    let mut prev = empty.clone();
    let mut cur = empty.clone();
    let mut cells_prev = vec![u32::MAX; cx * cy];
    let mut cells_cur = vec![u32::MAX; cx * cy];
    let mut mesh = Mesh::default();
    let fill = |field: &mut dyn Field, raw: &mut [f32], out: &mut [f32], z: usize| -> Result<(), String> {
        // Padded slice z (1..=nz) holds grid slice z - 1.
        field.slice(grid, z - 1, raw)?;
        for y in 0..ny {
            for x in 0..nx {
                // Not a number (a formula failing) counts as empty space.
                let v = raw[y * nx + x];
                out[(y + 1) * px + x + 1] = if v.is_nan() { f32::INFINITY } else { v - iso };
            }
        }
        Ok(())
    };
    // Planes 0 and nz + 1 of the padded grid are empty; cell layer k (0..=nz) lies between
    // planes k (prev) and k + 1 (cur).
    fill(field, &mut raw, &mut cur, 1)?;
    for k in 0..=nz {
        if k > 0 {
            std::mem::swap(&mut prev, &mut cur);
            if k < nz {
                fill(field, &mut raw, &mut cur, k + 1)?;
            } else {
                cur.copy_from_slice(&empty);
            }
        }
        std::mem::swap(&mut cells_prev, &mut cells_cur);
        cells_cur.fill(u32::MAX);
        let at = |plane: &[f32], x: usize, y: usize| plane[y * px + x];
        // Vertices of the cells of layer k.
        for y in 0..cy {
            for x in 0..cx {
                let c = [
                    at(&prev, x, y),
                    at(&prev, x + 1, y),
                    at(&prev, x, y + 1),
                    at(&prev, x + 1, y + 1),
                    at(&cur, x, y),
                    at(&cur, x + 1, y),
                    at(&cur, x, y + 1),
                    at(&cur, x + 1, y + 1),
                ];
                let inside = c.iter().filter(|v| **v < 0.0).count();
                if inside == 0 || inside == 8 {
                    continue;
                }
                // Mean of the points where the edges cross the surface.
                const EDGES: [(usize, usize); 12] = [(0, 1), (2, 3), (4, 5), (6, 7), (0, 2), (1, 3), (4, 6), (5, 7), (0, 4), (1, 5), (2, 6), (3, 7)];
                let corner = |i: usize| v3((i & 1) as f64, ((i >> 1) & 1) as f64, ((i >> 2) & 1) as f64);
                let (mut sum, mut n) = (V3::ZERO, 0.0);
                for (a, b) in EDGES {
                    let (va, vb) = (c[a], c[b]);
                    if (va < 0.0) != (vb < 0.0) {
                        // Against the empty padding the crossing lies on the box face.
                        let t = if va.is_finite() && vb.is_finite() { (va / (va - vb)) as f64 } else if va.is_finite() { 0.0 } else { 1.0 };
                        sum += corner(a) + (corner(b) - corner(a)) * t.clamp(0.0, 1.0);
                        n += 1.0;
                    }
                }
                let local = sum * (1.0 / n);
                // Padded sample (x, y, k) is grid sample (x - 1, y - 1, k - 1).
                let p = grid.origin + (v3(x as f64, y as f64, k as f64) + local - v3(1.0, 1.0, 1.0)) * grid.step;
                cells_cur[y * cx + x] = mesh.positions.len() as u32;
                mesh.positions.push([p.x as f32, p.y as f32, p.z as f32]);
            }
        }
        // Quads around the edges that cross the surface. Edges along z between planes k and
        // k + 1 touch cells of layer k only; edges along x and y in plane k touch layers k - 1
        // and k.
        let quad = |q: [u32; 4], flip: bool, mesh: &mut Mesh| {
            if q.contains(&u32::MAX) {
                return;
            }
            let q = if flip { [q[0], q[3], q[2], q[1]] } else { q };
            let p = |i: u32| {
                let a = mesh.positions[i as usize];
                v3(a[0] as f64, a[1] as f64, a[2] as f64)
            };
            // The shorter diagonal splits the quad.
            if (p(q[0]) - p(q[2])).length() <= (p(q[1]) - p(q[3])).length() {
                mesh.triangles.push([q[0], q[1], q[2]]);
                mesh.triangles.push([q[0], q[2], q[3]]);
            } else {
                mesh.triangles.push([q[0], q[1], q[3]]);
                mesh.triangles.push([q[1], q[2], q[3]]);
            }
        };
        // Edges from the padding into the box count too (the caps on the box faces).
        for y in 0..py - 1 {
            for x in 0..px - 1 {
                let a = at(&prev, x, y);
                let inner = x >= 1 && y >= 1;
                let b = at(&cur, x, y);
                if inner && (a < 0.0) != (b < 0.0) {
                    let cell = |cx_: usize, cy_: usize| cells_cur[cy_ * cx + cx_];
                    // Counter-clockwise seen from +z when the inside is below.
                    quad([cell(x - 1, y - 1), cell(x, y - 1), cell(x, y), cell(x - 1, y)], b < 0.0, &mut mesh);
                }
                if k == 0 {
                    continue;
                }
                let (lo, hi) = (&cells_prev, &cells_cur);
                let bx = at(&prev, x + 1, y);
                if y >= 1 && (a < 0.0) != (bx < 0.0) {
                    // Cells (x, y - 1..y, layers k - 1..k); counter-clockwise seen from +x.
                    quad([lo[(y - 1) * cx + x], lo[y * cx + x], hi[y * cx + x], hi[(y - 1) * cx + x]], bx < 0.0, &mut mesh);
                }
                let by = at(&prev, x, y + 1);
                if x >= 1 && (a < 0.0) != (by < 0.0) {
                    // Cells (x - 1..x, y, layers k - 1..k); counter-clockwise seen from +y.
                    quad([lo[y * cx + x - 1], hi[y * cx + x - 1], hi[y * cx + x], lo[y * cx + x]], by < 0.0, &mut mesh);
                }
            }
        }
        if !progress((k + 1) as f32 / (nz + 1) as f32) {
            return Err("cancelled".into());
        }
    }
    mesh.normals = vertex_normals(&mesh);
    Ok(mesh)
}

/// Removes the connected pieces with fewer than `min_share` (0..1) of the triangles; returns
/// how many pieces and triangles went.
pub fn remove_small_pieces(mesh: &mut Mesh, min_share: f64) -> (usize, usize) {
    if min_share <= 0.0 || mesh.triangles.is_empty() {
        return (0, 0);
    }
    // Union-find over the vertices.
    let mut parent: Vec<u32> = (0..mesh.positions.len() as u32).collect();
    fn root(p: &mut [u32], mut i: u32) -> u32 {
        while p[i as usize] != i {
            p[i as usize] = p[p[i as usize] as usize];
            i = p[i as usize];
        }
        i
    }
    for t in &mesh.triangles {
        let a = root(&mut parent, t[0]);
        for &v in &t[1..] {
            let b = root(&mut parent, v);
            if a != b {
                parent[b as usize] = a;
            }
        }
    }
    let mut count: std::collections::HashMap<u32, usize> = std::collections::HashMap::new();
    let roots: Vec<u32> = mesh.triangles.iter().map(|t| root(&mut parent, t[0])).collect();
    for r in &roots {
        *count.entry(*r).or_insert(0) += 1;
    }
    let min = (mesh.triangles.len() as f64 * min_share).ceil() as usize;
    let removed_pieces = count.values().filter(|n| **n < min).count();
    if removed_pieces == 0 {
        return (0, 0);
    }
    let before = mesh.triangles.len();
    let keep: Vec<[u32; 3]> = mesh.triangles.iter().zip(&roots).filter(|(_, r)| count[r] >= min).map(|(t, _)| *t).collect();
    // Only the vertices still used, renumbered.
    let mut map = vec![u32::MAX; mesh.positions.len()];
    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut colors = Vec::new();
    let mut tris = Vec::with_capacity(keep.len());
    for t in keep {
        let mut nt = [0u32; 3];
        for (k, &v) in t.iter().enumerate() {
            if map[v as usize] == u32::MAX {
                map[v as usize] = positions.len() as u32;
                positions.push(mesh.positions[v as usize]);
                if let Some(n) = mesh.normals.get(v as usize) {
                    normals.push(*n);
                }
                if let Some(c) = mesh.colors.get(v as usize) {
                    colors.push(*c);
                }
            }
            nt[k] = map[v as usize];
        }
        tris.push(nt);
    }
    mesh.positions = positions;
    mesh.normals = normals;
    mesh.colors = colors;
    mesh.triangles = tris;
    (removed_pieces, before - mesh.triangles.len())
}

/// Area-weighted vertex normals.
pub fn vertex_normals(mesh: &Mesh) -> Vec<[f32; 3]> {
    let mut acc = vec![V3::ZERO; mesh.positions.len()];
    let p = |i: u32| {
        let a = mesh.positions[i as usize];
        v3(a[0] as f64, a[1] as f64, a[2] as f64)
    };
    for t in &mesh.triangles {
        let n = (p(t[1]) - p(t[0])).cross(p(t[2]) - p(t[0]));
        for &i in t {
            acc[i as usize] += n;
        }
    }
    acc.iter()
        .map(|n| {
            let l = n.length();
            if l > 0.0 { [(n.x / l) as f32, (n.y / l) as f32, (n.z / l) as f32] } else { [0.0, 0.0, 1.0] }
        })
        .collect()
}

/// A few Newton steps along the normal toward the distance `iso` (sharper edges than the mean
/// of the grid's crossings). Moves at most half a cell, so the mesh cannot fold.
pub fn refine(mesh: &mut Mesh, prog: &Program, grid: &Grid, iso: f64) {
    let h = grid.step;
    mesh.positions.par_iter_mut().zip(mesh.normals.par_iter()).for_each_init(
        || prog.evaluator(),
        |ev, (pos, n)| {
            let start = v3(pos[0] as f64, pos[1] as f64, pos[2] as f64);
            let n = v3(n[0] as f64, n[1] as f64, n[2] as f64);
            let mut p = start;
            for _ in 0..3 {
                let d = ev.de(p, h * 0.5) - iso;
                if !d.is_finite() {
                    break;
                }
                // Inside the estimate is 0: step outward by what is left.
                p = p - n * d.clamp(-0.5 * h, 0.5 * h);
            }
            let off = p - start;
            let p = if off.length() > 0.5 * h { start + off * (0.5 * h / off.length()) } else { p };
            *pos = [p.x as f32, p.y as f32, p.z as f32];
        },
    );
}

/// Vertex colors from the scene's coloring (sRGB).
pub fn vertex_colors(mesh: &Mesh, scene: &Scene, prog: &Program) -> Vec<[u8; 3]> {
    let sc = crate::render::shading_scene(scene);
    let shader = crate::render::cpu::Shader::new(&sc, prog, scene.width.max(1), scene.height.max(1));
    let display = scene.post.display_space;
    mesh.positions
        .par_iter()
        .zip(mesh.normals.par_iter())
        .map_init(
            || prog.evaluator(),
            |ev, (p, n)| {
                let c = shader.surface_base(ev, v3(p[0] as f64, p[1] as f64, p[2] as f64), v3(n[0] as f64, n[1] as f64, n[2] as f64));
                c.map(|v| {
                    let v = if display { v } else { crate::render::srgb_encode(v) };
                    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
                })
            },
        )
        .collect()
}

fn vmin(a: V3, b: V3) -> V3 {
    v3(a.x.min(b.x), a.y.min(b.y), a.z.min(b.z))
}

fn vmax(a: V3, b: V3) -> V3 {
    v3(a.x.max(b.x), a.y.max(b.y), a.z.max(b.z))
}

/// Rough box around the scene's surfaces: a coarse grid over growing boxes until the surface
/// fits inside, padded by a cell. None when nothing is found within ±64.
pub fn fit_bounds(scene: &Scene) -> Option<(V3, V3)> {
    let prog = Program::for_scene(scene);
    const N: usize = 48;
    let mut r = 2.0;
    while r <= 64.0 {
        let step = 2.0 * r / N as f64;
        let grid = Grid { origin: v3(-r, -r, -r), step, n: [N + 1; 3] };
        let found: Vec<Option<(V3, V3)>> = (0..=N)
            .into_par_iter()
            .map_init(
                || prog.evaluator(),
                |ev, z| {
                    let mut b: Option<(V3, V3)> = None;
                    for y in 0..=N {
                        for x in 0..=N {
                            let p = grid.origin + v3(x as f64, y as f64, z as f64) * step;
                            // Within a cell's half diagonal: the surface may pass this cell.
                            if ev.de(p, step * 0.25) < step * 0.87 {
                                b = Some(b.map_or((p, p), |(lo, hi)| (vmin(lo, p), vmax(hi, p))));
                            }
                        }
                    }
                    b
                },
            )
            .collect();
        let b = found.into_iter().flatten().reduce(|a, b| (vmin(a.0, b.0), vmax(a.1, b.1)));
        if let Some((lo, hi)) = b {
            let touches = [lo.x, lo.y, lo.z].iter().any(|v| *v <= -r + step * 0.5) || [hi.x, hi.y, hi.z].iter().any(|v| *v >= r - step * 0.5);
            if !touches || r >= 64.0 {
                let pad = v3(step, step, step);
                return Some((lo - pad, hi + pad));
            }
        }
        r *= 2.0;
    }
    None
}

/// Exports the scene's surface to `path` (format from the settings). `progress` gets the share
/// done and returns false to cancel. Returns the vertex and triangle counts and the number of
/// loose pieces removed.
pub fn export(scene: &Scene, settings: &MeshSettings, field: Option<&mut dyn Field>, path: &std::path::Path, progress: &mut dyn FnMut(f32) -> bool) -> Result<(usize, usize, usize), String> {
    let driven = crate::animation::still_scene(scene);
    let scene = driven.as_ref().unwrap_or(scene);
    let prog = Program::for_scene(scene);
    let grid = Grid::new(settings);
    let iso = (settings.thickness.max(0.0) * grid.step) as f32;
    let mut cpu = CpuField { prog: &prog };
    let field: &mut dyn Field = match field {
        Some(f) => f,
        None => &mut cpu,
    };
    // The grid takes most of the time; refinement and colors the rest.
    let reduce = settings.reduce > 0.0 && settings.reduce < 100.0;
    let extra = (settings.refine as u32 + (settings.colors && settings.format != MeshFormat::Stl) as u32) as f32 * 0.1 + if reduce { 0.3 } else { 0.0 };
    let share = 1.0 / (1.0 + extra);
    let mut mesh = surface_nets(field, &grid, iso, &mut |f| progress(f * share))?;
    let (pieces, _) = remove_small_pieces(&mut mesh, settings.min_piece.max(0.0) / 100.0);
    if mesh.triangles.is_empty() {
        return Err("no surface in the box".into());
    }
    if settings.refine {
        refine(&mut mesh, &prog, &grid, iso as f64);
        mesh.normals = vertex_normals(&mesh);
        if !progress(share * 1.1) {
            return Err("cancelled".into());
        }
    }
    if reduce {
        let target = ((mesh.triangles.len() as f64 * settings.reduce / 100.0).round() as usize).max(4);
        crate::mesh_simplify::decimate(&mut mesh, target, &mut |f| progress(share * (1.1 + 0.3 * f)))?;
    }
    if settings.colors && settings.format != MeshFormat::Stl {
        mesh.colors = vertex_colors(&mesh, scene, &prog);
    }
    if settings.y_up {
        for v in mesh.positions.iter_mut().chain(mesh.normals.iter_mut()) {
            *v = [v[0], v[2], -v[1]];
        }
    }
    write(&mesh, settings.format, path).map_err(|e| format!("{}: {e}", path.display()))?;
    progress(1.0);
    Ok((mesh.positions.len(), mesh.triangles.len(), pieces))
}

/// Writes the mesh in `format`.
pub fn write(mesh: &Mesh, format: MeshFormat, path: &std::path::Path) -> std::io::Result<()> {
    let mut w = std::io::BufWriter::with_capacity(1 << 20, std::fs::File::create(path)?);
    match format {
        MeshFormat::Ply => write_ply(mesh, &mut w)?,
        MeshFormat::Obj => write_obj(mesh, &mut w)?,
        MeshFormat::Stl => write_stl(mesh, &mut w)?,
    }
    w.flush()
}

fn write_ply(m: &Mesh, w: &mut impl Write) -> std::io::Result<()> {
    let colors = !m.colors.is_empty();
    write!(w, "ply\nformat binary_little_endian 1.0\ncomment RMandelbulb3D\nelement vertex {}\nproperty float x\nproperty float y\nproperty float z\nproperty float nx\nproperty float ny\nproperty float nz\n", m.positions.len())?;
    if colors {
        write!(w, "property uchar red\nproperty uchar green\nproperty uchar blue\n")?;
    }
    write!(w, "element face {}\nproperty list uchar int vertex_indices\nend_header\n", m.triangles.len())?;
    for (i, (p, n)) in m.positions.iter().zip(&m.normals).enumerate() {
        for v in p.iter().chain(n) {
            w.write_all(&v.to_le_bytes())?;
        }
        if colors {
            w.write_all(&m.colors[i])?;
        }
    }
    for t in &m.triangles {
        w.write_all(&[3])?;
        for i in t {
            w.write_all(&(*i as i32).to_le_bytes())?;
        }
    }
    Ok(())
}

fn write_obj(m: &Mesh, w: &mut impl Write) -> std::io::Result<()> {
    writeln!(w, "# RMandelbulb3D mesh: {} vertices, {} triangles", m.positions.len(), m.triangles.len())?;
    for (i, p) in m.positions.iter().enumerate() {
        match m.colors.get(i) {
            Some(c) => writeln!(w, "v {} {} {} {:.4} {:.4} {:.4}", p[0], p[1], p[2], c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0)?,
            None => writeln!(w, "v {} {} {}", p[0], p[1], p[2])?,
        }
    }
    for n in &m.normals {
        writeln!(w, "vn {:.5} {:.5} {:.5}", n[0], n[1], n[2])?;
    }
    for t in &m.triangles {
        let (a, b, c) = (t[0] + 1, t[1] + 1, t[2] + 1);
        writeln!(w, "f {a}//{a} {b}//{b} {c}//{c}")?;
    }
    Ok(())
}

fn write_stl(m: &Mesh, w: &mut impl Write) -> std::io::Result<()> {
    let mut header = [0u8; 80];
    let text = b"RMandelbulb3D binary STL";
    header[..text.len()].copy_from_slice(text);
    w.write_all(&header)?;
    w.write_all(&(m.triangles.len() as u32).to_le_bytes())?;
    let p = |i: u32| {
        let a = m.positions[i as usize];
        v3(a[0] as f64, a[1] as f64, a[2] as f64)
    };
    for t in &m.triangles {
        let n = (p(t[1]) - p(t[0])).cross(p(t[2]) - p(t[0]));
        let n = if n.length() > 0.0 { n.normalize() } else { V3::ZERO };
        for v in [n.x as f32, n.y as f32, n.z as f32] {
            w.write_all(&v.to_le_bytes())?;
        }
        for &i in t {
            for v in m.positions[i as usize] {
                w.write_all(&v.to_le_bytes())?;
            }
        }
        w.write_all(&[0, 0])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Distance to a sphere of radius `r` at the origin.
    struct Sphere(f64);

    impl Field for Sphere {
        fn slice(&mut self, g: &Grid, z: usize, out: &mut [f32]) -> Result<(), String> {
            for (i, v) in out.iter_mut().enumerate() {
                let p = g.origin + v3((i % g.n[0]) as f64, (i / g.n[0]) as f64, z as f64) * g.step;
                *v = (p.length() - self.0) as f32;
            }
            Ok(())
        }
    }

    /// Every edge is shared by exactly two triangles in opposite directions (closed and
    /// consistently oriented).
    fn closed(m: &Mesh) -> bool {
        let mut edges = std::collections::HashMap::new();
        for t in &m.triangles {
            for k in 0..3 {
                *edges.entry((t[k], t[(k + 1) % 3])).or_insert(0) += 1;
            }
        }
        let bad: Vec<_> = edges.iter().filter(|&(&(a, b), &n)| !(n == 1 && edges.get(&(b, a)) == Some(&1))).collect();
        if !bad.is_empty() {
            let multi = bad.iter().filter(|(_, n)| **n > 1).count();
            let p = |i: u32| m.positions[i as usize];
            eprintln!("{} bad edges of {}, {multi} used more than once; e.g. {:?} {:?}", bad.len(), edges.len(), p(bad[0].0.0), p(bad[0].0.1));
        }
        bad.is_empty()
    }

    #[test]
    fn sphere_mesh_is_closed_round_and_faces_outward() {
        let s = MeshSettings { min: v3(-1.0, -1.0, -1.0), max: v3(1.0, 1.0, 1.0), resolution: 32, ..MeshSettings::default() };
        let g = Grid::new(&s);
        let m = surface_nets(&mut Sphere(0.7), &g, 0.0, &mut |_| true).unwrap();
        assert!(m.triangles.len() > 1000);
        assert!(closed(&m));
        for (p, n) in m.positions.iter().zip(&m.normals) {
            let p = v3(p[0] as f64, p[1] as f64, p[2] as f64);
            assert!((p.length() - 0.7).abs() < g.step * 0.6, "vertex off the sphere: {}", p.length());
            assert!(p.normalize().dot(v3(n[0] as f64, n[1] as f64, n[2] as f64)) > 0.8);
        }
    }

    #[test]
    fn small_loose_pieces_are_removed() {
        // A big sphere and a small one beside it.
        struct Two;
        impl Field for Two {
            fn slice(&mut self, g: &Grid, z: usize, out: &mut [f32]) -> Result<(), String> {
                for (i, v) in out.iter_mut().enumerate() {
                    let p = g.origin + v3((i % g.n[0]) as f64, (i / g.n[0]) as f64, z as f64) * g.step;
                    let big = p.length() - 0.6;
                    let small = (p - v3(0.85, 0.0, 0.0)).length() - 0.1;
                    *v = big.min(small) as f32;
                }
                Ok(())
            }
        }
        let s = MeshSettings { min: v3(-1.0, -1.0, -1.0), max: v3(1.0, 1.0, 1.0), resolution: 48, ..MeshSettings::default() };
        let mut m = surface_nets(&mut Two, &Grid::new(&s), 0.0, &mut |_| true).unwrap();
        let before = m.triangles.len();
        let (pieces, tris) = remove_small_pieces(&mut m, 0.1);
        assert_eq!(pieces, 1);
        assert!(tris > 0 && m.triangles.len() == before - tris);
        assert!(closed(&m));
        assert!(m.positions.iter().all(|p| (p[0] as f64) < 0.7), "the small sphere is gone");
        assert_eq!(m.normals.len(), m.positions.len());
    }

    #[test]
    fn surfaces_cut_by_the_box_are_closed() {
        // The sphere is larger than the box: the box faces close it.
        let s = MeshSettings { min: v3(-0.5, -0.5, -0.5), max: v3(0.5, 0.5, 0.5), resolution: 16, ..MeshSettings::default() };
        let m = surface_nets(&mut Sphere(0.6), &Grid::new(&s), 0.0, &mut |_| true).unwrap();
        assert!(!m.triangles.is_empty() && closed(&m));
    }

    #[test]
    fn mandelbulb_exports_with_colors() {
        let scene = Scene::default();
        let dir = std::env::temp_dir().join(format!("rmb-mesh-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for format in MeshFormat::ALL {
            let s = MeshSettings { resolution: 40, format, ..MeshSettings::default() };
            let path = dir.join(format!("bulb.{}", format.extension()));
            let (v, t, _) = export(&scene, &s, None, &path, &mut |_| true).unwrap();
            assert!(v > 1000 && t > 2000, "{v} vertices, {t} triangles");
            let len = std::fs::metadata(&path).unwrap().len();
            assert!(len > 50_000, "{format:?}: {len} bytes");
        }
        std::fs::remove_dir_all(&dir).ok();
        let (lo, hi) = fit_bounds(&scene).unwrap();
        assert!(lo.x < -0.9 && hi.x > 0.9 && hi.x < 1.6, "{lo:?} {hi:?}");
    }
}
