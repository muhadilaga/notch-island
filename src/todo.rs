//! Todo list + Pomodoro timer, persisted locally as JSON.
//!
//! Location: `%APPDATA%\notch-island\todos.json` (same folder as
//! `config.json`). Loading is forgiving: a missing file, an unset
//! `APPDATA`, or corrupt JSON all fall back to an empty list instead
//! of crashing the app. Saving is best-effort and never propagates
//! I/O errors to the UI.

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use windows::Win32::System::SystemInformation::GetLocalTime;

/// One todo item.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
    pub text: String,
    pub done: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PomodoroPhase {
    Focus,
    Break,
}

/// Persisted todo state: the list plus today's finished-session count.
/// The counter resets automatically when the calendar day changes
/// (checked on load, using the local date).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoState {
    pub items: Vec<TodoItem>,
    #[serde(default)]
    pub sessions_today: u32,
    #[serde(default)]
    pub day: String,
}

impl Default for TodoState {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            sessions_today: 0,
            day: today_key(),
        }
    }
}

/// Local calendar day as `YYYY-MM-DD` (for the daily session reset).
fn today_key() -> String {
    unsafe {
        let st = GetLocalTime();
        format!("{:04}-{:02}-{:02}", st.wYear, st.wMonth, st.wDay)
    }
}

impl TodoState {
    pub fn path() -> Option<PathBuf> {
        std::env::var_os("APPDATA")
            .map(|dir| PathBuf::from(dir).join("notch-island").join("todos.json"))
    }

    pub fn load() -> Self {
        let mut state: Self = match Self::path()
            .and_then(|p| fs::read_to_string(&p).ok())
            .and_then(|text| serde_json::from_str(&text).ok())
        {
            Some(s) => s,
            None => return Self::default(),
        };
        // New calendar day -> reset today's session counter. The
        // reset is persisted on the next save (any todo edit or
        // finished session); a stale file simply resets again here.
        let today = today_key();
        if state.day != today {
            state.day = today;
            state.sessions_today = 0;
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

/// Runtime Pomodoro countdown. Durations come from `AppConfig`
/// (minutes); the timer itself is NOT persisted — it restarts idle
/// when the app launches.
pub struct PomodoroTimer {
    pub phase: PomodoroPhase,
    pub running: bool,
    ends_at: Option<Instant>,
    remaining: Duration,
}

impl PomodoroTimer {
    pub fn new(focus_mins: u64) -> Self {
        Self {
            phase: PomodoroPhase::Focus,
            running: false,
            ends_at: None,
            remaining: Duration::from_secs(focus_mins * 60),
        }
    }

    pub fn remaining_secs(&self) -> u64 {
        if self.running {
            match self.ends_at {
                Some(end) => end.saturating_duration_since(Instant::now()).as_secs(),
                None => self.remaining.as_secs(),
            }
        } else {
            self.remaining.as_secs()
        }
    }

    pub fn start(&mut self) {
        if !self.running {
            self.ends_at = Some(Instant::now() + self.remaining);
            self.running = true;
        }
    }

    pub fn pause(&mut self) {
        if self.running {
            self.remaining = Duration::from_secs(self.remaining_secs());
            self.ends_at = None;
            self.running = false;
        }
    }

    pub fn reset(&mut self, focus_mins: u64, break_mins: u64) {
        let mins = match self.phase {
            PomodoroPhase::Focus => focus_mins,
            PomodoroPhase::Break => break_mins,
        };
        self.remaining = Duration::from_secs(mins * 60);
        self.ends_at = None;
        self.running = false;
    }

    /// Advance the countdown. Returns `Some(finished_phase)` exactly
    /// once when a phase runs out, so the caller can count the
    /// session and switch phases. Focus <-> Break switches
    /// automatically and keeps running.
    pub fn tick(&mut self, focus_mins: u64, break_mins: u64) -> Option<PomodoroPhase> {
        if !self.running {
            return None;
        }
        if self.remaining_secs() > 0 {
            return None;
        }
        let finished = self.phase;
        self.phase = match self.phase {
            PomodoroPhase::Focus => PomodoroPhase::Break,
            PomodoroPhase::Break => PomodoroPhase::Focus,
        };
        let mins = match self.phase {
            PomodoroPhase::Focus => focus_mins,
            PomodoroPhase::Break => break_mins,
        };
        self.remaining = Duration::from_secs(mins * 60);
        self.ends_at = Some(Instant::now() + self.remaining);
        Some(finished)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;

    #[test]
    fn test_todo_crud_and_serde() {
        let mut state = TodoState::default();
        assert_eq!(state.items.len(), 0);

        // Tambah todo
        state.items.push(TodoItem {
            text: "Beli kopi".to_string(),
            done: false,
        });
        state.items.push(TodoItem {
            text: "Coding notch".to_string(),
            done: false,
        });
        assert_eq!(state.items.len(), 2);

        // Centang todo
        state.items[0].done = true;
        assert!(state.items[0].done);

        // Serde roundtrip (simulasi restart app)
        let json = serde_json::to_string(&state).unwrap();
        let loaded: TodoState = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.items.len(), 2);
        assert!(loaded.items[0].done);
        assert_eq!(loaded.items[0].text, "Beli kopi");
        assert!(!loaded.items[1].done);

        // Hapus todo
        let mut state = loaded;
        state.items.remove(0);
        assert_eq!(state.items.len(), 1);
        assert_eq!(state.items[0].text, "Coding notch");
    }

    #[test]
    fn test_pomodoro_controls() {
        let mut timer = PomodoroTimer::new(25);
        assert_eq!(timer.phase, PomodoroPhase::Focus);
        assert!(!timer.running);
        assert_eq!(timer.remaining_secs(), 25 * 60);

        // Mulai -> timer berjalan
        timer.start();
        assert!(timer.running);

        // Jeda -> berhenti
        timer.pause();
        assert!(!timer.running);
        assert!(timer.remaining_secs() <= 25 * 60);

        // Reset -> kembali 25:00
        timer.reset(25, 5);
        assert_eq!(timer.remaining_secs(), 25 * 60);
        assert!(!timer.running);
    }

    #[test]
    fn test_pomodoro_auto_transition() {
        let mut timer = PomodoroTimer::new(1);
        timer.start();

        // Paksa ends_at ke masa lalu untuk simulasi 1 menit habis
        timer.ends_at = Some(Instant::now() - Duration::from_secs(1));
        assert_eq!(timer.remaining_secs(), 0);

        // Tick -> pindah ke Break (Istirahat), kembalikan Some(Focus)
        let finished = timer.tick(1, 1);
        assert_eq!(finished, Some(PomodoroPhase::Focus));
        assert_eq!(timer.phase, PomodoroPhase::Break);
        assert!(timer.remaining_secs() >= 59 && timer.remaining_secs() <= 60);
        assert!(timer.running);

        // Paksa break habis
        timer.ends_at = Some(Instant::now() - Duration::from_secs(1));
        assert_eq!(timer.remaining_secs(), 0);

        // Tick -> balik ke Focus, kembalikan Some(Break)
        let finished = timer.tick(1, 1);
        assert_eq!(finished, Some(PomodoroPhase::Break));
        assert_eq!(timer.phase, PomodoroPhase::Focus);
        assert!(timer.remaining_secs() >= 59 && timer.remaining_secs() <= 60);
        assert!(timer.running);
    }

    #[test]
    fn test_old_config_backward_compatibility() {
        let old_config_json = r#"{
            "mode": "FloatingIsland",
            "show_lyrics": true
        }"#;

        let config: AppConfig = serde_json::from_str(old_config_json).unwrap();
        assert_eq!(config.focus_mins, 25);
        assert_eq!(config.break_mins, 5);
        assert!(config.show_lyrics);
    }
}
