//! Interaksi window tingkat Win32: click-through saat notch menciut,
//! sembunyikan notch saat aplikasi fullscreen aktif, dan beri tahu UI
//! saat window foreground maximized (mode thin line).
//!
//! Desain fail-safe & anti-crash:
//! - Setiap callback hook (WH_MOUSE_LL) dan window procedure dibungkus catch_unwind
//!   dan selalu memanggil CallNextHookEx / CallWindowProcW.
//! - Non-blocking (< 1 ms), bebas unwrap/expect, penanganan Err Win32 lengkap.
//! - Mengabaikan Progman, WorkerW, dan window milik shell agar klik/refresh desktop
//!   tidak memicu status fullscreen palsu atau crash.
//! - Jika try_lock() gagal, menggunakan state rect terakhir yang diketahui via atomics
//!   sehingga event tidak dibuang.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, CallWindowProcW, DefWindowProcW, DispatchMessageW, FindWindowW,
    GetClassNameW, GetForegroundWindow, GetMessageW, GetSystemMetrics, GetWindowLongPtrW,
    GetWindowRect, GWLP_WNDPROC, GWL_EXSTYLE, GWL_STYLE, HHOOK, IsWindow, IsZoomed,
    MSG, MSLLHOOKSTRUCT, RegisterWindowMessageW, SetWindowLongPtrW, SetWindowPos,
    SetWindowsHookExW, SM_CXSCREEN, SM_CYSCREEN, SWP_FRAMECHANGED, SWP_HIDEWINDOW,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW, TranslateMessage,
    WH_MOUSE_LL, WINDOW_EX_STYLE, WINDOW_STYLE, WNDPROC, WM_DISPLAYCHANGE, WM_DPICHANGED,
    WM_SETTINGCHANGE, WS_CAPTION, WS_EX_APPWINDOW, WS_EX_LAYERED, WS_EX_TOOLWINDOW,
    WS_EX_TRANSPARENT,
};

/// Rect notch dalam piksel fisik layar, dipublish tiap frame oleh UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NotchRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// Hit-test murni: aman dari overflow aritmetika dan menangani ukuran nol/negatif.
pub fn is_point_inside_rect(px: i32, py: i32, rect: NotchRect) -> bool {
    if rect.w <= 0 || rect.h <= 0 {
        return false;
    }
    let right = match rect.x.checked_add(rect.w) {
        Some(r) => r,
        None => return false,
    };
    let bottom = match rect.y.checked_add(rect.h) {
        Some(b) => b,
        None => return false,
    };
    px >= rect.x && px < right && py >= rect.y && py < bottom
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
    _click_through: Mutex<bool>,
    fullscreen_hidden: Mutex<bool>,
}

static GLOBALS: OnceLock<Arc<HookGlobals>> = OnceLock::new();

// Cache atomic untuk akses lock-free cepat & fallback saat try_lock gagal.
static LAST_RECT_X: AtomicI32 = AtomicI32::new(0);
static LAST_RECT_Y: AtomicI32 = AtomicI32::new(0);
static LAST_RECT_W: AtomicI32 = AtomicI32::new(0);
static LAST_RECT_H: AtomicI32 = AtomicI32::new(0);
static LAST_RECT_VALID: AtomicBool = AtomicBool::new(false);

static LAST_HOOK: AtomicUsize = AtomicUsize::new(0);
static LAST_CLICK_THROUGH: AtomicBool = AtomicBool::new(false);
static LAST_HWND: AtomicUsize = AtomicUsize::new(0);

static PREV_WNDPROC: AtomicUsize = AtomicUsize::new(0);
static TASKBAR_CREATED_MSG: AtomicU32 = AtomicU32::new(0);
pub static SYSTEM_CHANGE_SIGNAL: AtomicBool = AtomicBool::new(false);
pub static TASKBAR_RECREATED_SIGNAL: AtomicBool = AtomicBool::new(false);

/// Simpan rect terbaru ke atomic cache agar callback hook tidak pernah kehilangan state.
pub fn update_cached_rect(rect: Option<NotchRect>) {
    if let Some(r) = rect {
        LAST_RECT_X.store(r.x, Ordering::Relaxed);
        LAST_RECT_Y.store(r.y, Ordering::Relaxed);
        LAST_RECT_W.store(r.w, Ordering::Relaxed);
        LAST_RECT_H.store(r.h, Ordering::Relaxed);
        LAST_RECT_VALID.store(true, Ordering::Relaxed);
    } else {
        LAST_RECT_VALID.store(false, Ordering::Relaxed);
    }
}

/// Deteksi window milik shell Windows (Progman, WorkerW, Taskbar, dll)
/// agar klik desktop / refresh F5 tidak dianggap fullscreen atau maximized.
pub fn is_shell_or_desktop_window(hwnd: HWND) -> bool {
    if hwnd.is_invalid() || unsafe { !IsWindow(hwnd).as_bool() } {
        return true;
    }
    let mut buf = [0u16; 64];
    let len = unsafe { GetClassNameW(hwnd, &mut buf) };
    if len <= 0 {
        return false;
    }
    let class_name = String::from_utf16_lossy(&buf[..len as usize]);
    let cls = class_name.as_str();
    cls == "Progman"
        || cls == "WorkerW"
        || cls == "Shell_TrayWnd"
        || cls == "Shell_SecondaryTrayWnd"
        || cls == "Windows.UI.Core.CoreWindow"
        || cls == "DV2ControlHost"
        || cls == "Button"
        || cls == "SysListView32"
}

/// Window procedure subclass untuk memantau pesan sistem Windows.
unsafe extern "system" fn notch_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let taskbar_msg = TASKBAR_CREATED_MSG.load(Ordering::Relaxed);
        if taskbar_msg != 0 && msg == taskbar_msg {
            TASKBAR_RECREATED_SIGNAL.store(true, Ordering::Relaxed);
        } else if msg == WM_DISPLAYCHANGE || msg == WM_DPICHANGED || msg == WM_SETTINGCHANGE {
            SYSTEM_CHANGE_SIGNAL.store(true, Ordering::Relaxed);
        }
    }));

    let prev = PREV_WNDPROC.load(Ordering::Relaxed);
    if prev != 0 {
        let proc: WNDPROC = unsafe { std::mem::transmute(prev) };
        unsafe { CallWindowProcW(proc, hwnd, msg, wparam, lparam) }
    } else {
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }
}

/// Jalankan dua thread pendamping: (1) low-level mouse hook untuk
/// click-through, (2) polling fullscreen tiap 500 ms. Dipanggil sekali
/// dari main() sebelum event loop eframe jalan.
pub fn spawn_interact_threads(state: SharedInteract) {
    let globals = Arc::new(HookGlobals {
        state,
        hwnd: Mutex::new(None),
        hook: Mutex::new(None),
        _click_through: Mutex::new(false),
        fullscreen_hidden: Mutex::new(false),
    });
    let _ = GLOBALS.set(Arc::clone(&globals));

    // Thread 1: mouse hook + subclass message monitor.
    let hook_globals = Arc::clone(&globals);
    std::thread::Builder::new()
        .name("interact-hook".to_string())
        .spawn(move || {
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
            LAST_HWND.store(hwnd_raw, Ordering::Relaxed);
            if let Ok(mut slot) = hook_globals.hwnd.lock() {
                *slot = Some(hwnd_raw);
            }

            let hwnd = HWND(hwnd_raw as *mut c_void);

            // Pasang subclass wndproc untuk TaskbarCreated & DisplayChange
            unsafe {
                let taskbar_msg = RegisterWindowMessageW(w!("TaskbarCreated"));
                if taskbar_msg != 0 {
                    TASKBAR_CREATED_MSG.store(taskbar_msg, Ordering::Relaxed);
                }
                let prev = SetWindowLongPtrW(
                    hwnd,
                    GWLP_WNDPROC,
                    notch_wndproc as *const () as usize as isize,
                );
                if prev != 0 {
                    PREV_WNDPROC.store(prev as usize, Ordering::Relaxed);
                }
            }

            set_click_through(hwnd, true);
            LAST_CLICK_THROUGH.store(true, Ordering::Relaxed);

            unsafe {
                match SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook_proc), HINSTANCE::default(), 0) {
                    Ok(h) => {
                        LAST_HOOK.store(h.0 as usize, Ordering::Relaxed);
                        if let Ok(mut slot) = hook_globals.hook.lock() {
                            *slot = Some(h.0 as usize);
                        }
                        // Message loop wajib ada agar LL hook tetap hidup.
                        let mut msg = MSG::default();
                        loop {
                            let ret = GetMessageW(&mut msg, HWND::default(), 0, 0);
                            if ret.0 <= 0 {
                                break;
                            }
                            let _ = TranslateMessage(&msg);
                            DispatchMessageW(&msg);
                        }
                    }
                    Err(_) => {
                        // Fallback aman jika hook ditolak OS.
                    }
                }
            }
        })
        .ok();

    // Thread 2: polling fullscreen & maximized.
    std::thread::Builder::new()
        .name("fullscreen-watcher".to_string())
        .spawn(move || loop {
            std::thread::sleep(Duration::from_millis(500));
            let Some(g) = GLOBALS.get() else { continue };
            let hwnd_raw = match g.hwnd.try_lock() {
                Ok(slot) => *slot,
                Err(_) => {
                    let h = LAST_HWND.load(Ordering::Relaxed);
                    if h != 0 { Some(h) } else { None }
                }
            };
            let Some(hwnd_raw) = hwnd_raw else { continue };
            let own = HWND(hwnd_raw as *mut c_void);
            if unsafe { !IsWindow(own).as_bool() } {
                continue;
            }

            let is_fs = is_fullscreen_active(own);

            // Thin line: kabari UI bahwa window foreground sedang maximized.
            if let Ok(mut s) = g.state.try_lock() {
                s.maximized_fg = is_maximized_active(own);
            }

            let mut hidden = match g.fullscreen_hidden.try_lock() {
                Ok(v) => v,
                Err(_) => continue,
            };
            if is_fs && !*hidden {
                unsafe {
                    let _ = SetWindowPos(
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
                    let _ = SetWindowPos(
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
        })
        .ok();
}

fn stored_hook() -> HHOOK {
    let h = LAST_HOOK.load(Ordering::Relaxed);
    if h != 0 {
        HHOOK(h as *mut c_void)
    } else {
        HHOOK::default()
    }
}

/// Hook proc: kursor di dalam rect notch -> interaktif (hover-expand tetap jalan);
/// kursor di luar rect -> click-through.
///
/// Selalu dibungkus catch_unwind dan selalu memanggil CallNextHookEx.
/// Jika try_lock gagal, mengambil rect dari atomic cache agar event tidak dibuang.
unsafe extern "system" fn mouse_hook_proc(
    code: i32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if code < 0 || lparam.0 == 0 {
            return;
        }

        let info = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
        let (cx, cy) = (info.pt.x, info.pt.y);

        // Baca rect: coba try_lock; jika gagal, gunakan state terakhir dari atomics.
        let current_rect = if let Some(g) = GLOBALS.get() {
            if let Ok(s) = g.state.try_lock() {
                s.rect
            } else if LAST_RECT_VALID.load(Ordering::Relaxed) {
                Some(NotchRect {
                    x: LAST_RECT_X.load(Ordering::Relaxed),
                    y: LAST_RECT_Y.load(Ordering::Relaxed),
                    w: LAST_RECT_W.load(Ordering::Relaxed),
                    h: LAST_RECT_H.load(Ordering::Relaxed),
                })
            } else {
                None
            }
        } else if LAST_RECT_VALID.load(Ordering::Relaxed) {
            Some(NotchRect {
                x: LAST_RECT_X.load(Ordering::Relaxed),
                y: LAST_RECT_Y.load(Ordering::Relaxed),
                w: LAST_RECT_W.load(Ordering::Relaxed),
                h: LAST_RECT_H.load(Ordering::Relaxed),
            })
        } else {
            None
        };

        let inside = match current_rect {
            Some(r) => is_point_inside_rect(cx, cy, r),
            None => false,
        };

        let want_click_through = !inside;
        let cur_ct = LAST_CLICK_THROUGH.load(Ordering::Relaxed);
        if cur_ct != want_click_through {
            let hwnd_raw = LAST_HWND.load(Ordering::Relaxed);
            if hwnd_raw != 0 {
                let hwnd = HWND(hwnd_raw as *mut c_void);
                set_click_through(hwnd, want_click_through);
                LAST_CLICK_THROUGH.store(want_click_through, Ordering::Relaxed);
            }
        }
    }));

    // SELALU panggil CallNextHookEx
    unsafe { CallNextHookEx(stored_hook(), code, wparam, lparam) }
}

fn set_click_through(hwnd: HWND, enable: bool) {
    if hwnd.is_invalid() || unsafe { !IsWindow(hwnd).as_bool() } {
        return;
    }
    unsafe {
        let cur = WINDOW_EX_STYLE(GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32);
        let mut next = (cur | WS_EX_LAYERED | WS_EX_TOOLWINDOW) & !WS_EX_APPWINDOW;
        if enable {
            next |= WS_EX_TRANSPARENT;
        } else {
            next &= !WS_EX_TRANSPARENT;
        }
        if next != cur {
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, next.0 as isize);
            let _ = SetWindowPos(
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
        if let Ok(hwnd) = FindWindowW(w!("Window Class"), w!("Notch Island")) {
            return Some(hwnd.0 as usize);
        }
        match FindWindowW(PCWSTR::null(), w!("Notch Island")) {
            Ok(hwnd) => Some(hwnd.0 as usize),
            Err(_) => None,
        }
    }
}

/// Window foreground sedang maximized (IsZoomed), bukan fullscreen
/// borderless. Diabaikan jika window foreground adalah shell / desktop.
fn is_maximized_active(own: HWND) -> bool {
    unsafe {
        let fg = GetForegroundWindow();
        if fg.is_invalid() || fg.0 == own.0 || !IsWindow(fg).as_bool() {
            return false;
        }
        if is_shell_or_desktop_window(fg) {
            return false;
        }
        IsZoomed(fg).as_bool()
    }
}

/// Heuristik fullscreen: window foreground menutupi seluruh layar DAN tidak punya
/// caption/border (borderless fullscreen). Diabaikan jika window foreground adalah shell / desktop.
fn is_fullscreen_active(own: HWND) -> bool {
    unsafe {
        let fg = GetForegroundWindow();
        if fg.is_invalid() || fg.0 == own.0 || !IsWindow(fg).as_bool() {
            return false;
        }
        if is_shell_or_desktop_window(fg) {
            return false;
        }
        let mut rect = RECT::default();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hit_test_inside_and_outside() {
        let rect = NotchRect {
            x: 100,
            y: 0,
            w: 200,
            h: 50,
        };
        // Titik di dalam
        assert!(is_point_inside_rect(100, 0, rect)); // Pojok kiri-atas (inklusif)
        assert!(is_point_inside_rect(150, 25, rect)); // Tengah
        assert!(is_point_inside_rect(299, 49, rect)); // Pojok kanan-bawah batas dalam

        // Titik di luar
        assert!(!is_point_inside_rect(99, 0, rect)); // Kiri
        assert!(!is_point_inside_rect(100, -1, rect)); // Atas
        assert!(!is_point_inside_rect(300, 25, rect)); // Kanan batas luar (eksklusif)
        assert!(!is_point_inside_rect(150, 50, rect)); // Bawah batas luar (eksklusif)
        assert!(!is_point_inside_rect(0, 0, rect)); // Jauh
    }

    #[test]
    fn test_hit_test_zero_and_negative_size() {
        // Lebar 0
        let zero_w = NotchRect { x: 50, y: 50, w: 0, h: 100 };
        assert!(!is_point_inside_rect(50, 50, zero_w));

        // Tinggi 0
        let zero_h = NotchRect { x: 50, y: 50, w: 100, h: 0 };
        assert!(!is_point_inside_rect(50, 50, zero_h));

        // Lebar negatif
        let neg_w = NotchRect { x: 50, y: 50, w: -50, h: 100 };
        assert!(!is_point_inside_rect(50, 50, neg_w));

        // Tinggi negatif
        let neg_h = NotchRect { x: 50, y: 50, w: 100, h: -50 };
        assert!(!is_point_inside_rect(50, 50, neg_h));
    }

    #[test]
    fn test_hit_test_overflow_safety() {
        // Overflow koordinat x + w
        let overflow_x = NotchRect {
            x: i32::MAX - 10,
            y: 0,
            w: 50,
            h: 50,
        };
        assert!(!is_point_inside_rect(i32::MAX, 10, overflow_x));

        // Overflow koordinat y + h
        let overflow_y = NotchRect {
            x: 0,
            y: i32::MAX - 10,
            w: 50,
            h: 50,
        };
        assert!(!is_point_inside_rect(10, i32::MAX, overflow_y));
    }

    #[test]
    fn test_hit_test_negative_coordinates() {
        let neg_rect = NotchRect {
            x: -100,
            y: -50,
            w: 200,
            h: 100,
        };
        assert!(is_point_inside_rect(-100, -50, neg_rect));
        assert!(is_point_inside_rect(0, 0, neg_rect));
        assert!(is_point_inside_rect(99, 49, neg_rect));
        assert!(!is_point_inside_rect(-101, 0, neg_rect));
        assert!(!is_point_inside_rect(100, 0, neg_rect));
    }

    #[test]
    fn test_shell_window_detection() {
        assert!(is_shell_or_desktop_window(HWND::default()));
    }

    #[test]
    fn test_hit_test_animated_pill_width() {
        // Pill teranimasi dari 140 px ke 300 px pada layar 1920
        // Saat w = 140: x = (1920 - 140) / 2 = 890
        let rect_140 = NotchRect {
            x: 890,
            y: 0,
            w: 140,
            h: 36,
        };
        assert!(!is_point_inside_rect(850, 18, rect_140));
        assert!(is_point_inside_rect(900, 18, rect_140));

        // Saat pill melebar ke 300: x = (1920 - 300) / 2 = 810, w = 300
        let rect_300 = NotchRect {
            x: 810,
            y: 0,
            w: 300,
            h: 36,
        };
        assert!(is_point_inside_rect(850, 18, rect_300));
        assert!(is_point_inside_rect(810, 0, rect_300));
        assert!(is_point_inside_rect(1109, 35, rect_300));
        assert!(!is_point_inside_rect(809, 18, rect_300));
        assert!(!is_point_inside_rect(1110, 18, rect_300));
    }
}
