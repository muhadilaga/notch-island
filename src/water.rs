//! Water reminder state, persisted locally as JSON.
//!
//! Location: `%APPDATA%\notch-island\water.json` (same folder as
//! `config.json`). Intake is tracked in millilitres with a daily
//! target, a bottle size, active reminder hours, and a 24-hour
//! history (per-hour volumes). The reminder countdown itself
//! restarts with the app. Loading is forgiving: a missing file,
//! an unset `APPDATA`, or corrupt JSON all fall back to defaults
//! instead of crashing the app.
//!
//! Backward compatibility (P3): files written by older versions
//! (glass count + `YYYY-MM-DD` day) are migrated on load — one
//! glass counts as one bottle, and the day is converted to
//! `YYYYMMDD`. The legacy glass field is never written back.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use windows::Win32::System::SystemInformation::GetLocalTime;

/// Local calendar day as `YYYYMMDD` (Nusa `water.txt` format).
fn today_yyyymmdd() -> String {
    unsafe {
        let st = GetLocalTime();
        format!("{:04}{:02}{:02}", st.wYear, st.wMonth, st.wDay)
    }
}

/// Minutes since local midnight (0..1439).
fn now_minutes() -> u32 {
    unsafe {
        let st = GetLocalTime();
        st.wHour as u32 * 60 + st.wMinute as u32
    }
}

/// Current local hour (0..23).
fn now_hour() -> usize {
    unsafe { GetLocalTime().wHour as usize }
}

fn default_bottle_ml() -> u32 {
    500
}
fn default_goal_ml() -> u32 {
    1500
}
fn default_start_min() -> u32 {
    480 // 08:00
}
fn default_end_min() -> u32 {
    1020 // 17:00
}
fn default_true() -> bool {
    true
}

/// Minutes since midnight as `HH:MM` (for the water UI).
pub fn fmt_hhmm(mins: u32) -> String {
    format!("{:02}:{:02}", mins / 60, mins % 60)
}

/// True when `now` (minutes since midnight) falls inside the active
/// reminder window `[start, end)`. A window with `start >= end` is
/// empty (no active hours) — overnight windows are not supported.
pub fn in_active_window(now: u32, start: u32, end: u32) -> bool {
    start < end && now >= start && now < end
}

/// Persisted water state: millilitres drunk today, with a daily
/// target, bottle size, active reminder hours, and per-hour history.
/// The counters reset automatically when the calendar day changes
/// (checked on load).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WaterState {
    #[serde(default = "default_bottle_ml")]
    pub bottle_ml: u32,
    #[serde(default = "default_goal_ml")]
    pub goal_ml: u32,
    #[serde(default)]
    pub taken_ml: u32,
    #[serde(default = "default_start_min")]
    pub start_min: u32,
    #[serde(default = "default_end_min")]
    pub end_min: u32,
    #[serde(default)]
    pub hourly_log: [u32; 24],
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub day: String,
    /// Legacy glass count from pre-P3 files. Never written anymore
    /// (`skip_serializing`); read only so `load()` can migrate it
    /// to millilitres once (1 glass = 1 bottle).
    #[serde(default, skip_serializing)]
    pub count_today: u32,
}

impl Default for WaterState {
    fn default() -> Self {
        Self {
            bottle_ml: default_bottle_ml(),
            goal_ml: default_goal_ml(),
            taken_ml: 0,
            start_min: default_start_min(),
            end_min: default_end_min(),
            hourly_log: [0; 24],
            enabled: true,
            day: today_yyyymmdd(),
            count_today: 0,
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
        state.normalize();
        state
    }

    /// Day rollover + legacy migration. Safe to call on any state:
    /// - Old `YYYY-MM-DD` day strings are converted to `YYYYMMDD`.
    /// - A day other than today resets today's counters.
    /// - An old glass count (`count_today`) from today is converted
    ///   to millilitres (1 glass = 1 bottle) and attributed to the
    ///   current hour, then cleared.
    /// - Zero bottle/goal values (e.g. hand-edited files) fall back
    ///   to defaults; start/end are clamped to the day (<= 1440).
    pub fn normalize(&mut self) {
        if self.day.len() == 10 && self.day.as_bytes().get(4) == Some(&b'-') {
            self.day = self.day.replace('-', "");
        }
        if self.bottle_ml == 0 {
            self.bottle_ml = default_bottle_ml();
        }
        if self.goal_ml == 0 {
            self.goal_ml = default_goal_ml();
        }
        self.start_min = self.start_min.min(1440);
        self.end_min = self.end_min.min(1440);
        let today = today_yyyymmdd();
        if self.day != today {
            self.day = today;
            self.taken_ml = 0;
            self.hourly_log = [0; 24];
            self.count_today = 0;
            return;
        }
        if self.count_today > 0 && self.taken_ml == 0 {
            let migrated = self.count_today.saturating_mul(self.bottle_ml);
            self.taken_ml = migrated;
            let hour = now_hour();
            self.hourly_log[hour] = self.hourly_log[hour].saturating_add(migrated);
            self.count_today = 0;
        }
    }

    /// True when a water reminder may fire right now: reminders
    /// enabled, inside the active hours, and the daily goal not
    /// yet reached.
    pub fn reminder_active(&self) -> bool {
        self.enabled
            && in_active_window(now_minutes(), self.start_min, self.end_min)
            && self.taken_ml < self.goal_ml
    }

    /// Record one bottle drunk now: adds `bottle_ml` to today's
    /// total and to the current hour's bucket.
    pub fn add_drink(&mut self) {
        self.add_drink_at(now_hour());
    }

    /// Same as `add_drink`, for an explicit hour (0..23). Hours
    /// outside 0..23 are ignored.
    pub fn add_drink_at(&mut self, hour: usize) {
        if hour >= 24 {
            return;
        }
        self.taken_ml = self.taken_ml.saturating_add(self.bottle_ml);
        self.hourly_log[hour] = self.hourly_log[hour].saturating_add(self.bottle_ml);
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
    fn test_water_state_serde_defaults() {
        // New-format JSON with only some fields: the rest default.
        let state: WaterState = serde_json::from_str(r#"{"taken_ml": 750}"#).unwrap();
        assert_eq!(state.bottle_ml, 500);
        assert_eq!(state.goal_ml, 1500);
        assert_eq!(state.taken_ml, 750);
        assert_eq!(state.start_min, 480);
        assert_eq!(state.end_min, 1020);
        assert_eq!(state.hourly_log, [0; 24]);
        assert!(state.enabled);
        assert_eq!(state.day, "");
        assert_eq!(state.count_today, 0);
    }

    #[test]
    fn test_water_state_legacy_migration() {
        // Old-format file: glass count + YYYY-MM-DD day (today).
        let today_dashes = unsafe {
            let st = GetLocalTime();
            format!("{:04}-{:02}-{:02}", st.wYear, st.wMonth, st.wDay)
        };
        let json = format!(r#"{{"count_today": 3, "day": "{}"}}"#, today_dashes);
        let mut state: WaterState = serde_json::from_str(&json).unwrap();
        assert_eq!(state.count_today, 3);
        state.normalize();
        // 3 glasses x 500 mL bottle, day converted to YYYYMMDD.
        assert_eq!(state.taken_ml, 1500);
        assert_eq!(state.count_today, 0);
        assert_eq!(state.hourly_log.iter().sum::<u32>(), 1500);
        assert_eq!(state.day.len(), 8);
        assert!(!state.day.contains('-'));

        // Re-saving must not write the legacy field back.
        let saved = serde_json::to_string(&state).unwrap();
        assert!(!saved.contains("count_today"));

        // Yesterday's old file resets instead of migrating.
        let mut old: WaterState =
            serde_json::from_str(r#"{"count_today": 2, "day": "2020-01-01"}"#).unwrap();
        old.normalize();
        assert_eq!(old.taken_ml, 0);
        assert_eq!(old.count_today, 0);
        assert_eq!(old.hourly_log, [0; 24]);
    }

    #[test]
    fn test_in_active_window() {
        assert!(!in_active_window(479, 480, 1020)); // 07:59
        assert!(in_active_window(480, 480, 1020)); // 08:00
        assert!(in_active_window(1019, 480, 1020)); // 16:59
        assert!(!in_active_window(1020, 480, 1020)); // 17:00
        assert!(!in_active_window(0, 480, 1020)); // midnight
        assert!(!in_active_window(600, 1020, 480)); // start >= end: empty
        assert!(!in_active_window(600, 480, 480)); // start == end: empty
    }

    #[test]
    fn test_add_drink_at() {
        let mut state = WaterState::default();
        state.add_drink_at(9);
        assert_eq!(state.taken_ml, 500);
        assert_eq!(state.hourly_log[9], 500);
        state.add_drink_at(9);
        assert_eq!(state.taken_ml, 1000);
        assert_eq!(state.hourly_log[9], 1000);
        state.add_drink_at(25); // out of range: ignored
        assert_eq!(state.taken_ml, 1000);
    }

    #[test]
    fn test_fmt_hhmm() {
        assert_eq!(fmt_hhmm(480), "08:00");
        assert_eq!(fmt_hhmm(1020), "17:00");
        assert_eq!(fmt_hhmm(0), "00:00");
        assert_eq!(fmt_hhmm(1439), "23:59");
    }
}
