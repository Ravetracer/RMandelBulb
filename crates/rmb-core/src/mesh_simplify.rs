// SPDX-License-Identifier: LGPL-2.1-or-later
//! Mesh decimation by quadric edge collapse (Garland and Heckbert): edges are collapsed in the
//! order of the squared distance the new vertex has to the planes of the original triangles
//! around it, until the triangle count is reached. Collapses that would make the surface
//! non-manifold or flip a triangle are skipped, so a closed mesh stays closed.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use crate::mesh_export::Mesh;

/// Symmetric 4x4 quadric: a², ab, ac, ad, b², bc, bd, c², cd, d².
#[derive(Clone, Copy, Default)]
struct Quadric([f64; 10]);

impl Quadric {
    fn plane(n: [f64; 3], d: f64, w: f64) -> Quadric {
        let [a, b, c] = n;
        Quadric([a * a * w, a * b * w, a * c * w, a * d * w, b * b * w, b * c * w, b * d * w, c * c * w, c * d * w, d * d * w])
    }

    fn add(&mut self, o: &Quadric) {
        for (a, b) in self.0.iter_mut().zip(o.0) {
            *a += b;
        }
    }

    fn error(&self, p: [f64; 3]) -> f64 {
        let q = &self.0;
        let [x, y, z] = p;
        q[0] * x * x + 2.0 * q[1] * x * y + 2.0 * q[2] * x * z + 2.0 * q[3] * x + q[4] * y * y + 2.0 * q[5] * y * z + 2.0 * q[6] * y + q[7] * z * z + 2.0 * q[8] * z + q[9]
    }

    /// The point of least error, when the quadric is well conditioned.
    fn optimum(&self) -> Option<[f64; 3]> {
        let q = &self.0;
        let (a, b, c, d, e, f) = (q[0], q[1], q[2], q[4], q[5], q[7]);
        let det = a * (d * f - e * e) - b * (b * f - e * c) + c * (b * e - d * c);
        let scale = (a + d + f).powi(3);
        if scale <= 0.0 || det.abs() < 1e-9 * scale {
            return None;
        }
        let r = [-q[3], -q[6], -q[8]];
        let x = (r[0] * (d * f - e * e) - b * (r[1] * f - e * r[2]) + c * (r[1] * e - d * r[2])) / det;
        let y = (a * (r[1] * f - e * r[2]) - r[0] * (b * f - e * c) + c * (b * r[2] - r[1] * c)) / det;
        let z = (a * (d * r[2] - r[1] * e) - b * (b * r[2] - r[1] * c) + r[0] * (b * e - d * c)) / det;
        Some([x, y, z])
    }
}

#[derive(PartialEq)]
struct Candidate {
    /// f32 keeps the heap entries small (the heap's memory traffic dominates the time).
    cost: f32,
    a: u32,
    b: u32,
    /// Versions of both vertices when the candidate was made (stale entries are skipped).
    va: u32,
    vb: u32,
}

impl Eq for Candidate {}

impl Ord for Candidate {
    fn cmp(&self, o: &Self) -> Ordering {
        // Smallest cost first.
        o.cost.total_cmp(&self.cost)
    }
}

impl PartialOrd for Candidate {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

struct Simplifier {
    pos: Vec<[f64; 3]>,
    quad: Vec<Quadric>,
    version: Vec<u32>,
    alive: Vec<bool>,
    tris: Vec<[u32; 3]>,
    tri_alive: Vec<bool>,
    /// Triangles around each vertex (may hold dead triangles; filtered on use).
    around: Vec<Vec<u32>>,
    /// Vertices where the surface is not a single closed fan (surfaces touching at an edge or
    /// point, open borders): never moved, so those places stay as they are.
    frozen: Vec<bool>,
}

impl Simplifier {
    fn normal(&self, t: [u32; 3]) -> [f64; 3] {
        let p = |i: u32| self.pos[i as usize];
        cross(sub(p(t[1]), p(t[0])), sub(p(t[2]), p(t[0])))
    }

    fn neighbors(&self, v: u32, out: &mut Vec<u32>) {
        out.clear();
        for &t in &self.around[v as usize] {
            if self.tri_alive[t as usize] {
                for &w in &self.tris[t as usize] {
                    if w != v && !out.contains(&w) {
                        out.push(w);
                    }
                }
            }
        }
    }

    /// Cost and position of collapsing `a`-`b`.
    fn plan(&self, a: u32, b: u32) -> (f64, [f64; 3]) {
        let mut q = self.quad[a as usize];
        q.add(&self.quad[b as usize]);
        let (pa, pb) = (self.pos[a as usize], self.pos[b as usize]);
        let mid = [(pa[0] + pb[0]) * 0.5, (pa[1] + pb[1]) * 0.5, (pa[2] + pb[2]) * 0.5];
        let len2 = dot(sub(pa, pb), sub(pa, pb));
        let mut best = (q.error(mid), mid);
        // The optimum only near the edge (far points come from nearly flat quadrics).
        if let Some(o) = q.optimum().filter(|o| dot(sub(*o, mid), sub(*o, mid)) <= len2) {
            let e = q.error(o);
            if e < best.0 {
                best = (e, o);
            }
        }
        for p in [pa, pb] {
            let e = q.error(p);
            if e < best.0 {
                best = (e, p);
            }
        }
        (best.0.max(0.0), best.1)
    }

    fn push(&self, heap: &mut BinaryHeap<Candidate>, a: u32, b: u32) {
        let (cost, _) = self.plan(a, b);
        heap.push(Candidate { cost: cost as f32, a, b, va: self.version[a as usize], vb: self.version[b as usize] });
    }

    /// Whether `a`-`b` can collapse to `p`: exactly two triangles share the edge, the two
    /// vertices share exactly the two opposite neighbors, and no remaining triangle flips or
    /// becomes degenerate.
    fn valid(&self, a: u32, b: u32, p: [f64; 3], na: &[u32], nb: &[u32]) -> bool {
        if self.frozen[a as usize] || self.frozen[b as usize] {
            return false;
        }
        let shared_tris = self.around[a as usize].iter().filter(|&&t| self.tri_alive[t as usize] && self.tris[t as usize].contains(&b)).count();
        if shared_tris != 2 {
            return false;
        }
        if na.iter().filter(|w| nb.contains(w)).count() != 2 {
            return false;
        }
        for (v, other) in [(a, b), (b, a)] {
            for &t in &self.around[v as usize] {
                let tri = self.tris[t as usize];
                if !self.tri_alive[t as usize] || tri.contains(&other) {
                    continue;
                }
                let before = self.normal(tri);
                let moved = tri.map(|w| if w == v { u32::MAX } else { w });
                let q = |w: u32| if w == u32::MAX { p } else { self.pos[w as usize] };
                let after = cross(sub(q(moved[1]), q(moved[0])), sub(q(moved[2]), q(moved[0])));
                let (lb, la) = (dot(before, before).sqrt(), dot(after, after).sqrt());
                if la <= 1e-30 || dot(before, after) < 0.2 * lb * la {
                    return false;
                }
            }
        }
        true
    }
}

/// Reduces `mesh` to at most `target` triangles (fewer when the surface allows no more
/// collapses). Normals are recomputed; colors are dropped (compute them afterwards). `progress`
/// gets the share done and returns false to cancel. Returns the triangle count.
pub fn decimate(mesh: &mut Mesh, target: usize, progress: &mut dyn FnMut(f32) -> bool) -> Result<usize, String> {
    let n_tris = mesh.triangles.len();
    if target >= n_tris || n_tris == 0 {
        return Ok(n_tris);
    }
    let pos: Vec<[f64; 3]> = mesh.positions.iter().map(|p| [p[0] as f64, p[1] as f64, p[2] as f64]).collect();
    let mut s = Simplifier {
        quad: vec![Quadric::default(); pos.len()],
        version: vec![0; pos.len()],
        alive: vec![true; pos.len()],
        tri_alive: vec![true; n_tris],
        around: vec![Vec::new(); pos.len()],
        frozen: vec![false; pos.len()],
        tris: mesh.triangles.clone(),
        pos,
    };
    for (i, t) in s.tris.iter().enumerate() {
        let n = s.normal(*t);
        let l = dot(n, n).sqrt();
        if l > 0.0 {
            let nn = [n[0] / l, n[1] / l, n[2] / l];
            let q = Quadric::plane(nn, -dot(nn, s.pos[t[0] as usize]), l * 0.5);
            for &v in t {
                s.quad[v as usize].add(&q);
            }
        }
        for &v in t {
            s.around[v as usize].push(i as u32);
        }
    }
    // A closed fan: every neighbor is in exactly two of the vertex's triangles.
    let mut count: Vec<(u32, u32)> = Vec::new();
    for v in 0..s.pos.len() {
        count.clear();
        for &t in &s.around[v] {
            for &w in &s.tris[t as usize] {
                if w as usize != v {
                    match count.iter_mut().find(|c| c.0 == w) {
                        Some(c) => c.1 += 1,
                        None => count.push((w, 1)),
                    }
                }
            }
        }
        s.frozen[v] = count.len() != s.around[v].len() || count.iter().any(|c| c.1 != 2);
    }
    // Built at once (linear) instead of pushing every edge.
    let mut first = Vec::with_capacity(n_tris * 3 / 2);
    for t in &s.tris {
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            if a < b {
                let (cost, _) = s.plan(a, b);
                first.push(Candidate { cost: cost as f32, a, b, va: 0, vb: 0 });
            }
        }
    }
    let mut heap = BinaryHeap::from(first);
    let mut live = n_tris;
    let (mut na, mut nb) = (Vec::new(), Vec::new());
    let mut steps = 0u32;
    while live > target {
        steps = steps.wrapping_add(1);
        if steps.is_multiple_of(16384) && !progress((n_tris - live) as f32 / (n_tris - target) as f32) {
            return Err("cancelled".into());
        }
        let Some(c) = heap.pop() else { break };
        let (a, b) = (c.a, c.b);
        if !s.alive[a as usize] || !s.alive[b as usize] || s.version[a as usize] != c.va || s.version[b as usize] != c.vb {
            continue;
        }
        let (_, p) = s.plan(a, b);
        s.neighbors(a, &mut na);
        s.neighbors(b, &mut nb);
        if !nb.contains(&a) || !s.valid(a, b, p, &na, &nb) {
            continue;
        }
        // Collapse b into a.
        s.pos[a as usize] = p;
        let qb = s.quad[b as usize];
        s.quad[a as usize].add(&qb);
        s.alive[b as usize] = false;
        let tb = std::mem::take(&mut s.around[b as usize]);
        for t in tb {
            if !s.tri_alive[t as usize] {
                continue;
            }
            let tri = &mut s.tris[t as usize];
            if tri.contains(&a) {
                s.tri_alive[t as usize] = false;
                live -= 1;
            } else {
                for w in tri.iter_mut() {
                    if *w == b {
                        *w = a;
                    }
                }
                s.around[a as usize].push(t);
            }
        }
        let tri_alive = &s.tri_alive;
        s.around[a as usize].retain(|t| tri_alive[*t as usize]);
        // Only the edges at a change their cost (its quadric and position changed); older
        // entries of them are stale by its version.
        s.version[a as usize] += 1;
        s.neighbors(a, &mut na);
        for &w in &na {
            s.push(&mut heap, a, w);
        }
    }
    // Compact.
    let mut map = vec![u32::MAX; s.pos.len()];
    let mut positions = Vec::new();
    let mut tris = Vec::with_capacity(live);
    for (t, alive) in s.tris.iter().zip(&s.tri_alive) {
        if !alive {
            continue;
        }
        let mut nt = [0u32; 3];
        for (k, &v) in t.iter().enumerate() {
            if map[v as usize] == u32::MAX {
                map[v as usize] = positions.len() as u32;
                let p = s.pos[v as usize];
                positions.push([p[0] as f32, p[1] as f32, p[2] as f32]);
            }
            nt[k] = map[v as usize];
        }
        tris.push(nt);
    }
    mesh.positions = positions;
    mesh.triangles = tris;
    mesh.colors.clear();
    mesh.normals = crate::mesh_export::vertex_normals(mesh);
    Ok(mesh.triangles.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh_export::{Grid, MeshSettings, surface_nets};

    #[test]
    fn a_sphere_keeps_its_shape_and_stays_closed() {
        // Sphere of radius 1 from the surface nets, reduced to a tenth.
        struct Sphere;
        impl crate::mesh_export::Field for Sphere {
            fn slice(&mut self, g: &Grid, z: usize, out: &mut [f32]) -> Result<(), String> {
                for (i, v) in out.iter_mut().enumerate() {
                    let (x, y) = (i % g.n[0], i / g.n[0]);
                    let p = g.origin + crate::math::v3(x as f64, y as f64, z as f64) * g.step;
                    *v = (p.length() - 1.0) as f32;
                }
                Ok(())
            }
        }
        let grid = Grid::new(&MeshSettings { resolution: 64, ..Default::default() });
        let mut mesh = surface_nets(&mut Sphere, &grid, 0.0, &mut |_| true).expect("mesh");
        let before = mesh.triangles.len();
        let target = before / 10;
        let after = decimate(&mut mesh, target, &mut |_| true).expect("decimate");
        assert!(after <= target && after > target / 2, "{before} -> {after}");
        // Every edge is shared by exactly two triangles (closed, manifold).
        let mut edges = std::collections::HashMap::new();
        for t in &mesh.triangles {
            for k in 0..3 {
                let (a, b) = (t[k].min(t[(k + 1) % 3]), t[k].max(t[(k + 1) % 3]));
                *edges.entry((a, b)).or_insert(0) += 1;
            }
        }
        assert!(edges.values().all(|n| *n == 2), "open or non-manifold edges");
        // Euler characteristic of a sphere.
        assert_eq!(mesh.positions.len() as i64 - edges.len() as i64 + mesh.triangles.len() as i64, 2);
        // Still on the sphere.
        let worst = mesh.positions.iter().map(|p| ((p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt() - 1.0).abs()).fold(0.0f32, f32::max);
        assert!(worst < 0.02, "max deviation {worst}");
    }
}
