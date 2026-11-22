// SPDX-License-Identifier: LGPL-2.1-or-later

//! 3D models (OBJ with MTL, STL, PLY, glTF/GLB) baked into signed distance volumes.
//!
//! [`load`] reads a model into a [`Mesh`] (triangles in Z-up model space plus optional color
//! information). [`bake`] samples the signed distance on a regular grid ([`Volume`]): the
//! unsigned distance is the exact point-triangle distance (BVH accelerated), the sign comes
//! from ray parity along the three axes with a majority vote, so small holes in the mesh do not
//! flip whole regions. Models with colors also get a color grid at half the resolution.
//!
//! Baked volumes are cached by canonical path, file modification time and options
//! ([`request`] bakes in the background, [`get`] blocks).

use std::collections::HashMap;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::time::SystemTime;

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

/// Range of the stored distances in voxels: grid values saturate at ±`DIST_RANGE` voxels.
pub const DIST_RANGE: f64 = 64.0;

/// Smallest and largest grid resolution (points along the longest axis).
pub const MIN_RESOLUTION: u32 = 16;
pub const MAX_RESOLUTION: u32 = 256;

/// Grid voxels between the tight mesh bounds and the grid box on every side.
const PADDING: f64 = 2.0;

/// Up axis of a model file. Models are converted to the renderer's Z-up space.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum UpAxis {
    /// By format: glTF and OBJ are Y-up, STL and PLY Z-up.
    #[default]
    Auto,
    Y,
    Z,
}

/// How a model is baked.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelOptions {
    /// Grid points along the longest axis (clamped to 16..=256).
    pub resolution: u32,
    pub up_axis: UpAxis,
    /// Keep the file's coordinates. Otherwise the model is centered at the origin and scaled
    /// so that its longest side is 2.
    pub keep_units: bool,
}

impl Default for ModelOptions {
    fn default() -> Self {
        Self { resolution: 128, up_axis: UpAxis::Auto, keep_units: false }
    }
}

impl ModelOptions {
    /// The resolution actually used.
    pub fn clamped_resolution(&self) -> u32 {
        self.resolution.clamp(MIN_RESOLUTION, MAX_RESOLUTION)
    }
}

// ---------------------------------------------------------------------------------------------
// Mesh

/// A triangle mesh in model space (Z-up after [`load`]).
#[derive(Clone, Debug, Default)]
pub struct Mesh {
    pub triangles: Vec<[[f64; 3]; 3]>,
    /// Color information per triangle: empty when the model has neither materials nor vertex
    /// colors, otherwise one entry per triangle.
    pub shading: Vec<Shading>,
    /// Materials referenced by [`Shading::material`].
    pub materials: Vec<MeshMaterial>,
    /// Textures referenced by [`MeshMaterial::texture`].
    pub textures: Vec<Texture>,
}

/// Material index of triangles without a material (plain white).
pub const NO_MATERIAL: u32 = u32::MAX;

/// Color information of one triangle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shading {
    /// Corner vertex colors in linear RGB (white without vertex colors).
    pub colors: [[f32; 3]; 3],
    /// Corner texture coordinates.
    pub uv: [[f32; 2]; 3],
    /// Index into [`Mesh::materials`] or [`NO_MATERIAL`].
    pub material: u32,
}

impl Default for Shading {
    fn default() -> Self {
        Self { colors: [[1.0; 3]; 3], uv: [[0.0; 2]; 3], material: NO_MATERIAL }
    }
}

/// A material of a model file.
#[derive(Clone, Debug, PartialEq)]
pub struct MeshMaterial {
    pub name: String,
    /// Base color in linear RGB.
    pub color: [f32; 3],
    /// Color texture (index into [`Mesh::textures`]).
    pub texture: Option<u32>,
    /// Image files the model names for color, bump and specular ("" if none; the color one may
    /// be set although it could not be loaded).
    pub texture_file: String,
    pub bump_file: String,
    pub specular_file: String,
}

impl Default for MeshMaterial {
    fn default() -> Self {
        Self { name: String::new(), color: [1.0; 3], texture: None, texture_file: String::new(), bump_file: String::new(), specular_file: String::new() }
    }
}

/// Largest texture side; larger images are scaled down when loaded.
pub const MAX_TEXTURE: u32 = 2048;

/// Texture addressing outside 0..1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wrap {
    Repeat,
    Clamp,
    Mirror,
}

/// An RGB texture (sRGB display values, top row first).
#[derive(Clone, Debug)]
pub struct Texture {
    pub width: u32,
    pub height: u32,
    pub texels: Vec<[u8; 3]>,
    /// Wrap modes for u and v.
    pub wrap: [Wrap; 2],
    /// `v = 0` is the bottom row (OBJ); otherwise the top row (glTF).
    pub v_up: bool,
}

impl Texture {
    fn from_rgb(img: image::RgbImage, wrap: [Wrap; 2], v_up: bool) -> Option<Texture> {
        let (width, height) = img.dimensions();
        if width.max(height) > MAX_TEXTURE {
            let s = MAX_TEXTURE as f64 / width.max(height) as f64;
            let (w, h) = (((width as f64 * s).round() as u32).max(1), ((height as f64 * s).round() as u32).max(1));
            return Texture::from_rgb(image::imageops::resize(&img, w, h, image::imageops::FilterType::Triangle), wrap, v_up);
        }
        if width == 0 || height == 0 {
            return None;
        }
        let texels = img.pixels().map(|p| p.0).collect();
        Some(Texture { width, height, texels, wrap, v_up })
    }

    fn texel(&self, x: i64, y: i64) -> [f32; 3] {
        let wrap = |i: i64, n: i64, mode: Wrap| match mode {
            Wrap::Repeat => i.rem_euclid(n),
            Wrap::Clamp => i.clamp(0, n - 1),
            Wrap::Mirror => {
                let m = i.rem_euclid(2 * n);
                if m >= n { 2 * n - 1 - m } else { m }
            }
        };
        let (w, h) = (self.width as i64, self.height as i64);
        let t = self.texels[(wrap(y, h, self.wrap[1]) * w + wrap(x, w, self.wrap[0])) as usize];
        let lut = srgb_lut();
        t.map(|c| lut[c as usize])
    }

    /// Bilinear sample at texture coordinates `u`, `v`, in linear RGB.
    pub fn sample(&self, u: f64, v: f64) -> [f32; 3] {
        if !(u.is_finite() && v.is_finite()) {
            return [0.0; 3];
        }
        let v = if self.v_up { 1.0 - v } else { v };
        // Keep the coordinates small so that the integer texel indices cannot overflow.
        let (u, v) = (reduce_coord(u, self.wrap[0]), reduce_coord(v, self.wrap[1]));
        let x = u * self.width as f64 - 0.5;
        let y = v * self.height as f64 - 0.5;
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = ((x - x0) as f32, (y - y0) as f32);
        let (xi, yi) = (x0 as i64, y0 as i64);
        let (a, b) = (self.texel(xi, yi), self.texel(xi + 1, yi));
        let (c, d) = (self.texel(xi, yi + 1), self.texel(xi + 1, yi + 1));
        std::array::from_fn(|k| {
            let top = a[k] + (b[k] - a[k]) * fx;
            let bottom = c[k] + (d[k] - c[k]) * fx;
            top + (bottom - top) * fy
        })
    }
}

fn reduce_coord(t: f64, mode: Wrap) -> f64 {
    match mode {
        Wrap::Repeat => t - t.floor(),
        Wrap::Mirror => t - 2.0 * (t * 0.5).floor(),
        Wrap::Clamp => t.clamp(-1.0, 2.0),
    }
}

impl Mesh {
    /// Adds a triangle (skipped if a coordinate is not finite).
    pub fn push(&mut self, tri: [[f64; 3]; 3], shading: Shading) {
        if tri.iter().flatten().all(|c| c.is_finite()) {
            self.triangles.push(tri);
            self.shading.push(shading);
        }
    }

    /// Tight bounding box (min, max); None without triangles.
    pub fn bounds(&self) -> Option<[[f64; 3]; 2]> {
        bounds_of(&self.triangles)
    }

    /// Whether the model has any color information (a material color, texture or vertex color
    /// other than white).
    pub fn has_colors(&self) -> bool {
        self.materials.iter().any(|m| m.color != [1.0; 3] || m.texture.is_some()) || self.shading.iter().any(|s| s.colors != [[1.0; 3]; 3])
    }

    /// Converts Y-up coordinates to Z-up: (x, y, z) -> (x, -z, y).
    pub fn y_up_to_z_up(&mut self) {
        for t in &mut self.triangles {
            for p in t.iter_mut() {
                *p = [p[0], -p[2], p[1]];
            }
        }
    }
}

fn bounds_of(tris: &[[[f64; 3]; 3]]) -> Option<[[f64; 3]; 2]> {
    let mut b = [[f64::INFINITY; 3], [f64::NEG_INFINITY; 3]];
    for p in tris.iter().flatten() {
        for k in 0..3 {
            b[0][k] = b[0][k].min(p[k]);
            b[1][k] = b[1][k].max(p[k]);
        }
    }
    (!tris.is_empty()).then_some(b)
}

// ---------------------------------------------------------------------------------------------
// Colors

fn srgb_to_linear(c: f64) -> f64 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn linear_to_srgb(c: f64) -> f64 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.0031308 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
}

/// Linear values of the 8-bit sRGB levels.
fn srgb_lut() -> &'static [f32; 256] {
    static LUT: OnceLock<[f32; 256]> = OnceLock::new();
    LUT.get_or_init(|| std::array::from_fn(|i| srgb_to_linear(i as f64 / 255.0) as f32))
}

fn display_to_linear(c: [f32; 3]) -> [f32; 3] {
    c.map(|v| srgb_to_linear(v.clamp(0.0, 1.0) as f64) as f32)
}

fn linear_to_display_u8(c: [f64; 3]) -> [u8; 4] {
    let e = c.map(|v| (linear_to_srgb(v) * 255.0).round() as u8);
    [e[0], e[1], e[2], 255]
}

// ---------------------------------------------------------------------------------------------
// Loaders

/// Loads a model file (OBJ, STL, PLY, glTF, GLB by extension) and converts it to Z-up.
pub fn load(path: &Path, up: UpAxis) -> Result<Mesh, String> {
    let ext = path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).unwrap_or_default();
    let (mut mesh, y_up) = match ext.as_str() {
        "obj" => (load_obj(path)?, true),
        "stl" => (load_stl(path)?, false),
        "ply" => (load_ply(path)?, false),
        "gltf" | "glb" => (load_gltf(path)?, true),
        _ => return Err(format!("{}: unsupported model format (use OBJ, STL, PLY, glTF or GLB)", path.display())),
    };
    if mesh.triangles.is_empty() {
        return Err(format!("{}: the model has no triangles", path.display()));
    }
    let convert = match up {
        UpAxis::Auto => y_up,
        UpAxis::Y => true,
        UpAxis::Z => false,
    };
    if convert {
        mesh.y_up_to_z_up();
    }
    Ok(mesh)
}

fn err_at(path: &Path) -> impl Fn(&dyn std::fmt::Display) -> String + '_ {
    move |e| format!("{}: {e}", path.display())
}

fn load_obj(path: &Path) -> Result<Mesh, String> {
    let err = err_at(path);
    let opts = tobj::LoadOptions { triangulate: true, single_index: true, ignore_points: true, ignore_lines: true };
    let (models, materials) = tobj::load_obj(path, &opts).map_err(|e| err(&e))?;
    // A missing or broken MTL file only loses the colors.
    let materials = materials.unwrap_or_default();
    let mut mesh = Mesh::default();
    let mut textures: HashMap<String, Option<u32>> = HashMap::new();
    for mat in &materials {
        let texture = mat.diffuse_texture.as_ref().and_then(|name| {
            *textures.entry(name.clone()).or_insert_with(|| {
                let tex = load_texture_file(path, name, [Wrap::Repeat; 2], true)?;
                mesh.textures.push(tex);
                Some(mesh.textures.len() as u32 - 1)
            })
        });
        mesh.materials.push(MeshMaterial {
            name: mat.name.clone(),
            color: mat.diffuse.map(display_to_linear).unwrap_or([1.0; 3]),
            texture,
            texture_file: mat.diffuse_texture.as_deref().map(texture_name).unwrap_or_default(),
            bump_file: mat.normal_texture.as_deref().map(texture_name).unwrap_or_default(),
            specular_file: mat.specular_texture.as_deref().map(texture_name).unwrap_or_default(),
        });
    }
    let mut detailed = !mesh.materials.is_empty();
    for model in &models {
        let m = &model.mesh;
        let pos = |i: u32| {
            let i = i as usize * 3;
            [m.positions[i] as f64, m.positions[i + 1] as f64, m.positions[i + 2] as f64]
        };
        let material = m.material_id.filter(|&i| i < materials.len()).map_or(NO_MATERIAL, |i| i as u32);
        let has_uv = m.texcoords.len() >= m.positions.len() / 3 * 2;
        let vertex_colors = !m.vertex_color.is_empty() && m.vertex_color.len() >= m.positions.len();
        detailed |= vertex_colors;
        for f in m.indices.as_chunks::<3>().0 {
            if f.iter().any(|&i| i as usize * 3 + 2 >= m.positions.len()) {
                continue;
            }
            let mut s = Shading { material, ..Shading::default() };
            for (c, &i) in f.iter().enumerate() {
                let i = i as usize;
                if vertex_colors {
                    s.colors[c] = display_to_linear([m.vertex_color[i * 3], m.vertex_color[i * 3 + 1], m.vertex_color[i * 3 + 2]]);
                }
                if has_uv {
                    s.uv[c] = [m.texcoords[i * 2], m.texcoords[i * 2 + 1]];
                }
            }
            mesh.push([pos(f[0]), pos(f[1]), pos(f[2])], s);
        }
    }
    if !detailed {
        mesh.shading.clear();
    }
    Ok(mesh)
}

/// The image file name of an MTL texture statement (options like `-s 1 1 1` before the name are
/// skipped; backslashes become slashes).
fn texture_name(spec: &str) -> String {
    let spec = spec.trim();
    let name = if spec.starts_with('-') { spec.split_whitespace().last().unwrap_or("") } else { spec };
    name.replace('\\', "/")
}

/// Finds the image `name` that a model file refers to: relative to the model's folder, then
/// by its file name among the images near the model ([`nearby_images`], case-insensitive).
pub fn find_image(model: &Path, name: &str) -> Option<PathBuf> {
    let name = texture_name(name);
    if name.is_empty() {
        return None;
    }
    let rel = Path::new(&name);
    if rel.is_absolute() && rel.is_file() {
        return Some(rel.to_path_buf());
    }
    let dir = model.parent().unwrap_or(Path::new("."));
    let file = rel.file_name()?;
    for p in [dir.join(rel), dir.join(file)] {
        if p.is_file() {
            return Some(p);
        }
    }
    let file = file.to_string_lossy().to_lowercase();
    nearby_images(model).into_iter().find(|p| p.file_name().is_some_and(|f| f.to_string_lossy().to_lowercase() == file))
}

/// Image extensions the texture loader reads.
pub const IMAGE_EXTENSIONS: [&str; 9] = ["png", "jpg", "jpeg", "bmp", "tga", "webp", "ppm", "pgm", "pnm"];

fn is_image(p: &Path) -> bool {
    p.is_file() && p.extension().and_then(|e| e.to_str()).is_some_and(|e| IMAGE_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// Images near a model file: in its folder, in the folders inside it (two levels deep) and in
/// a `textures` folder next to the model's folder. Sorted by path.
pub fn nearby_images(model: &Path) -> Vec<PathBuf> {
    let dir = model.parent().map(|d| if d.as_os_str().is_empty() { Path::new(".") } else { d }).unwrap_or(Path::new("."));
    let mut out = Vec::new();
    collect_images(dir, 2, &mut out);
    if let Some(parent) = dir.parent() {
        for name in ["textures", "Textures", "tex", "maps", "images"] {
            let d = parent.join(name);
            if d.is_dir() && d != dir {
                collect_images(&d, 1, &mut out);
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Adds the images in `d` and, `depth` levels down, in its subfolders (at most 4096 files).
fn collect_images(d: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(d) else { return };
    let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if out.len() >= 4096 {
            return;
        }
        if is_image(&p) {
            out.push(p);
        } else if depth > 0 && p.is_dir() {
            collect_images(&p, depth - 1, out);
        }
    }
}

/// Loads an image a model file refers to (see [`find_image`]).
fn load_texture_file(model: &Path, spec: &str, wrap: [Wrap; 2], v_up: bool) -> Option<Texture> {
    let file = find_image(model, spec)?;
    Texture::from_rgb(image::open(file).ok()?.to_rgb8(), wrap, v_up)
}

fn load_stl(path: &Path) -> Result<Mesh, String> {
    let err = err_at(path);
    let mut file = BufReader::new(std::fs::File::open(path).map_err(|e| err(&e))?);
    let reader = stl_io::create_stl_reader(&mut file).map_err(|e| err(&e))?;
    let mut mesh = Mesh::default();
    for t in reader {
        let t = t.map_err(|e| err(&e))?;
        mesh.push(t.vertices.map(|v| v.0.map(|c| c as f64)), Shading::default());
    }
    mesh.shading.clear();
    Ok(mesh)
}

fn ply_scalar(p: &ply_rs_bw::ply::Property) -> Option<f64> {
    use ply_rs_bw::ply::Property as P;
    Some(match *p {
        P::Char(v) => v as f64,
        P::UChar(v) => v as f64,
        P::Short(v) => v as f64,
        P::UShort(v) => v as f64,
        P::Int(v) => v as f64,
        P::UInt(v) => v as f64,
        P::Float(v) => v as f64,
        P::Double(v) => v,
        _ => return None,
    })
}

/// A color channel scaled to 0..1 (integers by their type's range, floats as they are).
fn ply_channel(p: &ply_rs_bw::ply::Property) -> Option<f32> {
    use ply_rs_bw::ply::Property as P;
    Some(match *p {
        P::UChar(v) => v as f32 / 255.0,
        P::Char(v) => v.max(0) as f32 / 127.0,
        P::UShort(v) => v as f32 / 65535.0,
        P::Float(v) => v,
        P::Double(v) => v as f32,
        _ => ply_scalar(p)? as f32 / 255.0,
    })
}

fn ply_list(p: &ply_rs_bw::ply::Property) -> Option<Vec<i64>> {
    use ply_rs_bw::ply::Property as P;
    Some(match p {
        P::ListChar(v) => v.iter().map(|&i| i as i64).collect(),
        P::ListUChar(v) => v.iter().map(|&i| i as i64).collect(),
        P::ListShort(v) => v.iter().map(|&i| i as i64).collect(),
        P::ListUShort(v) => v.iter().map(|&i| i as i64).collect(),
        P::ListInt(v) => v.iter().map(|&i| i as i64).collect(),
        P::ListUInt(v) => v.iter().map(|&i| i as i64).collect(),
        _ => return None,
    })
}

fn load_ply(path: &Path) -> Result<Mesh, String> {
    let err = err_at(path);
    let mut file = BufReader::new(std::fs::File::open(path).map_err(|e| err(&e))?);
    let parser = ply_rs_bw::parser::Parser::<ply_rs_bw::ply::DefaultElement>::new();
    let ply = parser.read_ply(&mut file).map_err(|e| err(&e))?;
    let vertices = ply.payload.get("vertex").ok_or_else(|| err(&"no vertex element"))?;
    let mut pos = Vec::with_capacity(vertices.len());
    let mut colors = Vec::with_capacity(vertices.len());
    for v in vertices {
        let coord = |k: &str| v.get(k).and_then(ply_scalar).unwrap_or(f64::NAN);
        pos.push([coord("x"), coord("y"), coord("z")]);
        let channel = |names: [&str; 2]| names.iter().find_map(|k| v.get(*k)).and_then(ply_channel);
        let rgb = [channel(["red", "diffuse_red"]), channel(["green", "diffuse_green"]), channel(["blue", "diffuse_blue"])];
        colors.push(match rgb {
            [Some(r), Some(g), Some(b)] => Some(display_to_linear([r, g, b])),
            _ => None,
        });
    }
    let colored = colors.iter().any(|c| c.is_some());
    let faces = ply.payload.get("face").ok_or_else(|| err(&"no face element"))?;
    let mut mesh = Mesh::default();
    for f in faces {
        let Some(idx) = ["vertex_indices", "vertex_index"].iter().find_map(|k| f.get(*k)).and_then(ply_list) else { continue };
        if idx.len() < 3 || idx.iter().any(|&i| i < 0 || i as usize >= pos.len()) {
            continue;
        }
        // Fan triangulation for polygons.
        for k in 1..idx.len() - 1 {
            let c = [idx[0], idx[k], idx[k + 1]].map(|i| i as usize);
            let colors = c.map(|i| colors[i].unwrap_or([1.0; 3]));
            mesh.push(c.map(|i| pos[i]), Shading { colors, ..Shading::default() });
        }
    }
    if !colored {
        mesh.shading.clear();
    }
    Ok(mesh)
}

type Mat4 = [[f64; 4]; 4];

/// Column-major product `a * b`.
fn mat_mul(a: &Mat4, b: &Mat4) -> Mat4 {
    std::array::from_fn(|c| std::array::from_fn(|r| (0..4).map(|k| a[k][r] * b[c][k]).sum()))
}

fn mat_apply(m: &Mat4, p: [f32; 3]) -> [f64; 3] {
    let p = p.map(|c| c as f64);
    std::array::from_fn(|r| m[0][r] * p[0] + m[1][r] * p[1] + m[2][r] * p[2] + m[3][r])
}

struct GltfLoader<'a> {
    buffers: Vec<gltf::buffer::Data>,
    base: Option<&'a Path>,
    path: &'a Path,
    /// Texture index per (image, wrap u, wrap v); None if the image cannot be decoded.
    textures: HashMap<(usize, [u8; 2]), Option<u32>>,
    mesh: Mesh,
    /// Whether the shading is worth keeping (materials, vertex colors).
    detailed: bool,
}

fn gltf_wrap(m: gltf::texture::WrappingMode) -> Wrap {
    match m {
        gltf::texture::WrappingMode::ClampToEdge => Wrap::Clamp,
        gltf::texture::WrappingMode::MirroredRepeat => Wrap::Mirror,
        gltf::texture::WrappingMode::Repeat => Wrap::Repeat,
    }
}

/// Decoded glTF image data as 8-bit sRGB RGB.
fn gltf_image_rgb(d: &gltf::image::Data) -> Option<image::RgbImage> {
    use gltf::image::Format as F;
    let px = &d.pixels;
    let n = d.width as usize * d.height as usize;
    let u16_at = |i: usize| (u16::from_ne_bytes([px[i], px[i + 1]]) >> 8) as u8;
    let f32_at = |i: usize| (linear_to_srgb(f32::from_ne_bytes([px[i], px[i + 1], px[i + 2], px[i + 3]]) as f64) * 255.0).round() as u8;
    let (stride, get): (usize, &dyn Fn(usize) -> [u8; 3]) = match d.format {
        F::R8 => (1, &|i| [px[i]; 3]),
        F::R8G8 => (2, &|i| [px[i]; 3]),
        F::R8G8B8 => (3, &|i| [px[i], px[i + 1], px[i + 2]]),
        F::R8G8B8A8 => (4, &|i| [px[i], px[i + 1], px[i + 2]]),
        F::R16 => (2, &|i| [u16_at(i); 3]),
        F::R16G16 => (4, &|i| [u16_at(i); 3]),
        F::R16G16B16 => (6, &|i| [u16_at(i), u16_at(i + 2), u16_at(i + 4)]),
        F::R16G16B16A16 => (8, &|i| [u16_at(i), u16_at(i + 2), u16_at(i + 4)]),
        F::R32G32B32FLOAT => (12, &|i| [f32_at(i), f32_at(i + 4), f32_at(i + 8)]),
        F::R32G32B32A32FLOAT => (16, &|i| [f32_at(i), f32_at(i + 4), f32_at(i + 8)]),
    };
    if px.len() < n * stride {
        return None;
    }
    let raw: Vec<u8> = (0..n).flat_map(|i| get(i * stride)).collect();
    image::RgbImage::from_raw(d.width, d.height, raw)
}

impl GltfLoader<'_> {
    fn texture(&mut self, doc: &gltf::Document, tex: &gltf::Texture) -> Option<u32> {
        let sampler = tex.sampler();
        let wrap = [gltf_wrap(sampler.wrap_s()), gltf_wrap(sampler.wrap_t())];
        let image = tex.source().index();
        let key = (image, wrap.map(|w| w as u8));
        if let Some(t) = self.textures.get(&key) {
            return *t;
        }
        let t = doc
            .images()
            .nth(image)
            .and_then(|img| {
                gltf::image::Data::from_source(img.source(), self.base, &self.buffers).ok().and_then(|data| gltf_image_rgb(&data)).or_else(|| match img.source() {
                    // A file that is not where the model says: look for it near the model.
                    gltf::image::Source::Uri { uri, .. } if !uri.starts_with("data:") => {
                        let name = percent_decode(uri);
                        Some(image::open(find_image(self.path, &name)?).ok()?.to_rgb8())
                    }
                    _ => None,
                })
            })
            .and_then(|rgb| Texture::from_rgb(rgb, wrap, false))
            .map(|t| {
                self.mesh.textures.push(t);
                self.mesh.textures.len() as u32 - 1
            });
        self.textures.insert(key, t);
        t
    }

    /// Fills `mesh.materials` from the document (same indices).
    fn materials(&mut self, doc: &gltf::Document) {
        for (i, m) in doc.materials().enumerate() {
            let pbr = m.pbr_metallic_roughness();
            let f = pbr.base_color_factor();
            let file = |t: Option<gltf::Texture>| match t.map(|t| t.source().source()) {
                Some(gltf::image::Source::Uri { uri, .. }) if !uri.starts_with("data:") => percent_decode(uri),
                Some(_) => "(embedded)".to_string(),
                None => String::new(),
            };
            let texture_file = file(pbr.base_color_texture().map(|t| t.texture()));
            self.mesh
                .materials
                .push(MeshMaterial { name: m.name().map_or_else(|| format!("Material {}", i + 1), str::to_string), color: [f[0], f[1], f[2]], texture: None, texture_file, ..MeshMaterial::default() });
        }
        self.detailed |= !self.mesh.materials.is_empty();
    }

    fn node(&mut self, doc: &gltf::Document, node: &gltf::Node, parent: &Mat4, depth: usize) {
        if depth > 64 {
            return;
        }
        let local = node.transform().matrix().map(|c| c.map(|v| v as f64));
        let m = mat_mul(parent, &local);
        if let Some(mesh) = node.mesh() {
            self.mesh_prims(doc, &mesh, &m);
        }
        for child in node.children() {
            self.node(doc, &child, &m, depth + 1);
        }
    }

    fn mesh_prims(&mut self, doc: &gltf::Document, mesh: &gltf::Mesh, m: &Mat4) {
        use gltf::mesh::Mode;
        for prim in mesh.primitives() {
            let mode = prim.mode();
            if !matches!(mode, Mode::Triangles | Mode::TriangleStrip | Mode::TriangleFan) {
                continue;
            }
            let buffers = &self.buffers;
            let reader = prim.reader(|b| buffers.get(b.index()).map(|d| &d.0[..]));
            let Some(positions) = reader.read_positions() else { continue };
            let pos: Vec<[f64; 3]> = positions.map(|p| mat_apply(m, p)).collect();
            let indices: Vec<u32> = match reader.read_indices() {
                Some(i) => i.into_u32().collect(),
                None => (0..pos.len() as u32).collect(),
            };
            let vcolors: Option<Vec<[f32; 4]>> = reader.read_colors(0).map(|c| c.into_rgba_f32().collect());
            let material = prim.material();
            let mat_index = material.index().filter(|&i| i < self.mesh.materials.len()).map_or(NO_MATERIAL, |i| i as u32);
            let tex_info = material.pbr_metallic_roughness().base_color_texture();
            let uv_set = tex_info.as_ref().map_or(0, |info| info.tex_coord());
            let uvs: Option<Vec<[f32; 2]>> = reader.read_tex_coords(uv_set).map(|t| t.into_f32().collect());
            if let Some(info) = &tex_info
                && uvs.is_some()
                && mat_index != NO_MATERIAL
                && self.mesh.materials[mat_index as usize].texture.is_none()
            {
                let t = self.texture(doc, &info.texture());
                self.mesh.materials[mat_index as usize].texture = t;
            }
            let vcolors = vcolors.filter(|c| c.len() >= pos.len());
            let uvs = uvs.filter(|u| u.len() >= pos.len());
            self.detailed |= vcolors.is_some();
            let n = indices.len();
            let corners: Vec<[usize; 3]> = match mode {
                Mode::TriangleStrip => (0..n.saturating_sub(2)).map(|i| if i % 2 == 0 { [i, i + 1, i + 2] } else { [i + 1, i, i + 2] }).collect(),
                Mode::TriangleFan => (1..n.saturating_sub(1)).map(|i| [0, i, i + 1]).collect(),
                _ => (0..n / 3).map(|i| [3 * i, 3 * i + 1, 3 * i + 2]).collect(),
            };
            for c in corners {
                let v = c.map(|k| indices[k] as usize);
                if v.iter().any(|&i| i >= pos.len()) {
                    continue;
                }
                let mut s = Shading { material: mat_index, ..Shading::default() };
                for (k, &i) in v.iter().enumerate() {
                    if let Some(c) = &vcolors {
                        s.colors[k] = [c[i][0], c[i][1], c[i][2]];
                    }
                    if let Some(u) = &uvs {
                        s.uv[k] = u[i];
                    }
                }
                self.mesh.push(v.map(|i| pos[i]), s);
            }
        }
    }
}

fn load_gltf(path: &Path) -> Result<Mesh, String> {
    let err = err_at(path);
    let gltf::Gltf { document, blob } = gltf::Gltf::open(path).map_err(|e| err(&e))?;
    let base = path.parent();
    let buffers = gltf::import_buffers(&document, base, blob).map_err(|e| err(&e))?;
    let mut l = GltfLoader { buffers, base, path, textures: HashMap::new(), mesh: Mesh::default(), detailed: false };
    l.materials(&document);
    const IDENTITY: Mat4 = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]];
    match document.default_scene().or_else(|| document.scenes().next()) {
        Some(scene) => {
            for node in scene.nodes() {
                l.node(&document, &node, &IDENTITY, 0);
            }
        }
        // No scene: every mesh untransformed.
        None => {
            for mesh in document.meshes() {
                l.mesh_prims(&document, &mesh, &IDENTITY);
            }
        }
    }
    let mut mesh = l.mesh;
    if !l.detailed {
        mesh.shading.clear();
    }
    Ok(mesh)
}

/// Decodes `%xx` escapes of a URI.
fn percent_decode(uri: &str) -> String {
    let b = uri.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("zz"), 16)
        {
            out.push(v);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

// ---------------------------------------------------------------------------------------------
// Geometry

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

/// Distance from `q` to the box `b` (0 inside).
fn box_distance(q: [f64; 3], b: &[[f64; 3]; 2]) -> f64 {
    box_d2(q, &b[0], &b[1]).sqrt()
}

fn box_d2(q: [f64; 3], lo: &[f64; 3], hi: &[f64; 3]) -> f64 {
    let mut d2 = 0.0;
    for k in 0..3 {
        let d = (lo[k] - q[k]).max(q[k] - hi[k]).max(0.0);
        d2 += d * d;
    }
    d2
}

/// A triangle prepared for closest-point queries.
#[derive(Clone, Copy)]
struct Tri {
    a: [f64; 3],
    ab: [f64; 3],
    ac: [f64; 3],
}

impl Tri {
    fn new(t: &[[f64; 3]; 3]) -> Tri {
        Tri { a: t[0], ab: sub(t[1], t[0]), ac: sub(t[2], t[0]) }
    }

    /// Squared distance from `p` to the triangle and the barycentric weights (v, w) of the
    /// closest point `a + v ab + w ac` (Ericson, Real-Time Collision Detection 5.1.5).
    fn closest(&self, p: [f64; 3]) -> (f64, f64, f64) {
        let (ab, ac) = (self.ab, self.ac);
        let ap = sub(p, self.a);
        let (d1, d2) = (dot(ab, ap), dot(ac, ap));
        let (v, w) = 'region: {
            if d1 <= 0.0 && d2 <= 0.0 {
                break 'region (0.0, 0.0);
            }
            let bp = sub(ap, ab);
            let (d3, d4) = (dot(ab, bp), dot(ac, bp));
            if d3 >= 0.0 && d4 <= d3 {
                break 'region (1.0, 0.0);
            }
            let vc = d1 * d4 - d3 * d2;
            if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
                break 'region (d1 / (d1 - d3), 0.0);
            }
            let cp = sub(ap, ac);
            let (d5, d6) = (dot(ab, cp), dot(ac, cp));
            if d6 >= 0.0 && d5 <= d6 {
                break 'region (0.0, 1.0);
            }
            let vb = d5 * d2 - d1 * d6;
            if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
                break 'region (0.0, d2 / (d2 - d6));
            }
            let va = d3 * d6 - d5 * d4;
            if va <= 0.0 && d4 - d3 >= 0.0 && d5 - d6 >= 0.0 {
                let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
                break 'region (1.0 - w, w);
            }
            let s = va + vb + vc;
            if s > 0.0 { (vb / s, vc / s) } else { (0.0, 0.0) }
        };
        let d = [0, 1, 2].map(|k| ap[k] - v * ab[k] - w * ac[k]);
        (dot(d, d), v, w)
    }
}

const NONE: u32 = u32::MAX;
const LEAF_SIZE: usize = 4;

#[derive(Clone, Copy)]
struct Node {
    lo: [f64; 3],
    hi: [f64; 3],
    /// Leaf: first triangle; inner node: index of the left child (the right one follows).
    start: u32,
    /// Triangles of a leaf; 0 for inner nodes.
    count: u32,
}

/// Bounding volume hierarchy over triangles (median split along the longest centroid axis).
struct Bvh {
    nodes: Vec<Node>,
    /// Triangles in leaf order.
    tris: Vec<Tri>,
    /// Original triangle index of each entry of `tris`.
    ids: Vec<u32>,
}

impl Bvh {
    fn build(tris: &[[[f64; 3]; 3]], ids: Vec<u32>) -> Bvh {
        let tri_box = |i: u32| {
            let t = &tris[i as usize];
            let lo: [f64; 3] = std::array::from_fn(|k| t[0][k].min(t[1][k]).min(t[2][k]));
            let hi: [f64; 3] = std::array::from_fn(|k| t[0][k].max(t[1][k]).max(t[2][k]));
            (lo, hi)
        };
        let centroid = |i: u32| {
            let t = &tris[i as usize];
            std::array::from_fn::<f64, 3, _>(|k| (t[0][k] + t[1][k] + t[2][k]) / 3.0)
        };
        let mut order = ids;
        let mut nodes = Vec::new();
        if !order.is_empty() {
            nodes.push(Node { lo: [0.0; 3], hi: [0.0; 3], start: 0, count: 0 });
            let mut stack = vec![(0usize, 0usize, order.len())];
            while let Some((ni, s, e)) = stack.pop() {
                let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
                let (mut clo, mut chi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
                for &i in &order[s..e] {
                    let (a, b) = tri_box(i);
                    let c = centroid(i);
                    for k in 0..3 {
                        lo[k] = lo[k].min(a[k]);
                        hi[k] = hi[k].max(b[k]);
                        clo[k] = clo[k].min(c[k]);
                        chi[k] = chi[k].max(c[k]);
                    }
                }
                nodes[ni].lo = lo;
                nodes[ni].hi = hi;
                let ext = [0, 1, 2].map(|k| chi[k] - clo[k]);
                let axis = if ext[0] >= ext[1] && ext[0] >= ext[2] { 0 } else if ext[1] >= ext[2] { 1 } else { 2 };
                if e - s <= LEAF_SIZE || ext[axis] <= 0.0 {
                    nodes[ni].start = s as u32;
                    nodes[ni].count = (e - s) as u32;
                    continue;
                }
                let mid = (s + e) / 2;
                order[s..e].select_nth_unstable_by(mid - s, |&a, &b| centroid(a)[axis].total_cmp(&centroid(b)[axis]));
                let l = nodes.len();
                nodes.push(nodes[ni]);
                nodes.push(nodes[ni]);
                nodes[ni].start = l as u32;
                nodes[ni].count = 0;
                stack.push((l, s, mid));
                stack.push((l + 1, mid, e));
            }
        }
        let tris = order.iter().map(|&i| Tri::new(&tris[i as usize])).collect();
        Bvh { nodes, tris, ids: order }
    }

    /// Nearest triangle (index into `tris`) closer than `sqrt(best_d2)`, starting from the
    /// candidate `best` (or NONE). Returns the squared distance and the triangle.
    fn nearest(&self, p: [f64; 3], mut best_d2: f64, mut best: u32) -> (f64, u32) {
        let Some(root) = self.nodes.first() else { return (best_d2, best) };
        let d = box_d2(p, &root.lo, &root.hi);
        if d >= best_d2 {
            return (best_d2, best);
        }
        let mut stack = [(0u32, 0.0f64); 96];
        stack[0] = (0, d);
        let mut sp = 1;
        while sp > 0 {
            sp -= 1;
            let (ni, nd) = stack[sp];
            if nd >= best_d2 {
                continue;
            }
            let n = &self.nodes[ni as usize];
            if n.count > 0 {
                for t in n.start..n.start + n.count {
                    let d2 = self.tris[t as usize].closest(p).0;
                    if d2 < best_d2 {
                        best_d2 = d2;
                        best = t;
                    }
                }
                continue;
            }
            let (l, r) = (n.start, n.start + 1);
            let (ln, rn) = (&self.nodes[l as usize], &self.nodes[r as usize]);
            let (dl, dr) = (box_d2(p, &ln.lo, &ln.hi), box_d2(p, &rn.lo, &rn.hi));
            let ((near, dn), (far, df)) = if dl <= dr { ((l, dl), (r, dr)) } else { ((r, dr), (l, dl)) };
            if df < best_d2 && sp < stack.len() {
                stack[sp] = (far, df);
                sp += 1;
            }
            if dn < best_d2 && sp < stack.len() {
                stack[sp] = (near, dn);
                sp += 1;
            }
        }
        (best_d2, best)
    }

    /// Nearest triangle with a warm start from triangle `hint` (an index into `tris`), limited
    /// to the squared distance `limit2`.
    fn nearest_from(&self, p: [f64; 3], hint: u32, limit2: f64) -> (f64, u32) {
        let (mut d2, mut best) = (limit2, NONE);
        if hint != NONE {
            let h = self.tris[hint as usize].closest(p).0;
            if h < d2 {
                d2 = h;
                best = hint;
            }
        }
        self.nearest(p, d2, best)
    }
}

// ---------------------------------------------------------------------------------------------
// Sign by ray parity

/// Ray hits of all grid lines along one axis: `t[starts[l]..starts[l + 1]]` are the sorted
/// hit coordinates of line `l`.
struct LineHits {
    starts: Vec<u32>,
    t: Vec<f64>,
}

/// Edge function of `p` against the edge `a`-`b`, evaluated with the endpoints in a canonical
/// order so that an edge shared by two triangles gives bit-identical values (with opposite sign
/// for opposite orientation). Returns the value in the canonical frame and the orientation
/// factor (+1 if `a`-`b` is canonical).
fn edge_fn(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> (f64, f64) {
    let (s, e, f) = if (a[0], a[1]) <= (b[0], b[1]) { (a, b, 1.0) } else { (b, a, -1.0) };
    ((e[0] - s[0]) * (p[1] - s[1]) - (e[1] - s[1]) * (p[0] - s[0]), f)
}

/// Casts one ray along `axis` (toward +) per grid line and records where it crosses the
/// triangles. Lines are offset by a tiny irrational fraction of a voxel to avoid hitting
/// vertices and edges exactly; remaining exact edge hits are assigned to exactly one of the two
/// triangles sharing the edge.
fn cast_lines(tris: &[[[f64; 3]; 3]], axis: usize, res: [u32; 3], min: [f64; 3], voxel: f64) -> LineHits {
    let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
    let (nu, nv) = (res[u] as usize, res[v] as usize);
    let jitter = [0.000_013_7, 0.000_021_3, 0.000_017_9].map(|j| j * voxel * std::f64::consts::SQRT_2);
    let ou = min[u] + jitter[u];
    let ov = min[v] + jitter[v];
    let mut hits: Vec<(u32, f64)> = tris
        .par_iter()
        .fold(Vec::new, |mut out, t| {
            let p = t.map(|c| [c[u], c[v]]);
            let area = (p[1][0] - p[0][0]) * (p[2][1] - p[0][1]) - (p[1][1] - p[0][1]) * (p[2][0] - p[0][0]);
            if area == 0.0 || !area.is_finite() {
                return out;
            }
            let orient = area.signum();
            let range = |k: usize, o: f64, n: usize| {
                let lo = p.iter().map(|q| q[k]).fold(f64::INFINITY, f64::min);
                let hi = p.iter().map(|q| q[k]).fold(f64::NEG_INFINITY, f64::max);
                let a = ((lo - o) / voxel).ceil().max(0.0);
                let b = ((hi - o) / voxel).floor().min(n as f64 - 1.0);
                (a as usize, b as i64)
            };
            let (iu0, iu1) = range(0, ou, nu);
            let (iv0, iv1) = range(1, ov, nv);
            for iv in iv0 as i64..=iv1 {
                let pv = ov + iv as f64 * voxel;
                for iu in iu0 as i64..=iu1 {
                    let q = [ou + iu as f64 * voxel, pv];
                    let mut e = [0.0; 3];
                    let mut inside = true;
                    for k in 0..3 {
                        let (val, f) = edge_fn(q, p[(k + 1) % 3], p[(k + 2) % 3]);
                        // Side of the triangle's interior in the canonical frame.
                        let side = f * orient;
                        if !(val * side > 0.0 || (val == 0.0 && side > 0.0)) {
                            inside = false;
                            break;
                        }
                        e[k] = val * f;
                    }
                    if inside {
                        let s = e[0] + e[1] + e[2];
                        let t = (e[0] * t[0][axis] + e[1] * t[1][axis] + e[2] * t[2][axis]) / s;
                        out.push(((iu as usize + iv as usize * nu) as u32, t));
                    }
                }
            }
            out
        })
        .reduce(Vec::new, |mut a, mut b| {
            if a.len() < b.len() {
                std::mem::swap(&mut a, &mut b);
            }
            a.append(&mut b);
            a
        });
    hits.par_sort_unstable_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
    let mut starts = vec![0u32; nu * nv + 1];
    for h in &hits {
        starts[h.0 as usize + 1] += 1;
    }
    for i in 0..nu * nv {
        starts[i + 1] += starts[i];
    }
    LineHits { starts, t: hits.into_iter().map(|h| h.1).collect() }
}

impl LineHits {
    /// Vote of line `line` for the point at coordinate `x` along it: +1 inside (odd number of
    /// hits before it), -1 outside, 0 if the line has an odd total (it passed through a hole).
    fn vote(&self, line: usize, x: f64) -> i32 {
        let hits = &self.t[self.starts[line] as usize..self.starts[line + 1] as usize];
        if hits.len() % 2 == 1 {
            return 0;
        }
        if hits.partition_point(|&t| t < x) % 2 == 1 { 1 } else { -1 }
    }
}

// ---------------------------------------------------------------------------------------------
// Volume

/// Largest error of the interpolated grid distance, in voxels: the distance from a point to the
/// farthest corner of its grid cell (a distance changes by at most that much in between).
pub const GRID_ERROR: f64 = 1.732_050_807_568_877_2;
/// Where the grid distance is below this many voxels, the exact distance to the triangles is
/// used: models keep their detail (thin parts, sharp edges) at any viewing distance.
pub const NEAR: f64 = 2.75;
/// The exact search looks this many voxels far (at least `NEAR + GRID_ERROR`, so it finds the
/// nearest triangle of every point in the near zone).
pub const SEARCH_RANGE: f64 = 4.5;
/// Depth of the traversal stack of the triangle search (CPU and GPU).
pub const STACK: usize = 32;

/// A node of the triangle hierarchy (march.wgsl reads the same 8 words).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TreeNode {
    pub lo: [f32; 3],
    pub hi: [f32; 3],
    /// Leaf: first triangle; inner node: the left child (the right one follows).
    pub start: u32,
    /// Triangles of a leaf; 0 for inner nodes.
    pub count: u32,
}

/// A baked model: signed distances on a regular grid for the distance away from the surface,
/// the triangles with per-cell lists for the exact distance and the colors near it.
#[derive(Clone, Debug)]
pub struct Volume {
    /// Grid points per axis.
    pub res: [u32; 3],
    /// Model-space position of grid point (0, 0, 0).
    pub min: [f64; 3],
    /// Grid spacing (same on all axes).
    pub voxel: f64,
    /// Tight bounding box of the (transformed) mesh: min, max.
    pub bounds: [[f64; 3]; 2],
    /// Distances, x fastest, then y, then z: `clamp(d / (voxel * DIST_RANGE), -1, 1) * 32767`.
    pub dist: Vec<i16>,
    /// Triangles in model space (corner a, edges ab and ac), in the leaf order of `nodes`.
    pub tris: Vec<[f32; 9]>,
    /// Bounding volume hierarchy over `tris` (node 0 is the root; empty without triangles).
    pub nodes: Vec<TreeNode>,
    /// Per triangle: material (index into `materials` or [`NO_MATERIAL`]).
    pub tri_material: Vec<u32>,
    /// Per triangle: texture coordinates u, v of the three corners.
    pub tri_uv: Vec<[f32; 6]>,
    /// Per triangle: corner vertex colors as sRGB RGBA8 (r in the low byte); empty without
    /// vertex colors.
    pub tri_colors: Vec<[u32; 3]>,
    pub materials: Vec<MeshMaterial>,
    /// Textures of the model file (referenced by the materials).
    pub textures: Vec<Arc<Texture>>,
}

/// Grid cell of `q`: lower corner index and fractions (the clamping scheme of the GPU lookup).
fn lattice(res: [u32; 3], min: [f64; 3], voxel: f64, q: [f64; 3]) -> ([usize; 3], [f64; 3]) {
    let mut i0 = [0usize; 3];
    let mut f = [0.0; 3];
    for k in 0..3 {
        let n = res[k].max(2) as f64;
        let g = ((q[k] - min[k]) / voxel).clamp(0.0, n - 1.0);
        let i = g.floor().min(n - 2.0);
        i0[k] = i as usize;
        f[k] = g - i;
    }
    (i0, f)
}

/// Trilinear interpolation of `value(index)` over the cell at `i0` with fractions `f`.
fn trilinear(res: [u32; 3], i0: [usize; 3], f: [f64; 3], value: impl Fn(usize) -> f64) -> f64 {
    let (nx, ny) = (res[0] as usize, res[1] as usize);
    let at = |dx: usize, dy: usize, dz: usize| value(i0[0] + dx + (i0[1] + dy) * nx + (i0[2] + dz) * nx * ny);
    let lerp = |a: f64, b: f64, t: f64| a + (b - a) * t;
    let x00 = lerp(at(0, 0, 0), at(1, 0, 0), f[0]);
    let x10 = lerp(at(0, 1, 0), at(1, 1, 0), f[0]);
    let x01 = lerp(at(0, 0, 1), at(1, 0, 1), f[0]);
    let x11 = lerp(at(0, 1, 1), at(1, 1, 1), f[0]);
    lerp(lerp(x00, x10, f[1]), lerp(x01, x11, f[1]), f[2])
}

/// The nearest triangle of a point: squared distance, triangle index and the barycentric
/// weights (v, w) of the closest point's second and third corner.
#[derive(Clone, Copy, Debug)]
pub struct Nearest {
    pub d2: f64,
    pub tri: u32,
    pub v: f64,
    pub w: f64,
}

/// sRGB RGBA8 (r in the low byte) as linear RGB.
fn unpack_linear(c: u32) -> [f64; 3] {
    let lut = srgb_lut();
    [lut[(c & 255) as usize] as f64, lut[(c >> 8 & 255) as usize] as f64, lut[(c >> 16 & 255) as usize] as f64]
}

impl Volume {
    /// Upper corner of the grid box.
    pub fn grid_max(&self) -> [f64; 3] {
        std::array::from_fn(|k| self.min[k] + (self.res[k] as f64 - 1.0) * self.voxel)
    }

    /// Decoded distance of a stored grid value.
    pub fn decode(&self, v: i16) -> f64 {
        v as f64 / 32767.0 * DIST_RANGE * self.voxel
    }

    /// Grid distance at `q`: trilinear interpolation of the decoded grid values with
    /// `g = (q - min) / voxel` clamped to `[0, res - 1]`, `i0 = floor(g)` clamped to `res - 2`,
    /// `f = g - i0`.
    fn grid_distance(&self, q: [f64; 3]) -> f64 {
        let (i0, f) = lattice(self.res, self.min, self.voxel, q);
        trilinear(self.res, i0, f, |i| self.decode(self.dist[i]))
    }

    /// Triangle `i` for closest-point queries.
    fn tri(&self, i: u32) -> Tri {
        let t = &self.tris[i as usize];
        let f = |k: usize| [t[k] as f64, t[k + 1] as f64, t[k + 2] as f64];
        Tri { a: f(0), ab: f(3), ac: f(6) }
    }

    /// The nearest triangle closer than `limit` (None if there is none). Traversal: a stack of
    /// `STACK` (node, squared box distance) entries; a node is skipped when its box is not
    /// closer than the best triangle so far; children are pushed far one first; when the stack
    /// is full, further children are dropped.
    pub fn nearest(&self, q: [f64; 3], limit: f64) -> Option<Nearest> {
        let node_d2 = |n: &TreeNode| box_d2(q, &n.lo.map(|v| v as f64), &n.hi.map(|v| v as f64));
        let root = self.nodes.first()?;
        let mut best_d2 = limit * limit;
        let mut best: Option<Nearest> = None;
        let mut stack = [(0u32, 0.0f64); STACK];
        stack[0] = (0, node_d2(root));
        let mut sp = 1;
        while sp > 0 {
            sp -= 1;
            let (ni, nd) = stack[sp];
            if nd >= best_d2 {
                continue;
            }
            let n = &self.nodes[ni as usize];
            if n.count > 0 {
                for t in n.start..n.start + n.count {
                    let (d2, v, w) = self.tri(t).closest(q);
                    if d2 < best_d2 {
                        best_d2 = d2;
                        best = Some(Nearest { d2, tri: t, v, w });
                    }
                }
                continue;
            }
            let (l, r) = (n.start, n.start + 1);
            let (dl, dr) = (node_d2(&self.nodes[l as usize]), node_d2(&self.nodes[r as usize]));
            let ((near, dn), (far, df)) = if dl <= dr { ((l, dl), (r, dr)) } else { ((r, dr), (l, dl)) };
            if df < best_d2 && sp < STACK {
                stack[sp] = (far, df);
                sp += 1;
            }
            if dn < best_d2 && sp < STACK {
                stack[sp] = (near, dn);
                sp += 1;
            }
        }
        best
    }

    /// Whether `q` is behind the face of its nearest triangle `n`.
    fn behind(&self, q: [f64; 3], n: &Nearest) -> bool {
        let t = self.tri(n.tri);
        let p: [f64; 3] = std::array::from_fn(|k| t.a[k] + n.v * t.ab[k] + n.w * t.ac[k]);
        dot(sub(q, p), cross(t.ab, t.ac)) < 0.0
    }

    /// Signed distance at model-space point `q` (a lower bound of the true distance).
    ///
    /// Inside the grid box: the grid distance `g`. Where `|g| < NEAR` voxels, the exact distance
    /// to the nearest triangle (searched up to `SEARCH_RANGE` voxels), negative where
    /// `g < -voxel` or where `g < 0` and `q` is behind that triangle's face (parts thinner
    /// than the grid and open surfaces are shells). Elsewhere `g` moved toward
    /// zero by `GRID_ERROR` voxels. Outside the grid box: the larger of the distance from `q` to
    /// `bounds` (the tight mesh box) and the value at the nearest point of the grid box minus
    /// the distance to it; both are lower bounds.
    pub fn distance(&self, q: [f64; 3]) -> f64 {
        if self.dist.is_empty() {
            return box_distance(q, &self.bounds);
        }
        let hi = self.grid_max();
        let g = self.grid_distance(q);
        let e = GRID_ERROR * self.voxel;
        let far = if g > 0.0 { (g - e).max(0.0) } else { (g + e).min(0.0) };
        if (0..3).any(|k| !(q[k] >= self.min[k] && q[k] <= hi[k])) {
            // Continuous at the grid faces, so soft shadows and occlusion see no box edges.
            let c: [f64; 3] = std::array::from_fn(|k| q[k].clamp(self.min[k], hi[k]));
            let away = (0..3).map(|k| (q[k] - c[k]) * (q[k] - c[k])).sum::<f64>().sqrt();
            return box_distance(q, &self.bounds).max(far - away);
        }
        if g.abs() >= NEAR * self.voxel {
            return far;
        }
        let range = SEARCH_RANGE * self.voxel;
        let Some(n) = self.nearest(q, range) else { return if g < -self.voxel { -range } else { range } };
        let d = n.d2.sqrt();
        let inside = g < -self.voxel || (g < 0.0 && self.behind(q, &n));
        if inside { -d } else { d }
    }

    /// Linear RGB at `q` with the materials of `look`: the nearest triangle's material color
    /// times its interpolated vertex color times its texture. None if the look has no colors or
    /// no triangle is near.
    pub fn color(&self, q: [f64; 3], look: &Look) -> Option<[f64; 3]> {
        if !look.colored {
            return None;
        }
        let n = self.nearest(q, SEARCH_RANGE * self.voxel)?;
        let t = n.tri as usize;
        let u = 1.0 - n.v - n.w;
        let m = look.materials.get(self.tri_material[t] as usize);
        let mut c = m.map_or([1.0; 3], |m| m.color.map(|v| v as f64));
        if let Some(vc) = self.tri_colors.get(t) {
            let [a, b, d] = vc.map(unpack_linear);
            for k in 0..3 {
                c[k] *= a[k] * u + b[k] * n.v + d[k] * n.w;
            }
        }
        if let Some(tex) = m.and_then(|m| m.texture).and_then(|i| look.textures.get(i as usize)) {
            let uv = &self.tri_uv[t];
            let tu = uv[0] as f64 * u + uv[2] as f64 * n.v + uv[4] as f64 * n.w;
            let tv = uv[1] as f64 * u + uv[3] as f64 * n.v + uv[5] as f64 * n.w;
            let s = tex.sample(tu, tv);
            for k in 0..3 {
                c[k] *= s[k] as f64;
            }
        }
        Some(c)
    }

    /// The look of the model as its file describes it.
    pub fn file_look(&self) -> Look {
        let materials: Vec<LookMaterial> = self.materials.iter().map(|m| LookMaterial { color: m.color, texture: m.texture }).collect();
        let colored = materials.iter().any(|m| m.color != [1.0; 3] || m.texture.is_some()) || !self.tri_colors.is_empty();
        Look { materials, textures: self.textures.clone(), colored }
    }
}

/// A baked model with the look it is shown with.
#[derive(Clone, Debug)]
pub struct Model {
    pub vol: Arc<Volume>,
    pub look: Arc<Look>,
}

impl Model {
    /// The model `vol` of the scene's model reference `m`. Looks are cached per volume and
    /// settings, so that an unchanged model keeps the same `look` (renderers upload a model
    /// again only when its volume or look changes).
    pub fn new(vol: Arc<Volume>, _m: &crate::compose::ModelRef) -> Model {
        type Entry = (std::sync::Weak<Volume>, String, Arc<Look>);
        static LOOKS: OnceLock<Mutex<Vec<Entry>>> = OnceLock::new();
        let fingerprint = String::new();
        let mut looks = LOOKS.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
        looks.retain(|(w, _, _)| w.strong_count() > 0);
        if let Some((_, _, look)) = looks.iter().find(|(w, f, _)| w.as_ptr() == Arc::as_ptr(&vol) && *f == fingerprint) {
            return Model { look: look.clone(), vol };
        }
        let look = Arc::new(vol.file_look());
        looks.push((Arc::downgrade(&vol), fingerprint, look.clone()));
        Model { vol, look }
    }
}

/// How a model's materials are shown: per material of the [`Volume`] a color and texture.
#[derive(Clone, Debug, Default)]
pub struct Look {
    pub materials: Vec<LookMaterial>,
    pub textures: Vec<Arc<Texture>>,
    /// Whether there is any color (otherwise the object's own material colors are used).
    pub colored: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LookMaterial {
    /// Linear RGB.
    pub color: [f32; 3],
    /// Index into [`Look::textures`].
    pub texture: Option<u32>,
}

/// `v` as f32, rounded toward negative infinity (boxes stay conservative).
fn round_down(v: f64) -> f32 {
    let f = v as f32;
    if f as f64 > v { f.next_down() } else { f }
}

/// `v` as f32, rounded toward positive infinity.
fn round_up(v: f64) -> f32 {
    let f = v as f32;
    if (f as f64) < v { f.next_up() } else { f }
}

/// Bakes `mesh` into a volume. `progress` receives 0..1 and may be called from worker threads.
pub fn bake(mesh: &Mesh, opts: &ModelOptions, progress: &(dyn Fn(f32) + Sync)) -> Volume {
    let resolution = opts.clamped_resolution();
    progress(0.0);

    // Model transform: center and scale unless the file units are kept.
    let raw_bounds = mesh.bounds().unwrap_or([[0.0; 3]; 2]);
    let (center, scale) = if opts.keep_units {
        ([0.0; 3], 1.0)
    } else {
        let c: [f64; 3] = std::array::from_fn(|k| 0.5 * (raw_bounds[0][k] + raw_bounds[1][k]));
        let longest = (0..3).map(|k| raw_bounds[1][k] - raw_bounds[0][k]).fold(0.0, f64::max);
        (c, if longest > 0.0 { 2.0 / longest } else { 1.0 })
    };
    let tris: Vec<[[f64; 3]; 3]> = mesh.triangles.iter().map(|t| t.map(|p| std::array::from_fn(|k| (p[k] - center[k]) * scale))).collect();
    let bounds = bounds_of(&tris).unwrap_or([[0.0; 3]; 2]);

    // Grid: `resolution` points along the longest side including the padding, the other axes
    // proportional, centered on the mesh.
    let ext: [f64; 3] = std::array::from_fn(|k| bounds[1][k] - bounds[0][k]);
    let longest = ext.iter().copied().fold(0.0, f64::max).max(1e-9);
    let long_axis = (0..3).fold(0, |a, k| if ext[k] > ext[a] { k } else { a });
    let voxel = longest / (resolution as f64 - 1.0 - 2.0 * PADDING);
    let res: [u32; 3] = std::array::from_fn(|k| {
        if k == long_axis {
            resolution
        } else {
            let inner = (ext[k] / voxel - 1e-9).ceil().max(0.0) as u32;
            (inner + 1 + 2 * PADDING as u32).clamp(4, resolution)
        }
    });
    let min: [f64; 3] = std::array::from_fn(|k| 0.5 * (bounds[0][k] + bounds[1][k]) - 0.5 * (res[k] as f64 - 1.0) * voxel);
    let (nx, ny, nz) = (res[0] as usize, res[1] as usize, res[2] as usize);

    // Triangles with an area (degenerate ones add nothing to the surface).
    let eps = (longest * 1e-12).powi(2);
    let ids: Vec<u32> = (0..tris.len() as u32)
        .filter(|&i| {
            let t = &tris[i as usize];
            let n = cross(sub(t[1], t[0]), sub(t[2], t[0]));
            dot(n, n) > eps * eps
        })
        .collect();
    let valid: Vec<[[f64; 3]; 3]> = ids.iter().map(|&i| tris[i as usize]).collect();
    let bvh = Bvh::build(&tris, ids);

    // Sign: ray parity along every axis.
    let lines: Vec<LineHits> = (0..3).map(|a| cast_lines(&valid, a, res, min, voxel)).collect();
    let (p_sign, p_dist) = (0.05, 0.8);
    progress(p_sign as f32);

    // Distances, z-slices in parallel.
    let cap = DIST_RANGE * voxel;
    let cap2 = cap * cap * (1.0 + 1e-9);
    let done = AtomicUsize::new(0);
    let mut dist = vec![0i16; nx * ny * nz];
    dist.par_chunks_mut(nx * ny).enumerate().for_each(|(k, slice)| {
        let z = min[2] + k as f64 * voxel;
        let mut row_hint = NONE;
        for j in 0..ny {
            let y = min[1] + j as f64 * voxel;
            let mut hint = row_hint;
            for i in 0..nx {
                let p = [min[0] + i as f64 * voxel, y, z];
                let (d2, tri) = bvh.nearest_from(p, hint, cap2);
                if tri != NONE {
                    hint = tri;
                    if i == 0 {
                        row_hint = tri;
                    }
                }
                let votes = lines[0].vote(j + k * ny, p[0]) + lines[1].vote(k + i * nz, p[1]) + lines[2].vote(i + j * nx, p[2]);
                let inside = match votes.signum() {
                    1 => true,
                    -1 => false,
                    // Undecided (all lines through holes or a tie): side of the nearest
                    // triangle's face normal.
                    _ => {
                        tri != NONE && {
                            let t = &bvh.tris[tri as usize];
                            let (_, v, w) = t.closest(p);
                            let q: [f64; 3] = std::array::from_fn(|c| t.a[c] + v * t.ab[c] + w * t.ac[c]);
                            dot(sub(p, q), cross(t.ab, t.ac)) < 0.0
                        }
                    }
                };
                let d = if tri == NONE { cap } else { d2.sqrt() };
                let d = if inside { -d } else { d };
                slice[i + j * nx] = ((d / cap).clamp(-1.0, 1.0) * 32767.0).round() as i16;
            }
        }
        let n = done.fetch_add(1, Ordering::Relaxed) + 1;
        progress((p_sign + (p_dist - p_sign) * n as f64 / nz as f64) as f32);
    });

    // Triangles in the hierarchy's leaf order, the hierarchy and the shading.
    let tris_f32: Vec<[f32; 9]> = bvh.tris.iter().map(|t| [t.a[0], t.a[1], t.a[2], t.ab[0], t.ab[1], t.ab[2], t.ac[0], t.ac[1], t.ac[2]].map(|v| v as f32)).collect();
    let nodes: Vec<TreeNode> = bvh.nodes.iter().map(|n| TreeNode { lo: n.lo.map(round_down), hi: n.hi.map(round_up), start: n.start, count: n.count }).collect();
    let shading = |i: u32| mesh.shading.get(i as usize).copied().unwrap_or_default();
    let ids = &bvh.ids;
    let tri_material = ids.iter().map(|&i| shading(i).material).collect();
    let tri_uv = ids.iter().map(|&i| shading(i).uv.as_flattened().try_into().unwrap_or([0.0; 6])).collect();
    let vertex_colors = mesh.shading.iter().any(|s| s.colors != [[1.0; 3]; 3]);
    let tri_colors = if vertex_colors { ids.iter().map(|&i| shading(i).colors.map(|c| u32::from_le_bytes(linear_to_display_u8(c.map(|v| v as f64))))).collect() } else { Vec::new() };
    progress(1.0);
    Volume { res, min, voxel, bounds, dist, tris: tris_f32, nodes, tri_material, tri_uv, tri_colors, materials: mesh.materials.clone(), textures: mesh.textures.iter().cloned().map(Arc::new).collect() }
}

// ---------------------------------------------------------------------------------------------
// Cache

/// Increments whenever a bake finishes (renderers then reload their volumes).
static GENERATION: AtomicU64 = AtomicU64::new(0);

pub fn generation() -> u64 {
    GENERATION.load(Ordering::Relaxed)
}

/// State of a cached model.
#[derive(Clone, Debug)]
pub enum Status {
    Ready(Arc<Volume>),
    /// Bake progress 0..1.
    Baking(f32),
    Failed(String),
}

/// Canonical path, modification time, resolution, up axis, keep units.
type Key = (PathBuf, SystemTime, u32, UpAxis, bool);

enum Entry {
    Ready(Arc<Volume>),
    /// Progress as f32 bits.
    Baking(Arc<AtomicU32>),
    Failed(String),
}

struct Cache {
    map: Mutex<HashMap<Key, Entry>>,
    /// Signalled whenever a bake finishes.
    done: Condvar,
}

fn cache() -> &'static Cache {
    static CACHE: OnceLock<Cache> = OnceLock::new();
    CACHE.get_or_init(|| Cache { map: Mutex::new(HashMap::new()), done: Condvar::new() })
}

fn lock(c: &Cache) -> MutexGuard<'_, HashMap<Key, Entry>> {
    c.map.lock().unwrap_or_else(|e| e.into_inner())
}

fn cache_key(path: &str, opts: &ModelOptions) -> Result<Key, String> {
    let canonical = std::fs::canonicalize(path).map_err(|e| format!("{path}: {e}"))?;
    let mtime = std::fs::metadata(&canonical).and_then(|m| m.modified()).map_err(|e| format!("{path}: {e}"))?;
    Ok((canonical, mtime, opts.clamped_resolution(), opts.up_axis, opts.keep_units))
}

/// Inserts a `Baking` entry for `key`, dropping finished entries of older versions of the file.
fn start_entry(map: &mut HashMap<Key, Entry>, key: &Key) -> Arc<AtomicU32> {
    map.retain(|k, e| k.0 != key.0 || k.1 == key.1 || matches!(e, Entry::Baking(_)));
    let progress = Arc::new(AtomicU32::new(0f32.to_bits()));
    map.insert(key.clone(), Entry::Baking(progress.clone()));
    progress
}

fn run_bake(key: &Key, opts: &ModelOptions, progress: &AtomicU32) -> Result<Arc<Volume>, String> {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mesh = load(&key.0, opts.up_axis)?;
        // Workers report out of order; keep the highest value (positive f32 bits order like the values).
        let report = |f: f32| {
            progress.fetch_max((0.05 + 0.95 * f).to_bits(), Ordering::Relaxed);
        };
        Ok(Arc::new(bake(&mesh, opts, &report)))
    }));
    let result = result.unwrap_or_else(|_| Err(format!("{}: baking the model failed", key.0.display())));
    let mut map = lock(cache());
    map.insert(
        key.clone(),
        match &result {
            Ok(v) => Entry::Ready(v.clone()),
            Err(e) => Entry::Failed(e.clone()),
        },
    );
    drop(map);
    GENERATION.fetch_add(1, Ordering::Relaxed);
    cache().done.notify_all();
    result
}

/// State of the model at `path` with `opts`; starts a background bake if it is neither cached
/// nor being baked. Never blocks on a bake.
pub fn request(path: &str, opts: &ModelOptions) -> Status {
    let key = match cache_key(path, opts) {
        Ok(k) => k,
        Err(e) => return Status::Failed(e),
    };
    let mut map = lock(cache());
    match map.get(&key) {
        Some(Entry::Ready(v)) => return Status::Ready(v.clone()),
        Some(Entry::Baking(p)) => return Status::Baking(f32::from_bits(p.load(Ordering::Relaxed))),
        Some(Entry::Failed(e)) => return Status::Failed(e.clone()),
        None => {}
    }
    let progress = start_entry(&mut map, &key);
    drop(map);
    let opts = opts.clone();
    let spawned = std::thread::Builder::new().name("rmb-model-bake".into()).spawn({
        let key = key.clone();
        move || {
            let _ = run_bake(&key, &opts, &progress);
        }
    });
    if let Err(e) = spawned {
        let msg = format!("cannot start the model bake: {e}");
        lock(cache()).insert(key, Entry::Failed(msg.clone()));
        return Status::Failed(msg);
    }
    Status::Baking(0.0)
}

/// The volume of the model at `path` with `opts`: from the cache, by waiting for a running
/// bake, or baked in the calling thread.
pub fn get(path: &str, opts: &ModelOptions) -> Result<Arc<Volume>, String> {
    let key = cache_key(path, opts)?;
    let c = cache();
    let mut map = lock(c);
    loop {
        match map.get(&key) {
            Some(Entry::Ready(v)) => return Ok(v.clone()),
            Some(Entry::Failed(e)) => return Err(e.clone()),
            Some(Entry::Baking(_)) => map = c.done.wait(map).unwrap_or_else(|e| e.into_inner()),
            None => break,
        }
    }
    let progress = start_entry(&mut map, &key);
    drop(map);
    run_bake(&key, opts, &progress)
}

/// Forgets all cached volumes (running bakes still finish and are stored).
pub fn clear() {
    lock(cache()).retain(|_, e| matches!(e, Entry::Baking(_)));
    GENERATION.fetch_add(1, Ordering::Relaxed);
}

// ---------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(resolution: u32) -> ModelOptions {
        ModelOptions { resolution, ..ModelOptions::default() }
    }

    fn no_progress(_: f32) {}

    /// Deterministic pseudo-random numbers in 0..1.
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> f64 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (self.0 >> 11) as f64 / (1u64 << 53) as f64
        }
    }

    /// Surface of revolution style mesh from a grid of points `f(i, j)` (i around, j along),
    /// wrapped in i (and in j if `wrap_j`), outward winding for the shapes below.
    fn grid_mesh(nu: usize, nv: usize, wrap_j: bool, f: impl Fn(usize, usize) -> [f64; 3]) -> Mesh {
        let mut m = Mesh::default();
        let jn = if wrap_j { nv } else { nv - 1 };
        for j in 0..jn {
            for i in 0..nu {
                let (i1, j1) = ((i + 1) % nu, (j + 1) % nv);
                let (a, b, c, d) = (f(i, j), f(i1, j), f(i1, j1), f(i, j1));
                m.push([a, b, c], Shading::default());
                m.push([a, c, d], Shading::default());
            }
        }
        m.shading.clear();
        m
    }

    fn sphere(nu: usize, nv: usize) -> Mesh {
        grid_mesh(nu, nv + 1, false, |i, j| {
            let th = std::f64::consts::PI * j as f64 / nv as f64;
            let ph = 2.0 * std::f64::consts::PI * i as f64 / nu as f64;
            [th.sin() * ph.cos(), th.sin() * ph.sin(), -th.cos()]
        })
    }

    fn torus(nu: usize, nv: usize, r1: f64, r2: f64) -> Mesh {
        grid_mesh(nu, nv, true, |i, j| {
            let ph = 2.0 * std::f64::consts::PI * i as f64 / nu as f64;
            let th = 2.0 * std::f64::consts::PI * j as f64 / nv as f64;
            let r = r1 + r2 * th.cos();
            [r * ph.cos(), r * ph.sin(), r2 * th.sin()]
        })
    }

    const BOX_FACES: [[usize; 4]; 6] = [[0, 2, 3, 1], [4, 5, 7, 6], [0, 1, 5, 4], [2, 6, 7, 3], [0, 4, 6, 2], [1, 3, 7, 5]];

    /// Corner `i` of the box (bit 0: x, bit 1: y, bit 2: z).
    fn box_corner(h: [f64; 3], i: usize) -> [f64; 3] {
        std::array::from_fn(|k| if i >> k & 1 == 1 { h[k] } else { -h[k] })
    }

    fn box_mesh(h: [f64; 3]) -> Mesh {
        let mut m = Mesh::default();
        for f in BOX_FACES {
            let c = f.map(|i| box_corner(h, i));
            m.push([c[0], c[1], c[2]], Shading::default());
            m.push([c[0], c[2], c[3]], Shading::default());
        }
        m.shading.clear();
        m
    }

    fn sd_box(p: [f64; 3], h: [f64; 3]) -> f64 {
        let q: [f64; 3] = std::array::from_fn(|k| p[k].abs() - h[k]);
        let out = q.map(|v| v.max(0.0));
        dot(out, out).sqrt() + q[0].max(q[1]).max(q[2]).min(0.0)
    }

    /// Compares the volume with `sdf` at random points of the grid box: exact near the surface,
    /// a lower bound everywhere, within the grid error elsewhere (`slack`: tessellation error).
    fn check_sdf(vol: &Volume, sdf: impl Fn([f64; 3]) -> f64, slack: f64) {
        let hi = vol.grid_max();
        let mut rng = Lcg(7);
        let cap = DIST_RANGE * vol.voxel;
        let v = vol.voxel;
        for i in 0..6000 {
            // Half of the points close to the surface.
            let mut p: [f64; 3] = std::array::from_fn(|k| vol.min[k] + rng.next() * (hi[k] - vol.min[k]));
            if i % 2 == 0 {
                for _ in 0..40 {
                    if sdf(p).abs() < 2.0 * v {
                        break;
                    }
                    p = std::array::from_fn(|k| vol.min[k] + rng.next() * (hi[k] - vol.min[k]));
                }
            }
            let want = sdf(p).clamp(-cap, cap);
            let got = vol.distance(p);
            assert!(got.abs() <= want.abs() + slack + 1e-5, "not a lower bound at {p:?}: got {got}, want {want}, voxel {v}");
            assert!((got - want).abs() < (2.0 * GRID_ERROR + 1.0) * v + slack, "at {p:?}: got {got}, want {want}, voxel {v}");
            if want.abs() < NEAR * v - GRID_ERROR * v {
                assert!((got.abs() - want.abs()).abs() < slack + 1e-5, "not exact at {p:?}: got {got}, want {want}");
            }
            if want > 0.0 || want < -(NEAR + GRID_ERROR) * v {
                assert_eq!(got < 0.0, want < 0.0, "sign at {p:?}: got {got}, want {want}");
            }
        }
    }

    #[test]
    fn sphere_matches_analytic() {
        let vol = bake(&sphere(48, 24), &opts(40), &no_progress);
        assert_eq!(vol.res, [40, 40, 40]);
        check_sdf(&vol, |p| dot(p, p).sqrt() - 1.0, 0.01);
        assert!(vol.distance([0.0; 3]) < -0.75);
        assert!(vol.color([0.0; 3], &vol.file_look()).is_none());
    }

    #[test]
    fn box_matches_analytic() {
        let h = [1.0, 0.5, 0.25];
        let vol = bake(&box_mesh(h), &opts(48), &no_progress);
        assert_eq!(vol.res[0], 48);
        assert!(vol.res[1] < vol.res[0] && vol.res[2] < vol.res[1]);
        check_sdf(&vol, |p| sd_box(p, h), 1e-9);
    }

    #[test]
    fn torus_matches_analytic() {
        let (r1, r2) = (1.0, 0.35);
        let vol = bake(&torus(64, 24, r1, r2), &opts(40), &no_progress);
        // Normalized: the outer diameter becomes 2.
        let s = 2.0 / (2.0 * (r1 + r2));
        check_sdf(
            &vol,
            |p| {
                let q = p.map(|c| c / s);
                let xy = (q[0] * q[0] + q[1] * q[1]).sqrt() - r1;
                s * ((xy * xy + q[2] * q[2]).sqrt() - r2)
            },
            0.01,
        );
    }

    #[test]
    fn thin_parts_are_exact() {
        // A plate much thinner than a voxel and an open quad above it.
        let mut m = box_mesh([1.0, 1.0, 0.002]);
        let q = [[-1.0, -1.0, 0.5], [1.0, -1.0, 0.5], [1.0, 1.0, 0.5], [-1.0, 1.0, 0.5]];
        m.push([q[0], q[1], q[2]], Shading::default());
        m.push([q[0], q[2], q[3]], Shading::default());
        m.shading.clear();
        let vol = bake(&m, &ModelOptions { keep_units: true, ..opts(32) }, &no_progress);
        assert!(vol.voxel > 0.05);
        for z in [0.0025, 0.004, 0.01, 0.05] {
            let d = vol.distance([0.2, 0.1, z]);
            assert!((d - (z - 0.002)).abs() < 1e-6, "plate at z {z}: {d}");
            let d = vol.distance([0.2, 0.1, -z]);
            assert!((d - (z - 0.002)).abs() < 1e-6, "plate below at z {z}: {d}");
        }
        // Inside a part thinner than the grid: the distance to its faces (a shell).
        assert!((vol.distance([0.2, 0.1, 0.001]) - 0.001).abs() < 1e-6);
        // The open quad is a surface seen from both sides.
        for dz in [0.001, 0.01, 0.1] {
            assert!((vol.distance([-0.3, 0.4, 0.5 + dz]) - dz).abs() < 1e-6);
            assert!((vol.distance([-0.3, 0.4, 0.5 - dz]) - dz).abs() < 1e-6);
        }
    }

    #[test]
    fn sphere_with_hole_keeps_sign() {
        let mut m = sphere(32, 16);
        // Remove the triangles at the top pole and one near the equator.
        let before = m.triangles.len();
        let keep: Vec<bool> = m.triangles.iter().map(|t| !(t.iter().all(|p| p[2] > 0.95 && p[0] >= 0.0) || t.iter().all(|p| p[0] > 0.97 && p[1] > 0.0 && p[1] < 0.25 && p[2].abs() < 0.25))).collect();
        let mut i = 0;
        m.triangles.retain(|_| {
            i += 1;
            keep[i - 1]
        });
        assert!(m.triangles.len() < before && m.triangles.len() + 40 > before && before - m.triangles.len() > 4, "removed {}", before - m.triangles.len());
        let vol = bake(&m, &opts(40), &no_progress);
        assert!(vol.distance([0.0; 3]) < -0.75);
        assert!(vol.distance([0.0, 0.0, 0.5]) < -0.3);
        assert!(vol.distance([0.5, 0.0, 0.0]) < -0.3);
        // Every grid point away from the surface has the right sign.
        let (nx, ny) = (vol.res[0] as usize, vol.res[1] as usize);
        for (n, &v) in vol.dist.iter().enumerate() {
            let p: [f64; 3] = std::array::from_fn(|k| vol.min[k] + [n % nx, n / nx % ny, n / (nx * ny)][k] as f64 * vol.voxel);
            let want = dot(p, p).sqrt() - 1.0;
            if want.abs() > 2.0 * vol.voxel {
                assert_eq!(v < 0, want < 0.0, "sign at {p:?}");
            }
        }
    }

    #[test]
    fn outside_distance_is_lower_bound() {
        let vol = bake(&sphere(32, 16), &opts(24), &no_progress);
        let mut rng = Lcg(3);
        for _ in 0..500 {
            let p: [f64; 3] = std::array::from_fn(|_| (rng.next() - 0.5) * 8.0);
            let d = vol.distance(p);
            assert!(d.abs() <= (dot(p, p).sqrt() - 1.0).abs() + vol.voxel, "at {p:?}: {d}");
        }
        assert!((vol.distance([3.0, 0.0, 0.0]) - 2.0).abs() < 1e-9);
    }

    #[test]
    fn up_axis_and_normalization() {
        let mut m = box_mesh([1.0, 1.0, 1.0]);
        for t in &mut m.triangles {
            for p in t.iter_mut() {
                // Y-up box from (1, 2, 3) to (5, 4, 4).
                *p = [3.0 + 2.0 * p[0], 3.0 + p[1], 3.5 + 0.5 * p[2]];
            }
        }
        m.y_up_to_z_up();
        let b = m.bounds().unwrap();
        assert_eq!(b, [[1.0, -4.0, 2.0], [5.0, -3.0, 4.0]]);
        let vol = bake(&m, &opts(32), &no_progress);
        let size: [f64; 3] = std::array::from_fn(|k| vol.bounds[1][k] - vol.bounds[0][k]);
        assert!((size[0] - 2.0).abs() < 1e-12 && (size[1] - 0.5).abs() < 1e-12 && (size[2] - 1.0).abs() < 1e-12);
        for k in 0..3 {
            assert!((vol.bounds[0][k] + vol.bounds[1][k]).abs() < 1e-12);
            // Grid box covers the bounds with two voxels of padding.
            assert!(vol.min[k] <= vol.bounds[0][k] - 2.0 * vol.voxel + 1e-9);
            assert!(vol.grid_max()[k] >= vol.bounds[1][k] + 2.0 * vol.voxel - 1e-9);
        }
        let kept = bake(&m, &ModelOptions { keep_units: true, ..opts(32) }, &no_progress);
        assert_eq!(kept.bounds, b);
        assert!(kept.distance([3.0, -3.5, 3.0]) < 0.0);
    }

    fn temp_dir(name: &str) -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let d = std::env::temp_dir().join(format!("rmb-mesh-test-{}-{}-{name}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Unit cube corners and quads (outward) for the file tests.
    fn cube_quads() -> ([[f64; 3]; 8], [[usize; 4]; 6]) {
        (std::array::from_fn(|i| box_corner([1.0; 3], i)), BOX_FACES)
    }

    fn assert_color(vol: &Volume, q: [f64; 3], want: [f64; 3]) {
        let c = vol.color(q, &vol.file_look()).expect("colors");
        for k in 0..3 {
            assert!((c[k] - want[k]).abs() < 0.01, "color {c:?}, want {want:?}");
        }
    }

    #[test]
    fn obj_with_material_and_texture() {
        let dir = temp_dir("obj");
        let (v, f) = cube_quads();
        let mut obj = String::from("mtllib m.mtl\n");
        for p in v {
            obj += &format!("v {} {} {}\n", p[0], p[1], p[2]);
        }
        obj += "vt 0 0\nvt 1 0\nvt 1 1\nvt 0 1\n";
        for (n, q) in f.iter().enumerate() {
            obj += if n < 3 { "usemtl red\n" } else { "usemtl tex\n" };
            obj += &format!("f {}/1 {}/2 {}/3 {}/4\n", q[0] + 1, q[1] + 1, q[2] + 1, q[3] + 1);
        }
        std::fs::write(dir.join("cube.obj"), obj).unwrap();
        std::fs::write(dir.join("m.mtl"), "newmtl red\nKd 1 0 0\nnewmtl tex\nKd 1 1 1\nmap_Kd tex/green.png\n").unwrap();
        std::fs::create_dir_all(dir.join("tex")).unwrap();
        image::RgbImage::from_pixel(4, 4, image::Rgb([0, 255, 0])).save(dir.join("tex/green.png")).unwrap();

        let mesh = load(&dir.join("CUBE.obj").with_file_name("cube.obj"), UpAxis::Z).unwrap();
        assert_eq!(mesh.triangles.len(), 12);
        assert_eq!(mesh.textures.len(), 1);
        let vol = bake(&mesh, &opts(24), &no_progress);
        // Faces 0..2 (red): -z, +z and -y; the others (texture): +y, -x, +x.
        assert_color(&vol, [0.0, 0.0, -1.05], [1.0, 0.0, 0.0]);
        assert_color(&vol, [0.0, 1.05, 0.0], [0.0, 1.0, 0.0]);
        assert_color(&vol, [1.05, 0.0, 0.0], [0.0, 1.0, 0.0]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn obj_is_y_up_and_extension_case_is_ignored() {
        let dir = temp_dir("objup");
        std::fs::write(dir.join("t.OBJ"), "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n").unwrap();
        let m = load(&dir.join("t.OBJ"), UpAxis::Auto).unwrap();
        assert_eq!(m.triangles, vec![[[0.0; 3], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]]]);
        assert!(!m.has_colors());
        let m = load(&dir.join("t.OBJ"), UpAxis::Z).unwrap();
        assert_eq!(m.triangles[0][2], [0.0, 1.0, 0.0]);
        assert!(load(&dir.join("t.xyz"), UpAxis::Auto).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stl_round_trip() {
        let dir = temp_dir("stl");
        let src = box_mesh([1.0, 0.5, 0.5]);
        let tris: Vec<stl_io::Triangle> = src
            .triangles
            .iter()
            .map(|t| stl_io::Triangle { normal: stl_io::Vector([0.0; 3]), vertices: t.map(|p| stl_io::Vector(p.map(|c| c as f32))) })
            .collect();
        let mut f = std::fs::File::create(dir.join("box.stl")).unwrap();
        stl_io::write_stl(&mut f, tris.iter()).unwrap();
        drop(f);
        let mesh = load(&dir.join("box.stl"), UpAxis::Auto).unwrap();
        assert_eq!(mesh.triangles, src.triangles);
        assert!(!mesh.has_colors());
        let vol = bake(&mesh, &opts(24), &no_progress);
        assert!(vol.distance([0.0; 3]) < -0.25 && !vol.file_look().colored);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ply_with_vertex_colors() {
        let dir = temp_dir("ply");
        let (v, f) = cube_quads();
        let mut s = String::from("ply\nformat ascii 1.0\nelement vertex 8\nproperty float x\nproperty float y\nproperty float z\n");
        s += "property uchar red\nproperty uchar green\nproperty uchar blue\nelement face 6\nproperty list uchar int vertex_indices\nend_header\n";
        for p in v {
            s += &format!("{} {} {} 0 0 255\n", p[0], p[1], p[2]);
        }
        for q in f {
            s += &format!("4 {} {} {} {}\n", q[0], q[1], q[2], q[3]);
        }
        std::fs::write(dir.join("cube.ply"), s).unwrap();
        let mesh = load(&dir.join("cube.ply"), UpAxis::Auto).unwrap();
        assert_eq!(mesh.triangles.len(), 12);
        let vol = bake(&mesh, &opts(24), &no_progress);
        assert_color(&vol, [0.3, 0.2, 1.1], [0.0, 0.0, 1.0]);
        assert!(vol.distance([0.0; 3]) < -0.75);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn base64(data: &[u8]) -> String {
        const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for c in data.chunks(3) {
            let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
            for k in 0..4 {
                out.push(if k <= c.len() { A[(n >> (18 - 6 * k) & 63) as usize] as char } else { '=' });
            }
        }
        out
    }

    #[test]
    fn gltf_with_factor_and_node_transform() {
        let dir = temp_dir("gltf");
        let (v, f) = cube_quads();
        let mut bin = Vec::new();
        for p in v {
            for c in p {
                bin.extend_from_slice(&(c as f32).to_le_bytes());
            }
        }
        for q in f {
            for i in [q[0], q[1], q[2], q[0], q[2], q[3]] {
                bin.extend_from_slice(&(i as u16).to_le_bytes());
            }
        }
        let json = format!(
            r#"{{"asset":{{"version":"2.0"}},"scene":0,"scenes":[{{"nodes":[0]}}],
            "nodes":[{{"children":[1],"translation":[10,0,0]}},{{"mesh":0,"scale":[2,1,1]}}],
            "meshes":[{{"primitives":[{{"attributes":{{"POSITION":0}},"indices":1,"material":0}}]}}],
            "materials":[{{"pbrMetallicRoughness":{{"baseColorFactor":[0.214041,0.0,1.0,1.0]}}}}],
            "buffers":[{{"byteLength":{},"uri":"data:application/octet-stream;base64,{}"}}],
            "bufferViews":[{{"buffer":0,"byteOffset":0,"byteLength":96}},{{"buffer":0,"byteOffset":96,"byteLength":72}}],
            "accessors":[{{"bufferView":0,"componentType":5126,"count":8,"type":"VEC3","min":[-1,-1,-1],"max":[1,1,1]}},
                         {{"bufferView":1,"componentType":5123,"count":36,"type":"SCALAR"}}]}}"#,
            bin.len(),
            base64(&bin)
        );
        std::fs::write(dir.join("cube.gltf"), json).unwrap();
        let mesh = load(&dir.join("cube.gltf"), UpAxis::Auto).unwrap();
        assert_eq!(mesh.triangles.len(), 12);
        // Scaled by 2 in x, moved by 10 in x; Y-up to Z-up keeps x.
        assert_eq!(mesh.bounds().unwrap(), [[8.0, -1.0, -1.0], [12.0, 1.0, 1.0]]);
        let vol = bake(&mesh, &ModelOptions { keep_units: true, ..opts(24) }, &no_progress);
        assert_color(&vol, [10.0, 0.0, 1.05], [0.214041, 0.0, 1.0]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn texture_sampling_wraps_and_flips() {
        let mut texels = vec![[0u8; 3]; 4];
        texels[0] = [255, 0, 0]; // top left
        let t = Texture { width: 2, height: 2, texels, wrap: [Wrap::Repeat; 2], v_up: false };
        assert!((t.sample(0.25, 0.25)[0] - 1.0).abs() < 1e-6);
        assert!((t.sample(1.25, -0.75)[0] - 1.0).abs() < 1e-6);
        let t = Texture { v_up: true, ..t };
        assert!((t.sample(0.25, 0.75)[0] - 1.0).abs() < 1e-6);
        assert!(t.sample(0.25, 0.25)[0].abs() < 1e-6);
    }

    #[test]
    fn cache_returns_shared_volumes() {
        let dir = temp_dir("cache");
        let file = dir.join("t.stl");
        let src = box_mesh([1.0, 1.0, 1.0]);
        let tris: Vec<stl_io::Triangle> = src
            .triangles
            .iter()
            .map(|t| stl_io::Triangle { normal: stl_io::Vector([0.0; 3]), vertices: t.map(|p| stl_io::Vector(p.map(|c| c as f32))) })
            .collect();
        stl_io::write_stl(&mut std::fs::File::create(&file).unwrap(), tris.iter()).unwrap();
        let path = file.to_str().unwrap();
        let g0 = generation();
        let a = get(path, &opts(20)).unwrap();
        let b = get(path, &opts(20)).unwrap();
        assert!(Arc::ptr_eq(&a, &b));
        assert!(generation() > g0);
        let o = opts(24);
        let start = std::time::Instant::now();
        let v = loop {
            match request(path, &o) {
                Status::Ready(v) => break v,
                Status::Baking(p) => assert!((0.0..=1.0).contains(&p)),
                Status::Failed(e) => panic!("{e}"),
            }
            assert!(start.elapsed().as_secs() < 60, "bake did not finish");
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        assert_eq!(v.res[0], 24);
        assert!(Arc::ptr_eq(&v, &get(path, &o).unwrap()));
        assert!(matches!(request(dir.join("missing.stl").to_str().unwrap(), &o), Status::Failed(_)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Timing of a 128³ bake of a 20k-triangle sphere:
    /// `cargo test --release -p rmb-core mesh::tests::bake_timing -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn bake_timing() {
        let mut m = sphere(100, 100);
        m.shading = vec![Shading { colors: [[0.5, 0.2, 0.1]; 3], ..Shading::default() }; m.triangles.len()];
        let start = std::time::Instant::now();
        let vol = bake(&m, &opts(128), &no_progress);
        eprintln!("{} triangles, {:?} grid: {:.2?}", m.triangles.len(), vol.res, start.elapsed());
        check_sdf(&vol, |p| dot(p, p).sqrt() - 1.0, 0.01);
    }
}
