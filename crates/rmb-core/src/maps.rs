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

#[derive(Clone)]
pub struct Map {
    pub number: i32,
    pub width: u32,
    pub height: u32,
    /// Pixels, top row first, in channel order blue, green, red (0..255).
    pub pixels: Vec<[u8; 3]>,
    /// Opacity per pixel (0..255) of images with an alpha channel.
    pub alpha: Option<Vec<u8>>,
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
    let img = image::open(path).ok()?;
    let has_alpha = img.color().has_alpha();
    let img = img.to_rgba8();
    let (width, height) = img.dimensions();
    if width < 2 || height < 2 {
        return None;
    }
    let pixels = img.pixels().map(|p| [p[2], p[1], p[0]]).collect();
    // Only images that use their alpha channel keep it.
    let alpha = has_alpha.then(|| img.pixels().map(|p| p[3]).collect::<Vec<u8>>()).filter(|a| a.iter().any(|v| *v < 255));
    Some(Map { number: n, width, height, pixels, alpha })
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

/// A background image prepared like Mandelbulb3D's LoadLightMap (`smooth`: soften the image,
/// `spherical`: "convert to spherical", `fit_border`: blend the left and right borders) and its
/// 16 × 8 copy for ambient light (MakeSmallLMimage). Cached.
pub fn background(name: &str, smooth: bool, spherical: bool, fit_border: bool) -> Option<(Arc<Map>, Arc<Map>)> {
    type Prepared = RwLock<HashMap<(String, bool, bool, bool, u64), Option<(Arc<Map>, Arc<Map>)>>>;
    static PREPARED: OnceLock<Prepared> = OnceLock::new();
    let cache = PREPARED.get_or_init(|| RwLock::new(HashMap::new()));
    let key = (name.to_string(), smooth, spherical, fit_border, generation());
    if let Some(m) = cache.read().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return m.clone();
    }
    let m = get_file(name).map(|m| {
        let smoothed;
        let src: &Map = if smooth {
            smoothed = m.smoothed();
            &smoothed
        } else {
            &m
        };
        let mut big = if spherical { src.to_spherical() } else { src.clone() };
        if fit_border {
            big.fit_border();
        }
        let small = Arc::new(big.ambient_map());
        (Arc::new(big), small)
    });
    cache.write().unwrap_or_else(|e| e.into_inner()).insert(key, m.clone());
    m
}

/// One line of Mandelbulb3D's smoothing (SmoothLightMap, `ipolC`): the 5-tap filter
/// (-0.05, 0.25, 0.6, 0.25, -0.05) with wrap-around, done in place like the original: the last
/// value reads `extra` (the copy MB3D keeps after the line) as its right neighbor and the
/// already smoothed first value as the one after it.
fn smooth_line(line: &mut [[u8; 3]], extra: [u8; 3]) {
    let n = line.len();
    if n < 3 {
        return;
    }
    let orig = line.to_vec();
    let at = |i: usize| if i == n { extra } else { orig[i % n] };
    for i in 0..n {
        let t = [orig[(i + n - 2) % n], orig[(i + n - 1) % n], orig[i], at(i + 1), if i == n - 1 { line[0] } else { at(i + 2) }];
        line[i] = [0, 1, 2].map(|c| {
            let v = (t[0][c] as f64 + t[4][c] as f64) * -0.05 + (t[1][c] as f64 + t[3][c] as f64) * 0.25 + t[2][c] as f64 * 0.6;
            // Delphi's Round: halves to even.
            v.round_ties_even().clamp(0.0, 255.0) as u8
        });
    }
}

impl Map {
    /// Mandelbulb3D's "smooth on load" (SmoothLightMap): a 5-tap filter down every column, then
    /// along every row, both wrapping around.
    pub fn smoothed(&self) -> Map {
        let (w, h) = (self.width as usize, self.height as usize);
        let mut px = self.pixels.clone();
        let mut col = vec![[0u8; 3]; h];
        for x in 0..w {
            for y in 0..h {
                col[y] = px[y * w + x];
            }
            smooth_line(&mut col, self.pixels[x]);
            for y in 0..h {
                px[y * w + x] = col[y];
            }
        }
        // MB3D's copy after each row still holds the row's first pixel before the column pass.
        for y in 0..h {
            smooth_line(&mut px[y * w..(y + 1) * w], self.pixels[y * w]);
        }
        Map { number: self.number, width: self.width, height: self.height, pixels: px, alpha: self.alpha.clone() }
    }

    /// Mandelbulb3D's "convert to spherical" (SmoothLightMap): every row is squeezed toward the
    /// center by the cosine of its latitude, with its cubic interpolation (which reads one pixel
    /// to the right).
    pub fn to_spherical(&self) -> Map {
        let (w, h) = (self.width as usize, self.height as usize);
        let mut out = self.pixels.clone();
        let sp = (w as f64 - 1.0) * 0.5;
        let sw = w as f64 - 1.99;
        for y in 0..h {
            let v = 2.0 * (y as f64 + 0.5) / h as f64 - 1.0;
            let s = (1.0 - v * v).max(0.0).sqrt();
            for x in 0..w {
                let s2 = ((x as f64 - sp) * s + sp).clamp(0.0, sw.max(0.0));
                let i = s2.trunc() as usize;
                let f = s2 - i as f64;
                let (ff, fff) = (f * f, f * f * f);
                let wt = [3.0 * ff - 2.0 * f - fff, 3.0 * (fff - 2.0 * ff - f + 2.0), 3.0 * (ff + 2.0 * f - fff), fff - f];
                out[y * w + x] = [0, 1, 2].map(|c| {
                    let v: f64 = (0..4).map(|k| self.pixels[y * w + (i + k) % w][c] as f64 * wt[k]).sum();
                    (v / 6.0).round().clamp(0.0, 255.0) as u8
                });
            }
        }
        Map { number: self.number, width: self.width, height: self.height, pixels: out, alpha: None }
    }

    /// Blends the six outer columns of each side into each other (FitLMborder), so the
    /// panorama has no seam.
    pub fn fit_border(&mut self) {
        let (w, h) = (self.width as usize, self.height as usize);
        if w < 12 {
            return;
        }
        for x in 0..6 {
            let s2 = 0.5 - 0.5 * (std::f64::consts::FRAC_PI_2 * (x as f64 + 0.5) / 6.0).sin();
            let s1 = 1.0 - s2;
            for y in 0..h {
                let (a, b) = (self.pixels[y * w + x], self.pixels[y * w + w - 1 - x]);
                let mix = |p: f64, q: f64| [0, 1, 2].map(|c| (a[c] as f64 * p + b[c] as f64 * q).round().clamp(0.0, 255.0) as u8);
                self.pixels[y * w + x] = mix(s1, s2);
                self.pixels[y * w + w - 1 - x] = mix(s2, s1);
            }
        }
    }

    /// The 16 × 8 copy for ambient light (MakeSmallLMimage): box averages, then rows widened
    /// toward the poles.
    pub fn ambient_map(&self) -> Map {
        const W: usize = 16;
        const H: usize = 8;
        let (bw, bh) = (self.width as usize, self.height as usize);
        let avg = |s: [f64; 3], n: usize| s.map(|v| (v / n.max(1) as f64).round());
        // Columns of each row, then rows of each column (the last source pixel is left out).
        let dx = (bw as f64 - 1.0) / W as f64;
        let mut tmp = vec![[0.0; 3]; W * bh];
        for y in 0..bh {
            let mut x2 = 1;
            for x in 1..=W {
                let xe = (dx * x as f64).round() as usize;
                let (mut s, mut n) = ([0.0; 3], 0);
                while x2 <= xe {
                    let p = self.pixels[y * bw + x2 - 1];
                    for c in 0..3 {
                        s[c] += p[c] as f64;
                    }
                    x2 += 1;
                    n += 1;
                }
                tmp[y * W + x - 1] = avg(s, n);
            }
        }
        let dy = (bh as f64 - 1.0) / H as f64;
        let mut small = vec![[0.0; 3]; W * H];
        for x in 0..W {
            let mut y2 = 1;
            for y in 1..=H {
                let ye = (dy * y as f64).round() as usize;
                let (mut s, mut n) = ([0.0; 3], 0);
                while y2 <= ye {
                    let p = tmp[(y2 - 1) * W + x];
                    for c in 0..3 {
                        s[c] += p[c];
                    }
                    y2 += 1;
                    n += 1;
                }
                small[(y - 1) * W + x] = avg(s, n);
            }
        }
        // Rows near the poles cover less of the sphere: average them over more columns.
        for y in 0..H {
            let wd = if y == 0 || y >= H - 1 { W as f64 } else { 2.0 * W as f64 / ((y as f64 * std::f64::consts::PI / (H - 1) as f64).sin() + 1.0) - W as f64 };
            if wd > 0.5 {
                let b = wd.round() as i64;
                let n = -(b / 2);
                let b = b + n;
                let row: Vec<[f64; 3]> = small[y * W..(y + 1) * W].to_vec();
                for x in 0..W as i64 {
                    let mut s = [0.0; 3];
                    for x2 in n..=b {
                        let p = row[((x + x2) & 15) as usize];
                        for c in 0..3 {
                            s[c] += p[c];
                        }
                    }
                    small[y * W + x as usize] = avg(s, (b - n + 1) as usize);
                }
            }
        }
        Map { number: self.number, width: W as u32, height: H as u32, pixels: small.iter().map(|p| p.map(|v| v.clamp(0.0, 255.0) as u8)).collect(), alpha: None }
    }

    /// Like [`Map::sample_panorama`], clamped in both directions (images in screen
    /// coordinates).
    pub fn sample_clamped(&self, u: f64, v: f64) -> [f64; 3] {
        let (w, h) = (self.width as f64, self.height as f64);
        let (x, y) = (u.clamp(0.0, 1.0) * w, v.clamp(0.0, 1.0) * h);
        let (xf, yf) = (x.floor().min(w - 1.0), y.floor().min(h - 1.0));
        let (wx, wy) = (spline_weights(x - xf), spline_weights(y - yf));
        let mut out = [0.0; 3];
        for (j, wyj) in wy.iter().enumerate() {
            let row = (yf as i64 - 1 + j as i64).clamp(0, self.height as i64 - 1);
            for (i, wxi) in wx.iter().enumerate() {
                let col = (xf as i64 - 1 + i as i64).clamp(0, self.width as i64 - 1);
                let t = self.pixels[(row * self.width as i64 + col) as usize];
                for c in 0..3 {
                    out[c] += t[c] as f64 * wxi * wyj;
                }
            }
        }
        out.map(|v| v / 255.0)
    }
}

/// An image by file name (background images): an existing path, or the name in the search
/// folders. Loaded on first use.
pub fn get_file(name: &str) -> Option<Arc<Map>> {
    type Files = RwLock<HashMap<(String, u64), Option<Arc<Map>>>>;
    static FILES: OnceLock<Files> = OnceLock::new();
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

/// A map by number (all digits) or by file name.
pub fn get_any(name: &str) -> Option<Arc<Map>> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    match name.parse::<i32>() {
        Ok(n) => get(n),
        Err(_) => get_file(name),
    }
}

impl Map {
    /// RGB (0..1, display values) at `u`, `v` in 0..1, tiled in both directions.
    pub fn rgb_tiled(&self, u: f64, v: f64) -> [f64; 3] {
        let s = self.sample((u - u.floor()) * self.width as f64, (v - v.floor()) * self.height as f64);
        [s[2], s[1], s[0]]
    }

    /// Average RGB (0..1, display values).
    pub fn average(&self) -> [f64; 3] {
        let n = self.pixels.len().max(1) as f64;
        let mut a = [0.0; 3];
        for p in &self.pixels {
            for k in 0..3 {
                a[k] += p[2 - k] as f64;
            }
        }
        a.map(|v| v / (255.0 * n))
    }
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
        let m = Map { number: 1, width: 4, height: 3, pixels: vec![[10, 20, 30]; 12], alpha: None };
        let v = m.sample(3.7, 0.2);
        assert!((v[0] - 10.0 / 255.0).abs() < 1e-12 && (v[2] - 30.0 / 255.0).abs() < 1e-12);
    }

    #[test]
    fn smoothing_spreads_a_pixel_with_mb3d_weights() {
        let mut pixels = vec![[0u8; 3]; 64];
        pixels[4 * 8 + 4] = [200, 100, 0];
        let m = Map { number: 1, width: 8, height: 8, pixels, alpha: None }.smoothed();
        let at = |x: usize, y: usize| m.pixels[y * 8 + x][0];
        // 200 · 0.6 · 0.6 = 72, 200 · 0.6 · 0.25 = 30, 200 · 0.25 · 0.25 = 12.5 → 12 (halves to even).
        assert_eq!((at(4, 4), at(5, 4), at(4, 3), at(5, 5)), (72, 30, 30, 12));
        // Negative lobes clip at 0.
        assert_eq!(at(6, 4), 0);
        let flat = Map { number: 1, width: 5, height: 4, pixels: vec![[10, 20, 30]; 20], alpha: None }.smoothed();
        assert!(flat.pixels.iter().all(|p| *p == [10, 20, 30]));
    }
}
