use serde::{Deserialize, Serialize};
use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum DisplayMode {
    FloatingIsland,
    #[default]
    MinimalBezel,
}

/// Radius telinga cekung (P2): sudut atas notch menjorok 12 px ke
/// luar bodi di tiap sisi agar menyatu mulus dengan bezel layar.
/// Jendela OS selalu 2*EAR_RADIUS lebih lebar dari bodi; bodi =
/// jendela dikurangi EAR_RADIUS di kiri/kanan (lihat
/// `body_rect_for_window` di ui.rs).
pub const EAR_RADIUS: f32 = 12.0;

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
        _is_playing: bool,
        pixels_per_point: f32,
        thin_line: bool,
        pill_width: Option<f32>,
    ) -> (f32, f32, f32, f32) {
        let ppp = if pixels_per_point > 0.0 { pixels_per_point } else { 1.0 };
        let logical_screen_w = self.screen_width / ppp;

        if thin_line {
            // Garis tipis ala Nusa Minimal notch: tetap terlihat dan
            // tetap bisa di-hover, tanpa teks atau konten.
            // Lebar jendela sudah termasuk strip telinga (P2).
            return (
                (logical_screen_w - (280.0 + 2.0 * EAR_RADIUS)) / 2.0,
                0.0,
                280.0 + 2.0 * EAR_RADIUS,
                8.0,
            );
        }

        let body_width = if is_expanded {
            if has_lyrics { 580.0 } else { 460.0 }
        } else if let Some(pw) = pill_width {
            pw.clamp(140.0, 420.0)
        } else {
            140.0
        };
        // P2: jendela OS 2*EAR_RADIUS lebih lebar dari bodi agar
        // telinga cekung 12 px punya ruang gambar di tiap sisi.
        // Bodi tetap di posisi & ukuran lama.
        let width = body_width + 2.0 * EAR_RADIUS;

        let height = if is_expanded {
            // P1: panel musik 144 px; bila lirik tersinkron tampil,
            // memanjang 58 px menjadi 202 px (LYRIC_SHIFT).
            if has_lyrics { 202.0 } else { 144.0 }
        } else {
            36.0
        };

        let x = (logical_screen_w - width) / 2.0;
        let y = match mode {
            DisplayMode::FloatingIsland => 0.0,
            DisplayMode::MinimalBezel => 0.0,
        };

        (x, y, width, height)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_window_rect_pill_animated_width() {
        let geom = ScreenGeometry {
            screen_width: 1920.0,
            screen_height: 1080.0,
        };

        // P2: semua lebar jendela = bodi + 2*EAR_RADIUS (24 px)
        // untuk strip telinga; bodi tetap di ukuran lama.

        // 1. Collapsed pill width min 140 -> jendela 164
        let (x, y, w, h) = geom.calculate_window_rect(
            DisplayMode::MinimalBezel,
            false,
            false,
            true,
            1.0,
            false,
            Some(140.0),
        );
        assert_eq!(w, 164.0);
        assert_eq!(h, 36.0);
        assert_eq!(x, (1920.0 - 164.0) / 2.0);
        assert_eq!(y, 0.0);

        // 2. Collapsed pill width animated 280 -> jendela 304
        let (x, y, w, h) = geom.calculate_window_rect(
            DisplayMode::MinimalBezel,
            false,
            false,
            true,
            1.0,
            false,
            Some(280.0),
        );
        assert_eq!(w, 304.0);
        assert_eq!(h, 36.0);
        assert_eq!(x, (1920.0 - 304.0) / 2.0);
        assert_eq!(y, 0.0);

        // 3. Clamping pill width
        let (_, _, w_min, _) = geom.calculate_window_rect(
            DisplayMode::MinimalBezel,
            false,
            false,
            true,
            1.0,
            false,
            Some(100.0),
        );
        assert_eq!(w_min, 164.0);

        let (_, _, w_max, _) = geom.calculate_window_rect(
            DisplayMode::MinimalBezel,
            false,
            false,
            true,
            1.0,
            false,
            Some(500.0),
        );
        assert_eq!(w_max, 444.0);

        // 4. Expanded ignores pill_width (460 -> 484)
        let (x_exp, y_exp, w_exp, h_exp) = geom.calculate_window_rect(
            DisplayMode::MinimalBezel,
            true,
            false,
            true,
            1.0,
            false,
            Some(280.0),
        );
        assert_eq!(w_exp, 484.0);
        assert_eq!(h_exp, 144.0);
        assert_eq!(x_exp, (1920.0 - 484.0) / 2.0);
        assert_eq!(y_exp, 0.0);

        // 5. Expanded dengan lirik: panel memanjang 144 -> 202 px (580 -> 604).
        let (x_lyr, y_lyr, w_lyr, h_lyr) = geom.calculate_window_rect(
            DisplayMode::MinimalBezel,
            true,
            true,
            true,
            1.0,
            false,
            Some(280.0),
        );
        assert_eq!(w_lyr, 604.0);
        assert_eq!(h_lyr, 202.0);
        assert_eq!(x_lyr, (1920.0 - 604.0) / 2.0);
        assert_eq!(y_lyr, 0.0);

        // 6. Thin line mode (280 -> 304)
        let (x_tl, y_tl, w_tl, h_tl) = geom.calculate_window_rect(
            DisplayMode::MinimalBezel,
            false,
            false,
            true,
            1.0,
            true,
            Some(280.0),
        );
        assert_eq!(w_tl, 304.0);
        assert_eq!(h_tl, 8.0);
        assert_eq!(x_tl, (1920.0 - 304.0) / 2.0);
        assert_eq!(y_tl, 0.0);
    }
}
