//! Interaksi window tingkat Win32: click-through saat notch menciut,
//! sembunyikan notch saat aplikasi fullscreen aktif, dan beri tahu UI
//! saat window foreground maximized (mode thin line).
//!
//! Desain fail-safe: setiap kegagalan Win32 di sini berujung ke perilaku
//! lama (notch selalu interaktif dan selalu terlihat). Thread-thread di
//! sini tidak pernah panic; kegagalan hanya membuat fitur ini nonaktif.

use std::ffi::c_void;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, FindWindowW, GetForegroundWindow, GetMessageW,
    GetSystemMetrics, GetWindowLongPtrW, GetWindowRect, GWL_EXSTYLE, GWL_STYLE, HHOOK,
    IsZoomed, MSG, MSLLHOOKSTRUCT, SetWindowLongPtrW, SetWindowPos, SetWindowsHookExW, SM_CXSCREEN,
    SM_CYSCREEN, SWP_FRAMECHANGED, SWP_HIDEWINDOW, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    SWP_NOZORDER, SWP_SHOWWINDOW, TranslateMessage, WH_MOUSE_LL, WINDOW_EX_STYLE,
    WINDOW_STYLE, WS_CAPTION, WS_EX_LAYERED, WS_EX_TRANSPARENT,
};

/// Rect notch dalam piksel fisik layar, dipublish tiap frame oleh UI.
#[derive(Debug, Clone, Copy, Default)]
pub struct NotchRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

#[derive(Debug, Default)]
pub struct InteractState {
    pub rect: Option<NotchRect>,
    /// Window foreground sedang maximized (bukan fullscreen borderless).
    /// Dibaca UI tiap frame untuk mode thin line.
    pub maximized_fg: bool,
}

pub type SharedInteract = Arc<Mutex<InteractState>>;

struct HookGlobals {
    state: SharedInteract,
    hwnd: Mutex<Option<usize>>,
    hook: Mutex<Option<usize>>,
    click_through: Mutex<bool>,
    fullscreen_hidden: Mutex<bool>,
}

static GLOBALS: OnceLock<Arc<HookGlobals>> = OnceLock::new();

/// Jalankan dua thread pendamping: (1) low-level mouse hook untuk
/// click-through, (2) polling fullscreen tiap 500 ms. Dipanggil sekali
/// dari main() sebelum event loop eframe jalan.
pub fn spawn_interact_threads(state: SharedInteract) {
    let globals = Arc::new(HookGlobals {
        state,
        hwnd: Mutex::new(None),
        hook: Mutex::new(None),
        click_through: Mutex::new(false),
        fullscreen_hidden: Mutex::new(false),
    });
    let _ = GLOBALS.set(Arc::clone(&globals));

    // Thread 1: mouse hook. Hook proc hanya jalan di thread yang
    // memasangnya dan punya message loop, jadi thread ini khusus.
    let hook_globals = Arc::clone(&globals);
    std::thread::spawn(move || {
        // Tunggu window notch dibuat (maks ~10 detik).
        let mut hwnd_raw: Option<usize> = None;
        for _ in 0..100 {
            if let Some(h) = resolve_notch_hwnd() {
                hwnd_raw = Some(h);
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let Some(hwnd_raw) = hwnd_raw else { return };
        if let Ok(mut slot) = hook_globals.hwnd.lock() {
            *slot = Some(hwnd_raw);
        }
        unsafe {
            match SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook_proc), HINSTANCE::default(), 0)
            {
                Ok(h) => {
                    if let Ok(mut slot) = hook_globals.hook.lock() {
                        *slot = Some(h.0 as usize);
                    }
                    // Message loop wajib ada agar LL hook tetap hidup.
                    let mut msg = MSG::default();
                    while GetMessageW(&mut msg, HWND::default(), 0, 0).as_bool() {
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                }
                Err(_) => {
                    // Hook gagal: diam-diam fallback ke perilaku lama.
                }
            }
        }
    });

    // Thread 2: polling fullscreen (pola sama seperti media poller).
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(500));
        let Some(g) = GLOBALS.get() else { continue };
        let hwnd_raw = match g.hwnd.lock() {
            Ok(slot) => *slot,
            Err(_) => continue,
        };
        let Some(hwnd_raw) = hwnd_raw else { continue };
        let own = HWND(hwnd_raw as *mut c_void);
        let is_fs = is_fullscreen_active(own);
        // Thin line: kabari UI bahwa window foreground sedang maximized.
        // Kunci hwnd sudah dilepas di atas, jadi mengunci state sesudahnya
        // tidak berlawanan urutan dengan hook proc (state -> hwnd).
        if let Ok(mut s) = g.state.lock() {
            s.maximized_fg = is_maximized_active(own);
        }
        let mut hidden = match g.fullscreen_hidden.lock() {
            Ok(v) => v,
            Err(_) => continue,
        };
        if is_fs && !*hidden {
            unsafe {
                SetWindowPos(
                    own,
                    HWND::default(),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_HIDEWINDOW,
                );
            }
            *hidden = true;
        } else if !is_fs && *hidden {
            unsafe {
                SetWindowPos(
                    own,
                    HWND::default(),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE
                        | SWP_NOSIZE
                        | SWP_NOZORDER
                        | SWP_SHOWWINDOW
                        | SWP_NOACTIVATE,
                );
            }
            *hidden = false;
        }
    });
}

fn stored_hook() -> HHOOK {
    if let Some(g) = GLOBALS.get() {
        if let Ok(slot) = g.hook.lock() {
            if let Some(h) = *slot {
                return HHOOK(h as *mut c_void);
            }
        }
    }
    HHOOK::default()
}

/// Hook proc: kursor di dalam rect notch -> interaktif (hover-expand
/// tetap jalan); kursor di luar rect -> click-through. Toggle hanya
/// saat keadaan berubah, agar tidak spam SetWindowLongPtrW.
unsafe extern "system" fn mouse_hook_proc(
    code: i32,
    _wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if code >= 0 {
        if let Some(g) = GLOBALS.get() {
            let info = &*(lparam.0 as *const MSLLHOOKSTRUCT);
            let (cx, cy) = (info.pt.x, info.pt.y);
            let inside = match g.state.lock() {
                Ok(s) => match s.rect {
                    Some(r) => cx >= r.x && cx < r.x + r.w && cy >= r.y && cy < r.y + r.h,
                    None => false,
                },
                Err(_) => false,
            };
            // Di luar rect -> click-through; di dalam -> interaktif.
            let want_click_through = !inside;
            if let Ok(mut ct) = g.click_through.lock() {
                if *ct != want_click_through {
                    if let Ok(slot) = g.hwnd.lock() {
                        if let Some(hwnd_raw) = *slot {
                            set_click_through(HWND(hwnd_raw as *mut c_void), want_click_through);
                            *ct = want_click_through;
                        }
                    }
                }
            }
        }
    }
    CallNextHookEx(stored_hook(), code, _wparam, lparam)
}

fn set_click_through(hwnd: HWND, enable: bool) {
    unsafe {
        let cur = WINDOW_EX_STYLE(GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32);
        let mut next = cur | WS_EX_LAYERED;
        if enable {
            next = next | WS_EX_TRANSPARENT;
        } else {
            next = next & !WS_EX_TRANSPARENT;
        }
        if next != cur {
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, next.0 as isize);
            SetWindowPos(
                hwnd,
                HWND::default(),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_FRAMECHANGED,
            );
        }
    }
}

fn resolve_notch_hwnd() -> Option<usize> {
    unsafe {
        // FindWindowW di windows 0.58 mengembalikan Result: Err = tidak ketemu.
        match FindWindowW(PCWSTR::null(), w!("Notch Island")) {
            Ok(hwnd) => Some(hwnd.0 as usize),
            Err(_) => None,
        }
    }
}

/// Window foreground sedang maximized (IsZoomed), bukan fullscreen
/// borderless. Dipakai UI untuk mode thin line.
fn is_maximized_active(own: HWND) -> bool {
    unsafe {
        let fg = GetForegroundWindow();
        if fg.is_invalid() || fg.0 == own.0 {
            return false;
        }
        IsZoomed(fg).as_bool()
    }
}

/// Heuristik fullscreen (monitor utama saja, sesuai geometri app saat
/// ini): window foreground menutupi seluruh layar DAN tidak punya
/// caption/border (borderless fullscreen / popup). Window maximized
/// biasa punya caption sehingga tidak ikut tersembunyi.
fn is_fullscreen_active(own: HWND) -> bool {
    unsafe {
        let fg = GetForegroundWindow();
        if fg.is_invalid() || fg.0 == own.0 {
            return false;
        }
        let mut rect = RECT::default();
        // GetWindowRect di windows 0.58 mengembalikan Result, bukan BOOL.
        if GetWindowRect(fg, &mut rect).is_err() {
            return false;
        }
        let sw = GetSystemMetrics(SM_CXSCREEN);
        let sh = GetSystemMetrics(SM_CYSCREEN);
        if sw <= 0 || sh <= 0 {
            return false;
        }
        let covers = rect.left <= 0 && rect.top <= 0 && rect.right >= sw && rect.bottom >= sh;
        if !covers {
            return false;
        }
        let style = WINDOW_STYLE(GetWindowLongPtrW(fg, GWL_STYLE) as u32);
        (style & WS_CAPTION).0 == 0
    }
}
