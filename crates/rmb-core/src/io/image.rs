// SPDX-License-Identifier: LGPL-2.1-or-later

//! PNG output with the scene embedded as metadata (open the image to get the scene back).

use std::path::Path;

use crate::scene::Scene;

const KEY: &str = "rmandelbulb3d";

/// Stores the scene as compressed text: Latin-1 (zTXt) when it fits, which older versions read
/// too, else UTF-8 (iTXt; e.g. imported titles with other characters).
fn write_scene_chunk<W: std::io::Write>(w: &mut png::Writer<W>, scene: &Scene) -> Result<(), String> {
    use png::text_metadata::{ITXtChunk, ZTXtChunk};
    let json = scene.to_json();
    if json.chars().all(|c| (c as u32) < 256) {
        w.write_text_chunk(&ZTXtChunk::new(KEY, json)).map_err(|e| e.to_string())
    } else {
        let mut c = ITXtChunk::new(KEY, json);
        c.compressed = true;
        w.write_text_chunk(&c).map_err(|e| e.to_string())
    }
}

/// Writes 8-bit RGBA; `scene` is stored in a compressed text chunk.
pub fn write_png(path: &Path, width: u32, height: u32, rgba: &[u8], scene: Option<&Scene>) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header().map_err(|e| e.to_string())?;
    if let Some(s) = scene {
        write_scene_chunk(&mut w, s)?;
    }
    w.write_image_data(rgba).map_err(|e| e.to_string())?;
    w.finish().map_err(|e| e.to_string())
}

/// Writes 16-bit RGBA; `scene` as in [`write_png`].
pub fn write_png16(path: &Path, width: u32, height: u32, rgba: &[u16], scene: Option<&Scene>) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Sixteen);
    let mut w = enc.write_header().map_err(|e| e.to_string())?;
    if let Some(s) = scene {
        write_scene_chunk(&mut w, s)?;
    }
    let bytes: Vec<u8> = rgba.iter().flat_map(|v| v.to_be_bytes()).collect();
    w.write_image_data(&bytes).map_err(|e| e.to_string())?;
    w.finish().map_err(|e| e.to_string())
}

/// Writes an OpenEXR file (32-bit float, lossless ZIP compression): the linear color as R, G,
/// B, and the passes `coverage` (share of the samples that hit a surface), `Z` (distance of
/// the hits from the camera, infinite where none), `albedo.R/G/B` (surface colors) and
/// `normal.X/Y/Z` (world normals). Passes the image lacks are left out. The scene goes into a
/// text attribute.
pub fn write_exr(path: &Path, img: &crate::render::denoise::HdrImage, scene: Option<&Scene>) -> Result<(), String> {
    use exr::prelude::*;
    let (w, h) = (img.width as usize, img.height as usize);
    let n = w * h;
    let plane = |v: &[f32], k: usize, stride: usize| -> Vec<f32> { (0..n).map(|i| v[i * stride + k]).collect() };
    let mut channels: SmallVec<[AnyChannel<FlatSamples>; 4]> = SmallVec::new();
    let mut add = |name: &str, data: Vec<f32>| channels.push(AnyChannel::new(name, FlatSamples::F32(data)));
    if img.color.len() != n * 3 {
        return Err("no image".into());
    }
    for (k, c) in ["R", "G", "B"].iter().enumerate() {
        add(c, plane(&img.color, k, 3));
    }
    if img.alpha.len() == n {
        add("coverage", img.alpha.clone());
    }
    if img.depth.len() == n {
        add("Z", img.depth.clone());
    }
    if img.albedo.len() == n * 3 {
        for (k, c) in ["albedo.R", "albedo.G", "albedo.B"].iter().enumerate() {
            add(c, plane(&img.albedo, k, 3));
        }
    }
    if img.normal.len() == n * 3 {
        for (k, c) in ["normal.X", "normal.Y", "normal.Z"].iter().enumerate() {
            add(c, plane(&img.normal, k, 3));
        }
    }
    let mut attrs = LayerAttributes::default();
    if let Some(s) = scene {
        // UTF-8 bytes (EXR text is bytes; the checked constructor panics outside Latin-1).
        attrs.other.insert(Text::from(KEY), AttributeValue::Text(Text::from_slice_unchecked(s.to_json().as_bytes())));
    }
    let layer = Layer::new(Vec2(w, h), attrs, Encoding::SMALL_LOSSLESS, AnyChannels::sort(channels));
    Image::from_layer(layer).write().to_file(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Image file kinds of a finished render.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ImageKind {
    /// 8 bits per channel, as shown.
    #[default]
    Png8,
    /// 16 bits per channel (smooth gradients for further editing).
    Png16,
    /// OpenEXR: linear light before the post processing, with passes.
    Exr,
}

impl ImageKind {
    pub const ALL: [ImageKind; 3] = [ImageKind::Png8, ImageKind::Png16, ImageKind::Exr];

    pub fn label(self) -> &'static str {
        match self {
            ImageKind::Png8 => "PNG 8-bit",
            ImageKind::Png16 => "PNG 16-bit",
            ImageKind::Exr => "EXR (linear, passes)",
        }
    }

    pub fn extension(self) -> &'static str {
        if self == ImageKind::Exr { "exr" } else { "png" }
    }

    /// EXR for `.exr` paths, else PNG (16-bit when `sixteen`).
    pub fn of_path(path: &Path, sixteen: bool) -> ImageKind {
        match path.extension().and_then(|e| e.to_str()) {
            Some(e) if e.eq_ignore_ascii_case("exr") => ImageKind::Exr,
            _ if sixteen => ImageKind::Png16,
            _ => ImageKind::Png8,
        }
    }
}

/// Finishes a rendered image (denoiser as the scene says; PNG with flares and post processing,
/// EXR linear with passes) and writes it with the scene inside.
pub fn save_render(path: &Path, img: crate::render::denoise::HdrImage, scene: &Scene, kind: ImageKind) -> Result<crate::render::denoise::Outcome, String> {
    save_render_ex(path, img, scene, kind, false)
}

/// [`save_render`] for an image that is `denoised` already (tiled renders denoise every
/// tile): the denoiser is skipped, the scene inside the file stays as it is.
pub fn save_render_ex(path: &Path, img: crate::render::denoise::HdrImage, scene: &Scene, kind: ImageKind, denoised: bool) -> Result<crate::render::denoise::Outcome, String> {
    // Post processing and flares with the still image's driven values.
    let driven = crate::animation::still_scene(scene);
    let scene = driven.as_ref().unwrap_or(scene);
    let (w, h) = (img.width, img.height);
    let finish_scene;
    let fs = if denoised {
        let mut s = scene.clone();
        s.post.denoise = false;
        finish_scene = s;
        &finish_scene
    } else {
        scene
    };
    match kind {
        ImageKind::Png8 => {
            let (rgba, outcome) = img.finish_scene(fs);
            write_png(path, w, h, &rgba, Some(scene)).map(|_| outcome)
        }
        ImageKind::Png16 => {
            let (rgba, outcome) = img.finish_scene16(fs);
            write_png16(path, w, h, &rgba, Some(scene)).map(|_| outcome)
        }
        ImageKind::Exr => {
            let (lin, outcome) = img.finish_linear(fs);
            write_exr(path, &lin, Some(scene)).map(|_| outcome)
        }
    }
}

/// Tile size for an image of `width` × `height`: `Some` (2048) above about 16 megapixels,
/// where the GPU buffers of a whole image get large (52 bytes per pixel).
pub fn auto_tile(width: u32, height: u32) -> Option<u32> {
    (width as u64 * height as u64 > 4096 * 4096).then_some(2048)
}

/// The scene stored in a PNG written by `write_png`.
pub fn scene_from_png(bytes: &[u8]) -> Option<Scene> {
    let dec = png::Decoder::new(std::io::Cursor::new(bytes));
    let reader = dec.read_info().ok()?;
    let info = reader.info();
    for c in &info.compressed_latin1_text {
        if c.keyword == KEY {
            return Scene::from_json(&c.get_text().ok()?).ok();
        }
    }
    for c in &info.uncompressed_latin1_text {
        if c.keyword == KEY {
            return Scene::from_json(&c.text).ok();
        }
    }
    for c in &info.utf8_text {
        if c.keyword == KEY {
            return Scene::from_json(&c.get_text().ok()?).ok();
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_keeps_a_scene_with_characters_beyond_latin1() {
        let dir = std::env::temp_dir().join(format!("rmb-png-utf8-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (k, notes) in ["plain café", "Spirals ★ – 渦"].iter().enumerate() {
            let scene = Scene { notes: notes.to_string(), ..Scene::default() };
            let path = dir.join(format!("{k}.png"));
            write_png(&path, 2, 2, &[0u8; 16], Some(&scene)).unwrap();
            let back = scene_from_png(&std::fs::read(&path).unwrap()).expect("scene");
            assert_eq!(back.notes, *notes);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn exr_has_linear_color_and_passes_and_png16_keeps_the_scene() {
        let scene = Scene::default();
        let mut r = crate::render::cpu::CpuRenderer::new(scene.clone(), 48, 32);
        r.render_pass();
        r.render_pass();
        let img = r.hdr_image();
        let dir = std::env::temp_dir().join(format!("rmb-exr-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.exr");
        save_render(&path, img.clone(), &scene, ImageKind::Exr).unwrap();
        let exr = exr::prelude::read_all_flat_layers_from_file(&path).unwrap();
        let layer = &exr.layer_data[0];
        let ch = |name: &str| -> Vec<f32> { layer.channel_data.list.iter().find(|c| c.name.to_string() == name).unwrap_or_else(|| panic!("no {name}")).sample_data.values_as_f32().collect() };
        for name in ["R", "G", "B", "coverage", "Z", "albedo.R", "normal.X", "normal.Z"] {
            assert_eq!(ch(name).len(), 48 * 32, "{name}");
        }
        let (cov, z, r_) = (ch("coverage"), ch("Z"), ch("R"));
        // The bulb fills the middle, the corners are background.
        let mid = 16 * 48 + 24;
        assert_eq!(cov[mid], 1.0);
        assert!(z[mid].is_finite() && z[mid] > 1.0 && z[mid] < 5.0, "{}", z[mid]);
        assert_eq!(cov[0], 0.0);
        assert!(z[0].is_infinite());
        assert!((r_[mid] - img.color[mid * 3]).abs() < 1e-6, "color is the linear render");
        let png = dir.join("t.png");
        save_render(&png, img, &scene, ImageKind::Png16).unwrap();
        let back = image::open(&png).unwrap();
        assert!(matches!(back.color(), image::ColorType::Rgba16));
        assert!(scene_from_png(&std::fs::read(&png).unwrap()).is_some());
        std::fs::remove_dir_all(&dir).ok();
    }
}
