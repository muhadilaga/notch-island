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
    pub show_lyrics: bool,
    // New fields carry serde defaults so a config.json written before
    // they existed still parses (otherwise the whole file would fall
    // back to defaults and lose mode/show_lyrics).
    #[serde(default = "default_focus_mins")]
    pub focus_mins: u64,
    #[serde(default = "default_break_mins")]
    pub break_mins: u64,
    #[serde(default = "default_water_mins")]
    pub water_mins: u64,
    #[serde(default = "default_enable_fillet")]
    pub enable_fillet: bool,
    #[serde(default = "default_lyric_offset_ms")]
    pub lyric_offset_ms: i64,
    #[serde(default = "default_enable_media_diag")]
    pub enable_media_diag: bool,
    #[serde(default = "default_song_offsets")]
    pub song_offsets: std::collections::HashMap<String, i64>,
    #[serde(default = "default_accent")]
    pub accent: String,
    #[serde(default = "default_accent_progress")]
    pub accent_progress: bool,
}

fn default_accent() -> String {
    "#1ED760".to_string()
}

fn default_accent_progress() -> bool {
    true
}

fn default_song_offsets() -> std::collections::HashMap<String, i64> {
    std::collections::HashMap::new()
}

fn default_focus_mins() -> u64 {
    25
}

fn default_break_mins() -> u64 {
    5
}

fn default_water_mins() -> u64 {
    60
}

fn default_enable_fillet() -> bool {
    false
}

fn default_lyric_offset_ms() -> i64 {
    0
}

fn default_enable_media_diag() -> bool {
    false
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            mode: DisplayMode::MinimalBezel,
            show_lyrics: false,
            focus_mins: default_focus_mins(),
            break_mins: default_break_mins(),
            water_mins: default_water_mins(),
            enable_fillet: default_enable_fillet(),
            lyric_offset_ms: default_lyric_offset_ms(),
            enable_media_diag: default_enable_media_diag(),
            song_offsets: default_song_offsets(),
            accent: default_accent(),
            accent_progress: default_accent_progress(),
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

pub fn parse_hex_color(hex: &str) -> Option<egui::Color32> {
    let s = hex.trim().strip_prefix('#').unwrap_or(hex.trim());
    if s.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some(egui::Color32::from_rgb(r, g, b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_backward_compatibility() {
        let old_json = r#"{
            "mode": "MinimalBezel",
            "show_lyrics": true
        }"#;
        let cfg: AppConfig = serde_json::from_str(old_json).unwrap();
        assert_eq!(cfg.accent, "#1ED760");
        assert_eq!(cfg.accent_progress, true);
        assert_eq!(
            parse_hex_color(&cfg.accent),
            Some(egui::Color32::from_rgb(0x1E, 0xD7, 0x60))
        );
    }

    #[test]
    fn test_parse_hex_color() {
        assert_eq!(
            parse_hex_color("#1ED760"),
            Some(egui::Color32::from_rgb(0x1E, 0xD7, 0x60))
        );
        assert_eq!(
            parse_hex_color("1ED760"),
            Some(egui::Color32::from_rgb(0x1E, 0xD7, 0x60))
        );
        assert_eq!(
            parse_hex_color("#ffffff"),
            Some(egui::Color32::from_rgb(255, 255, 255))
        );
        assert_eq!(parse_hex_color("invalid"), None);
        assert_eq!(parse_hex_color("#12345"), None);
    }
}
