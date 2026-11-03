//! System tray icon + menu (tray-icon crate).
//!
//! The tray is best-effort: if building it fails, the app simply
//! runs without a tray icon. The TrayIcon handle must be kept alive
//! (dropping it removes the icon), so it lives in NotchApp.
//! Menu: Tampilkan Notch / Jalankan saat startup (checkable) / Keluar.

use tray_icon::{
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem},
    Icon, TrayIcon, TrayIconBuilder,
};

pub struct TrayHandles {
    pub _icon: TrayIcon,
    pub show_id: tray_icon::menu::MenuId,
    pub quit_id: tray_icon::menu::MenuId,
    pub autostart_id: tray_icon::menu::MenuId,
    pub autostart_item: CheckMenuItem,
}

/// 32x32 icon: dark disc with a cyan center dot (placeholder art;
/// design pass owns the final look).
fn build_icon() -> Option<Icon> {
    let size: u32 = 32;
    let mut rgba = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let dx = x as i32 - 16;
            let dy = y as i32 - 16;
            let dist = ((dx * dx + dy * dy) as f32).sqrt();
            let idx = ((y * size + x) * 4) as usize;
            if dist <= 14.0 {
                // Dark disc.
                rgba[idx] = 10;
                rgba[idx + 1] = 12;
                rgba[idx + 2] = 16;
                rgba[idx + 3] = 255;
                if dist <= 6.0 {
                    // Cyan center.
                    rgba[idx] = 100;
                    rgba[idx + 1] = 210;
                    rgba[idx + 2] = 255;
                    rgba[idx + 3] = 255;
                }
            }
        }
    }
    Icon::from_rgba(rgba, size, size).ok()
}

pub fn build_tray(autostart_enabled: bool) -> Option<TrayHandles> {
    let icon = build_icon()?;
    let menu = Menu::new();
    let show_item = MenuItem::with_id("tray-show", "Tampilkan Notch", true, None);
    let autostart_item = CheckMenuItem::with_id(
        "tray-autostart",
        "Jalankan saat startup",
        true,
        autostart_enabled,
        None,
    );
    let quit_item = MenuItem::with_id("tray-quit", "Keluar", true, None);
    // Append failures (not expected for plain items) just leave the
    // menu partial; the tray still builds.
    let _ = menu.append(&show_item);
    let _ = menu.append(&autostart_item);
    let _ = menu.append(&PredefinedMenuItem::separator());
    let _ = menu.append(&quit_item);
    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_icon(icon)
        .with_tooltip("Notch Island")
        .build()
        .ok()?;
    Some(TrayHandles {
        _icon: tray,
        show_id: show_item.id().clone(),
        quit_id: quit_item.id().clone(),
        autostart_id: autostart_item.id().clone(),
        autostart_item,
    })
}
