// SPDX-License-Identifier: LGPL-2.1-or-later

//! Preferences kept between sessions (`$XDG_CONFIG_HOME/rmandelbulb3d/prefs.json`).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub font_percent: u32,
    /// Size of the whole interface (texts, controls, spacing) in percent.
    pub ui_scale_percent: u32,
    /// The preview stops refining after this many seconds (0: no limit).
    pub preview_seconds: f64,
    /// Scenes that come in without the denoiser (new, opened, pasted, from the explore lane)
    /// get it, with `denoise_blend`.
    pub always_denoise: bool,
    pub denoise_blend: f64,
    /// CPU cores the CPU work (CPU renderer, model baking, thumbnails) leaves free; takes effect
    /// at the next start.
    pub free_cores: usize,
    /// Image size of new scenes.
    pub new_width: u32,
    pub new_height: u32,
    /// Start folders of the file dialogs per kind (see `Folder`).
    pub folders: Folders,
    /// Where File › Quick save puts scenes (None: the scenes folder, else the config folder).
    pub quick_save_dir: Option<PathBuf>,
    /// Scenes per batch in the explore lane.
    pub explore_batch: usize,
    /// Preview resolution in percent of the canvas size.
    pub preview_percent: u32,
    /// Resolution while navigating, in percent of the preview resolution.
    pub navigate_percent: u32,
    /// The preview stops refining after this many samples per pixel.
    pub preview_samples: u32,
    /// Show the image in the scene's aspect ratio (else fill the canvas).
    pub keep_aspect: bool,
    /// Compile a shader with all settings baked in when the preview rests (faster passes).
    pub optimize_resting: bool,
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
    /// Settings sections the user closed (`widgets::section` keys).
    pub closed_sections: Vec<String>,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            font_percent: 100,
            ui_scale_percent: 100,
            preview_seconds: 0.0,
            always_denoise: false,
            denoise_blend: 1.0,
            free_cores: 0,
            new_width: 1280,
            new_height: 720,
            folders: Folders::default(),
            quick_save_dir: None,
            explore_batch: 12,
            preview_percent: 100,
            navigate_percent: 50,
            preview_samples: 256,
            keep_aspect: true,
            optimize_resting: true,
            export_samples: 128,
            last_dir: None,
            recent: Vec::new(),
            formula_dir: None,
            mouse_speed: 1.0,
            invert_y: false,
            maps_dir: None,
            ffmpeg: None,
            closed_sections: Vec::new(),
        }
    }
}

/// Kinds of files with their own start folder in the file dialogs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Folder {
    Scenes,
    Images,
    Movies,
    Models,
    Textures,
    Sounds,
}

impl Folder {
    pub const ALL: [Folder; 6] = [Folder::Scenes, Folder::Images, Folder::Movies, Folder::Models, Folder::Textures, Folder::Sounds];

    pub fn label(self) -> &'static str {
        match self {
            Folder::Scenes => "Scenes",
            Folder::Images => "Saved images",
            Folder::Movies => "Movies",
            Folder::Models => "3D models",
            Folder::Textures => "Textures and backgrounds",
            Folder::Sounds => "Sound tracks",
        }
    }
}

/// Start folder per `Folder` kind (None: the last folder used).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Folders {
    pub scenes: Option<PathBuf>,
    pub images: Option<PathBuf>,
    pub movies: Option<PathBuf>,
    pub models: Option<PathBuf>,
    pub textures: Option<PathBuf>,
    pub sounds: Option<PathBuf>,
}

impl Folders {
    pub fn get_mut(&mut self, f: Folder) -> &mut Option<PathBuf> {
        match f {
            Folder::Scenes => &mut self.scenes,
            Folder::Images => &mut self.images,
            Folder::Movies => &mut self.movies,
            Folder::Models => &mut self.models,
            Folder::Textures => &mut self.textures,
            Folder::Sounds => &mut self.sounds,
        }
    }

    pub fn get(&self, f: Folder) -> Option<&PathBuf> {
        match f {
            Folder::Scenes => self.scenes.as_ref(),
            Folder::Images => self.images.as_ref(),
            Folder::Movies => self.movies.as_ref(),
            Folder::Models => self.models.as_ref(),
            Folder::Textures => self.textures.as_ref(),
            Folder::Sounds => self.sounds.as_ref(),
        }
    }
}

/// The folders of the preferences and the last folder used, for dialogs anywhere in the app.
static DIALOG_DIRS: std::sync::Mutex<(Folders, Option<PathBuf>)> = std::sync::Mutex::new((
    Folders { scenes: None, images: None, movies: None, models: None, textures: None, sounds: None },
    None,
));

/// Hands the folders of `prefs` to `dialog` (at start and after changes).
pub fn set_dialog_dirs(prefs: &Prefs) {
    if let Ok(mut d) = DIALOG_DIRS.lock() {
        *d = (prefs.folders.clone(), prefs.last_dir.clone());
    }
}

/// A file dialog that starts in the folder for `kind` (else the last folder used).
pub fn dialog(kind: Folder) -> rfd::FileDialog {
    let dir = DIALOG_DIRS.lock().ok().and_then(|d| d.0.get(kind).cloned().or_else(|| d.1.clone()));
    let d = rfd::FileDialog::new();
    match dir {
        Some(dir) if dir.is_dir() => d.set_directory(dir),
        _ => d,
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
