use serde::{Deserialize, Serialize};
use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DisplayMode {
    FloatingIsland,
    MinimalBezel,
}

impl Default for DisplayMode {
    fn default() -> Self {
        Self::MinimalBezel
    }
}

pub struct ScreenGeometry {
    pub screen_width: f32,
    #[allow(dead_code)]
    pub screen_height: f32,
}

impl ScreenGeometry {
    pub fn current() -> Self {
        unsafe {
            let w = GetSystemMetrics(SM_CXSCREEN) as f32;
            let h = GetSystemMetrics(SM_CYSCREEN) as f32;
            Self {
                screen_width: if w > 0.0 { w } else { 1920.0 },
                screen_height: if h > 0.0 { h } else { 1080.0 },
            }
        }
    }

    pub fn calculate_window_rect(
        &self,
        mode: DisplayMode,
        is_expanded: bool,
        has_lyrics: bool,
        is_playing: bool,
        pixels_per_point: f32,
        thin_line: bool,
    ) -> (f32, f32, f32, f32) {
        let ppp = if pixels_per_point > 0.0 { pixels_per_point } else { 1.0 };
        let logical_screen_w = self.screen_width / ppp;

        if thin_line {
            // Garis tipis ala Nusa Minimal notch: tetap terlihat dan
            // tetap bisa di-hover, tanpa teks atau konten.
            return ((logical_screen_w - 280.0) / 2.0, 0.0, 280.0, 8.0);
        }

        let width = if is_expanded {
            if has_lyrics { 580.0 } else { 460.0 }
        } else if !is_playing {
            // Slit kamera ala Nusa saat idle/pause: strip 48 px
            // (bukan pill 140 px berisi dot + jam). Hover di atas
            // slit tetap mengembang; target hover jadi sekecil
            // slit (trade-off Nusa, bukan bug).
            48.0
        } else {
            // Pill play minimal ala Nusa: satu lebar 140 px untuk
            // semua mode (dulu 280/300/380 px berisi ticker).
            140.0
        };

        let height = if is_expanded {
            // Panel pendek 165 -> 195: ruang baris jam terpusat +
            // header carousel di semua view (bukan cuma lirik).
            if has_lyrics { 300.0 } else { 195.0 }
        } else if is_playing {
            36.0
        } else {
            // Slit idle/pause: tinggi 8 px ala Nusa.
            8.0
        };

        let x = (logical_screen_w - width) / 2.0;
        let y = match mode {
            DisplayMode::FloatingIsland => 0.0,
            DisplayMode::MinimalBezel => 0.0,
        };

        (x, y, width, height)
    }
}
