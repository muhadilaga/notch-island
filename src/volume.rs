//! System volume control via the Windows Core Audio endpoint APIs.
//!
//! The slider in the music view reads the default render endpoint's
//! master volume once at startup and sets it live while dragging.
//! Calls follow the same pattern as the media module: each operation
//! initializes COM first (errors ignored — COM may already be up),
//! and sets run on a short-lived thread so slider drags never block
//! the UI.

use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{
    eConsole, eRender, IMMDeviceEnumerator, MMDeviceEnumerator,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED,
};

fn endpoint_volume() -> windows::core::Result<IAudioEndpointVolume> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
        device.Activate(CLSCTX_ALL, None)
    }
}

/// Current system volume as 0.0-1.0, or None when the endpoint is
/// unavailable. Called once at startup; the UI keeps its own copy
/// afterwards (external changes are not tracked live in v1).
pub fn get_system_volume() -> Option<f32> {
    endpoint_volume()
        .ok()
        .and_then(|v| unsafe { v.GetMasterVolumeLevelScalar().ok() })
}

/// Set the system volume (0.0-1.0, clamped). Fire-and-forget on a
/// short-lived thread so slider drags never block the UI.
pub fn set_system_volume(level: f32) {
    let level = level.clamp(0.0, 1.0);
    std::thread::spawn(move || {
        if let Ok(v) = endpoint_volume() {
            let _ = unsafe { v.SetMasterVolumeLevelScalar(level, std::ptr::null()) };
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_system_volume_endpoint() {
        let vol = get_system_volume();
        println!("Current system volume: {:?}", vol);
        if let Some(v) = vol {
            assert!(v >= 0.0 && v <= 1.0);
        }
    }

    #[test]
    fn test_volume_clamping() {
        assert_eq!((-0.5f32).clamp(0.0, 1.0), 0.0);
        assert_eq!((1.5f32).clamp(0.0, 1.0), 1.0);
        assert_eq!((0.42f32).clamp(0.0, 1.0), 0.42);
    }

    #[test]
    fn test_volume_percentage_roundtrip() {
        let vol = 0.73f32;
        let percent = (vol * 100.0).round();
        assert_eq!(percent, 73.0);
        let restored = percent / 100.0;
        assert!((restored - vol).abs() < 1e-4);
    }
}
