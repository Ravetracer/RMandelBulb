// SPDX-License-Identifier: LGPL-2.1-or-later

//! Loading and saving scenes and images.

pub mod m3p;
pub mod image;

use std::path::Path;

use crate::scene::Scene;

/// Native scene file extension.
pub const EXTENSION: &str = "rmb";

/// Loads a scene: native JSON (`.rmb`), Mandelbulb3D parameters (`.m3p` or clipboard text in a
/// `.txt`) or a PNG written by this program (scene in its metadata).
pub fn load(path: &Path) -> Result<Scene, String> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    match ext.as_str() {
        "m3p" => m3p::import_binary(&bytes).map(|r| r.scene),
        "png" => image::scene_from_png(&bytes).ok_or_else(|| "the PNG has no RMandelbulb3D scene in its metadata".to_string()),
        _ => {
            let text = String::from_utf8_lossy(&bytes);
            parse_text(&text)
        }
    }
}

/// Parses a scene from text: native JSON or Mandelbulb3D clipboard parameters.
pub fn parse_text(text: &str) -> Result<Scene, String> {
    let t = text.trim_start();
    if t.starts_with('{') {
        Scene::from_json(t)
    } else if m3p::looks_like_text(t) {
        m3p::import_text(t).map(|r| r.scene)
    } else {
        Err("not a scene: expected RMandelbulb3D JSON or Mandelbulb3D parameters".into())
    }
}

pub fn save(scene: &Scene, path: &Path) -> Result<(), String> {
    std::fs::write(path, scene.to_json()).map_err(|e| format!("{}: {e}", path.display()))
}
