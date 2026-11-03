//! File shelf: files dropped onto the notch are kept as a quick-open
//! list, persisted locally as JSON.
//!
//! Location: `%APPDATA%\notch-island\shelf.json` (same folder as
//! `config.json`). Loading is forgiving: a missing file, an unset
//! `APPDATA`, or corrupt JSON all fall back to an empty shelf.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// One shelved file. The path is stored as a string so the JSON stays
/// simple; `name` is just the file name for display.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShelfItem {
    pub path: String,
    pub name: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ShelfState {
    #[serde(default)]
    pub items: Vec<ShelfItem>,
}

impl ShelfState {
    pub fn path() -> Option<PathBuf> {
        std::env::var_os("APPDATA")
            .map(|dir| PathBuf::from(dir).join("notch-island").join("shelf.json"))
    }

    pub fn load() -> Self {
        Self::path()
            .and_then(|p| fs::read_to_string(&p).ok())
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
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

    /// Add a dropped file. Returns false when the path is already on
    /// the shelf (drop events can repeat across frames).
    pub fn add(&mut self, path: &Path) -> bool {
        let path_str = path.to_string_lossy().into_owned();
        if self.items.iter().any(|i| i.path == path_str) {
            return false;
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path_str.clone());
        self.items.push(ShelfItem {
            path: path_str,
            name,
        });
        true
    }
}

/// Open a file with its default app (best-effort, silent on failure).
pub fn open_file(path: &str) {
    let _ = std::process::Command::new("cmd")
        .args(["/C", "start", "", path])
        .spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shelf_add_dedupe_and_remove() {
        let mut shelf = ShelfState::default();
        let p1 = Path::new(r"C:\test\sample1.txt");
        let p2 = Path::new(r"C:\test\sample2.pdf");

        assert!(shelf.add(p1));
        assert_eq!(shelf.items.len(), 1);
        assert_eq!(shelf.items[0].name, "sample1.txt");

        // Dedupe
        assert!(!shelf.add(p1));
        assert_eq!(shelf.items.len(), 1);

        // Add second file
        assert!(shelf.add(p2));
        assert_eq!(shelf.items.len(), 2);
        assert_eq!(shelf.items[1].name, "sample2.pdf");

        // Serde roundtrip
        let json = serde_json::to_string(&shelf).unwrap();
        let loaded: ShelfState = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.items.len(), 2);

        // Remove item
        shelf.items.remove(0);
        assert_eq!(shelf.items.len(), 1);
        assert_eq!(shelf.items[0].name, "sample2.pdf");
    }
}
