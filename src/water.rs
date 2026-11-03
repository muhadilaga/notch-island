//! Water reminder state, persisted locally as JSON.
//!
//! Location: `%APPDATA%\notch-island\water.json` (same folder as
//! `config.json`). Only the daily glass count is persisted; the
//! reminder countdown itself restarts with the app. Loading is
//! forgiving: a missing file, an unset `APPDATA`, or corrupt JSON
//! all fall back to zero glasses instead of crashing the app.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::todo::today_key;

/// Persisted water state: glasses drunk today. The counter resets
/// automatically when the calendar day changes (checked on load).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WaterState {
    #[serde(default)]
    pub count_today: u32,
    #[serde(default)]
    pub day: String,
}

impl Default for WaterState {
    fn default() -> Self {
        Self {
            count_today: 0,
            day: today_key(),
        }
    }
}

impl WaterState {
    pub fn path() -> Option<PathBuf> {
        std::env::var_os("APPDATA")
            .map(|dir| PathBuf::from(dir).join("notch-island").join("water.json"))
    }

    pub fn load() -> Self {
        let mut state: Self = match Self::path()
            .and_then(|p| fs::read_to_string(&p).ok())
            .and_then(|text| serde_json::from_str(&text).ok())
        {
            Some(s) => s,
            None => return Self::default(),
        };
        let today = today_key();
        if state.day != today {
            state.day = today;
            state.count_today = 0;
        }
        state
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_water_state_serde_and_reset() {
        let json = r#"{"count_today": 3, "day": "2020-01-01"}"#;
        let mut state: WaterState = serde_json::from_str(json).unwrap();
        assert_eq!(state.count_today, 3);
        assert_eq!(state.day, "2020-01-01");

        // Day mismatch resets count
        let today = today_key();
        if state.day != today {
            state.day = today;
            state.count_today = 0;
        }
        assert_eq!(state.count_today, 0);
    }

    #[test]
    fn test_water_reminder_due() {
        use std::time::{Duration, Instant};
        let water_mins = 1;
        let mut next_due = Instant::now() - Duration::from_secs(1);
        let due = Instant::now() >= next_due;
        assert!(due);

        // Click "Sudah minum"
        let mut count_today = 0;
        count_today += 1;
        next_due = Instant::now() + Duration::from_secs(water_mins * 60);
        let due_after = Instant::now() >= next_due;
        assert_eq!(count_today, 1);
        assert!(!due_after);
    }
}
