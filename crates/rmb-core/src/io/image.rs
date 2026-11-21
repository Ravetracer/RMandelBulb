// SPDX-License-Identifier: LGPL-2.1-or-later

//! PNG output with the scene embedded as metadata (open the image to get the scene back).

use std::path::Path;

use crate::scene::Scene;

const KEY: &str = "rmandelbulb3d";

/// Writes 8-bit RGBA; `scene` is stored in a compressed text chunk.
pub fn write_png(path: &Path, width: u32, height: u32, rgba: &[u8], scene: Option<&Scene>) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    if let Some(s) = scene {
        enc.add_ztxt_chunk(KEY.to_string(), s.to_json()).map_err(|e| e.to_string())?;
    }
    let mut w = enc.write_header().map_err(|e| e.to_string())?;
    w.write_image_data(rgba).map_err(|e| e.to_string())?;
    w.finish().map_err(|e| e.to_string())
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
    None
}
