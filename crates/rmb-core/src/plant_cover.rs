// SPDX-License-Identifier: LGPL-2.1-or-later

//! Plants on any surface: the main fractal, fractal objects, primitives and 3D models.
//!
//! The seabed finds the ground under a plant from its height map ([`crate::plants::PlantField`]);
//! other surfaces have no such shortcut, so their plants' roots are baked once: an octree over
//! the region keeps the cells the surface may pass (distance estimate below the half diagonal),
//! every cell of the species' spacing gets one candidate by hash (share and patches), the
//! candidate is moved onto the surface along the estimate's gradient, and it is kept when it
//! stays in its cell (an even density per area) and the surface is not too steep. The roots
//! are in the host's own coordinates, so moving or animating the host never bakes again.
//!
//! Bakes run in the background like the model bakes (`mesh::request`): [`request`] starts one,
//! [`get`] waits for it (renders without a window), [`generation`] changes when one finishes.
//! [`CoverField`] sorts the roots into a grid; `nearest` mirrors march.wgsl `cover_de`.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::compose::{Primitive, Shape};
use crate::fractal::{Evaluator, Program};
use crate::math::{V3, v3};
use crate::plants::{MAX_SPECIES, PLANT_SWAY, PlacedSpecies, PlantHit, PlantSpecies};
use crate::render::hash3;

/// Plants growing on a surface (the main fractal: `Scene::plant_cover`; objects:
/// `SceneObject::cover`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlantCover {
    pub species: Vec<PlantSpecies>,
    /// Box (in the host's own units) where plants grow; None: fitted to the surface.
    pub region: Option<[[f64; 3]; 2]>,
    /// Swaying speed (phase per second in animations) and the phase now.
    pub sway_speed: f64,
    pub phase: f64,
}

impl Default for PlantCover {
    fn default() -> Self {
        PlantCover { species: Vec::new(), region: None, sway_speed: 1.0, phase: 0.0 }
    }
}

impl PlantCover {
    /// No species at all (not written to scene files).
    pub fn is_empty(&self) -> bool {
        self.species.is_empty()
    }

    /// The species that grow (at most `MAX_SPECIES`, as `PlantField::new`).
    pub fn growing(&self) -> Vec<&PlantSpecies> {
        self.species.iter().filter(|s| s.enabled && s.density > 0.0 && s.height > 0.0).take(MAX_SPECIES).collect()
    }

    pub fn active(&self) -> bool {
        !self.growing().is_empty()
    }
}

/// Most roots of one bake (all species).
pub const MAX_ROOTS: usize = 250_000;
/// Most octree cells of one level (more: the spacing is raised).
const MAX_CELLS: usize = 4_000_000;
/// Smallest cosine between a plant's axis and the surface normal.
const MIN_OUT: f64 = 0.25;
/// Patches of the clumping are this many spacings wide.
const PATCH_SPACINGS: f64 = 8.0;

/// A baked root: position and growth axis (host units), a random number for its size, and the
/// species (bits 0-1), its turn (bits 2-16) and its sway phase (bits 17-31).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CoverRoot {
    pub pos: [f32; 3],
    pub axis: [f32; 3],
    pub size: f32,
    pub bits: u32,
}

impl CoverRoot {
    pub fn species(&self) -> usize {
        (self.bits & 3) as usize
    }

    /// Turn around the axis and sway phase offset (radians).
    pub fn angles(&self) -> (f64, f64) {
        let tau = std::f64::consts::TAU;
        (((self.bits >> 2) & 0x7fff) as f64 / 32768.0 * tau, ((self.bits >> 17) & 0x7fff) as f64 / 32768.0 * tau)
    }
}

/// The result of a bake.
#[derive(Clone, Debug, Default)]
pub struct RootSet {
    pub roots: Vec<CoverRoot>,
    /// The region baked (host units).
    pub region: [[f64; 3]; 2],
    /// What the bake changed (raised spacings, a limited region).
    pub notes: Vec<String>,
}

// ---------------------------------------------------------------------------------------------
// Hosts

/// The cover of host `host` of the scene: 0 the main fractal, i + 1 the scene's object i
/// (enabled, not a seabed or water).
pub fn cover_of(scene: &crate::Scene, host: usize) -> Option<&PlantCover> {
    if host == 0 {
        return Some(&scene.plant_cover);
    }
    let o = scene.objects.get(host - 1)?;
    (o.enabled && can_carry(&o.shape)).then_some(&o.cover)
}

/// Whether plants can be baked onto this shape (the seabed has its own plants, water none).
pub fn can_carry(shape: &Shape) -> bool {
    !matches!(shape, Shape::Primitive(Primitive::Seabed { .. } | Primitive::Water { .. }))
}

/// Index into `Program::objects` of the scene's object `i` (`Program::for_scene` skips
/// disabled objects and keeps at most `MAX_OBJECTS`).
pub fn program_index(scene: &crate::Scene, i: usize) -> Option<usize> {
    let o = scene.objects.get(i)?;
    let j = scene.objects[..i].iter().filter(|o| o.enabled).count();
    (o.enabled && j < crate::compose::MAX_OBJECTS).then_some(j)
}

/// Placement settings of a species (what the roots depend on).
fn placement_key(s: &PlantSpecies) -> String {
    format!("{}|{}|{}|{}|{}|{}", s.density, s.clumping, s.spacing, s.upright, s.max_slope, s.seed)
}

/// Bake key of host `host`: its shape (without its placement) and the cover's placement.
pub fn cover_key(scene: &crate::Scene, host: usize) -> Option<u64> {
    let cover = cover_of(scene, host)?;
    if !cover.active() {
        return None;
    }
    let shape = if host == 0 {
        let mut f = scene.fractal.clone();
        f.transform = Default::default();
        serde_json::to_string(&f).ok()?
    } else {
        serde_json::to_string(&scene.objects[host - 1].shape).ok()?
    };
    let mut h = std::collections::hash_map::DefaultHasher::new();
    shape.hash(&mut h);
    for s in cover.growing() {
        placement_key(s).hash(&mut h);
    }
    format!("{:?}", cover.region).hash(&mut h);
    Some(h.finish())
}

// ---------------------------------------------------------------------------------------------
// Bake

/// Error of a bake stopped by `cancel`.
const CANCELLED: &str = "cancelled";

/// Bakes the roots of host `host` (blocking; uses all rayon threads).
pub fn bake(scene: &crate::Scene, host: usize, progress: &dyn Fn(f32)) -> Result<RootSet, String> {
    bake_ex(scene, host, progress, &AtomicBool::new(false))
}

/// [`bake`] that stops with an error when `cancel` is set.
pub fn bake_ex(scene: &crate::Scene, host: usize, progress: &dyn Fn(f32), cancel: &AtomicBool) -> Result<RootSet, String> {
    let cover = cover_of(scene, host).ok_or("this object cannot carry plants")?.clone();
    // The host alone: no covers (no recursive bakes), its model loaded.
    let mut s = scene.clone();
    s.plant_cover = PlantCover::default();
    for o in &mut s.objects {
        o.cover = PlantCover::default();
    }
    let hp = if host == 0 {
        0
    } else {
        if let Shape::Model(m) = &s.objects[host - 1].shape {
            crate::mesh::get(&m.path, &m.options)?;
        }
        program_index(&s, host - 1).ok_or("the object is not part of the scene")? + 1
    };
    let prog = Program::for_scene(&s);
    let mut notes = Vec::new();
    let region = match cover.region {
        Some(r) => [[0, 1, 2].map(|k| r[0][k].min(r[1][k])), [0, 1, 2].map(|k| r[0][k].max(r[1][k]))],
        None => {
            let (r, limited) = fit_host(&prog, hp).ok_or("no surface found within ±64 units: set a region")?;
            if limited {
                notes.push("the surface reaches beyond ±64 units: plants grow within ±64 only (set a region)".into());
            }
            r
        }
    };
    let species = cover.growing();
    let per_species = MAX_ROOTS / species.len().max(1);
    let mut roots = Vec::new();
    for (k, sp) in species.iter().enumerate() {
        let mut spacing = sp.spacing.abs().max(1e-6);
        let found = loop {
            let report = |f: f32| progress((k as f32 + f) / species.len() as f32);
            match scatter(&prog, hp, region, sp, k, spacing, per_species, &report, cancel) {
                Some(r) => break r,
                None if cancel.load(Ordering::Relaxed) => return Err(CANCELLED.into()),
                None => spacing *= 1.5,
            }
        };
        if spacing > sp.spacing.abs() * 1.0001 {
            notes.push(format!("{}: spacing raised to {spacing:.4} (too many plants)", sp.name));
        }
        roots.extend(found);
    }
    Ok(RootSet { roots, region, notes })
}

/// Box around the host's surface (as `mesh_export::fit_bounds`, host alone) and whether it was
/// limited to ±64.
fn fit_host(prog: &Program, hp: usize) -> Option<([[f64; 3]; 2], bool)> {
    const N: usize = 48;
    let mut r = 2.0;
    while r <= 64.0 {
        let step = 2.0 * r / N as f64;
        let found: Vec<Option<(V3, V3)>> = (0..=N)
            .into_par_iter()
            .map_init(
                || prog.evaluator(),
                |ev, z| {
                    let mut b: Option<(V3, V3)> = None;
                    for y in 0..=N {
                        for x in 0..=N {
                            let p = v3(-r, -r, -r) + v3(x as f64, y as f64, z as f64) * step;
                            if ev.host_distance(hp, p, step * 0.25) < step * 0.87 {
                                b = Some(b.map_or((p, p), |(lo, hi)| (vmin(lo, p), vmax(hi, p))));
                            }
                        }
                    }
                    b
                },
            )
            .collect();
        if let Some((lo, hi)) = found.into_iter().flatten().reduce(|a, b| (vmin(a.0, b.0), vmax(a.1, b.1))) {
            let touches = [lo.x, lo.y, lo.z].iter().any(|v| *v <= -r + step * 0.5) || [hi.x, hi.y, hi.z].iter().any(|v| *v >= r - step * 0.5);
            if !touches || r >= 64.0 {
                let (lo, hi) = (lo - v3(step, step, step), hi + v3(step, step, step));
                return Some(([lo.to_array(), hi.to_array()], touches));
            }
        }
        r *= 2.0;
    }
    None
}

fn vmin(a: V3, b: V3) -> V3 {
    v3(a.x.min(b.x), a.y.min(b.y), a.z.min(b.z))
}

fn vmax(a: V3, b: V3) -> V3 {
    v3(a.x.max(b.x), a.y.max(b.y), a.z.max(b.z))
}

/// Unit gradient of the host's estimate at `p` (tetrahedron differences, offset `e`).
fn gradient(ev: &mut Evaluator, hp: usize, p: V3, e: f64, h: f64) -> Option<V3> {
    let ks = [v3(1.0, -1.0, -1.0), v3(-1.0, -1.0, 1.0), v3(-1.0, 1.0, -1.0), v3(1.0, 1.0, 1.0)];
    let mut g = V3::ZERO;
    for k in ks {
        g += k * ev.host_distance(hp, p + k * e, h);
    }
    let l = g.length();
    (l > 1e-300 && l.is_finite()).then(|| g * (1.0 / l))
}

/// The roots of one species at `spacing` (None: too many, raise the spacing; or cancelled).
#[allow(clippy::too_many_arguments)]
fn scatter(prog: &Program, hp: usize, region: [[f64; 3]; 2], sp: &PlantSpecies, k: usize, spacing: f64, max_roots: usize, progress: &dyn Fn(f32), cancel: &AtomicBool) -> Option<Vec<CoverRoot>> {
    let lo = V3::from(region[0]);
    let ext = V3::from(region[1]) - lo;
    let longest = ext.x.max(ext.y).max(ext.z).max(spacing);
    let mut levels = 0u32;
    while spacing * (1u64 << levels) as f64 + 1e-12 < longest {
        levels += 1;
    }
    let tol = spacing * 0.002;
    let mut cells: Vec<[i64; 3]> = vec![[0, 0, 0]];
    for l in 0..=levels {
        let s = spacing * (1u64 << (levels - l)) as f64;
        cells = cells
            .into_par_iter()
            .map_init(
                || prog.evaluator(),
                |ev, c| {
                    if cancel.load(Ordering::Relaxed) {
                        return None;
                    }
                    // Inside the region, and the surface may pass the cell.
                    let cl = lo + v3(c[0] as f64, c[1] as f64, c[2] as f64) * s;
                    if cl.x >= lo.x + ext.x || cl.y >= lo.y + ext.y || cl.z >= lo.z + ext.z {
                        return None;
                    }
                    let center = cl + v3(0.5, 0.5, 0.5) * s;
                    (ev.host_distance(hp, center, tol) < 0.8661 * s + tol).then_some(c)
                },
            )
            .flatten()
            .collect();
        progress(0.6 * (l + 1) as f32 / (levels + 1) as f32);
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        if l < levels {
            if cells.len() * 8 > MAX_CELLS {
                return None;
            }
            cells = cells.iter().flat_map(|c| (0..8).map(move |b| [c[0] * 2 + (b & 1), c[1] * 2 + ((b >> 1) & 1), c[2] * 2 + ((b >> 2) & 1)])).collect();
        }
    }
    let share = sp.density.clamp(0.0, 1.0);
    if cells.len() as f64 * share > max_roots as f64 * 1.5 {
        return None;
    }
    let clumping = sp.clumping.clamp(0.0, 1.0);
    let cos_slope = sp.max_slope.clamp(0.0, 180.0).to_radians().cos();
    let upright = sp.upright.clamp(0.0, 1.0);
    let salt = 300 + 7 * k as i32 + (sp.seed as i32).wrapping_mul(7919);
    let up = v3(0.0, 0.0, 1.0);
    let roots: Vec<CoverRoot> = cells
        .par_iter()
        .map_init(
            || prog.evaluator(),
            |ev, c| {
                if cancel.load(Ordering::Relaxed) {
                    return None;
                }
                let (i, j, kk) = (c[0] as i32, c[1] as i32, c[2] as i32);
                let h = |m: i32| hash3(i.wrapping_add(m.wrapping_mul(613)), j, kk ^ salt) as f64;
                let cl = lo + v3(c[0] as f64, c[1] as f64, c[2] as f64) * spacing;
                let h0 = h(0);
                if h0 >= share {
                    return None;
                }
                if clumping > 0.0 {
                    let q = (cl + v3(0.5, 0.5, 0.5) * spacing) * (1.0 / (PATCH_SPACINGS * spacing));
                    let clump = (2.5 * crate::render::value_noise([q.x + 3.7, q.y + 9.1, q.z + 1.3]) - 1.0).clamp(0.0, 1.0);
                    if h0 >= share * (1.0 + (clump - 1.0) * clumping) {
                        return None;
                    }
                }
                // A start outside the solid (the estimate is 0 inside escape-time fractals).
                let mut p = V3::ZERO;
                let mut d = 0.0;
                for a in 0..3 {
                    p = cl + v3(0.15 + 0.7 * h(1 + 3 * a), 0.15 + 0.7 * h(2 + 3 * a), 0.15 + 0.7 * h(3 + 3 * a)) * spacing;
                    d = ev.host_distance(hp, p, tol);
                    if d > tol {
                        break;
                    }
                }
                if d <= 0.0 {
                    return None;
                }
                // Onto the surface: steps of the estimate along the gradient stay outside; they
                // aim half a tolerance above it (exact estimates would land on 0, which counts
                // as inside).
                let e = spacing * 0.01;
                for _ in 0..64 {
                    if d < tol {
                        break;
                    }
                    let n = gradient(ev, hp, p, e, tol)?;
                    p = p - n * (d - 0.5 * tol);
                    d = ev.host_distance(hp, p, tol);
                }
                if d >= tol || d <= 0.0 {
                    return None;
                }
                let inside = |v: f64, a: f64| v >= a && v < a + spacing;
                if !(inside(p.x, cl.x) && inside(p.y, cl.y) && inside(p.z, cl.z)) {
                    return None;
                }
                // Cells on the region's edge reach beyond it.
                let rel = p - lo;
                if rel.x > ext.x || rel.y > ext.y || rel.z > ext.z {
                    return None;
                }
                let n = gradient(ev, hp, p, e, tol)?;
                if n.z < cos_slope {
                    return None;
                }
                let axis = n * (1.0 - upright) + up * upright;
                let mut axis = if axis.length() > 1e-6 { axis.normalize() } else { n };
                // Never into the host (under overhangs up points into it): at most 75° from
                // the normal.
                let out = axis.dot(n);
                if out < MIN_OUT {
                    axis = (axis + n * (MIN_OUT - out) * 2.0).normalize();
                    let o2 = axis.dot(n);
                    if o2 < MIN_OUT {
                        axis = n;
                    }
                }
                let bits = k as u32 | ((h(10) * 32768.0) as u32 & 0x7fff) << 2 | ((h(11) * 32768.0) as u32 & 0x7fff) << 17;
                Some(CoverRoot { pos: p.to_f32(), axis: axis.to_f32(), size: h(12) as f32, bits })
            },
        )
        .flatten()
        .collect();
    progress(1.0);
    (roots.len() <= max_roots && !cancel.load(Ordering::Relaxed)).then_some(roots)
}

// ---------------------------------------------------------------------------------------------
// Cache (as mesh.rs)

static GENERATION: AtomicU64 = AtomicU64::new(0);

/// Changes whenever a bake finishes (renderers then build their program again).
pub fn generation() -> u64 {
    GENERATION.load(Ordering::Relaxed)
}

/// State of a host's bake.
#[derive(Clone, Debug)]
pub enum Status {
    Ready(Arc<RootSet>),
    /// Bake progress 0..1.
    Baking(f32),
    Failed(String),
}

enum Entry {
    Ready(Arc<RootSet>),
    Baking(Arc<AtomicU32>),
    Failed(String),
}

struct Cache {
    map: Mutex<HashMap<u64, (u64, Entry)>>,
    done: Condvar,
    clock: AtomicU64,
}

fn cache() -> &'static Cache {
    static CACHE: OnceLock<Cache> = OnceLock::new();
    CACHE.get_or_init(|| Cache { map: Mutex::new(HashMap::new()), done: Condvar::new(), clock: AtomicU64::new(0) })
}

fn lock(c: &Cache) -> MutexGuard<'_, HashMap<u64, (u64, Entry)>> {
    c.map.lock().unwrap_or_else(|e| e.into_inner())
}

/// Entries kept (animations with changing hosts bake every frame).
const KEEP: usize = 24;

fn start_entry(map: &mut HashMap<u64, (u64, Entry)>, key: u64) -> Arc<AtomicU32> {
    while map.len() >= KEEP {
        let oldest = map.iter().filter(|(_, (_, e))| !matches!(e, Entry::Baking(_))).min_by_key(|(_, (t, _))| *t).map(|(k, _)| *k);
        match oldest {
            Some(k) => {
                map.remove(&k);
            }
            None => break,
        }
    }
    let progress = Arc::new(AtomicU32::new(0f32.to_bits()));
    map.insert(key, (cache().clock.fetch_add(1, Ordering::Relaxed), Entry::Baking(progress.clone())));
    progress
}

fn run_bake(scene: &crate::Scene, host: usize, key: u64, progress: &AtomicU32, cancel: &AtomicBool) -> Result<Arc<RootSet>, String> {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        bake_ex(
            scene,
            host,
            &|f| {
                progress.fetch_max(f.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
            },
            cancel,
        )
        .map(Arc::new)
    }))
    .unwrap_or_else(|_| Err("baking the plants failed".into()));
    let c = cache();
    let mut map = lock(c);
    if result.as_ref().is_err_and(|e| e == CANCELLED) {
        // Superseded by a newer setting: forget it (a later request bakes it again).
        map.remove(&key);
        drop(map);
        c.done.notify_all();
        return result;
    }
    let t = c.clock.fetch_add(1, Ordering::Relaxed);
    map.insert(
        key,
        (
            t,
            match &result {
                Ok(r) => Entry::Ready(r.clone()),
                Err(e) => Entry::Failed(e.clone()),
            },
        ),
    );
    drop(map);
    GENERATION.fetch_add(1, Ordering::Relaxed);
    c.done.notify_all();
    result
}

/// State of host `host`'s plants; starts a background bake when they are neither baked nor
/// baking. None when the host has no growing plants.
pub fn request(scene: &crate::Scene, host: usize) -> Option<Status> {
    let key = cover_key(scene, host)?;
    let c = cache();
    let mut map = lock(c);
    if let Some((t, e)) = map.get_mut(&key) {
        *t = c.clock.fetch_add(1, Ordering::Relaxed);
        return Some(match e {
            Entry::Ready(r) => Status::Ready(r.clone()),
            Entry::Baking(p) => Status::Baking(f32::from_bits(p.load(Ordering::Relaxed))),
            Entry::Failed(e) => Status::Failed(e.clone()),
        });
    }
    let progress = start_entry(&mut map, key);
    drop(map);
    // A newer setting of the same host stops the older background bake.
    let cancel = Arc::new(AtomicBool::new(false));
    {
        static RUNNING: OnceLock<Mutex<HashMap<usize, Arc<AtomicBool>>>> = OnceLock::new();
        let mut running = RUNNING.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
        if let Some(old) = running.insert(host, cancel.clone()) {
            old.store(true, Ordering::Relaxed);
        }
    }
    let scene = scene.clone();
    let spawned = std::thread::Builder::new().name("rmb-plant-bake".into()).spawn(move || {
        let _ = run_bake(&scene, host, key, &progress, &cancel);
    });
    if let Err(e) = spawned {
        let msg = format!("cannot start the plant bake: {e}");
        lock(c).insert(key, (0, Entry::Failed(msg.clone())));
        return Some(Status::Failed(msg));
    }
    Some(Status::Baking(0.0))
}

/// Host `host`'s roots: cached, by waiting for a running bake, or baked in this thread.
pub fn get(scene: &crate::Scene, host: usize) -> Option<Result<Arc<RootSet>, String>> {
    let key = cover_key(scene, host)?;
    let c = cache();
    let mut map = lock(c);
    loop {
        match map.get(&key).map(|e| &e.1) {
            Some(Entry::Ready(r)) => return Some(Ok(r.clone())),
            Some(Entry::Failed(e)) => return Some(Err(e.clone())),
            Some(Entry::Baking(_)) => map = c.done.wait(map).unwrap_or_else(|e| e.into_inner()),
            None => break,
        }
    }
    let progress = start_entry(&mut map, key);
    drop(map);
    Some(run_bake(scene, host, key, &progress, &AtomicBool::new(false)))
}

/// Hosts of the scene with growing plants (0 the main fractal, i + 1 object i).
pub fn hosts(scene: &crate::Scene) -> Vec<usize> {
    (0..=scene.objects.len()).filter(|&h| cover_of(scene, h).is_some_and(|c| c.active())).collect()
}

/// Bakes the plants of all hosts (blocking); returns the failures.
pub fn load(scene: &crate::Scene) -> Vec<String> {
    let mut failed = Vec::new();
    for h in hosts(scene) {
        if let Some(Err(e)) = get(scene, h) {
            let name = if h == 0 { "main fractal".to_string() } else { scene.objects[h - 1].name.clone() };
            failed.push(format!("plants on {name}: {e}"));
        }
    }
    failed
}

// ---------------------------------------------------------------------------------------------
// Field


/// The roots of one species sorted into a grid: corner, cell size, cells per axis; the roots
/// of cell c are `roots[starts[c]..starts[c + 1]]` (cells x fastest).
#[derive(Clone, Debug)]
pub struct SpeciesGrid {
    pub origin: V3,
    pub cell: f64,
    pub n: [usize; 3],
    pub starts: Vec<u32>,
    pub roots: Vec<CoverRoot>,
    /// Farthest reach of its plants from their roots (host units).
    pub r_max: f64,
}

/// The roots of one host, one grid per species (cells twice the species' reach).
#[derive(Clone, Debug)]
pub struct CoverField {
    pub species: Vec<PlacedSpecies>,
    /// Reach of each species around its root in template units (extent plus the sway).
    pub reach: Vec<f64>,
    pub grids: Vec<SpeciesGrid>,
}

/// Most grid cells per species.
const MAX_GRID: usize = 1 << 21;
/// The scan reaches at most this many cells beyond the point's cell (march.wgsl `COVER_SPAN`).
pub const COVER_SPAN: i64 = 1;

/// Turned frame around a unit axis (mirrors march.wgsl `cover_frame`): branch-free
/// orthonormal basis (Duff et al. 2017), turned by `turn`.
pub fn cover_frame(a: V3, turn: f64) -> (V3, V3) {
    let sign = if a.z >= 0.0 { 1.0 } else { -1.0 };
    let k = -1.0 / (sign + a.z);
    let b = a.x * a.y * k;
    let t1 = v3(1.0 + sign * a.x * a.x * k, sign * b, -sign * a.x);
    let t2 = v3(b, sign + a.y * a.y * k, -a.y);
    let (sn, cs) = turn.sin_cos();
    (t1 * cs + t2 * sn, t2 * cs - t1 * sn)
}

impl SpeciesGrid {
    fn new(roots: Vec<CoverRoot>, r_max: f64) -> Option<SpeciesGrid> {
        if roots.is_empty() {
            return None;
        }
        let (mut lo, mut hi) = (V3::from([f64::MAX; 3]), V3::from([f64::MIN; 3]));
        for r in &roots {
            let p = V3::from(r.pos.map(|v| v as f64));
            lo = vmin(lo, p);
            hi = vmax(hi, p);
        }
        // The box holds every plant whole.
        let pad = v3(r_max, r_max, r_max);
        let (lo, hi) = (lo - pad, hi + pad);
        let ext = hi - lo;
        let mut cell = (2.0 * r_max).max(ext.x.max(ext.y).max(ext.z) / 256.0);
        let count = |c: f64| [ext.x, ext.y, ext.z].map(|e| ((e / c).ceil() as usize).max(1));
        while count(cell).iter().product::<usize>() > MAX_GRID {
            cell *= 1.25;
        }
        let n = count(cell);
        let index = |r: &CoverRoot| {
            let c = [0, 1, 2].map(|k| (((r.pos[k] as f64 - [lo.x, lo.y, lo.z][k]) / cell).floor().max(0.0) as usize).min(n[k] - 1));
            c[0] + n[0] * (c[1] + n[1] * c[2])
        };
        let mut starts = vec![0u32; n[0] * n[1] * n[2] + 1];
        for r in &roots {
            starts[index(r) + 1] += 1;
        }
        for i in 1..starts.len() {
            starts[i] += starts[i - 1];
        }
        let mut fill = starts.clone();
        let mut sorted = vec![roots[0]; roots.len()];
        for r in &roots {
            let c = index(r);
            sorted[fill[c] as usize] = *r;
            fill[c] += 1;
        }
        Some(SpeciesGrid { origin: lo, cell, n, starts, roots: sorted, r_max })
    }
}

impl CoverField {
    pub fn new(set: &RootSet, cover: &PlantCover) -> Option<CoverField> {
        let species: Vec<PlacedSpecies> = cover.growing().into_iter().enumerate().map(|(i, s)| PlacedSpecies::new(s, i)).collect();
        if species.is_empty() || set.roots.is_empty() {
            return None;
        }
        let reach: Vec<f64> = species.iter().map(|s| s.template.extent + PLANT_SWAY * s.sway * s.template.top * s.template.top).collect();
        let grids: Vec<SpeciesGrid> = (0..species.len())
            .filter_map(|k| {
                let roots: Vec<CoverRoot> = set.roots.iter().filter(|r| r.species() == k).copied().collect();
                SpeciesGrid::new(roots, (species[k].height * reach[k]).max(1e-9))
            })
            .collect();
        (!grids.is_empty()).then_some(CoverField { species, reach, grids })
    }

    /// The color of a species' branches or leaves at `height` (0 root .. 1 top).
    pub fn color(&self, species: usize, leaf: bool, height: f64) -> [f64; 3] {
        let Some(s) = self.species.get(species) else { return [0.5; 3] };
        let c = if leaf { &s.leaf_colors } else { &s.branch_colors };
        [0, 1, 2].map(|i| c[0][i] as f64 + (c[1][i] as f64 - c[0][i] as f64) * height)
    }

    /// Nearest plant to the host point `q` within `limit` (mirrors march.wgsl `cover_de`): a
    /// distance bound, with the species, leaf and height of the plant when one is nearest.
    /// Per species: far from its roots the bound is the distance to its grid's box; else the
    /// cells within reach of the bound so far are scanned (at most `COVER_SPAN` beyond the
    /// point's cell), and the roots beyond them are bounded by the scanned block's edge.
    /// `min_size`: level of detail, the thinnest branch drawn (host units; 0 all).
    pub fn nearest(&self, q: V3, phase: f64, limit: f64, min_size: f64) -> Option<PlantHit> {
        let p = [q.x, q.y, q.z];
        let mut best = PlantHit { d: limit, species: 0, leaf: false, height: 0.0 };
        for g in &self.grids {
            let o = [g.origin.x, g.origin.y, g.origin.z];
            let c = g.cell;
            let mut out2 = 0.0;
            for k in 0..3 {
                let hi = o[k] + g.n[k] as f64 * c;
                let d = (o[k] - p[k]).max(p[k] - hi).max(0.0);
                out2 += d * d;
            }
            let out = out2.sqrt();
            if out >= best.d {
                continue;
            }
            if out > 2.0 * c {
                best.d = out;
                continue;
            }
            let ci = [0, 1, 2].map(|k| (((p[k] - o[k]) / c).floor() as i64).clamp(0, g.n[k] as i64 - 1));
            // The cells within reach of the bound, at most COVER_SPAN beyond the point's cell.
            let r = best.d + g.r_max;
            let lo = [0, 1, 2].map(|k| (((p[k] - r - o[k]) / c).floor() as i64).max(ci[k] - COVER_SPAN).max(0));
            let hi = [0, 1, 2].map(|k| (((p[k] + r - o[k]) / c).floor() as i64).min(ci[k] + COVER_SPAN).min(g.n[k] as i64 - 1));
            // Roots beyond the scanned block are at least its edge away (sides with cells beyond).
            let mut edge = f64::MAX;
            for k in 0..3 {
                if lo[k] > 0 {
                    edge = edge.min(p[k] - (o[k] + lo[k] as f64 * c));
                }
                if hi[k] < g.n[k] as i64 - 1 {
                    edge = edge.min(o[k] + (hi[k] + 1) as f64 * c - p[k]);
                }
            }
            let far = (edge - g.r_max).max(out);
            if far < best.d {
                best = PlantHit { d: far, species: 0, leaf: false, height: 0.0 };
            }
            for z in lo[2]..=hi[2] {
                for y in lo[1]..=hi[1] {
                    for x in lo[0]..=hi[0] {
                        let cc = [x, y, z];
                        // Gap to the cell's box: its plants reach at most r_max beyond it.
                        let mut g2 = 0.0;
                        for k in 0..3 {
                            let a = o[k] + cc[k] as f64 * c;
                            let d = (a - p[k]).max(p[k] - a - c).max(0.0);
                            g2 += d * d;
                        }
                        if g2.sqrt() - g.r_max >= best.d {
                            continue;
                        }
                        let idx = x as usize + g.n[0] * (y as usize + g.n[1] * z as usize);
                        for root in &g.roots[g.starts[idx] as usize..g.starts[idx + 1] as usize] {
                            self.root_distance(root, q, phase, min_size, &mut best);
                        }
                    }
                }
            }
        }
        (best.d < limit).then_some(best)
    }

    /// The plant of `r` if it is nearer than `best`.
    fn root_distance(&self, r: &CoverRoot, q: V3, phase: f64, min_size: f64, best: &mut PlantHit) {
        let k = r.species();
        let s = &self.species[k];
        let t = &s.template;
        let size = s.height * (1.0 - s.variation * r.size as f64);
        let d = q - V3::from(r.pos.map(|v| v as f64));
        if d.length() - size * self.reach[k] >= best.d {
            return;
        }
        let axis = V3::from(r.axis.map(|v| v as f64));
        let (turn, ph) = r.angles();
        let (x, y) = cover_frame(axis, turn);
        let mut lp = v3(d.dot(x), d.dot(y), d.dot(axis)) * (1.0 / size);
        let z = lp.z.clamp(0.0, t.top);
        lp.x -= PLANT_SWAY * s.sway * z * z * (phase + ph - 2.0 * z).sin();
        let zt = (lp.z + best.d / size).clamp(0.0, t.top);
        let warp = 1.0 + PLANT_SWAY * s.sway * (2.0 * zt + 2.0 * zt * zt);
        if let Some((dd, seg)) = t.distance(lp, best.d / size * warp, min_size / size) {
            let dd = dd * size / warp;
            if dd < best.d {
                *best = PlantHit { d: dd, species: k, leaf: t.segs[seg].leaf, height: (lp.z / t.top).clamp(0.0, 1.0) };
            }
        }
    }
}

/// A host's plants prepared for evaluation (`Program::covers`).
#[derive(Clone, Debug)]
pub struct HostCover {
    /// 0 the main fractal, j + 1 `Program::objects[j]`.
    pub host: usize,
    pub field: Arc<CoverField>,
    pub phase: f64,
}

/// The field of `set` with `cover`'s species (cached: building the grid takes a moment).
pub fn field(set: &Arc<RootSet>, cover: &PlantCover) -> Option<Arc<CoverField>> {
    type Entry = (Arc<RootSet>, String, Arc<CoverField>);
    static CACHE: OnceLock<Mutex<Vec<Entry>>> = OnceLock::new();
    let key = cover.growing().iter().map(|s| serde_json::to_string(s).unwrap_or_default()).collect::<Vec<_>>().join(";");
    let cache = CACHE.get_or_init(Default::default);
    if let Some((_, _, f)) = cache.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|(r, k, _)| Arc::ptr_eq(r, set) && *k == key) {
        return Some(f.clone());
    }
    let f = Arc::new(CoverField::new(set, cover)?);
    let mut c = cache.lock().unwrap_or_else(|e| e.into_inner());
    if c.len() >= 16 {
        c.remove(0);
    }
    c.push((set.clone(), key, f.clone()));
    Some(f)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compose::SceneObject;

    /// The default scene with a sphere (radius 0.5) far from the bulb, covered with kelp.
    fn sphere_scene(slope: f64, upright: f64) -> crate::Scene {
        let mut s = crate::Scene::default();
        let mut o = SceneObject::default();
        o.transform.position = v3(5.0, 0.0, 0.0);
        let mut k = PlantSpecies::preset("Kelp").unwrap();
        (k.spacing, k.height, k.density, k.clumping, k.max_slope, k.upright) = (0.05, 0.04, 1.0, 0.0, slope, upright);
        o.cover.species.push(k);
        s.objects.push(o);
        s
    }

    #[test]
    fn cover_roots_lie_on_the_surface_evenly() {
        let s = sphere_scene(180.0, 0.0);
        let set = bake(&s, 1, &|_| {}).unwrap();
        let n = set.roots.len();
        // Area / spacing²: about 1257 cells on the surface, most keep their root.
        assert!((700..1600).contains(&n), "{n} roots");
        let (mut upper, mut lower) = (0, 0);
        for r in &set.roots {
            let p = V3::from(r.pos.map(|v| v as f64));
            assert!((p.length() - 0.5).abs() < 1e-3, "root off the surface: {p:?}");
            // Along the normal (upright 0).
            let a = V3::from(r.axis.map(|v| v as f64));
            assert!(a.dot(p.normalize()) > 0.99, "axis {a:?} at {p:?}");
            if p.z > 0.0 { upper += 1 } else { lower += 1 }
        }
        let ratio = upper as f64 / lower as f64;
        assert!((0.8..1.25).contains(&ratio), "{upper} / {lower}");
        // Deterministic.
        assert_eq!(bake(&s, 1, &|_| {}).unwrap().roots, set.roots);
    }

    #[test]
    fn cover_grows_on_the_bulb_and_obeys_slope_and_upright() {
        let mut s = crate::Scene::default();
        let mut k = PlantSpecies::preset("Kelp").unwrap();
        (k.spacing, k.height, k.density, k.clumping, k.max_slope, k.upright) = (0.06, 0.05, 1.0, 0.0, 40.0, 0.0);
        s.plant_cover.species.push(k);
        let set = bake(&s, 0, &|_| {}).unwrap();
        assert!(set.roots.len() > 100, "{}", set.roots.len());
        let prog = Program::for_scene(&crate::Scene { plant_cover: PlantCover::default(), ..s.clone() });
        let mut ev = prog.evaluator();
        for r in &set.roots {
            let p = V3::from(r.pos.map(|v| v as f64));
            let d = ev.host_distance(0, p, 1e-4);
            assert!(d < 0.06 * 0.002 + 1e-6, "root {p:?} {d} above the bulb");
            assert!(r.axis[2] as f64 >= 40f64.to_radians().cos() - 1e-3, "too steep: {:?}", r.axis);
        }
        // Straight up with upright 1.
        s.plant_cover.species[0].upright = 1.0;
        let up = bake(&s, 0, &|_| {}).unwrap();
        assert!(up.roots.iter().all(|r| r.axis == [0.0, 0.0, 1.0]));
        // Never into the surface.
        s.plant_cover.species[0].max_slope = 180.0;
        for r in &bake(&s, 0, &|_| {}).unwrap().roots {
            let p = V3::from(r.pos.map(|v| v as f64));
            let Some(n) = gradient(&mut ev, 0, p, 0.06 * 0.01, 0.06 * 0.002) else { continue };
            assert!(V3::from(r.axis.map(|v| v as f64)).dot(n) > MIN_OUT - 0.02, "axis {:?} into the bulb at {p:?}", r.axis);
        }
    }

    #[test]
    fn cover_stays_a_distance_bound() {
        let s = sphere_scene(180.0, 0.6);
        let set = Arc::new(bake(&s, 1, &|_| {}).unwrap());
        let field = CoverField::new(&set, &s.objects[0].cover).unwrap();
        let mut checked = 0;
        for i in 0..3000 {
            let f = |a: f64| (i as f64 * a) % 1.0 * 2.0 - 1.0;
            let p = v3(f(0.6180339), f(0.7548776), f(0.5698403)) * 0.62;
            let Some(h) = field.nearest(p, 1.3, 1e9, 0.0) else { continue };
            if h.d <= 1e-4 {
                continue;
            }
            checked += 1;
            for k in 0..26 {
                let dir = v3(((k % 3) as f64) - 1.0, ((k / 3 % 3) as f64) - 1.0, ((k / 9) as f64) - 1.0 + if k == 13 { 1.0 } else { 0.0 });
                let q = p + dir.normalize() * (h.d * 0.98);
                let inside = field.nearest(q, 1.3, 1e9, 0.0).is_some_and(|g| g.d < -1e-9);
                assert!(!inside, "bound {} at {p:?} reaches into a plant at {q:?}", h.d);
            }
        }
        assert!(checked > 500, "{checked}");
    }

    #[test]
    fn cover_grows_on_exact_surfaces() {
        // A plane (exact distance) in a region box.
        let mut s = crate::Scene::default();
        let mut o = SceneObject { shape: Shape::Primitive(Primitive::Plane), ..Default::default() };
        let mut k = PlantSpecies::preset("Kelp").unwrap();
        (k.spacing, k.density, k.clumping) = (0.2, 1.0, 0.0);
        o.cover.species.push(k);
        o.cover.region = Some([[-1.0, -1.0, -0.1], [1.0, 1.0, 0.1]]);
        s.objects.push(o);
        let set = bake(&s, 1, &|_| {}).unwrap();
        assert!(set.roots.len() > 60, "{}", set.roots.len());
        assert!(set.roots.iter().all(|r| r.pos[2] > 0.0 && r.pos[2] < 0.001 && r.pos[0].abs() <= 1.0));
    }

    #[test]
    fn plants_join_the_scene_once_baked() {
        let s = sphere_scene(180.0, 0.6);
        assert!(load(&s).is_empty());
        let prog = Program::for_scene(&s);
        assert_eq!(prog.covers.len(), 1);
        assert_eq!(prog.covers[0].host, 1);
        // Just above a root: the plant is nearer than the sphere and colors the hit.
        let r = *prog.covers[0].field.grids[0].roots.iter().max_by(|a, b| a.pos[2].total_cmp(&b.pos[2])).unwrap();
        let p = v3(5.0, 0.0, 0.0) + V3::from(r.pos.map(|v| v as f64)) + V3::from(r.axis.map(|v| v as f64)) * 0.01;
        let o = prog.evaluator().orbit(p, 1e-5);
        assert!(o.plant >= crate::plants::PLANT_COLOR, "{o:?}");
        assert_eq!(o.object, 1);
        let (obj, ..) = crate::plants::decode_color(o.plant);
        assert_eq!(obj, crate::compose::MAX_OBJECTS);
    }
}
