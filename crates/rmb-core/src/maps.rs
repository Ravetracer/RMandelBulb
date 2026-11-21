// SPDX-License-Identifier: LGPL-2.1-or-later

//! Mandelbulb3D's maps: numbered images (`M3Maps/<number>.jpg|png|bmp|pgm`) that formulas read
//! (height maps, map translations). Maps are loaded on first use from the search folders and
//! sampled like Mandelbulb3D does (`Calc.pas`: `SplineIpolMap`, a uniform cubic B-spline over
//! the pixels with wrap-around).
//!
//! Channels keep Mandelbulb3D's order: channel 0 is blue, 1 green, 2 red (the bytes of a
//! Windows bitmap), values 0..1.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, RwLock};

/// Changes whenever the search folders change (renderers then reload their maps).
static GENERATION: AtomicU64 = AtomicU64::new(0);

pub fn generation() -> u64 {
    GENERATION.load(Ordering::Relaxed)
}

/// Environment variable with an extra folder of maps.
pub const DIR_ENV: &str = "RMB_MAPS_DIR";

pub struct Map {
    pub number: i32,
    pub width: u32,
    pub height: u32,
    /// Pixels, top row first, in channel order blue, green, red (0..255).
    pub pixels: Vec<[u8; 3]>,
}

impl Map {
    fn texel(&self, x: i64, y: i64) -> [u8; 3] {
        let (w, h) = (self.width as i64, self.height as i64);
        self.pixels[(y.rem_euclid(h) * w + x.rem_euclid(w)) as usize]
    }

    /// Like [`Map::sample`] for `u`, `v` in 0..1, wrapping horizontally and clamping vertically
    /// (Mandelbulb3D's spherical background images).
    pub fn sample_panorama(&self, u: f64, v: f64) -> [f64; 3] {
        let (w, h) = (self.width as f64, self.height as f64);
        let (x, y) = (u.clamp(0.0, 1.0) * w, v.clamp(0.0, 1.0) * h);
        let (xf, yf) = (x.floor().min(w - 1.0), y.floor().min(h - 1.0));
        let (wx, wy) = (spline_weights(x - xf), spline_weights(y - yf));
        let mut out = [0.0; 3];
        for (j, wyj) in wy.iter().enumerate() {
            let row = (yf as i64 - 1 + j as i64).clamp(0, self.height as i64 - 1);
            for (i, wxi) in wx.iter().enumerate() {
                let t = self.texel(xf as i64 - 1 + i as i64, row);
                for c in 0..3 {
                    out[c] += t[c] as f64 * wxi * wyj;
                }
            }
        }
        out.map(|v| v / 255.0)
    }

    /// Value at pixel coordinates (`x` in 0..width, `y` in 0..height), B-spline interpolated.
    pub fn sample(&self, x: f64, y: f64) -> [f64; 3] {
        let (xf, yf) = (x.floor(), y.floor());
        let (wx, wy) = (spline_weights(x - xf), spline_weights(y - yf));
        let (xi, yi) = (xf as i64, yf as i64);
        let mut out = [0.0; 3];
        for (j, wyj) in wy.iter().enumerate() {
            for (i, wxi) in wx.iter().enumerate() {
                let t = self.texel(xi - 1 + i as i64, yi - 1 + j as i64);
                let w = wxi * wyj;
                for c in 0..3 {
                    out[c] += t[c] as f64 * w;
                }
            }
        }
        out.map(|v| v / 255.0)
    }
}

/// Uniform cubic B-spline weights for the fraction `t` (Mandelbulb3D's `MakeSplineCoeff`).
pub fn spline_weights(t: f64) -> [f64; 4] {
    let w3 = t * t * t / 6.0;
    let w0 = 1.0 / 6.0 + 0.5 * t * (t - 1.0) - w3;
    let w2 = t + w0 - 2.0 * w3;
    [w0, 1.0 - w0 - w2 - w3, w2, w3]
}

fn dirs_lock() -> &'static RwLock<Vec<PathBuf>> {
    static DIRS: OnceLock<RwLock<Vec<PathBuf>>> = OnceLock::new();
    DIRS.get_or_init(|| RwLock::new(Vec::new()))
}

fn cache() -> &'static RwLock<HashMap<i32, Option<Arc<Map>>>> {
    static CACHE: OnceLock<RwLock<HashMap<i32, Option<Arc<Map>>>>> = OnceLock::new();
    CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Sets extra folders searched first (e.g. Mandelbulb3D's M3Maps folder from the preferences);
/// forgets the loaded maps.
pub fn set_dirs(dirs: Vec<PathBuf>) {
    *dirs_lock().write().unwrap_or_else(|e| e.into_inner()) = dirs;
    cache().write().unwrap_or_else(|e| e.into_inner()).clear();
    GENERATION.fetch_add(1, Ordering::Relaxed);
}

/// All folders searched for maps, in order.
pub fn search_dirs() -> Vec<PathBuf> {
    let mut d = dirs_lock().read().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some(e) = std::env::var_os(DIR_ENV) {
        d.push(PathBuf::from(e));
    }
    if let Some(exe_dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.to_path_buf())) {
        d.push(exe_dir.join("maps"));
        d.push(exe_dir.join("M3Maps"));
    }
    // Development: Mandelbulb3D's source tree next to the repository.
    d.push(PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../../mb3d/M3Maps")));
    d
}

/// File of map `n`: `<n>.ext`, or a name starting with the number followed by a non-digit
/// (Mandelbulb3D's `FindHeightMap`).
pub fn find(n: i32) -> Option<PathBuf> {
    const EXT: [&str; 5] = ["jpg", "jpeg", "png", "bmp", "pgm"];
    let ok = |p: &std::path::Path| p.extension().and_then(|e| e.to_str()).is_some_and(|e| EXT.contains(&e.to_ascii_lowercase().as_str()));
    let num = n.to_string();
    for dir in search_dirs() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        let mut files: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| ok(p)).collect();
        files.sort();
        let stem = |p: &PathBuf| p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        if let Some(p) = files.iter().find(|p| stem(p) == num) {
            return Some(p.clone());
        }
        if let Some(p) = files.iter().find(|p| {
            let s = stem(p);
            s.len() > num.len() && s.starts_with(&num) && !s[num.len()..].starts_with(|c: char| c.is_ascii_digit())
        }) {
            return Some(p.clone());
        }
    }
    None
}

fn load(n: i32) -> Option<Map> {
    load_path(&find(n)?, n)
}

fn load_path(path: &std::path::Path, n: i32) -> Option<Map> {
    let img = image::open(path).ok()?.to_rgb8();
    let (width, height) = img.dimensions();
    if width < 2 || height < 2 {
        return None;
    }
    let pixels = img.pixels().map(|p| [p[2], p[1], p[0]]).collect();
    Some(Map { number: n, width, height, pixels })
}

/// Map `n`, loaded on first use (None if there is no such file).
pub fn get(n: i32) -> Option<Arc<Map>> {
    if let Some(m) = cache().read().unwrap_or_else(|e| e.into_inner()).get(&n) {
        return m.clone();
    }
    let m = load(n).map(Arc::new);
    cache().write().unwrap_or_else(|e| e.into_inner()).insert(n, m.clone());
    m
}

/// An image by file name (background images): an existing path, or the name in the search
/// folders. Loaded on first use.
pub fn get_file(name: &str) -> Option<Arc<Map>> {
    static FILES: OnceLock<RwLock<HashMap<(String, u64), Option<Arc<Map>>>>> = OnceLock::new();
    let files = FILES.get_or_init(|| RwLock::new(HashMap::new()));
    let key = (name.to_string(), generation());
    if let Some(m) = files.read().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return m.clone();
    }
    let p = std::path::Path::new(name);
    let path = if p.is_file() { Some(p.to_path_buf()) } else { search_dirs().into_iter().map(|d| d.join(name)).find(|f| f.is_file()) };
    let m = path.and_then(|p| load_path(&p, -1)).map(Arc::new);
    files.write().unwrap_or_else(|e| e.into_inner()).insert(key, m.clone());
    m
}

/// Rotation of a background image (Mandelbulb3D's `BuildRotMatrixS`, angles in degrees).
pub fn rotation_matrix(deg: [f64; 3]) -> [[f64; 3]; 3] {
    let [(sx, cx), (sy, cy), (sz, cz)] = deg.map(|a| a.to_radians().sin_cos());
    [
        [cy * cz, -cy * sz, sy],
        [sx * sy * cz + cx * sz, cx * cz - sx * sy * sz, -sx * cy],
        [sx * sz - cx * sy * cz, cx * sy * sz + sx * cz, cx * cy],
    ]
}

/// Image coordinates (0..1) of the world direction `d` for a background image rotated by `m`
/// (`GetLightMapPixelSphere`: the transposed matrix, then latitude and longitude).
pub fn panorama_uv(m: &[[f64; 3]; 3], d: [f64; 3]) -> (f64, f64) {
    let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-30);
    let v = [0, 1, 2].map(|j| (m[0][j] * d[0] + m[1][j] * d[1] + m[2][j] * d[2]) / l);
    (v[0].atan2(v[2]) * (-0.5 / std::f64::consts::PI) + 0.5, 0.5 - v[1].clamp(-1.0, 1.0).asin() / std::f64::consts::PI)
}

/// Channel `ch` (0..2) of map `n` at `u`, `v` (0..1, tiled): Mandelbulb3D's direct lookup
/// (`GetMapPixelDirectXYspline`). Missing maps give 0.
pub fn sample_xy(n: f64, u: f64, v: f64, ch: f64) -> f64 {
    let Some(m) = get(n.round() as i32) else { return 0.0 };
    let frac = |t: f64| t - t.floor();
    m.sample(frac(u) * m.width as f64, frac(v) * m.height as f64)[channel(ch)]
}

/// Channel `ch` of map `n` in the direction (`x`, `y`, `z`): Mandelbulb3D's spherical lookup
/// (`GetMapPixelSphereSpline`).
pub fn sample_sphere(n: f64, x: f64, y: f64, z: f64, ch: f64) -> f64 {
    let Some(m) = get(n.round() as i32) else { return 0.0 };
    let (u, v) = sphere_uv(x, y, z);
    m.sample((u * m.width as f64).clamp(0.0, m.width as f64), (v * m.height as f64).clamp(0.0, m.height as f64))[channel(ch)]
}

/// Map coordinates (0..1) of a direction.
pub fn sphere_uv(x: f64, y: f64, z: f64) -> (f64, f64) {
    let l = (x * x + y * y + z * z).sqrt();
    let (dx, dy, dz) = if l > 0.0 { (x / l, y / l, z / l) } else { (0.0, 0.0, 0.0) };
    (dx.atan2(dy) * (-0.5 / std::f64::consts::PI) + 0.5, 0.5 - dz.clamp(-1.0, 1.0).asin() / std::f64::consts::PI)
}

fn channel(ch: f64) -> usize {
    (ch.round() as i64).clamp(0, 2) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spline_weights_sum_to_one() {
        for t in [0.0, 0.25, 0.5, 0.99] {
            assert!((spline_weights(t).iter().sum::<f64>() - 1.0).abs() < 1e-12);
        }
    }

    #[test]
    fn constant_map_samples_constant() {
        let m = Map { number: 1, width: 4, height: 3, pixels: vec![[10, 20, 30]; 12] };
        let v = m.sample(3.7, 0.2);
        assert!((v[0] - 10.0 / 255.0).abs() < 1e-12 && (v[2] - 30.0 / 255.0).abs() < 1e-12);
    }
}
