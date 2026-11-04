use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use egui::{
    vec2, Align, Color32, CornerRadius, FontId, Layout, Pos2, Rect, RichText, ScrollArea,
    Sense, Stroke,
};

use crate::autostart::{disable_autostart, enable_autostart, is_autostart_enabled};
use crate::config::AppConfig;
use crate::interact::{NotchRect, SharedInteract};
use crate::lyrics::{fetch_lyrics_async, LyricLine, LyricsState, SharedLyrics};
use crate::media::{seek_media_position, send_media_command, MediaTrack, SharedMedia};
use crate::shelf::{open_file, ShelfState};
use crate::todo::{PomodoroPhase, PomodoroTimer, TodoState};
use crate::tray::{build_tray, TrayHandles};
use crate::volume::{get_system_volume, set_system_volume};
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
    pub tray: Option<TrayHandles>,
}

impl NotchApp {
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
        Self {
            shared_media,
            shared_lyrics,
            lyrics_cache,
            shared_interact,
            mode: config.mode,
            is_expanded: false,
            show_lyrics: config.show_lyrics,
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
            volume: get_system_volume().unwrap_or(0.5),
            // Best-effort: a failed tray build just means no tray icon.
            tray: build_tray(is_autostart_enabled()),
        }
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
        let mut track = self.shared_media.lock().unwrap();
        track.artist = artist.to_string();
        track.title = title.to_string();
        track.duration_ms = dur;
        track.is_playing = true;
        track.source_app = "Demo Player".to_string();
    }
}

impl eframe::App for NotchApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0] // 100% transparan untuk layered window
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Minta repaint berkala jika ada animasi atau musik berputar
        ctx.request_repaint_after(std::time::Duration::from_millis(30));

        let current_track = {
            let t = self.shared_media.lock().unwrap();
            t.clone()
        };

        // Otomatis fetch lirik bila lagu berubah
        if !current_track.title.is_empty()
            && current_track.title != self.last_track_title
            && current_track.title != "Tidak ada media aktif"
        {
            self.last_track_title = current_track.title.clone();
            fetch_lyrics_async(
                Arc::clone(&self.shared_lyrics),
                Arc::clone(&self.lyrics_cache),
                current_track.artist.clone(),
                current_track.title.clone(),
                current_track.duration_ms / 1000,
            );
        }

        let lyrics_state = {
            let l = self.shared_lyrics.lock().unwrap();
            l.clone()
        };

        // Update animasi
        if current_track.is_playing {
            self.visualizer_phase = (self.visualizer_phase + 0.15) % (std::f32::consts::PI * 2.0);
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
        let has_active_lyrics = self.show_lyrics
            || (!lyrics_state.lines.is_empty() && current_track.is_playing)
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

        // Morph animation (ala Nusa): window size/position ease toward
        // the target each frame instead of jumping instantly. Factor
        // 0.4 per frame (~30 ms) converges in ~300 ms; snaps under
        // 0.5 px. First frame snaps (sentinel init) so launch behavior
        // is unchanged; only later resizes animate.
        if self.last_applied_size.0 < 0.0 {
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(target_w, target_h)));
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(Pos2::new(x, y)));
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
            let new_w = step(self.last_applied_size.0, target_w);
            let new_h = step(self.last_applied_size.1, target_h);
            let new_x = step(self.last_applied_pos.0, x);
            let new_y = step(self.last_applied_pos.1, y);
            if (new_w - self.last_applied_size.0).abs() > 0.01
                || (new_h - self.last_applied_size.1).abs() > 0.01
            {
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(new_w, new_h)));
                self.last_applied_size = (new_w, new_h);
            }
            if (new_x - self.last_applied_pos.0).abs() > 0.01
                || (new_y - self.last_applied_pos.1).abs() > 0.01
            {
                ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(Pos2::new(new_x, new_y)));
                self.last_applied_pos = (new_x, new_y);
            }
        }

        // Publish rect notch (piksel fisik) untuk mouse hook
        // click-through: kursor di dalam rect -> interaktif, di luar -> tembus.
        // Rect mengikuti ukuran/posisi YANG DITERAPKAN (bukan target),
        // agar klik tetap akurat selama animasi morph.
        if let Ok(mut s) = self.shared_interact.lock() {
            s.rect = Some(NotchRect {
                x: (self.last_applied_pos.0 * ppp) as i32,
                y: (self.last_applied_pos.1 * ppp) as i32,
                w: (self.last_applied_size.0 * ppp) as i32,
                h: (self.last_applied_size.1 * ppp) as i32,
            });
        }

        // Frame rendering transparan
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(Color32::TRANSPARENT))
            .show(ctx, |ui| {
                let max_rect = ui.max_rect();
                let is_hovered = ui.rect_contains_pointer(max_rect);

                // State machine interaksi hover & expand
                if !self.is_expanded {
                    if is_hovered && !self.collapse_latched {
                        self.is_expanded = true;
                        self.hover_exit_instant = None;
                    } else if !is_hovered {
                        // Kursor pergi: lepas latch Ciutkan, hover
                        // berikutnya boleh mengembang lagi.
                        self.collapse_latched = false;
                    }
                } else {
                    if is_hovered {
                        self.hover_exit_instant = None;
                    } else {
                        match self.hover_exit_instant {
                            None => self.hover_exit_instant = Some(Instant::now()),
                            Some(exit_time) => {
                                // Debounce collapse ala Nusa: 150 ms
                                // (dulu 350 ms) sebelum menciut.
                                if exit_time.elapsed().as_millis() > 150 {
                                    self.is_expanded = false;
                                    self.hover_exit_instant = None;
                                }
                            }
                        }
                    }
                }

                // Radius kontainer notch: satu gaya rounded penuh
                // ala Nusa untuk semua mode (sebelumnya mode Bar
                // memakai sudut atas siku, bikin tampilan tak konsisten).
                let radius = CornerRadius::same(18);

                let bg_color = Color32::from_rgb(10, 12, 16);
                let border_color = if self.is_expanded {
                    Color32::from_rgb(55, 62, 75)
                } else {
                    Color32::from_rgb(30, 34, 44)
                };

                // Kontainer Notch dengan Margin rapi di dalam kanvas
                egui::Frame::NONE
                    .fill(bg_color)
                    .corner_radius(radius)
                    .stroke(Stroke::new(1.0_f32, border_color))
                    .inner_margin(egui::Margin {
                        left: 10,
                        right: 10,
                        top: if !self.is_expanded { 0 } else { 4 },
                        bottom: 4,
                    })
                    .show(ui, |ui| {
                        if !self.is_expanded {
                            self.render_collapsed_notch(ui, &current_track, &lyrics_state);
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
    ) {
        // Tidak ada teks/dot/jam di sini — Frame container sudah
        // mengecat strip 48x8; fungsi ini langsung kembali.
        // Hover di atas slit tetap mengembang seperti biasa.
        if !track.is_playing {
            return;
        }

        // Pill play minimal ala Nusa: dot + jam + mini EQ, TANPA
        // ticker judul/lirik (dihapus sesuai perilaku Nusa; judul
        // dan lirik tetap tampil di panel expanded).
        ui.horizontal_centered(|ui| {
            if self.water_due() {
                ui.label(RichText::new("💧").font(FontId::proportional(12.0)));
                ui.add_space(2.0);
            }
            ui.label(
                RichText::new("●")
                    .font(FontId::proportional(11.0))
                    .color(Color32::from_rgb(52, 199, 89)),
            );
            ui.add_space(4.0);
            ui.label(
                RichText::new(Self::now_clock_hhmm())
                    .font(FontId::proportional(12.0))
                    .color(Color32::WHITE)
                    .strong(),
            );
            ui.add_space(6.0);

            // Mini EQ (kode visualizer lama, dipakai ulang).
            let (response, painter) = ui.allocate_painter(vec2(22.0, 16.0), Sense::hover());
            let bar_count = 4;
            let bar_w = 2.5;
            let spacing = 2.0;
            let total_w = bar_count as f32 * bar_w + (bar_count - 1) as f32 * spacing;
            let start_x = response.rect.center().x - total_w / 2.0;
            let center_y = response.rect.center().y;

            for i in 0..bar_count {
                let bx = start_x + i as f32 * (bar_w + spacing);
                let height = if track.is_playing {
                    4.0 + ((self.visualizer_phase + i as f32 * 1.3).sin().abs() * 10.0)
                } else {
                    3.0
                };
                let bar_rect = Rect::from_center_size(
                    Pos2::new(bx + bar_w / 2.0, center_y),
                    vec2(bar_w, height),
                );
                painter.rect_filled(
                    bar_rect,
                    CornerRadius::same(1),
                    if track.is_playing {
                        Color32::from_rgb(52, 199, 89)
                    } else {
                        Color32::from_rgb(115, 120, 130)
                    },
                );
            }
        });
    }

    /// Kolom musik di panel expanded: judul, progress, kontrol,
    /// volume, water. Dipakai kolom tunggal (lirik mati) maupun
    /// kolom kiri (lirik aktif, side-by-side ala Nusa).
    fn render_music_column(
        &mut self,
        ui: &mut egui::Ui,
        track: &MediaTrack,
        lyrics: &LyricsState,
    ) {
        // Informasi Lagu: Judul & Artist
        ui.horizontal(|ui| {
            ui.add_space(14.0);
            ui.vertical(|ui| {
                let title_text = if track.title.is_empty() {
                    "Tidak Ada Musik Aktif"
                } else {
                    &track.title
                };
                ui.label(
                    RichText::new(title_text)
                        .font(FontId::proportional(15.0))
                        .color(Color32::WHITE)
                        .strong(),
                );
                ui.label(
                    RichText::new(&track.artist)
                        .font(FontId::proportional(12.0))
                        .color(Color32::from_rgb(155, 162, 175)),
                );
            });
        });

        ui.add_space(6.0);

        // Playback Progress Bar & Time (Interactive Seek Bar)
        ui.horizontal(|ui| {
            ui.add_space(14.0);
            let pos_sec = track.position_ms / 1000;
            let dur_sec = track.duration_ms / 1000;

            ui.label(
                RichText::new(format!("{:02}:{:02}", pos_sec / 60, pos_sec % 60))
                    .font(FontId::monospace(10.0))
                    .color(Color32::from_rgb(140, 145, 155)),
            );

            let bar_width = (ui.available_width() - 55.0).max(60.0);
            let (response, painter) = ui.allocate_painter(vec2(bar_width, 14.0), Sense::click_and_drag());
            let rect = response.rect;

            let is_hover = response.hovered() || response.dragged();
            let bar_h = if is_hover { 6.0 } else { 4.0 };
            let bar_rect = Rect::from_center_size(rect.center(), vec2(rect.width(), bar_h));

            let mut current_ratio = if dur_sec > 0 {
                (track.position_ms as f32 / track.duration_ms as f32).clamp(0.0, 1.0)
            } else {
                0.0
            };

            // Tangani interaksi klik / drag untuk seek maju mundur
            if (response.clicked() || response.dragged()) && track.duration_ms > 0 {
                if let Some(pos) = response.interact_pointer_pos() {
                    let new_ratio = ((pos.x - bar_rect.min.x) / bar_rect.width()).clamp(0.0, 1.0);
                    let new_pos_ms = (new_ratio * track.duration_ms as f32) as u64;

                    // Update state optimis instan
                    {
                        let mut t = self.shared_media.lock().unwrap();
                        t.position_ms = new_pos_ms;
                    }
                    if !self.demo_mode {
                        seek_media_position(new_pos_ms);
                    }
                    current_ratio = new_ratio;
                }
            }

            // Gambar background track
            painter.rect_filled(
                bar_rect,
                CornerRadius::same((bar_h / 2.0).round() as u8),
                Color32::from_rgb(45, 50, 62),
            );

            // Gambar active filled track
            let filled_w = bar_rect.width() * current_ratio;
            if filled_w > 0.0 {
                let filled_rect = Rect::from_min_max(
                    bar_rect.min,
                    Pos2::new(bar_rect.min.x + filled_w, bar_rect.max.y),
                );
                let bar_color = if is_hover {
                    Color32::from_rgb(70, 225, 110)
                } else {
                    Color32::from_rgb(52, 199, 89)
                };
                painter.rect_filled(
                    filled_rect,
                    CornerRadius::same((bar_h / 2.0).round() as u8),
                    bar_color,
                );
            }

            // Gambar thumb / handle jika di-hover atau di-drag
            if is_hover && track.duration_ms > 0 {
                let thumb_x = bar_rect.min.x + filled_w;
                painter.circle_filled(
                    Pos2::new(thumb_x, bar_rect.center().y),
                    5.0,
                    Color32::WHITE,
                );
            }

            ui.label(
                RichText::new(format!("{:02}:{:02}", dur_sec / 60, dur_sec % 60))
                    .font(FontId::monospace(10.0))
                    .color(Color32::from_rgb(140, 145, 155)),
            );
        });

        ui.add_space(8.0);

        // Control Buttons: Previous, Play/Pause, Next, Lyrics Toggle
        ui.horizontal(|ui| {
            ui.add_space(14.0);

            let btn_prev = ui.button(RichText::new("⏮").font(FontId::proportional(14.0)));
            if btn_prev.clicked() {
                if self.demo_mode {
                    self.demo_track_idx = self.demo_track_idx.saturating_sub(1);
                    self.apply_demo_track();
                } else {
                    send_media_command("prev");
                }
            }

            let play_icon = if track.is_playing { "⏸" } else { "▶" };
            let btn_play = ui.button(
                RichText::new(play_icon)
                    .font(FontId::proportional(16.0))
                    .color(Color32::from_rgb(52, 199, 89)),
            );
            if btn_play.clicked() {
                if self.demo_mode {
                    let mut t = self.shared_media.lock().unwrap();
                    t.is_playing = !t.is_playing;
                } else {
                    {
                        let mut t = self.shared_media.lock().unwrap();
                        t.is_playing = !t.is_playing;
                    }
                    send_media_command("toggle");
                }
            }

            let btn_next = ui.button(RichText::new("⏭").font(FontId::proportional(14.0)));
            if btn_next.clicked() {
                if self.demo_mode {
                    self.demo_track_idx += 1;
                    self.apply_demo_track();
                } else {
                    send_media_command("next");
                }
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(14.0);

                // Toggle Lyrics
                let lyrics_btn_color = if self.show_lyrics {
                    Color32::from_rgb(100, 210, 255)
                } else {
                    Color32::from_rgb(150, 155, 165)
                };
                if ui
                    .button(RichText::new("📝 Lirik LRCLIB").font(FontId::proportional(11.0)).color(lyrics_btn_color))
                    .clicked()
                {
                    self.show_lyrics = !self.show_lyrics;
                    self.save_config();
                    if self.show_lyrics && lyrics.lines.is_empty() && !lyrics.is_loading {
                        self.shared_lyrics.lock().unwrap().current_key.clear();
                        fetch_lyrics_async(
                            Arc::clone(&self.shared_lyrics),
                            Arc::clone(&self.lyrics_cache),
                            track.artist.clone(),
                            track.title.clone(),
                            track.duration_ms / 1000,
                        );
                    }
                }
            });
        });

        // Volume slider: system volume (default audio endpoint).
        // Applies live per whole-percent step; external volume
        // changes (keyboard keys, other apps) are NOT reflected
        // until restart (v1 limitation).
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.add_space(14.0);
            ui.label(RichText::new("🔊").font(FontId::proportional(12.0)));
            let mut vol = (self.volume * 100.0).round();
            ui.spacing_mut().slider_width = (ui.available_width() - 70.0).max(60.0);
            let resp = ui.add(
                egui::Slider::new(&mut vol, 0.0..=100.0)
                    .show_value(false),
            );
            if resp.changed() {
                self.volume = vol.round() / 100.0;
                set_system_volume(self.volume);
            }
            ui.label(
                RichText::new(format!("{}%", (self.volume * 100.0).round() as i32))
                    .font(FontId::proportional(11.0))
                    .color(Color32::from_rgb(150, 155, 165)),
            );
            ui.add_space(14.0);
        });

        // Water reminder row: glass count + drink button. When
        // due, the row turns into a highlighted reminder.
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.add_space(14.0);
            let due = self.water_due();
            let (water_text, water_color) = if due {
                (
                    "💧 Waktunya minum!".to_string(),
                    Color32::from_rgb(100, 210, 255),
                )
            } else {
                (
                    format!("💧 Minum: {} gelas hari ini", self.water.count_today),
                    Color32::from_rgb(150, 155, 165),
                )
            };
            ui.label(
                RichText::new(water_text)
                    .font(FontId::proportional(11.0))
                    .color(water_color),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(14.0);
                if ui.button(RichText::new("🚰 Sudah minum").font(FontId::proportional(11.0))).clicked() {
                    self.water.count_today += 1;
                    self.water_next_due =
                        Instant::now() + Duration::from_secs(self.water_mins * 60);
                    self.water.save();
                }
            });
        });
    }

    /// Kolom lirik di panel expanded (side-by-side ala Nusa):
    /// panel kanan setinggi kolom, scroll mengikuti baris aktif,
    /// klik baris -> seek (perilaku lama dipertahankan).
    fn render_lyrics_panel(
        &mut self,
        ui: &mut egui::Ui,
        track: &MediaTrack,
        lyrics: &LyricsState,
    ) {
        egui::Frame::NONE
            .fill(Color32::from_rgb(14, 16, 20))
            .inner_margin(egui::Margin::symmetric(14, 6))
            .show(ui, |ui| {
                if lyrics.is_loading {
                    ui.vertical_centered(|ui| {
                        ui.add_space(16.0);
                        ui.spinner();
                        ui.label(
                            RichText::new("Sinkronisasi lirik dari LRCLIB...")
                                .font(FontId::proportional(11.0))
                                .color(Color32::from_rgb(140, 180, 220)),
                        );
                    });
                } else if let Some(ref err) = lyrics.error {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!("⚠️ {}", err))
                                .font(FontId::proportional(11.0))
                                .color(Color32::from_rgb(240, 140, 140)),
                        );
                        if ui.button(RichText::new("🔄 Coba Lagi").font(FontId::proportional(10.0))).clicked() {
                            self.shared_lyrics.lock().unwrap().current_key.clear();
                            fetch_lyrics_async(
                                Arc::clone(&self.shared_lyrics),
                                Arc::clone(&self.lyrics_cache),
                                track.artist.clone(),
                                track.title.clone(),
                                track.duration_ms / 1000,
                            );
                        }
                    });
                } else if lyrics.lines.is_empty() {
                    ui.label(
                        RichText::new("Lyrics not available")
                            .font(FontId::proportional(11.0))
                            .color(Color32::GRAY),
                    );
                } else {
                    // Temukan baris lirik yang sedang aktif
                    let cur_ms = track.position_ms;
                    let mut active_idx = 0;
                    for (i, line) in lyrics.lines.iter().enumerate() {
                        if line.time_ms <= cur_ms {
                            active_idx = i;
                        } else {
                            break;
                        }
                    }

                    ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            for (i, line) in lyrics.lines.iter().enumerate() {
                                let is_current = i == active_idx;
                                let text_color = if is_current {
                                    Color32::from_rgb(100, 230, 255) // Bright cyan highlight
                                } else if i < active_idx {
                                    Color32::from_rgb(110, 115, 125) // Past lines
                                } else {
                                    Color32::from_rgb(150, 155, 165) // Upcoming
                                };

                                let font = if is_current {
                                    FontId::proportional(13.5)
                                } else {
                                    FontId::proportional(11.5)
                                };

                                // Klik baris lirik -> seek ke timestamp baris itu
                                // (pola sama seperti seek bar: update posisi optimis
                                // + seek media asli, kecuali mode Demo).
                                let label = ui.add(
                                    egui::Label::new(
                                        RichText::new(&line.text)
                                            .font(font)
                                            .color(text_color)
                                            .strong(),
                                    )
                                    .sense(Sense::click()),
                                );

                                if label.clicked() {
                                    {
                                        let mut t = self.shared_media.lock().unwrap();
                                        t.position_ms = line.time_ms;
                                    }
                                    if !self.demo_mode {
                                        seek_media_position(line.time_ms);
                                    }
                                }

                                if is_current {
                                    label.scroll_to_me(Some(Align::Center));
                                }
                            }
                        });
                }
            });
    }

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
        ui.vertical(|ui| {
            ui.add_space(8.0);

            // Header bersama (jam terpusat + carousel + Ciutkan).
            self.render_view_header(ui, "🎵 Musik", Some(track.source_app.as_str()), track.is_playing);

            ui.add_space(8.0);

            // Layout: side-by-side ala Nusa saat lirik aktif,
            // kolom tunggal saat lirik mati.
            if self.show_lyrics {
                ui.horizontal_top(|ui| {
                    ui.allocate_ui_with_layout(
                        vec2(300.0, ui.available_height()),
                        Layout::top_down(Align::LEFT),
                        |ui| {
                            self.render_music_column(ui, track, lyrics);
                        },
                    );
                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(8.0);
                    ui.vertical(|ui| {
                        self.render_lyrics_panel(ui, track, lyrics);
                    });
                });
            } else {
                self.render_music_column(ui, track, lyrics);
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
