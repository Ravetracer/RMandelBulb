// SPDX-License-Identifier: LGPL-2.1-or-later

//! Preferences kept between sessions (`$XDG_CONFIG_HOME/rmandelbulb3d/prefs.json`).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub font_percent: u32,
    /// Preview resolution in percent of the canvas size.
    pub preview_percent: u32,
    /// Resolution while navigating, in percent of the preview resolution.
    pub navigate_percent: u32,
    /// The preview stops refining after this many samples per pixel.
    pub preview_samples: u32,
    /// Show the image in the scene's aspect ratio (else fill the canvas).
    pub keep_aspect: bool,
    pub export_samples: u32,
    pub last_dir: Option<PathBuf>,
    pub recent: Vec<PathBuf>,
    /// Extra formula folder (`*.mbf`), besides the one in the config folder.
    pub formula_dir: Option<PathBuf>,
    /// Mouse sensitivity of orbit and look.
    pub mouse_speed: f64,
    pub invert_y: bool,
    /// Mandelbulb3D's map folder (M3Maps) for formulas that read maps.
    pub maps_dir: Option<PathBuf>,
    /// The ffmpeg program for movies (None: `ffmpeg` on the search path).
    pub ffmpeg: Option<PathBuf>,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            font_percent: 100,
            preview_percent: 100,
            navigate_percent: 50,
            preview_samples: 256,
            keep_aspect: true,
            export_samples: 128,
            last_dir: None,
            recent: Vec::new(),
            formula_dir: None,
            mouse_speed: 1.0,
            invert_y: false,
            maps_dir: None,
            ffmpeg: None,
        }
    }
}

pub fn config_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("APPDATA").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("rmandelbulb3d")
}

/// User formula folder inside the config folder.
pub fn user_formula_dir() -> PathBuf {
    config_dir().join("formulas")
}

impl Prefs {
    pub fn load() -> Prefs {
        std::fs::read_to_string(config_dir().join("prefs.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let dir = config_dir();
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(dir.join("prefs.json"), json).map_err(|e| e.to_string())
    }

    pub fn add_recent(&mut self, p: PathBuf) {
        self.recent.retain(|r| *r != p);
        self.recent.insert(0, p);
        self.recent.truncate(12);
    }
}
