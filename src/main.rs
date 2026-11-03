#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod interact;
mod lyrics;
mod media;
mod shelf;
mod todo;
mod ui;
mod water;
mod window;

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

    eframe::run_native(
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
    )
}

