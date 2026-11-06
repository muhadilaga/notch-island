//! System volume control via Windows Core Audio endpoint APIs.
//!
//! Architecture:
//! - All Core Audio COM calls run on a dedicated worker thread with its own
//!   `CoInitializeEx` apartment and `IAudioEndpointVolume` cache.
//! - UI and input hooks send non-blocking commands (`VolCmd`) over an `mpsc` channel.
//! - Rapid consecutive scroll events are coalesced into a ~40 ms window before
//!   committing to the hardware endpoint, preventing COM RPC contention or UI freeze.
//! - External volume adjustments are periodically synchronized (~500 ms).

use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{
    eConsole, eRender, IMMDeviceEnumerator, MMDeviceEnumerator,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VolCmd {
    #[allow(dead_code)]
    Delta(f32),
    Set(f32),
}

pub trait VolumeBackend {
    /// Return current volume level (0.0..=1.0) and mute status.
    fn get(&mut self) -> Option<(f32, bool)>;
    /// Set master volume level (0.0..=1.0).
    fn set(&mut self, level: f32);
}

pub struct WindowsVolumeBackend {
    endpoint_vol: Option<IAudioEndpointVolume>,
}

// Safety: WindowsVolumeBackend is constructed, manipulated, and dropped entirely
// within the dedicated volume-worker thread apartment.
unsafe impl Send for WindowsVolumeBackend {}

impl WindowsVolumeBackend {
    pub fn new() -> Option<Self> {
        let mut backend = Self { endpoint_vol: None };
        backend.ensure_endpoint();
        Some(backend)
    }

    fn ensure_endpoint(&mut self) {
        if self.endpoint_vol.is_none() {
            self.endpoint_vol = unsafe {
                let enumerator: IMMDeviceEnumerator =
                    match CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) {
                        Ok(e) => e,
                        Err(_) => return,
                    };
                let device = match enumerator.GetDefaultAudioEndpoint(eRender, eConsole) {
                    Ok(d) => d,
                    Err(_) => return,
                };
                device.Activate(CLSCTX_ALL, None).ok()
            };
        }
    }
}

impl VolumeBackend for WindowsVolumeBackend {
    fn get(&mut self) -> Option<(f32, bool)> {
        self.ensure_endpoint();
        if let Some(vol) = &self.endpoint_vol {
            let level = unsafe { vol.GetMasterVolumeLevelScalar().ok() };
            let mute = unsafe { vol.GetMute().map(|m| m.as_bool()).ok().unwrap_or(false) };
            if let Some(l) = level {
                return Some((l, mute));
            }
        }
        // If query failed (device changed or unplugged), reset cache and retry once
        self.endpoint_vol = None;
        self.ensure_endpoint();
        if let Some(vol) = &self.endpoint_vol {
            let level = unsafe { vol.GetMasterVolumeLevelScalar().ok() };
            let mute = unsafe { vol.GetMute().map(|m| m.as_bool()).ok().unwrap_or(false) };
            if let Some(l) = level {
                return Some((l, mute));
            }
        }
        None
    }

    fn set(&mut self, level: f32) {
        let level = level.clamp(0.0, 1.0);
        self.ensure_endpoint();
        let success = if let Some(vol) = &self.endpoint_vol {
            unsafe { vol.SetMasterVolumeLevelScalar(level, std::ptr::null()).is_ok() }
        } else {
            false
        };
        if !success {
            // Re-acquire default endpoint and retry once
            self.endpoint_vol = None;
            self.ensure_endpoint();
            if let Some(vol) = &self.endpoint_vol {
                let _ = unsafe { vol.SetMasterVolumeLevelScalar(level, std::ptr::null()) };
            }
        }
    }
}

#[derive(Clone)]
pub struct VolumeHandle {
    tx: Sender<VolCmd>,
}

impl VolumeHandle {
    pub fn send(&self, cmd: VolCmd) {
        let _ = self.tx.send(cmd);
    }

    #[allow(dead_code)]
    pub fn set_volume(&self, level: f32) {
        self.send(VolCmd::Set(level));
    }

    #[allow(dead_code)]
    pub fn adjust_volume(&self, delta: f32) {
        self.send(VolCmd::Delta(delta));
    }
}

fn apply_cmd(cmd: VolCmd, level: &mut f32) {
    match cmd {
        VolCmd::Delta(d) => *level = (*level + d).clamp(0.0, 1.0),
        VolCmd::Set(v) => *level = v.clamp(0.0, 1.0),
    }
}

pub fn spawn_volume_worker<B, F>(
    make_backend: impl FnOnce() -> Option<B> + Send + 'static,
    on_change: F,
) -> VolumeHandle
where
    B: VolumeBackend + 'static,
    F: Fn(f32, bool) + Send + 'static,
{
    let (tx, rx) = mpsc::channel::<VolCmd>();
    std::thread::Builder::new()
        .name("volume-worker".to_string())
        .spawn(move || {
            unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            }
            {
                let Some(mut backend) = make_backend() else {
                    unsafe {
                        CoUninitialize();
                    }
                    return;
                };
                let mut level = backend.get().map(|v| v.0).unwrap_or(0.5);

                loop {
                    let cmd = match rx.recv_timeout(Duration::from_millis(500)) {
                        Ok(c) => Some(c),
                        Err(RecvTimeoutError::Timeout) => None,
                        Err(RecvTimeoutError::Disconnected) => break,
                    };

                    if let Some(c) = cmd {
                        apply_cmd(c, &mut level);
                        // Coalesce rapid consecutive commands within a 40 ms window
                        let deadline = Instant::now() + Duration::from_millis(40);
                        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
                            match rx.recv_timeout(left) {
                                Ok(c) => apply_cmd(c, &mut level),
                                Err(_) => break,
                            }
                        }
                        backend.set(level);
                    }

                    if let Some((l, m)) = backend.get() {
                        level = l;
                        on_change(l, m);
                    }
                }
                // Explicitly drop backend and any held COM objects before CoUninitialize
                drop(backend);
            }
            unsafe {
                CoUninitialize();
            }
        })
        .expect("Failed to spawn volume worker thread");

    VolumeHandle { tx }
}

pub fn spawn_system_volume_worker<F>(on_change: F) -> VolumeHandle
where
    F: Fn(f32, bool) + Send + 'static,
{
    spawn_volume_worker(WindowsVolumeBackend::new, on_change)
}

/// Fallback helper to fetch current volume synchronously (e.g. at initial startup).
pub fn get_system_volume() -> Option<f32> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let res = {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
            let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole).ok()?;
            let vol: IAudioEndpointVolume = device.Activate(CLSCTX_ALL, None).ok()?;
            vol.GetMasterVolumeLevelScalar().ok()
        };
        CoUninitialize();
        res
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

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

    struct MockBackend {
        level: f32,
        muted: bool,
        set_count: Arc<AtomicUsize>,
    }

    impl VolumeBackend for MockBackend {
        fn get(&mut self) -> Option<(f32, bool)> {
            Some((self.level, self.muted))
        }
        fn set(&mut self, level: f32) {
            self.level = level;
            self.set_count.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn test_volume_worker_coalescing() {
        let set_count = Arc::new(AtomicUsize::new(0));
        let set_count_clone = Arc::clone(&set_count);

        let handle = spawn_volume_worker(
            move || {
                Some(MockBackend {
                    level: 0.5,
                    muted: false,
                    set_count: set_count_clone,
                })
            },
            |_level, _muted| {},
        );

        // Send 100 rapid delta commands
        for _ in 0..100 {
            handle.adjust_volume(0.01);
        }

        // Wait for coalescing window and flush
        std::thread::sleep(Duration::from_millis(150));

        // The 100 commands should be coalesced into very few backend.set() calls
        let calls = set_count.load(Ordering::SeqCst);
        assert!(calls < 10, "Expected coalescing, but got {} backend calls", calls);
    }
}
