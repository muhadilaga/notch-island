#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod autostart;
mod config;
mod interact;
mod lyrics;
mod media;
mod shelf;
mod todo;
mod tray;
mod ui;
mod volume;
mod water;
mod window;

pub static UI_HEARTBEAT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn bump_ui_heartbeat() {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    UI_HEARTBEAT.store(now, std::sync::atomic::Ordering::Relaxed);
}

pub fn get_timestamp_string() -> String {
    use windows::Win32::System::SystemInformation::GetLocalTime;
    let st = unsafe { GetLocalTime() };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}",
        st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond, st.wMilliseconds
    )
}

pub fn log_to_crash_file(content: &str) {
    let local_app_data = std::env::var("LOCALAPPDATA")
        .or_else(|_| std::env::var("APPDATA"))
        .unwrap_or_else(|_| ".".to_string());
    let dir = std::path::Path::new(&local_app_data).join("notch-island");
    let _ = std::fs::create_dir_all(&dir);
    let log_path = dir.join("crash.log");
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
    {
        use std::io::Write;
        let _ = file.write_all(content.as_bytes());
        let _ = file.flush();
    }
}

fn setup_panic_hook() {
    let old_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let timestamp = get_timestamp_string();
        let thread = std::thread::current();
        let thread_name = thread.name().unwrap_or("unnamed");
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "unknown location".to_string());

        let msg = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "unknown panic payload".to_string()
        };

        let backtrace = std::backtrace::Backtrace::capture();

        let log_entry = format!(
            "================================================================================\n\
             [PANIC] Timestamp: {}\n\
             Thread: {}\n\
             Location: {}\n\
             Message: {}\n\
             Backtrace:\n{:?}\n\
             ================================================================================\n\n",
            timestamp, thread_name, location, msg, backtrace
        );

        log_to_crash_file(&log_entry);
        old_hook(info);
    }));
}

fn spawn_watchdog_thread() {
    std::thread::Builder::new()
        .name("ui-watchdog".to_string())
        .spawn(move || {
            let mut last_warning: Option<std::time::Instant> = None;
            loop {
                std::thread::sleep(std::time::Duration::from_secs(1));
                let hb = UI_HEARTBEAT.load(std::sync::atomic::Ordering::Relaxed);
                if hb == 0 {
                    continue;
                }
                let now = match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
                    Ok(d) => d.as_secs(),
                    Err(_) => continue,
                };
                if now > hb && now - hb > 5 {
                    let should_log = match last_warning {
                        Some(t) => t.elapsed() >= std::time::Duration::from_secs(10),
                        None => true,
                    };
                    if should_log {
                        last_warning = Some(std::time::Instant::now());
                        let diag = format!(
                            "[WATCHDOG DIAGNOSTIC] {} - UI thread unresponsive for {}s (heartbeat: {}, now: {})\n",
                            get_timestamp_string(),
                            now - hb,
                            hb,
                            now
                        );
                        log_to_crash_file(&diag);
                    }
                } else {
                    last_warning = None;
                }
            }
        })
        .ok();
}

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use eframe::egui;

use config::AppConfig;
use interact::{spawn_interact_threads, InteractState, SharedInteract};
use shelf::ShelfState;
use todo::TodoState;
use water::WaterState;

use lyrics::LyricsState;
use media::{start_media_poller, MediaTrack};
use ui::NotchApp;
use window::ScreenGeometry;

fn main() -> eframe::Result<()> {
    setup_panic_hook();
    spawn_watchdog_thread();

    let geom = ScreenGeometry::current();
    let initial_width = 300.0;
    let initial_height = 36.0;
    let initial_x = (geom.screen_width - initial_width) / 2.0;
    let initial_y = 0.0;

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Notch Island")
            .with_decorations(false)
            .with_transparent(true)
            .with_always_on_top()
            .with_resizable(false)
            .with_taskbar(false)
            .with_inner_size(egui::vec2(initial_width, initial_height))
            .with_position(egui::pos2(initial_x, initial_y)),
        ..Default::default()
    };

    let shared_media = Arc::new(Mutex::new(MediaTrack::default()));
    let shared_lyrics = Arc::new(Mutex::new(LyricsState::default()));
    let lyrics_cache = Arc::new(Mutex::new(HashMap::new()));

    // Load persisted preferences (mode, lyrics toggle, pomodoro
    // durations); defaults when the config file is missing or
    // unreadable.
    let config = AppConfig::load();

    // Load persisted todos (own file); empty list when missing or
    // unreadable.
    let todo_state = TodoState::load();

    // Load persisted water count + file shelf (own files).
    let water_state = WaterState::load();
    let shelf_state = ShelfState::load();

    // Start Win32 interact threads (click-through mouse hook +
    // fullscreen watcher) alongside the media poller below.
    let shared_interact: SharedInteract = Arc::new(Mutex::new(InteractState::default()));
    spawn_interact_threads(Arc::clone(&shared_interact));

    // Start background Windows GSMTC Media Poller
    start_media_poller(Arc::clone(&shared_media));

    let res = eframe::run_native(
        "Notch Island",
        native_options,
        Box::new(|_cc| {
            Ok(Box::new(NotchApp::new(
                shared_media,
                shared_lyrics,
                lyrics_cache,
                config,
                shared_interact,
                todo_state,
                water_state,
                shelf_state,
            )))
        }),
    );
    if let Err(ref e) = res {
        let err_entry = format!(
            "[EFRAME ERROR] {} - eframe::run_native exited with error: {:?}\n",
            get_timestamp_string(),
            e
        );
        log_to_crash_file(&err_entry);
    }
    res
}

