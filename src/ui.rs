use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use egui::{
    pos2, vec2, Align, Align2, Color32, CornerRadius, CursorIcon, FontId, Id, Layout, Pos2, Rect,
    RichText, ScrollArea, Sense, Stroke,
};

use crate::autostart::{disable_autostart, enable_autostart, is_autostart_enabled};
use crate::config::AppConfig;
use crate::interact::{NotchRect, SharedInteract};
use crate::lyrics::{fetch_lyrics_async, line_index, LyricLine, LyricsState, LyricSync, SharedLyrics};
use crate::media::{seek_media_position, send_media_command, MediaTrack, SharedMedia};
use crate::shelf::{open_file, ShelfState};
use crate::todo::{PomodoroPhase, PomodoroTimer, TodoState};
use crate::tray::{build_tray, show_version_notification, TrayHandles};
use crate::volume::{
    get_system_volume, spawn_system_volume_worker, VolCmd, VolumeHandle,
};

#[inline]
pub fn safe_lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match m.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}
use crate::water::WaterState;
use crate::window::{DisplayMode, ScreenGeometry};

use tray_icon::menu::MenuEvent;

/// Which content the expanded panel shows. Music is the default;
/// Todo and Shelf are alternate views (music keeps playing).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelView {
    Music,
    Todo,
    Shelf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PillContentKind {
    Productivity,
    Title,
    Lyric,
}

pub fn select_pill_content(
    has_media: bool,
    show_lyrics: bool,
    has_active_lyric: bool,
) -> PillContentKind {
    if !has_media {
        PillContentKind::Productivity
    } else if show_lyrics && has_active_lyric {
        PillContentKind::Lyric
    } else {
        PillContentKind::Title
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CollapseGuard {
    pub last_inside: Instant,
    pub hold_until: Instant,
}

impl Default for CollapseGuard {
    fn default() -> Self {
        let now = Instant::now();
        Self {
            last_inside: now,
            hold_until: now,
        }
    }
}

impl CollapseGuard {
    pub fn new(now: Instant) -> Self {
        Self {
            last_inside: now,
            hold_until: now,
        }
    }

    pub fn on_resize(&mut self, now: Instant) {
        self.hold_until = now + Duration::from_millis(400);
    }

    pub fn reset_hover(&mut self, now: Instant) {
        self.last_inside = now;
    }

    pub fn should_collapse(&mut self, cursor_inside: bool, mouse_down: bool, now: Instant) -> bool {
        if cursor_inside || mouse_down {
            self.last_inside = now;
            return false;
        }
        if now < self.hold_until {
            return false;
        }
        now.saturating_duration_since(self.last_inside) > Duration::from_millis(150)
    }
}

#[derive(Debug, Clone)]
pub struct PillLyricState {
    pub prev: String,
    pub cur: String,
    pub changed_at: Instant,
}

impl Default for PillLyricState {
    fn default() -> Self {
        Self {
            prev: String::new(),
            cur: String::new(),
            changed_at: Instant::now(),
        }
    }
}

impl PillLyricState {
    pub fn new(now: Instant) -> Self {
        Self {
            prev: String::new(),
            cur: String::new(),
            changed_at: now,
        }
    }

    pub fn set(&mut self, text: &str, now: Instant) {
        if text != self.cur {
            self.prev = std::mem::take(&mut self.cur);
            self.cur = text.to_owned();
            self.changed_at = now;
        }
    }

    pub fn t(&self, now: Instant) -> f32 {
        (now.saturating_duration_since(self.changed_at).as_secs_f32() / 0.25).clamp(0.0, 1.0)
    }
}

pub fn lerp_color(c1: Color32, c2: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let r = (c1.r() as f32 + (c2.r() as f32 - c1.r() as f32) * t) as u8;
    let g = (c1.g() as f32 + (c2.g() as f32 - c1.g() as f32) * t) as u8;
    let b = (c1.b() as f32 + (c2.b() as f32 - c1.b() as f32) * t) as u8;
    let a = (c1.a() as f32 + (c2.a() as f32 - c1.a() as f32) * t) as u8;
    Color32::from_rgba_premultiplied(r, g, b, a)
}

pub fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

pub fn calc_text_width(ctx: &egui::Context, text: &str, font: &FontId) -> f32 {
    ctx.fonts(|f| f.layout_no_wrap(text.to_owned(), font.clone(), Color32::WHITE).size().x)
}

pub fn fit_text(ctx: &egui::Context, text: &str, font: &FontId, max_w: f32) -> String {
    if calc_text_width(ctx, text, font) <= max_w {
        return text.to_owned();
    }
    let mut s = text.to_owned();
    while !s.is_empty() {
        let candidate = format!("{s}…");
        if calc_text_width(ctx, &candidate, font) <= max_w {
            return candidate;
        }
        s.pop();
    }
    "…".to_string()
}

pub fn pill_target_width(ctx: &egui::Context, text: &str) -> f32 {
    let font = FontId::proportional(12.5);
    let tw = calc_text_width(ctx, text, &font);
    // Art mini (20px + margin) + visualizer (18px + margin) + padding = ~76px
    (tw + 76.0).clamp(140.0, 420.0)
}

pub struct NotchApp {
    pub shared_media: SharedMedia,
    pub shared_lyrics: SharedLyrics,
    pub lyrics_cache: Arc<Mutex<HashMap<String, Vec<LyricLine>>>>,
    pub shared_interact: SharedInteract,
    pub mode: DisplayMode,
    pub is_expanded: bool,
    pub show_lyrics: bool,
    pub demo_mode: bool,
    pub demo_track_idx: usize,
    pub geometry: ScreenGeometry,
    pub last_track_title: String,
    pub visualizer_phase: f32,
    pub hover_exit_instant: Option<Instant>,
    /// Latch Ciutkan: setelah Ciutkan diklik, hover di atas bar/pill
    /// TIDAK boleh langsung mengembang lagi sampai kursor pergi
    /// (sebelumnya Tutup kelihatan mati karena hover tiap frame
    /// langsung mengembang lagi).
    pub collapse_latched: bool,
    /// Tinggi konten terukur frame sebelumnya untuk Todo / Shelf
    /// (0 = belum terukur -> pakai default). Dipakai update() agar
    /// tinggi panel mengikuti konten, bukan dipaksa 300 px.
    pub measured_todo_h: f32,
    pub measured_shelf_h: f32,
    pub last_applied_size: (f32, f32),
    pub last_applied_pos: (f32, f32),
    pub view: PanelView,
    pub todo: TodoState,
    pub todo_input: String,
    pub pomodoro: PomodoroTimer,
    pub focus_mins: u64,
    pub break_mins: u64,
    pub water: WaterState,
    pub water_next_due: Instant,
    pub water_mins: u64,
    pub shelf: ShelfState,
    pub volume: f32,
    pub volume_handle: VolumeHandle,
    pub shared_volume: Arc<Mutex<(f32, bool)>>,
    pub volume_muted: bool,
    pub tray: Option<TrayHandles>,
    pub art: Option<egui::TextureHandle>,
    pub current_art_key: String,
    pub enable_fillet: bool,
    pub volume_hud_until: Option<Instant>,
    pub shuffle: bool,
    pub favorite: bool,
    pub lyric_sync: LyricSync,
    pub last_media_seq: u64,
    pub enable_media_diag: bool,
    pub lyric_offset_hud_until: Option<Instant>,
    pub song_offsets: std::collections::HashMap<String, i64>,
    pub default_lyric_offset_ms: i64,
    pub pill_width: f32,
    pub playing_factor: f32,
    pub accent: String,
    pub accent_progress: bool,
    pub collapse_guard: CollapseGuard,
    pub pill_lyric_state: PillLyricState,
}

impl NotchApp {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        shared_media: SharedMedia,
        shared_lyrics: SharedLyrics,
        lyrics_cache: Arc<Mutex<HashMap<String, Vec<LyricLine>>>>,
        config: AppConfig,
        shared_interact: SharedInteract,
        todo_state: TodoState,
        water_state: WaterState,
        shelf_state: ShelfState,
    ) -> Self {
        let initial_vol = get_system_volume().unwrap_or(0.5);
        let shared_volume = Arc::new(Mutex::new((initial_vol, false)));
        let sv_clone = Arc::clone(&shared_volume);
        let volume_handle = spawn_system_volume_worker(move |level, muted| {
            if let Ok(mut lock) = sv_clone.lock() {
                *lock = (level, muted);
            }
        });

        let mut lyric_sync = LyricSync::new(Instant::now());
        lyric_sync.offset_ms = config.lyric_offset_ms;

        Self {
            shared_media,
            shared_lyrics,
            lyrics_cache,
            shared_interact,
            mode: config.mode,
            is_expanded: false,
            show_lyrics: false,
            demo_mode: false,
            demo_track_idx: 0,
            geometry: ScreenGeometry::current(),
            last_track_title: String::new(),
            visualizer_phase: 0.0,
            hover_exit_instant: None,
            collapse_latched: false,
            measured_todo_h: 0.0,
            measured_shelf_h: 0.0,
            last_applied_size: (-1.0, -1.0),
            last_applied_pos: (-999.0, -999.0),
            view: PanelView::Music,
            todo: todo_state,
            todo_input: String::new(),
            pomodoro: PomodoroTimer::new(config.focus_mins),
            focus_mins: config.focus_mins,
            break_mins: config.break_mins,
            water: water_state,
            water_next_due: Instant::now() + Duration::from_secs(config.water_mins * 60),
            water_mins: config.water_mins,
            shelf: shelf_state,
            volume: initial_vol,
            volume_handle,
            shared_volume,
            volume_muted: false,
            // Best-effort: a failed tray build just means no tray icon.
            tray: {
                show_version_notification();
                build_tray(is_autostart_enabled())
            },
            art: None,
            current_art_key: String::new(),
            enable_fillet: config.enable_fillet,
            volume_hud_until: None,
            shuffle: false,
            favorite: false,
            lyric_sync,
            last_media_seq: 0,
            enable_media_diag: config.enable_media_diag,
            lyric_offset_hud_until: None,
            song_offsets: config.song_offsets,
            default_lyric_offset_ms: config.lyric_offset_ms,
            pill_width: 140.0,
            playing_factor: 0.0,
            accent: config.accent,
            accent_progress: config.accent_progress,
            collapse_guard: CollapseGuard::new(Instant::now()),
            pill_lyric_state: PillLyricState::new(Instant::now()),
        }
    }

    pub fn accent_color(&self) -> Color32 {
        crate::config::parse_hex_color(&self.accent)
            .unwrap_or_else(|| Color32::from_rgb(0x1e, 0xd7, 0x60))
    }

    /// Persist the current display preferences to disk (best-effort,
    /// never fails the UI). Call after any persisted toggle changes.
    fn save_config(&self) {
        AppConfig {
            mode: self.mode,
            show_lyrics: self.show_lyrics,
            focus_mins: self.focus_mins,
            break_mins: self.break_mins,
            water_mins: self.water_mins,
            enable_fillet: self.enable_fillet,
            lyric_offset_ms: self.default_lyric_offset_ms,
            enable_media_diag: self.enable_media_diag,
            song_offsets: self.song_offsets.clone(),
            accent: self.accent.clone(),
            accent_progress: self.accent_progress,
        }
        .save();
    }

    /// True when the water reminder interval has elapsed.
    fn water_due(&self) -> bool {
        Instant::now() >= self.water_next_due
    }

    fn apply_demo_track(&mut self) {
        let demos = [
            ("Lumen Coast", "Paper Sky", 193_000),
            ("Nadhif Basalamah", "Penjaga Hati", 243_000),
            ("The Weeknd", "Blinding Lights", 200_000),
        ];
        let (artist, title, dur) = demos[self.demo_track_idx % demos.len()];
        let mut track = safe_lock(&self.shared_media);
        track.artist = artist.to_string();
        track.title = title.to_string();
        track.duration_ms = dur;
        track.is_playing = true;
        track.source_app = "Demo Player".to_string();
        track.thumbnail = None;
    }
}

impl eframe::App for NotchApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0] // 100% transparan untuk layered window
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        crate::bump_ui_heartbeat();

        // Tangani sinyal perubahan sistem (DPI/resolusi atau Explorer restart)
        if crate::interact::SYSTEM_CHANGE_SIGNAL.swap(false, std::sync::atomic::Ordering::Relaxed) {
            self.geometry = crate::window::ScreenGeometry::current();
        }
        if crate::interact::TASKBAR_RECREATED_SIGNAL.swap(false, std::sync::atomic::Ordering::Relaxed) {
            self.tray = crate::tray::build_tray(crate::autostart::is_autostart_enabled());
        }

        // Minta repaint berkala jika ada animasi atau musik berputar
        ctx.request_repaint_after(std::time::Duration::from_millis(30));

        // Sinkronisasi volume dari worker thread jika sedang tidak ada interaksi scroll aktif
        if self.volume_hud_until.is_none() {
            if let Ok(lock) = self.shared_volume.try_lock() {
                self.volume = lock.0;
                self.volume_muted = lock.1;
            }
        }

        let current_track = {
            let t = safe_lock(&self.shared_media);
            t.clone()
        };

        // Cache / update cover art TextureHandle
        let song_key = format!("{} - {}", current_track.artist.trim(), current_track.title.trim());
        if self.current_art_key != song_key {
            self.current_art_key = song_key.clone();
            // Buang tekstur lama saat lagu berganti
            self.art = None;
            if let Some(&saved_off) = self.song_offsets.get(&song_key) {
                self.lyric_sync.offset_ms = saved_off;
            } else {
                self.lyric_sync.offset_ms = self.default_lyric_offset_ms;
            }
            if let Some(color_image) = &current_track.thumbnail {
                self.art = Some(ctx.load_texture(
                    "cover_art",
                    (**color_image).clone(),
                    egui::TextureOptions::LINEAR,
                ));
            }
        } else if self.art.is_none() {
            if let Some(color_image) = &current_track.thumbnail {
                self.art = Some(ctx.load_texture(
                    "cover_art",
                    (**color_image).clone(),
                    egui::TextureOptions::LINEAR,
                ));
            }
        }

        let now = Instant::now();

        // Otomatis fetch lirik bila lagu berubah
        if !current_track.title.is_empty()
            && current_track.title != self.last_track_title
            && current_track.title != "Tidak ada media aktif"
        {
            self.last_track_title = current_track.title.clone();
            self.lyric_sync.reset(now);
            fetch_lyrics_async(
                Arc::clone(&self.shared_lyrics),
                Arc::clone(&self.lyrics_cache),
                current_track.artist.clone(),
                current_track.title.clone(),
                current_track.duration_ms / 1000,
            );
        }

        // Timeline sync & seek detection dari GSMTC poller
        if current_track.update_seq != self.last_media_seq {
            self.last_media_seq = current_track.update_seq;
            let reported_secs = current_track.timeline_pos_secs;
            let age = Duration::from_secs_f64(current_track.timeline_age_secs.max(0.0));
            let playing = current_track.is_playing;
            let rate = current_track.playback_rate;

            let (jumped, drift) = self.lyric_sync.on_timeline(reported_secs, age, playing, rate, now);
            if self.enable_media_diag {
                let source = if current_track.app_id.to_lowercase().contains("spotify") {
                    "Spotify"
                } else if current_track.app_id.to_lowercase().contains("chrome")
                    || current_track.app_id.to_lowercase().contains("msedge")
                    || current_track.app_id.to_lowercase().contains("firefox")
                    || current_track.app_id.to_lowercase().contains("brave")
                {
                    "Browser"
                } else if current_track.app_id.is_empty() {
                    "None/Demo"
                } else {
                    &current_track.app_id
                };
                eprintln!(
                    "[MediaDiag] source: {}, drift: {:.3}s, seek_detected: {}, total_seeks: {}",
                    source, drift, jumped, self.lyric_sync.seek_count
                );
            }
        } else if self.demo_mode && current_track.is_playing {
            // Dalam mode demo, update_seq disimulasikan dari position_ms
            let demo_secs = current_track.position_ms as f64 / 1000.0;
            self.lyric_sync.on_timeline(demo_secs, Duration::ZERO, true, 1.0, now);
        }

        // Pintasan offset lirik +/- 100 ms (hanya aktif jika user tidak mengetik teks)
        if !ctx.wants_keyboard_input() {
            let minus_pressed = ctx.input(|i| {
                i.key_pressed(egui::Key::OpenBracket)
                    || i.key_pressed(egui::Key::Minus)
                    || (i.modifiers.alt && i.key_pressed(egui::Key::ArrowLeft))
            });
            let plus_pressed = ctx.input(|i| {
                i.key_pressed(egui::Key::CloseBracket)
                    || i.key_pressed(egui::Key::Plus)
                    || i.key_pressed(egui::Key::Equals)
                    || (i.modifiers.alt && i.key_pressed(egui::Key::ArrowRight))
            });
            if minus_pressed {
                self.lyric_sync.offset_ms -= 100;
                self.lyric_offset_hud_until = Some(now + Duration::from_millis(1800));
                let song_id = format!("{} - {}", current_track.artist.trim(), current_track.title.trim());
                if !song_id.trim().is_empty() && song_id != " - " {
                    self.song_offsets.insert(song_id, self.lyric_sync.offset_ms);
                }
                self.save_config();
                ctx.request_repaint();
            } else if plus_pressed {
                self.lyric_sync.offset_ms += 100;
                self.lyric_offset_hud_until = Some(now + Duration::from_millis(1800));
                let song_id = format!("{} - {}", current_track.artist.trim(), current_track.title.trim());
                if !song_id.trim().is_empty() && song_id != " - " {
                    self.song_offsets.insert(song_id, self.lyric_sync.offset_ms);
                }
                self.save_config();
                ctx.request_repaint();
            }
        }

        let lyrics_state = {
            let l = safe_lock(&self.shared_lyrics);
            l.clone()
        };

        // Update animasi
        if current_track.is_playing {
            self.visualizer_phase = (self.visualizer_phase + 0.15) % (std::f32::consts::PI * 2.0);
        }

        // Transisi halus playing_factor (~150 ms)
        let dt = ctx.input(|i| i.unstable_dt).clamp(0.0, 0.1);
        let target_pf = if current_track.is_playing && current_track.has_media() { 1.0 } else { 0.0 };
        let pf_speed = dt / 0.150;
        if (self.playing_factor - target_pf).abs() > 0.001 {
            if self.playing_factor < target_pf {
                self.playing_factor = (self.playing_factor + pf_speed).min(target_pf);
            } else {
                self.playing_factor = (self.playing_factor - pf_speed).max(target_pf);
            }
            ctx.request_repaint();
        }

        let has_media = current_track.has_media();

        // Cari baris lirik aktif untuk pill
        let active_lyric_str: Option<String> = if self.show_lyrics && !lyrics_state.lines.is_empty() {
            let pos_secs = self.lyric_sync.position_secs(now);
            line_index(&lyrics_state.lines, pos_secs).map(|idx| {
                let txt = lyrics_state.lines[idx].text.trim();
                if txt.is_empty() {
                    "♪ ♪ ♪".to_string()
                } else {
                    txt.to_string()
                }
            })
        } else {
            None
        };

        let content_kind = select_pill_content(
            has_media,
            self.show_lyrics,
            active_lyric_str.is_some(),
        );

        if let Some(ref text) = active_lyric_str {
            self.pill_lyric_state.set(text, now);
        }

        let target_pill_w = match content_kind {
            PillContentKind::Productivity => 140.0,
            PillContentKind::Title => pill_target_width(ctx, &current_track.title),
            PillContentKind::Lyric => {
                let cur_text = active_lyric_str.as_deref().unwrap_or(self.pill_lyric_state.cur.as_str());
                pill_target_width(ctx, cur_text)
            }
        };

        if (self.pill_width - target_pill_w).abs() > 0.5 {
            self.pill_width += (target_pill_w - self.pill_width) * 0.35;
            ctx.request_repaint();
        } else {
            self.pill_width = target_pill_w;
        }

        // Pomodoro countdown: advances on the 30 ms repaint above.
        // A finished Focus phase counts as one session today.
        if let Some(finished) = self.pomodoro.tick(self.focus_mins, self.break_mins) {
            if finished == PomodoroPhase::Focus {
                self.todo.sessions_today += 1;
                self.todo.save();
            }
        }

        // File shelf: terima file yang di-drop ke window. Dedupe di
        // ShelfState::add menahan event drop yang berulang.
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        if !dropped.is_empty() {
            let mut changed = false;
            for f in dropped {
                if let Some(path) = f.path {
                    if self.shelf.add(&path) {
                        changed = true;
                    }
                }
            }
            if changed {
                self.shelf.save();
            }
        }

        // Tray menu events (Show / autostart toggle / Quit). Drained
        // every frame so rapid clicks are all handled.
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            if let Some(tray) = &self.tray {
                if event.id == tray.quit_id {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                } else if event.id == tray.version_id {
                    show_version_notification();
                } else if event.id == tray.show_id {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                } else if event.id == tray.autostart_id {
                    let enabled = !is_autostart_enabled();
                    if enabled {
                        enable_autostart();
                    } else {
                        disable_autostart();
                    }
                    tray.autostart_item.set_checked(enabled);
                }
            }
        }

        // Dinamis sesuaikan ukuran viewport window hanya ketika berubah
        let ppp = ctx.pixels_per_point().max(1.0);
        let has_active_lyrics = (self.show_lyrics && self.view == PanelView::Music)
            // Todo/Shelf views need the tall panel too, or their
            // content would clip at the 195 px short height. (Only
            // while expanded: collapsing keeps the normal bar width.)
            || (self.view != PanelView::Music && self.is_expanded);
        // Nusa parity: no thin line. The notch (bar, or expanded panel)
        // stays visible over maximized windows instead of collapsing
        // to an 8 px line, so lyrics/shelf stay reachable. The
        // geometry thin-line path is kept but never triggered.
        let (x, y, target_w, mut target_h) = self.geometry.calculate_window_rect(
            self.mode,
            self.is_expanded,
            has_active_lyrics,
            current_track.is_playing,
            ppp,
            false,
            Some(self.pill_width),
        );

        // Todo/Shelf: tinggi mengikuti konten terukur di frame
        // sebelumnya (diukur di akhir render view; 0 = belum ada
        // pengukuran -> pakai default dari geometri). Gagal ukur =
        // tetap tinggi default (tidak lebih buruk dari sekarang).
        if self.is_expanded {
            let measured = match self.view {
                PanelView::Todo => self.measured_todo_h,
                PanelView::Shelf => self.measured_shelf_h,
                PanelView::Music => 0.0,
            };
            if measured > 0.0 {
                target_h = (measured + 16.0).clamp(150.0, 300.0);
            }
        }

        // VALIDASI SANITASI: lewati jika nilai tidak valid (NaN, inf, nol, atau negatif)
        let valid_geom = x.is_finite()
            && y.is_finite()
            && target_w.is_finite()
            && target_h.is_finite()
            && target_w >= 48.0
            && target_h >= 8.0;

        if valid_geom {
            let target_w = target_w.max(48.0);
            let target_h = target_h.max(8.0);

            if self.last_applied_size.0 < 0.0 {
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(target_w, target_h)));
                ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(Pos2::new(x, y)));
                self.collapse_guard.on_resize(now);
                self.last_applied_size = (target_w, target_h);
                self.last_applied_pos = (x, y);
            } else {
                let step = |cur: f32, tgt: f32| -> f32 {
                    let next = cur + (tgt - cur) * 0.4;
                    if (tgt - next).abs() < 0.5 {
                        tgt
                    } else {
                        next
                    }
                };
                let new_w = step(self.last_applied_size.0, target_w).max(48.0);
                let new_h = step(self.last_applied_size.1, target_h).max(8.0);
                let new_x = step(self.last_applied_pos.0, x);
                let new_y = step(self.last_applied_pos.1, y);
                if new_w.is_finite() && new_h.is_finite() {
                    if (new_w - self.last_applied_size.0).abs() > 0.01
                        || (new_h - self.last_applied_size.1).abs() > 0.01
                    {
                        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(new_w, new_h)));
                        self.collapse_guard.on_resize(now);
                        self.last_applied_size = (new_w, new_h);
                    }
                }
                if new_x.is_finite() && new_y.is_finite() {
                    if (new_x - self.last_applied_pos.0).abs() > 0.01
                        || (new_y - self.last_applied_pos.1).abs() > 0.01
                    {
                        ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(Pos2::new(new_x, new_y)));
                        self.last_applied_pos = (new_x, new_y);
                    }
                }
            }
        }

        // Publish rect notch (piksel fisik) untuk mouse hook
        let notch_rect = if self.last_applied_size.0 > 0.0 && self.last_applied_size.1 > 0.0 {
            Some(NotchRect {
                x: (self.last_applied_pos.0 * ppp) as i32,
                y: (self.last_applied_pos.1 * ppp) as i32,
                w: (self.last_applied_size.0 * ppp) as i32,
                h: (self.last_applied_size.1 * ppp) as i32,
            })
        } else {
            None
        };

        if let Ok(mut s) = self.shared_interact.try_lock() {
            s.rect = notch_rect;
        }
        crate::interact::update_cached_rect(notch_rect);

        // Frame rendering transparan
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(Color32::TRANSPARENT))
            .show(ctx, |ui| {
                let max_rect = ui.max_rect();
                let is_hovered = ui.rect_contains_pointer(max_rect);
                let mouse_down = ctx.input(|i| i.pointer.any_down());

                let pin_expanded = self.show_lyrics && self.view == PanelView::Music;

                // State machine interaksi hover & expand dengan CollapseGuard
                if !self.is_expanded {
                    if (is_hovered || pin_expanded) && !self.collapse_latched {
                        self.is_expanded = true;
                        self.collapse_guard.reset_hover(now);
                    } else if !is_hovered {
                        // Kursor pergi: lepas latch Ciutkan, hover
                        // berikutnya boleh mengembang lagi.
                        self.collapse_latched = false;
                    }
                } else {
                    if is_hovered || pin_expanded {
                        self.collapse_guard.reset_hover(now);
                    } else if self.collapse_guard.should_collapse(is_hovered, mouse_down, now) {
                        self.is_expanded = false;
                        self.collapse_guard.on_resize(now);
                    }
                }

                // Samakan gaya pill dengan panel: latar #000000 tanpa border,
                // sisi atas rata dengan tepi layar (radius atas 0), hanya sudut bawah membulat.
                let radius = if !self.is_expanded {
                    CornerRadius { nw: 0, ne: 0, sw: 18, se: 18 }
                } else {
                    CornerRadius { nw: 0, ne: 0, sw: 28, se: 28 }
                };

                let bg_color = Color32::from_rgb(0, 0, 0);
                let border_stroke = Stroke::NONE;
                let margin = egui::Margin::ZERO;

                // Kontainer Notch dengan Margin rapi di dalam kanvas
                egui::Frame::NONE
                    .fill(bg_color)
                    .corner_radius(radius)
                    .stroke(border_stroke)
                    .inner_margin(margin)
                    .show(ui, |ui| {
                        if !self.is_expanded {
                            self.render_collapsed_notch(
                                ui,
                                &current_track,
                                &lyrics_state,
                                content_kind,
                                active_lyric_str.as_deref(),
                                now,
                            );
                        } else {
                            match self.view {
                                PanelView::Music => {
                                    self.render_expanded_notch(ui, &current_track, &lyrics_state);
                                }
                                PanelView::Todo => self.render_todo_view(ui, current_track.is_playing),
                                PanelView::Shelf => self.render_shelf_view(ui, current_track.is_playing),
                            }
                        }
                    });
            });
    }
}

impl NotchApp {
    fn render_collapsed_notch(
        &self,
        ui: &mut egui::Ui,
        track: &MediaTrack,
        _lyrics: &LyricsState,
        content_kind: PillContentKind,
        active_lyric: Option<&str>,
        now: Instant,
    ) {
        let body = ui.max_rect();
        let p = ui.painter();
        let cy = body.center().y;
        let is_playing = track.is_playing && track.has_media();
        let accent = self.accent_color();
        let active_accent = lerp_color(
            Color32::from_rgb(185, 185, 185),
            accent,
            self.playing_factor,
        );

        if content_kind == PillContentKind::Productivity {
            // Widget produktivitas HANYA tampil saat tidak ada media
            ui.horizontal_centered(|ui| {
                ui.add_space(8.0);
                if self.water_due() {
                    ui.label(RichText::new("💧").font(FontId::proportional(12.0)));
                    ui.add_space(2.0);
                }
                ui.label(
                    RichText::new("●")
                        .font(FontId::proportional(11.0))
                        .color(Color32::from_rgb(140, 140, 145)),
                );
                ui.add_space(4.0);
                ui.label(
                    RichText::new(Self::now_clock_hhmm())
                        .font(FontId::proportional(12.0))
                        .color(Color32::WHITE)
                        .strong(),
                );
            });
            return;
        }

        // --- Konten Media (playing atau paused dengan sesi aktif) ---
        // 1. Kiri: Mini Art 20 px rounded (atau vektor draw_note)
        let art_rect = Rect::from_center_size(pos2(body.left() + 18.0, cy), vec2(20.0, 20.0));
        let art_alpha = if is_playing { 1.0 } else { 0.60 };
        if let Some(ref t) = self.art {
            draw_art(p, art_rect, 5, Some(t), art_alpha);
        } else {
            p.rect_filled(art_rect, CornerRadius::same(5), Color32::from_rgb(26, 26, 30));
            draw_note(p, art_rect.center(), Color32::from_gray(170).gamma_multiply(art_alpha));
        }

        // 2. Kanan: Visualizer 5 bar
        // Saat paused: bar diam berujung bulat tinggi min 4-5 px
        // Saat playing: animasi dinamis
        let bars_c = pos2(body.right() - 18.0, cy);
        draw_bars(
            p,
            bars_c,
            is_playing,
            self.visualizer_phase,
            active_accent,
            12.0,
        );

        // 3. Tengah: Teks (Lirik atau Judul Lagu)
        let text_rect = Rect::from_min_max(
            pos2(body.left() + 34.0, body.top()),
            pos2(body.right() - 32.0, body.bottom()),
        );
        let tp = p.with_clip_rect(text_rect);
        let font = FontId::proportional(12.5);
        let font_weight_col = if is_playing {
            Color32::WHITE
        } else {
            Color32::WHITE.gamma_multiply(0.60) // Paused: redup ~60%
        };

        let t = ease_out(self.pill_lyric_state.t(now));
        let cx = text_rect.center().x;
        let slide = 6.0;

        match content_kind {
            PillContentKind::Lyric => {
                // Animasi crossfade + geser untuk lirik
                if t < 1.0 && !self.pill_lyric_state.prev.is_empty() {
                    let s_prev = fit_text(ui.ctx(), &self.pill_lyric_state.prev, &font, text_rect.width());
                    let prev_col = font_weight_col.gamma_multiply(1.0 - t);
                    tp.text(
                        pos2(cx, cy - slide * t),
                        Align2::CENTER_CENTER,
                        s_prev,
                        font.clone(),
                        prev_col,
                    );
                }
                let cur_text = active_lyric.unwrap_or(self.pill_lyric_state.cur.as_str());
                let s_cur = fit_text(ui.ctx(), cur_text, &font, text_rect.width());
                let cur_col = font_weight_col.gamma_multiply(t);
                tp.text(
                    pos2(cx, cy + slide * (1.0 - t)),
                    Align2::CENTER_CENTER,
                    s_cur,
                    font,
                    cur_col,
                );
            }
            PillContentKind::Title => {
                let s = fit_text(ui.ctx(), &track.title, &font, text_rect.width());
                tp.text(
                    pos2(cx, cy),
                    Align2::CENTER_CENTER,
                    s,
                    font,
                    font_weight_col,
                );
            }
            PillContentKind::Productivity => {}
        }
    }


}

fn draw_ear(p: &egui::Painter, corner: Pos2, is_left: bool) {
    let r = 12.0;
    let center = if is_left {
        pos2(corner.x - r, corner.y + r)
    } else {
        pos2(corner.x + r, corner.y + r)
    };
    let (a0, a1) = if is_left {
        (-std::f32::consts::FRAC_PI_2, 0.0)
    } else {
        (-std::f32::consts::PI, -std::f32::consts::FRAC_PI_2)
    };
    fill_fan(p, center, corner, r, a0, a1, Color32::from_rgb(0, 0, 0));
}

fn fill_fan(p: &egui::Painter, center: Pos2, corner: Pos2, r: f32, a0: f32, a1: f32, fill: Color32) {
    let steps = 16;
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(corner, fill);
    for i in 0..=steps {
        let frac = i as f32 / steps as f32;
        let ang = a0 + (a1 - a0) * frac;
        let pt = pos2(center.x + ang.cos() * r, center.y + ang.sin() * r);
        mesh.colored_vertex(pt, fill);
    }
    for i in 1..=steps {
        mesh.add_triangle(0, i as u32, (i + 1) as u32);
    }
    p.add(egui::Shape::mesh(mesh));
}

fn draw_art(p: &egui::Painter, r: Rect, rounding: u8, tex: Option<&egui::TextureHandle>, al: f32) {
    if let Some(t) = tex {
        p.image(
            t.id(),
            r,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::WHITE.gamma_multiply(al),
        );
        return;
    }
    p.rect_filled(r, CornerRadius::same(rounding), Color32::from_rgb(26, 26, 30).gamma_multiply(al));
    let c = r.center();
    let s = r.width();
    p.circle_stroke(c, s * 0.36, Stroke::new(s * 0.04, Color32::from_gray(160).gamma_multiply(al)));
    p.circle_stroke(c, s * 0.24, Stroke::new(s * 0.04, Color32::from_gray(180).gamma_multiply(al)));
    p.circle_filled(c, s * 0.10, Color32::from_gray(220).gamma_multiply(al));
}

pub fn draw_note(p: &egui::Painter, c: Pos2, col: Color32) {
    let st = Stroke::new(1.5_f32, col);
    p.circle_filled(c + vec2(-2.5, 4.5), 3.0, col);
    p.line_segment([c + vec2(0.4, 4.0), c + vec2(0.4, -6.5)], st);
    p.line_segment([c + vec2(0.4, -6.5), c + vec2(5.5, -3.5)], st);
}

fn draw_bars(p: &egui::Painter, center: Pos2, playing: bool, t: f32, color: Color32, max_h: f32) {
    let (bw, gap) = (2.0, 2.0);
    let total = 5.0 * bw + 4.0 * gap;
    let speeds = [5.2, 7.1, 4.3, 6.4, 5.8];
    let phase = [0.0, 1.3, 2.1, 0.7, 1.9];
    for i in 0..5 {
        let bh = if playing {
            let k = 0.30 + 0.70 * (t * speeds[i] + phase[i]).sin().abs();
            (max_h * k).max(4.5)
        } else {
            4.5
        };
        let x = center.x - total * 0.5 + i as f32 * (bw + gap) + bw * 0.5;
        p.rect_filled(
            Rect::from_center_size(pos2(x, center.y), vec2(bw, bh)),
            CornerRadius::same((bw * 0.5) as u8),
            color,
        );
    }
}

fn draw_tri(p: &egui::Painter, c: Pos2, r: f32, dir: f32, col: Color32) {
    let a = pos2(c.x + dir * r, c.y);
    let b = pos2(c.x - dir * r * 0.6, c.y - r * 0.9);
    let d = pos2(c.x - dir * r * 0.6, c.y + r * 0.9);
    p.add(egui::Shape::convex_polygon(vec![a, b, d], col, Stroke::NONE));
}

fn draw_skip(p: &egui::Painter, c: Pos2, dir: f32, col: Color32) {
    let r = 8.0;
    let gap = 6.0;
    let c1 = pos2(c.x - dir * gap * 0.5, c.y);
    let c2 = pos2(c.x + dir * gap * 0.5, c.y);
    draw_tri(p, c1, r, dir, col);
    draw_tri(p, c2, r, dir, col);
}

fn draw_shuffle(p: &egui::Painter, c: Pos2, col: Color32) {
    let w = 7.0;
    let h = 4.0;
    let stroke = Stroke::new(1.8_f32, col);
    p.line_segment([pos2(c.x - w, c.y - h), pos2(c.x - 1.0, c.y - h)], stroke);
    p.line_segment([pos2(c.x - 1.0, c.y - h), pos2(c.x + 2.0, c.y + h)], stroke);
    p.line_segment([pos2(c.x + 2.0, c.y + h), pos2(c.x + w, c.y + h)], stroke);

    p.line_segment([pos2(c.x - w, c.y + h), pos2(c.x - 1.0, c.y + h)], stroke);
    p.line_segment([pos2(c.x - 1.0, c.y + h), pos2(c.x + 2.0, c.y - h)], stroke);
    p.line_segment([pos2(c.x + 2.0, c.y - h), pos2(c.x + w, c.y - h)], stroke);

    draw_tri(p, pos2(c.x + w + 1.0, c.y - h), 3.0, 1.0, col);
    draw_tri(p, pos2(c.x + w + 1.0, c.y + h), 3.0, 1.0, col);
}

fn draw_star(p: &egui::Painter, c: Pos2, r: f32, col: Color32, filled: bool) {
    let r_inner = r * 0.42;
    let mut pts = Vec::with_capacity(10);
    for i in 0..10 {
        let rad = if i % 2 == 0 { r } else { r_inner };
        let ang = -std::f32::consts::FRAC_PI_2 + (i as f32) * std::f32::consts::PI / 5.0;
        pts.push(pos2(c.x + ang.cos() * rad, c.y + ang.sin() * rad));
    }
    if filled {
        p.add(egui::Shape::convex_polygon(pts, col, Stroke::NONE));
    } else {
        pts.push(pts[0]);
        p.add(egui::Shape::line(pts, Stroke::new(1.4_f32, col)));
    }
}

fn fmt_time(ms: u64) -> String {
    let s = ms / 1000;
    let m = s / 60;
    let r = s % 60;
    format!("{}:{:02}", m, r)
}

impl NotchApp {
    /// Tanggal/jam lokal ala Nusa ("Tue 3 Nov 2026 19:36") via
    /// Win32 GetLocalTime (tanpa dep baru; wDayOfWeek langsung ada).
    /// Dipakai status row header di expanded penuh (lirik).
    fn now_header_text() -> String {
        use windows::Win32::System::SystemInformation::GetLocalTime;
        let st = unsafe { GetLocalTime() };
        const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
        const MONTHS: [&str; 12] = [
            "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ];
        let day = DAYS.get(st.wDayOfWeek as usize).copied().unwrap_or("");
        let month = MONTHS
            .get(st.wMonth.saturating_sub(1) as usize)
            .copied()
            .unwrap_or("");
        format!(
            "{} {} {} {} {:02}:{:02}",
            day, st.wDay, month, st.wYear, st.wHour, st.wMinute
        )
    }

    /// Jam HH:MM lokal untuk mini pill idle (Nusa: pill hanya jam).
    fn now_clock_hhmm() -> String {
        use windows::Win32::System::SystemInformation::GetLocalTime;
        let st = unsafe { GetLocalTime() };
        format!("{:02}:{:02}", st.wHour, st.wMinute)
    }

    /// Urutan carousel view: Musik -> Todo -> Shelf -> Musik.
    fn prev_view(v: PanelView) -> PanelView {
        match v {
            PanelView::Music => PanelView::Shelf,
            PanelView::Todo => PanelView::Music,
            PanelView::Shelf => PanelView::Todo,
        }
    }

    fn next_view(v: PanelView) -> PanelView {
        match v {
            PanelView::Music => PanelView::Todo,
            PanelView::Todo => PanelView::Shelf,
            PanelView::Shelf => PanelView::Music,
        }
    }

    /// Header bersama semua view expanded, ala Nusa: baris jam
    /// terpusat + baris judul dengan navigasi carousel (panah ‹ ›
    /// dan titik halaman yang bisa diklik). Menggantikan 3 header
    /// lama yang tombolnya beda-beda per view, plus tombol Bar /
    /// Island (nyaris tak mengubah apa-apa: hanya lebar collapsed
    /// 280 vs 300) dan Demo (tak punya tombol off; track palsunya
    /// ditimpa poller tiap 500 ms). Mode tampilan kini hanya bisa
    /// diubah lewat config.json; demo tak lagi bisa diaktifkan
    /// dari UI (kodenya masih ada tapi tak terjangkau).
    fn render_view_header(
        &mut self,
        ui: &mut egui::Ui,
        title: &str,
        badge: Option<&str>,
        is_playing: bool,
    ) {
        // Baris 1: dot status + tanggal/jam, di tengah, SEMUA view
        // (sebelumnya hanya saat lirik aktif).
        ui.vertical_centered(|ui| {
            ui.horizontal(|ui| {
                let dot_color = if is_playing {
                    Color32::from_rgb(52, 199, 89)
                } else {
                    Color32::from_rgb(115, 120, 130)
                };
                ui.label(
                    RichText::new("●")
                        .font(FontId::proportional(10.0))
                        .color(dot_color),
                );
                ui.label(
                    RichText::new(Self::now_header_text())
                        .font(FontId::proportional(11.0))
                        .color(Color32::from_rgb(190, 195, 205)),
                );
            });
        });
        ui.add_space(2.0);

        // Baris 2: [‹] judul (+badge sumber, hanya view Musik)
        // [titik halaman] [›] ... [▲ Ciutkan].
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            if ui
                .button(RichText::new("‹").font(FontId::proportional(14.0)))
                .clicked()
            {
                self.view = Self::prev_view(self.view);
            }
            ui.label(
                RichText::new(title)
                    .font(FontId::proportional(13.0))
                    .color(Color32::WHITE)
                    .strong(),
            );
            if let Some(source) = badge {
                let badge_bg = match source {
                    "Spotify" => Color32::from_rgb(30, 80, 45),
                    "Google Chrome" | "Microsoft Edge" => Color32::from_rgb(35, 60, 95),
                    _ => Color32::from_rgb(45, 48, 56),
                };
                egui::Frame::NONE
                    .fill(badge_bg)
                    .corner_radius(CornerRadius::same(6))
                    .inner_margin(egui::Margin::symmetric(6, 2))
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new(source)
                                .font(FontId::proportional(10.0))
                                .color(Color32::from_rgb(200, 235, 210)),
                        );
                    });
            }
            // Titik halaman carousel (bisa diklik untuk pindah view).
            for v in [PanelView::Music, PanelView::Todo, PanelView::Shelf] {
                let (ch, col) = if v == self.view {
                    ("●", Color32::from_rgb(110, 220, 255))
                } else {
                    ("○", Color32::from_rgb(90, 95, 110))
                };
                if ui
                    .add(
                        egui::Label::new(
                            RichText::new(ch)
                                .font(FontId::proportional(9.0))
                                .color(col),
                        )
                        .sense(Sense::click()),
                    )
                    .clicked()
                {
                    self.view = v;
                }
            }
            if ui
                .button(RichText::new("›").font(FontId::proportional(14.0)))
                .clicked()
            {
                self.view = Self::next_view(self.view);
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(4.0);
                // Ciutkan (bukan keluar; keluar tetap via tray):
                // latch menahan hover agar tidak langsung mengembang
                // lagi selama kursor masih di atas bar/pill.
                if ui
                    .button(RichText::new("▲ Ciutkan").font(FontId::proportional(11.0)).color(Color32::from_rgb(180, 185, 195)))
                    .clicked()
                {
                    self.is_expanded = false;
                    self.show_lyrics = false;
                    self.save_config();
                    self.collapse_latched = true;
                    self.hover_exit_instant = None;
                }
            });
        });
    }

    fn render_expanded_notch(
        &mut self,
        ui: &mut egui::Ui,
        track: &MediaTrack,
        lyrics: &LyricsState,
    ) {
        let (rect, resp) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
        let p = ui.painter_at(rect);
        let body = rect;
        let is_hovered = resp.hovered();

        let has_media = track.has_media();
        let al = if has_media { 1.0 } else { 0.35 };
        let active_accent = lerp_color(
            Color32::from_rgb(185, 185, 185),
            self.accent_color(),
            self.playing_factor,
        );

        // Fillet / telinga cekung 12 px (hanya jika enable_fillet di config)
        if self.enable_fillet {
            draw_ear(&p, body.left_top(), true);
            draw_ear(&p, body.right_top(), false);
        }

        // Volume: scroll mouse di atas notch mengubah volume sistem
        let scroll_y = ui.input(|i| i.raw_scroll_delta.y);
        if is_hovered && scroll_y.abs() > 0.1 {
            let delta = (scroll_y / 120.0).clamp(-0.1, 0.1) * 0.05;
            self.volume = (self.volume + delta).clamp(0.0, 1.0);
            self.volume_handle.send(VolCmd::Set(self.volume));
            self.volume_hud_until = Some(Instant::now() + Duration::from_millis(1200));
        }

        // Indikator volume tipis sesaat (~1.2 detik)
        if let Some(until) = self.volume_hud_until {
            if Instant::now() < until {
                ui.ctx().request_repaint();
                let hud_w = 100.0;
                let hud_h = 3.0;
                let hud_rect = Rect::from_center_size(pos2(body.center().x, body.top() + 6.0), vec2(hud_w, hud_h));
                p.rect_filled(hud_rect, CornerRadius::same(1), Color32::from_rgba_unmultiplied(255, 255, 255, 60));
                let fill_w = hud_w * self.volume;
                if fill_w > 0.0 {
                    let fill_rect = Rect::from_min_size(hud_rect.min, vec2(fill_w, hud_h));
                    p.rect_filled(fill_rect, CornerRadius::same(1), Color32::WHITE);
                }
            } else {
                self.volume_hud_until = None;
            }
        }

        const PAD: f32 = 20.0;

        // Baris 1 (y 16-62), header
        // cover art 46x46, radius 10, x=20
        let art_rect = Rect::from_min_size(pos2(body.left() + PAD, body.top() + 16.0), vec2(46.0, 46.0));
        draw_art(&p, art_rect, 10, if has_media { self.art.as_ref() } else { None }, al);

        // Kanan: visualizer 5 bar
        let bars_c = pos2(body.right() - PAD - 12.0, body.top() + 39.0);
        let t = ui.input(|i| i.time as f32);
        if track.is_playing && has_media {
            ui.ctx().request_repaint();
        }
        draw_bars(&p, bars_c, track.is_playing && has_media, t, active_accent.gamma_multiply(al), 18.0);

        // Di kiri visualizer: pill "LRC" 34x18, radius 9, outline abu; terisi putih dengan teks hitam saat mode lirik aktif
        let lrc_c = pos2(body.right() - PAD - 52.0, body.top() + 39.0);
        let lrc_rect = Rect::from_center_size(lrc_c, vec2(34.0, 18.0));
        let lrc_resp = ui.interact(lrc_rect, ui.id().with("lrc_toggle"), Sense::click());
        if lrc_resp.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        }
        if lrc_resp.clicked() {
            self.show_lyrics = !self.show_lyrics;
            self.save_config();
            if self.show_lyrics {
                self.is_expanded = true;
                self.collapse_latched = false;
                if lyrics.lines.is_empty() {
                    crate::lyrics::fetch_lyrics_async(
                        Arc::clone(&self.shared_lyrics),
                        Arc::clone(&self.lyrics_cache),
                        track.artist.clone(),
                        track.title.clone(),
                        track.duration_ms / 1000,
                    );
                }
            }
        }

        if self.show_lyrics {
            p.rect_filled(lrc_rect, CornerRadius::same(9), Color32::WHITE);
            p.text(lrc_c, Align2::CENTER_CENTER, "LRC", FontId::proportional(9.5), Color32::BLACK);

            // Tombol vektor '-' dan '+' untuk fine-tuning offset lirik saat mode lirik aktif
            let btn_size = vec2(16.0, 18.0);
            let plus_rect = Rect::from_center_size(pos2(lrc_c.x - 26.0, lrc_c.y), btn_size);
            let minus_rect = Rect::from_center_size(pos2(plus_rect.center().x - 18.0, lrc_c.y), btn_size);

            let plus_resp = ui.interact(plus_rect, ui.id().with("btn_offset_plus"), Sense::click());
            let minus_resp = ui.interact(minus_rect, ui.id().with("btn_offset_minus"), Sense::click());

            if plus_resp.hovered() || minus_resp.hovered() {
                ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
            }

            let song_id = format!("{} - {}", track.artist.trim(), track.title.trim());
            if plus_resp.clicked() {
                self.lyric_sync.offset_ms += 100;
                self.lyric_offset_hud_until = Some(Instant::now() + Duration::from_millis(1800));
                if !song_id.trim().is_empty() && song_id != " - " {
                    self.song_offsets.insert(song_id.clone(), self.lyric_sync.offset_ms);
                }
                self.save_config();
            }
            if minus_resp.clicked() {
                self.lyric_sync.offset_ms -= 100;
                self.lyric_offset_hud_until = Some(Instant::now() + Duration::from_millis(1800));
                if !song_id.trim().is_empty() && song_id != " - " {
                    self.song_offsets.insert(song_id, self.lyric_sync.offset_ms);
                }
                self.save_config();
            }

            // Glyph vektor '-'
            let m_col = if minus_resp.hovered() { Color32::WHITE } else { Color32::from_rgb(140, 140, 145) };
            let mc = minus_rect.center();
            p.line_segment([pos2(mc.x - 4.5, mc.y), pos2(mc.x + 4.5, mc.y)], Stroke::new(1.4_f32, m_col));

            // Glyph vektor '+'
            let p_col = if plus_resp.hovered() { Color32::WHITE } else { Color32::from_rgb(140, 140, 145) };
            let pc = plus_rect.center();
            p.line_segment([pos2(pc.x - 4.5, pc.y), pos2(pc.x + 4.5, pc.y)], Stroke::new(1.4_f32, p_col));
            p.line_segment([pos2(pc.x, pc.y - 4.5), pos2(pc.x, pc.y + 4.5)], Stroke::new(1.4_f32, p_col));
        } else {
            let outline_col = if lrc_resp.hovered() {
                Color32::WHITE
            } else {
                Color32::from_rgb(150, 150, 153)
            };
            p.rect_stroke(lrc_rect, CornerRadius::same(9), Stroke::new(1.0_f32, outline_col.gamma_multiply(al)), egui::StrokeKind::Inside);
            p.text(lrc_c, Align2::CENTER_CENTER, "LRC", FontId::proportional(9.5), outline_col.gamma_multiply(al));
        }

        // Judul: 15 px putih, di x = art.right+12, tengah vertikal y~31, dipotong (clip) jika panjang
        // Artis: 12 px abu #969699, y~49
        let tx = art_rect.right() + 12.0;
        let title_y = body.top() + 31.0;
        let artist_y = body.top() + 49.0;
        let max_tx = if self.show_lyrics { lrc_rect.left() - 48.0 } else { lrc_rect.left() - 10.0 };
        let text_clip = Rect::from_min_max(pos2(tx, body.top() + 12.0), pos2(max_tx, body.top() + 66.0));
        let p_text = p.with_clip_rect(text_clip);

        let title_str = if has_media {
            track.title.as_str()
        } else {
            "Tidak ada media aktif"
        };
        let artist_str = if has_media {
            track.artist.as_str()
        } else {
            "Putar lagu di Spotify / Browser"
        };

        p_text.text(
            pos2(tx, title_y),
            Align2::LEFT_CENTER,
            title_str,
            FontId::proportional(15.0),
            Color32::WHITE.gamma_multiply(al),
        );
        p_text.text(
            pos2(tx, artist_y),
            Align2::LEFT_CENTER,
            artist_str,
            FontId::proportional(12.0),
            Color32::from_rgb(150, 150, 153).gamma_multiply(al),
        );

        // Halaman Lirik: di antara header dan progress
        const LYRIC_SHIFT: f32 = 76.0;
        if self.show_lyrics {
            let lyric_zone = Rect::from_min_max(
                pos2(body.left() + PAD, body.top() + 66.0),
                pos2(body.right() - PAD, body.top() + 66.0 + LYRIC_SHIFT + 8.0),
            );
            let p_lyric = p.with_clip_rect(lyric_zone);

            if lyrics.is_loading {
                p_lyric.text(
                    lyric_zone.center(),
                    Align2::CENTER_CENTER,
                    "Memuat lirik dari LRCLIB...",
                    FontId::proportional(13.0),
                    Color32::from_rgb(110, 220, 255),
                );
            } else if lyrics.lines.is_empty() {
                p_lyric.text(
                    lyric_zone.center(),
                    Align2::CENTER_CENTER,
                    "Lirik tidak tersedia",
                    FontId::proportional(13.0),
                    Color32::from_rgb(150, 150, 153),
                );
            } else {
                let pos_secs = self.lyric_sync.position_secs(Instant::now());
                let cur_idx = line_index(&lyrics.lines, pos_secs).unwrap_or(0);
                let smooth = ui.ctx().animate_value_with_time(Id::new("lyric_scroll"), cur_idx as f32, 0.35);
                let spacing = 26.0;
                let cy = lyric_zone.center().y;
                let lo = (smooth - 2.5).floor().max(0.0) as usize;
                let hi = ((smooth + 2.5).ceil() as usize).min(lyrics.lines.len() - 1);

                for i in lo..=hi {
                    let line = &lyrics.lines[i];
                    let off = i as f32 - smooth;
                    let d = off.abs();
                    let y = cy + off * spacing;
                    let size = 16.0 - (d.min(1.0) * 3.5);
                    let line_al = (1.0 - d * 0.42).clamp(0.0, 1.0);
                    let col = if d < 0.5 {
                        Color32::WHITE
                    } else {
                        Color32::from_rgb(150, 150, 155)
                    };
                    let text = if line.text.trim().is_empty() {
                        "♪ ♪ ♪"
                    } else {
                        line.text.as_str()
                    };

                    let line_rect = Rect::from_center_size(
                        pos2(lyric_zone.center().x, y),
                        vec2(lyric_zone.width(), spacing),
                    );
                    let lr = ui.interact(line_rect, ui.id().with(("lyric_line", i)), Sense::click());
                    if lr.hovered() {
                        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                    }
                    if lr.clicked() {
                        let t_ms = line.time_ms;
                        safe_lock(&self.shared_media).position_ms = t_ms;
                        if !self.demo_mode {
                            seek_media_position(t_ms);
                        }
                        self.lyric_sync.on_timeline(
                            t_ms as f64 / 1000.0,
                            Duration::ZERO,
                            track.is_playing,
                            track.playback_rate,
                            Instant::now(),
                        );
                    }

                    p_lyric.text(
                        pos2(lyric_zone.center().x, y),
                        Align2::CENTER_CENTER,
                        text,
                        FontId::proportional(size),
                        col.gamma_multiply(line_al),
                    );
                }
            }
        }

        // Transient HUD untuk Offset Lirik (+/-100 ms)
        if let Some(until) = self.lyric_offset_hud_until {
            if Instant::now() < until {
                let hud_y = body.top() + if self.show_lyrics { 70.0 } else { 45.0 };
                let hud_rect = Rect::from_center_size(
                    pos2(body.center().x, hud_y),
                    vec2(160.0, 24.0),
                );
                p.rect_filled(hud_rect, CornerRadius::same(6), Color32::from_black_alpha(220));
                p.rect_stroke(
                    hud_rect,
                    CornerRadius::same(6),
                    Stroke::new(1.0_f32, Color32::from_white_alpha(60)),
                    egui::StrokeKind::Inside,
                );
                let sign = if self.lyric_sync.offset_ms > 0 { "+" } else { "" };
                p.text(
                    hud_rect.center(),
                    Align2::CENTER_CENTER,
                    format!("Offset: {}{} ms", sign, self.lyric_sync.offset_ms),
                    FontId::proportional(11.0),
                    Color32::WHITE,
                );
            }
        }

        // Baris 2 (y~88): progress bar
        let py = body.top() + 88.0 + if self.show_lyrics { LYRIC_SHIFT } else { 0.0 };

        // Waktu berjalan kiri (10.5 px abu)
        let el_str = fmt_time(track.position_ms);
        p.text(
            pos2(body.left() + PAD, py),
            Align2::LEFT_CENTER,
            el_str,
            FontId::proportional(10.5),
            Color32::from_rgb(150, 150, 153).gamma_multiply(al),
        );

        // Sisa waktu kanan berformat "-m:ss"
        let rem_ms = track.duration_ms.saturating_sub(track.position_ms);
        let rem_str = format!("-{}", fmt_time(rem_ms));
        p.text(
            pos2(body.right() - PAD, py),
            Align2::RIGHT_CENTER,
            rem_str,
            FontId::proportional(10.5),
            Color32::from_rgb(150, 150, 153).gamma_multiply(al),
        );

        // Bar tinggi 4 px (track putih 28% alpha, isi putih, membesar sedikit saat hover, bisa klik/drag untuk seek)
        let x1 = body.left() + PAD + 38.0;
        let x2 = body.right() - PAD - 42.0;
        let track_rect = Rect::from_min_max(pos2(x1, py - 2.0), pos2(x2, py + 2.0));
        let interact_rect = track_rect.expand2(vec2(0.0, 8.0));
        let seek_resp = ui.interact(interact_rect, ui.id().with("seek_bar"), Sense::click_and_drag());
        let is_bar_hovered = has_media && (seek_resp.hovered() || seek_resp.dragged());
        if is_bar_hovered {
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        }

        let bar_h = if is_bar_hovered { 6.0 } else { 4.0 };
        let actual_track_rect = Rect::from_center_size(track_rect.center(), vec2(track_rect.width(), bar_h));
        p.rect_filled(
            actual_track_rect,
            CornerRadius::same((bar_h * 0.5) as u8),
            Color32::from_rgba_unmultiplied(255, 255, 255, 71).gamma_multiply(al),
        );

        let prog = if track.duration_ms > 0 {
            (track.position_ms as f32 / track.duration_ms as f32).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let fill_w = actual_track_rect.width() * prog;
        if fill_w > 0.0 {
            let fill_rect = Rect::from_min_size(actual_track_rect.min, vec2(fill_w, bar_h));
            let fill_col = if self.accent_progress {
                active_accent
            } else {
                Color32::WHITE
            };
            p.rect_filled(
                fill_rect,
                CornerRadius::same((bar_h * 0.5) as u8),
                fill_col.gamma_multiply(al),
            );
        }

        if has_media && (seek_resp.clicked() || seek_resp.dragged()) {
            if let Some(pos) = seek_resp.interact_pointer_pos() {
                let frac = ((pos.x - x1) / (x2 - x1)).clamp(0.0, 1.0);
                let seek_ms = (frac * track.duration_ms as f32) as u64;
                safe_lock(&self.shared_media).position_ms = seek_ms;
                if !self.demo_mode {
                    seek_media_position(seek_ms);
                }
                self.lyric_sync.on_timeline(
                    seek_ms as f64 / 1000.0,
                    Duration::ZERO,
                    track.is_playing,
                    track.playback_rate,
                    Instant::now(),
                );
            }
        }

        // Baris 3 (y~118): kontrol, 5 ikon vektor digambar dengan painter pada posisi 10%, 30%, 50%, 70%, 90%
        let cy = body.top() + 118.0 + if self.show_lyrics { LYRIC_SHIFT } else { 0.0 };
        let inner = body.width() - PAD * 2.0;

        let c_shuffle = pos2(body.left() + PAD + inner * 0.10, cy);
        let c_prev = pos2(body.left() + PAD + inner * 0.30, cy);
        let c_play = pos2(body.left() + PAD + inner * 0.50, cy);
        let c_next = pos2(body.left() + PAD + inner * 0.70, cy);
        let c_fav = pos2(body.left() + PAD + inner * 0.90, cy);

        // 1. Shuffle
        let shuf_resp = ui.interact(Rect::from_center_size(c_shuffle, vec2(32.0, 32.0)), ui.id().with("btn_shuf"), Sense::click());
        if has_media && shuf_resp.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        }
        if has_media && shuf_resp.clicked() {
            self.shuffle = !self.shuffle;
        }
        let shuf_col = if self.shuffle {
            self.accent_color()
        } else if has_media && shuf_resp.hovered() {
            Color32::WHITE
        } else {
            active_accent
        };
        draw_shuffle(&p, c_shuffle, shuf_col.gamma_multiply(al));

        // 2. Previous (dua segitiga)
        let prev_resp = ui.interact(Rect::from_center_size(c_prev, vec2(32.0, 32.0)), ui.id().with("btn_prev"), Sense::click());
        if has_media && prev_resp.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        }
        if has_media && prev_resp.clicked() {
            if self.demo_mode {
                self.demo_track_idx = self.demo_track_idx.saturating_sub(1);
                self.apply_demo_track();
            } else {
                send_media_command("prev");
            }
        }
        let prev_col = if has_media && prev_resp.hovered() {
            Color32::WHITE
        } else {
            active_accent
        };
        draw_skip(&p, c_prev, -1.0, prev_col.gamma_multiply(al));

        // 3. Play/Pause (pause = dua batang 6x22)
        let play_resp = ui.interact(Rect::from_center_size(c_play, vec2(36.0, 36.0)), ui.id().with("btn_play"), Sense::click());
        if play_resp.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        }
        if play_resp.clicked() {
            if self.demo_mode {
                let mut t = safe_lock(&self.shared_media);
                t.is_playing = !t.is_playing;
            } else {
                send_media_command("toggle");
            }
        }
        let play_col = if play_resp.hovered() {
            Color32::WHITE
        } else {
            active_accent
        };
        if track.is_playing && has_media {
            for dx in [-6.0, 6.0] {
                p.rect_filled(
                    Rect::from_center_size(c_play + vec2(dx, 0.0), vec2(6.0, 22.0)),
                    CornerRadius::same(2),
                    play_col.gamma_multiply(al),
                );
            }
        } else {
            draw_tri(&p, c_play + vec2(2.0, 0.0), 12.0, 1.0, play_col.gamma_multiply(al));
        }

        // 4. Next (dua segitiga)
        let next_resp = ui.interact(Rect::from_center_size(c_next, vec2(32.0, 32.0)), ui.id().with("btn_next"), Sense::click());
        if has_media && next_resp.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        }
        if has_media && next_resp.clicked() {
            if self.demo_mode {
                self.demo_track_idx += 1;
                self.apply_demo_track();
            } else {
                send_media_command("next");
            }
        }
        let next_col = if has_media && next_resp.hovered() {
            Color32::WHITE
        } else {
            active_accent
        };
        draw_skip(&p, c_next, 1.0, next_col.gamma_multiply(al));

        // 5. Favorit (bintang, terisi putih saat aktif)
        let fav_resp = ui.interact(Rect::from_center_size(c_fav, vec2(32.0, 32.0)), ui.id().with("btn_fav"), Sense::click());
        if has_media && fav_resp.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        }
        if has_media && fav_resp.clicked() {
            self.favorite = !self.favorite;
        }
        let fav_col = if self.favorite {
            self.accent_color()
        } else if has_media && fav_resp.hovered() {
            Color32::WHITE
        } else {
            active_accent
        };
        draw_star(&p, c_fav, 8.5, fav_col.gamma_multiply(al), self.favorite);

        // Elemen sekunder (hanya saat hover)
        if is_hovered {
            // Panah carousel ‹ › di tepi kiri/kanan tengah vertikal (opacity 40%)
            let left_c = pos2(body.left() + 10.0, body.center().y);
            let r_left = ui.interact(Rect::from_center_size(left_c, vec2(20.0, 36.0)), ui.id().with("car_prev"), Sense::click());
            if r_left.hovered() {
                ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
            }
            if r_left.clicked() {
                self.view = Self::prev_view(self.view);
            }
            let left_al = if r_left.hovered() { 0.9 } else { 0.4 };
            p.text(left_c, Align2::CENTER_CENTER, "‹", FontId::proportional(20.0), Color32::WHITE.gamma_multiply(left_al));

            let right_c = pos2(body.right() - 10.0, body.center().y);
            let r_right = ui.interact(Rect::from_center_size(right_c, vec2(20.0, 36.0)), ui.id().with("car_next"), Sense::click());
            if r_right.hovered() {
                ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
            }
            if r_right.clicked() {
                self.view = Self::next_view(self.view);
            }
            let right_al = if r_right.hovered() { 0.9 } else { 0.4 };
            p.text(right_c, Align2::CENTER_CENTER, "›", FontId::proportional(20.0), Color32::WHITE.gamma_multiply(right_al));

            // Titik halaman kecil di bawah tengah
            let dot_y = body.bottom() - 12.0;
            let views = [PanelView::Music, PanelView::Todo, PanelView::Shelf];
            for (i, v) in views.iter().enumerate() {
                let dot_x = body.center().x + (i as f32 - 1.0) * 14.0;
                let dot_c = pos2(dot_x, dot_y);
                let r_dot = ui.interact(Rect::from_center_size(dot_c, vec2(12.0, 12.0)), ui.id().with(("pdot", i)), Sense::click());
                if r_dot.hovered() {
                    ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                }
                if r_dot.clicked() {
                    self.view = *v;
                }
                if self.view == *v {
                    p.circle_filled(dot_c, 2.5, Color32::WHITE);
                } else {
                    p.circle_filled(dot_c, 2.0, Color32::from_rgba_unmultiplied(255, 255, 255, 75));
                }
            }
        }

        resp.context_menu(|ui| {
            if ui.button("Keluar").clicked() {
                std::process::exit(0);
            }
        });
    }


    /// Todo list + Pomodoro view, replacing the music content of the
    /// expanded panel while music keeps playing in the background.
    /// v1 scope: visual timer only, no sound; timer state is not
    /// persisted across restarts (only the list + session count are).
    fn render_todo_view(&mut self, ui: &mut egui::Ui, is_playing: bool) {
        ui.vertical(|ui| {
            ui.add_space(8.0);

            // Header bersama (jam terpusat + carousel + Ciutkan).
            self.render_view_header(ui, "✅ Todo & Pomodoro", None, is_playing);

            ui.add_space(8.0);

            // Pomodoro block: phase + countdown + controls
            let (phase_text, phase_color) = match self.pomodoro.phase {
                PomodoroPhase::Focus => ("🎯 Fokus", Color32::from_rgb(52, 199, 89)),
                PomodoroPhase::Break => ("☕ Istirahat", Color32::from_rgb(100, 210, 255)),
            };
            ui.horizontal(|ui| {
                ui.add_space(14.0);
                ui.label(
                    RichText::new(phase_text)
                        .font(FontId::proportional(13.0))
                        .color(phase_color)
                        .strong(),
                );
                let secs = self.pomodoro.remaining_secs();
                ui.label(
                    RichText::new(format!("{:02}:{:02}", secs / 60, secs % 60))
                        .font(FontId::monospace(20.0))
                        .color(Color32::WHITE)
                        .strong(),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add_space(14.0);
                    if ui.button(RichText::new("↺ Reset").font(FontId::proportional(11.0))).clicked() {
                        self.pomodoro.reset(self.focus_mins, self.break_mins);
                    }
                    let start_label = if self.pomodoro.running { "⏸ Jeda" } else { "▶ Mulai" };
                    if ui.button(RichText::new(start_label).font(FontId::proportional(11.0))).clicked() {
                        if self.pomodoro.running {
                            self.pomodoro.pause();
                        } else {
                            self.pomodoro.start();
                        }
                    }
                });
            });
            ui.horizontal(|ui| {
                ui.add_space(14.0);
                ui.label(
                    RichText::new(format!(
                        "Sesi selesai hari ini: {}  •  Fokus {} mnt / Istirahat {} mnt",
                        self.todo.sessions_today, self.focus_mins, self.break_mins
                    ))
                    .font(FontId::proportional(11.0))
                    .color(Color32::from_rgb(150, 155, 165)),
                );
            });

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(8.0);

            // Add todo input (Enter works like the Tambah button)
            ui.horizontal(|ui| {
                ui.add_space(14.0);
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut self.todo_input)
                        .hint_text("Tulis tugas baru...")
                        .desired_width((ui.available_width() - 90.0).max(60.0)),
                );
                let enter_pressed =
                    resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if (ui.button(RichText::new("＋ Tambah").font(FontId::proportional(11.0))).clicked()
                    || enter_pressed)
                    && !self.todo_input.trim().is_empty()
                {
                    self.todo.items.push(crate::todo::TodoItem {
                        text: self.todo_input.trim().to_string(),
                        done: false,
                    });
                    self.todo_input.clear();
                    self.todo.save();
                }
                ui.add_space(14.0);
            });

            ui.add_space(6.0);

            // Todo list, scrollable so it fits the expanded panel
            ScrollArea::vertical()
                .max_height(120.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    let mut changed = false;
                    let mut remove_idx: Option<usize> = None;
                    for (i, item) in self.todo.items.iter_mut().enumerate() {
                        ui.horizontal(|ui| {
                            ui.add_space(14.0);
                            if ui.checkbox(&mut item.done, "").changed() {
                                changed = true;
                            }
                            let text = if item.done {
                                RichText::new(&item.text)
                                    .font(FontId::proportional(12.0))
                                    .color(Color32::from_rgb(110, 115, 125))
                                    .strikethrough()
                            } else {
                                RichText::new(&item.text)
                                    .font(FontId::proportional(12.0))
                                    .color(Color32::from_rgb(225, 230, 240))
                            };
                            ui.label(text);
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.add_space(14.0);
                                if ui.button(RichText::new("✕").font(FontId::proportional(11.0))).clicked() {
                                    remove_idx = Some(i);
                                }
                            });
                        });
                    }
                    if let Some(i) = remove_idx {
                        self.todo.items.remove(i);
                        changed = true;
                    }
                    if changed {
                        self.todo.save();
                    }
                });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.add_space(14.0);
                let water_text = format!("💧 {} gelas hari ini", self.water.count_today);
                ui.label(
                    RichText::new(water_text)
                        .font(FontId::proportional(11.0))
                        .color(Color32::from_rgb(100, 200, 255)),
                );
                if ui.button(RichText::new("+1 Minum").font(FontId::proportional(11.0))).clicked() {
                    self.water.count_today += 1;
                    self.water.save();
                    self.water_next_due = Instant::now() + Duration::from_secs(self.water_mins * 60);
                }
            });

            // Ukur tinggi konten untuk target geometri frame berikut.
            self.measured_todo_h = ui.min_rect().height();
        });
    }

    /// File shelf view: files dropped onto the notch, click to open
    /// with the default app, ✕ to remove. Drop intake happens in
    /// update() so files can be dropped onto any panel state.
    fn render_shelf_view(&mut self, ui: &mut egui::Ui, is_playing: bool) {
        ui.vertical(|ui| {
            ui.add_space(8.0);

            // Header bersama (jam terpusat + carousel + Ciutkan).
            self.render_view_header(ui, "📁 Shelf", None, is_playing);

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.add_space(14.0);
                ui.label(
                    RichText::new("Drag & drop file ke notch saat mengembang untuk menyimpannya di sini.")
                        .font(FontId::proportional(11.0))
                        .color(Color32::from_rgb(150, 155, 165)),
                );
            });
            ui.add_space(6.0);

            if self.shelf.items.is_empty() {
                ui.horizontal(|ui| {
                    ui.add_space(14.0);
                    ui.label(
                        RichText::new("Shelf masih kosong.")
                            .font(FontId::proportional(12.0))
                            .color(Color32::GRAY),
                    );
                });
            }

            ScrollArea::vertical()
                .max_height(150.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    let mut remove_idx: Option<usize> = None;
                    for (i, item) in self.shelf.items.iter().enumerate() {
                        ui.horizontal(|ui| {
                            ui.add_space(14.0);
                            let resp = ui
                                .add(
                                    egui::Label::new(
                                        RichText::new(&item.name)
                                            .font(FontId::proportional(12.0))
                                            .color(Color32::from_rgb(225, 230, 240)),
                                    )
                                    .sense(Sense::click()),
                                )
                                .on_hover_text(&item.path);
                            if resp.clicked() {
                                open_file(&item.path);
                            }
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.add_space(14.0);
                                if ui.button(RichText::new("✕").font(FontId::proportional(11.0))).clicked() {
                                    remove_idx = Some(i);
                                }
                            });
                        });
                    }
                    if let Some(i) = remove_idx {
                        self.shelf.items.remove(i);
                        self.shelf.save();
                    }
                });
            // Ukur tinggi konten untuk target geometri frame berikut.
            self.measured_shelf_h = ui.min_rect().height();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_select_pill_content_combinations() {
        // 1. Tanpa media (selalu tampilkan widget produktivitas)
        assert_eq!(
            select_pill_content(false, false, false),
            PillContentKind::Productivity
        );
        assert_eq!(
            select_pill_content(false, true, false),
            PillContentKind::Productivity
        );
        assert_eq!(
            select_pill_content(false, true, true),
            PillContentKind::Productivity
        );

        // 2. Ada media + LRC aktif + ada baris lirik aktif -> Lyric
        assert_eq!(
            select_pill_content(true, true, true),
            PillContentKind::Lyric
        );

        // 3. Ada media + LRC aktif + baris lirik belum/tidak ada -> Title
        assert_eq!(
            select_pill_content(true, true, false),
            PillContentKind::Title
        );

        // 4. Ada media + LRC non-aktif -> Title
        assert_eq!(
            select_pill_content(true, false, false),
            PillContentKind::Title
        );
        assert_eq!(
            select_pill_content(true, false, true),
            PillContentKind::Title
        );
    }

    #[test]
    fn test_collapse_guard_unhover_and_resize() {
        let t0 = Instant::now();
        let mut guard = CollapseGuard::new(t0);

        // Hover aktif -> tidak boleh ciut
        assert!(!guard.should_collapse(true, false, t0));

        // Kursor keluar sejenak (< 400 ms) -> hold, tidak boleh ciut
        let t_unhover_early = t0 + Duration::from_millis(150);
        assert!(!guard.should_collapse(false, false, t_unhover_early));

        // Kursor keluar >= 400 ms -> ciut
        let t_unhover_late = t0 + Duration::from_millis(450);
        assert!(guard.should_collapse(false, false, t_unhover_late));

        // Sedang drag/klik mouse -> jangan ciut walaupun unhover >= 400 ms
        assert!(!guard.should_collapse(false, true, t_unhover_late));

        // Resize window baru saja terjadi -> jangan ciut sampai settling time lewat
        guard.on_resize(t_unhover_late);
        assert!(!guard.should_collapse(false, false, t_unhover_late + Duration::from_millis(200)));
        assert!(guard.should_collapse(false, false, t_unhover_late + Duration::from_millis(450)));
    }

    #[test]
    fn test_pill_lyric_state_crossfade() {
        let t0 = Instant::now();
        let mut state = PillLyricState::new(t0);
        assert_eq!(state.cur, "");

        state.set("Line 1", t0);
        assert_eq!(state.cur, "Line 1");
        assert_eq!(state.prev, "");

        let t1 = t0 + Duration::from_secs(5);
        state.set("Line 2", t1);
        assert_eq!(state.cur, "Line 2");
        assert_eq!(state.prev, "Line 1");
        assert_eq!(state.changed_at, t1);

        // Abaikan set teks yang sama persis
        state.set("Line 2", t1 + Duration::from_millis(50));
        assert_eq!(state.changed_at, t1);
    }
}
