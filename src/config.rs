//! Persistent user preferences, stored locally as JSON.
//!
//! Location: `%APPDATA%\notch-island\config.json`
//! (e.g. `C:\Users\<user>\AppData\Roaming\notch-island\config.json`)
//!
//! Loading is intentionally forgiving: a missing file, an unset
//! `APPDATA`, or a corrupt/unknown JSON value all fall back to the
//! default config instead of crashing the app. Saving is best-effort
//! and never propagates I/O errors to the UI.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::window::DisplayMode;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub mode: DisplayMode,
    pub is_pinned: bool,
    pub show_lyrics: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            mode: DisplayMode::MinimalBezel,
            is_pinned: false,
            show_lyrics: false,
        }
    }
}

impl AppConfig {
    pub fn path() -> Option<PathBuf> {
        std::env::var_os("APPDATA")
            .map(|dir| PathBuf::from(dir).join("notch-island").join("config.json"))
    }

    pub fn load() -> Self {
        let Some(path) = Self::path() else {
            return Self::default();
        };
        let Ok(text) = fs::read_to_string(&path) else {
            return Self::default();
        };
        serde_json::from_str(&text).unwrap_or_default()
    }

    pub fn save(&self) {
        let Some(path) = Self::path() else {
            return;
        };
        if let Some(parent) = path.parent() {
            if fs::create_dir_all(parent).is_err() {
                return;
            }
        }
        if let Ok(text) = serde_json::to_string_pretty(self) {
            let _ = fs::write(&path, text);
        }
    }
}
