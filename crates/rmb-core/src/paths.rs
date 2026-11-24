// SPDX-License-Identifier: LGPL-2.1-or-later
//! The user's folders for settings and caches, per platform. `XDG_CONFIG_HOME` and
//! `XDG_CACHE_HOME` are honored everywhere (also handy for tests); otherwise:
//!
//! | | Settings | Caches |
//! |---|---|---|
//! | Linux | `~/.config/rmandelbulb3d` | `~/.cache/rmandelbulb` |
//! | macOS | `~/Library/Application Support/rmandelbulb3d` | `~/Library/Caches/rmandelbulb` |
//! | Windows | `%APPDATA%\rmandelbulb3d` | `%LOCALAPPDATA%\rmandelbulb` |

use std::path::PathBuf;

fn env_dir(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from)
}

fn home() -> Option<PathBuf> {
    env_dir("HOME").or_else(|| env_dir("USERPROFILE"))
}

/// Folder of the settings (preferences, palettes, formulas, render queue).
pub fn config_dir() -> PathBuf {
    let base = env_dir("XDG_CONFIG_HOME").or_else(|| {
        if cfg!(windows) {
            env_dir("APPDATA")
        } else if cfg!(target_os = "macos") {
            home().map(|h| h.join("Library").join("Application Support"))
        } else {
            home().map(|h| h.join(".config"))
        }
    });
    base.unwrap_or_else(|| PathBuf::from(".")).join("rmandelbulb3d")
}

/// Folder of the caches (baked 3D models, network render files).
pub fn cache_dir() -> PathBuf {
    let base = env_dir("XDG_CACHE_HOME").or_else(|| {
        if cfg!(windows) {
            env_dir("LOCALAPPDATA")
        } else if cfg!(target_os = "macos") {
            home().map(|h| h.join("Library").join("Caches"))
        } else {
            home().map(|h| h.join(".cache"))
        }
    });
    base.unwrap_or_else(std::env::temp_dir).join("rmandelbulb")
}
