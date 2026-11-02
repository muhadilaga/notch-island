use std::sync::{Arc, Mutex};
use std::time::Duration;
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession,
    GlobalSystemMediaTransportControlsSessionManager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus,
};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    keybd_event, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, VK_MEDIA_NEXT_TRACK,
    VK_MEDIA_PLAY_PAUSE, VK_MEDIA_PREV_TRACK,
};

#[derive(Debug, Clone)]
pub struct MediaTrack {
    pub title: String,
    pub artist: String,
    pub is_playing: bool,
    pub position_ms: u64,
    pub duration_ms: u64,
    pub source_app: String,
}

impl Default for MediaTrack {
    fn default() -> Self {
        Self {
            title: "Tidak ada media aktif".to_string(),
            artist: "Putar lagu di Spotify / Browser".to_string(),
            is_playing: false,
            position_ms: 0,
            duration_ms: 180_000,
            source_app: "Windows".to_string(),
        }
    }
}

pub type SharedMedia = Arc<Mutex<MediaTrack>>;

pub fn start_media_poller(shared: SharedMedia) {
    std::thread::spawn(move || {
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };

        let mut simulated_ms: u64 = 0;
        let mut last_real_update = std::time::Instant::now();

        loop {
            std::thread::sleep(Duration::from_millis(500));
            
            let fetch_res = poll_windows_media();
            let mut current = shared.lock().unwrap();

            match fetch_res {
                Ok(Some(track)) => {
                    *current = track;
                    simulated_ms = current.position_ms;
                    last_real_update = std::time::Instant::now();
                }
                Ok(None) => {
                    // Jika tidak ada session GSMTC, cek apakah dalam mode demo atau standby
                    if current.is_playing {
                        // Majukan posisi simulasi
                        let elapsed = last_real_update.elapsed().as_millis() as u64;
                        if elapsed > 0 {
                            simulated_ms += elapsed;
                            if current.duration_ms > 0 && simulated_ms > current.duration_ms {
                                simulated_ms = 0;
                            }
                            current.position_ms = simulated_ms;
                            last_real_update = std::time::Instant::now();
                        }
                    }
                }
                Err(_) => {}
            }
        }
    });
}

fn resolve_smtc_session(
    manager: &GlobalSystemMediaTransportControlsSessionManager,
) -> Option<GlobalSystemMediaTransportControlsSession> {
    // 1. Prioritaskan session yang sedang playing
    if let Ok(sessions) = manager.GetSessions() {
        for s in sessions.into_iter() {
            if let Ok(playback) = s.GetPlaybackInfo() {
                if let Ok(status) = playback.PlaybackStatus() {
                    if status == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing {
                        return Some(s);
                    }
                }
            }
        }
    }

    // 2. Jika tidak ada yang playing, ambil session yang tidak closed (misal paused)
    if let Ok(sessions) = manager.GetSessions() {
        for s in sessions.into_iter() {
            if let Ok(playback) = s.GetPlaybackInfo() {
                if let Ok(status) = playback.PlaybackStatus() {
                    if status != GlobalSystemMediaTransportControlsSessionPlaybackStatus::Closed {
                        return Some(s);
                    }
                }
            }
        }
    }

    // 3. Fallback ke GetCurrentSession bawaan Windows
    manager.GetCurrentSession().ok()
}

fn poll_windows_media() -> Result<Option<MediaTrack>, Box<dyn std::error::Error>> {
    let async_op = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()?;
    let manager = async_op.get()?;
    let session = match resolve_smtc_session(&manager) {
        Some(s) => s,
        None => return Ok(None),
    };

    let source_id = session.SourceAppUserModelId().unwrap_or_default().to_string();
    let app_name = if source_id.to_lowercase().contains("spotify") {
        "Spotify".to_string()
    } else if source_id.to_lowercase().contains("chrome") {
        "Google Chrome".to_string()
    } else if source_id.to_lowercase().contains("msedge") {
        "Microsoft Edge".to_string()
    } else if source_id.is_empty() {
        "Media Player".to_string()
    } else {
        source_id
    };

    let media_props = session.TryGetMediaPropertiesAsync()?.get()?;
    let title = media_props.Title()?.to_string();
    let artist = media_props.Artist()?.to_string();

    if title.trim().is_empty() {
        return Ok(None);
    }

    let playback_info = session.GetPlaybackInfo()?;
    let is_playing = playback_info.PlaybackStatus()? == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing;

    let mut position_ms = 0;
    let mut duration_ms = 0;
    if let Ok(timeline) = session.GetTimelineProperties() {
        if let Ok(pos) = timeline.Position() {
            position_ms = (pos.Duration / 10_000) as u64; // 100ns units to ms
        }
        if let Ok(end) = timeline.EndTime() {
            duration_ms = (end.Duration / 10_000) as u64;
        }
    }

    Ok(Some(MediaTrack {
        title,
        artist,
        is_playing,
        position_ms,
        duration_ms,
        source_app: app_name,
    }))
}

pub fn send_media_command(action: &str) {
    let action = action.to_string();
    std::thread::spawn(move || {
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };

        let mut smtc_success = false;

        if let Ok(async_op) = GlobalSystemMediaTransportControlsSessionManager::RequestAsync() {
            if let Ok(manager) = async_op.get() {
                if let Some(session) = resolve_smtc_session(&manager) {
                    let res = match action.as_str() {
                        "toggle" => session.TryTogglePlayPauseAsync().and_then(|op| op.get()),
                        "play" => session.TryPlayAsync().and_then(|op| op.get()),
                        "pause" => session.TryPauseAsync().and_then(|op| op.get()),
                        "next" => session.TrySkipNextAsync().and_then(|op| op.get()),
                        "prev" => session.TrySkipPreviousAsync().and_then(|op| op.get()),
                        _ => Ok(false),
                    };
                    if let Ok(true) = res {
                        smtc_success = true;
                    }
                }
            }
        }

        // Fallback Win32 Virtual Media Key jika SMTC tidak berhasil
        if !smtc_success {
            let vk = match action.as_str() {
                "toggle" | "play" | "pause" => Some(VK_MEDIA_PLAY_PAUSE),
                "next" => Some(VK_MEDIA_NEXT_TRACK),
                "prev" => Some(VK_MEDIA_PREV_TRACK),
                _ => None,
            };

            if let Some(vk_code) = vk {
                unsafe {
                    keybd_event(vk_code.0 as u8, 0, KEYBD_EVENT_FLAGS(0), 0);
                    keybd_event(vk_code.0 as u8, 0, KEYEVENTF_KEYUP, 0);
                }
            }
        }
    });
}

pub fn seek_media_position(position_ms: u64) {
    std::thread::spawn(move || {
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if let Ok(async_op) = GlobalSystemMediaTransportControlsSessionManager::RequestAsync() {
            if let Ok(manager) = async_op.get() {
                if let Some(session) = resolve_smtc_session(&manager) {
                    let requested_100ns = (position_ms as i64) * 10_000;
                    let _ = session.TryChangePlaybackPositionAsync(requested_100ns);
                }
            }
        }
    });
}
