//! Windows autostart via the per-user Run registry key, driven by
//! reg.exe (no registry FFI surface to get wrong).
//!
//! Key: `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`,
//! value name `Notch Island`, value = quoted path of the current
//! executable. All operations are best-effort booleans; failures
//! never crash the UI. reg.exe runs with CREATE_NO_WINDOW so no
//! console window flashes.

use std::os::windows::process::CommandExt;

const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "Notch Island";

fn reg(args: &[&str]) -> bool {
    std::process::Command::new("reg")
        .args(args)
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

pub fn is_autostart_enabled() -> bool {
    reg(&["query", RUN_KEY, "/v", VALUE_NAME])
}

pub fn enable_autostart() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let quoted = format!("\"{}\"", exe.display());
    reg(&[
        "add", RUN_KEY, "/v", VALUE_NAME, "/t", "REG_SZ", "/d", &quoted, "/f",
    ])
}

pub fn disable_autostart() -> bool {
    reg(&["delete", RUN_KEY, "/v", VALUE_NAME, "/f"])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_autostart_lifecycle() {
        let was_enabled = is_autostart_enabled();
        assert!(enable_autostart());
        assert!(is_autostart_enabled());
        assert!(disable_autostart());
        assert!(!is_autostart_enabled());
        if was_enabled {
            let _ = enable_autostart();
        }
    }
}
