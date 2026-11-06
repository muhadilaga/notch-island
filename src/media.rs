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
    pub app_id: String,
    pub thumbnail: Option<Arc<egui::ColorImage>>,
    #[allow(dead_code)]
    pub shuffle: bool,
    #[allow(dead_code)]
    pub favorite: bool,
    pub timeline_pos_secs: f64,
    pub timeline_age_secs: f64,
    pub playback_rate: f64,
    pub update_seq: u64,
    pub has_session: bool,
}

impl MediaTrack {
    pub fn has_media(&self) -> bool {
        self.has_session && !self.title.trim().is_empty() && self.title != "Tidak ada media aktif"
    }
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
            app_id: String::new(),
            thumbnail: None,
            shuffle: false,
            favorite: false,
            timeline_pos_secs: 0.0,
            timeline_age_secs: 0.0,
            playback_rate: 1.0,
            update_seq: 0,
            has_session: false,
        }
    }
}

pub type SharedMedia = Arc<Mutex<MediaTrack>>;

pub fn start_media_poller(shared: SharedMedia) {
    std::thread::spawn(move || {
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };

        let mut simulated_ms: u64 = 0;
        let mut last_real_update = std::time::Instant::now();
        // Sesi terakhir diketahui ada (poll asli terakhir berhasil).
        // Interpolasi hanya boleh jalan saat ini true.
        let mut session_alive = false;
        let mut tick: u64 = 0;
        let mut seq: u64 = 0;
        let mut thumb_cache: std::collections::HashMap<String, Option<Arc<egui::ColorImage>>> =
            std::collections::HashMap::new();

        loop {
            // Tick 100 ms: poll asli tiap 5 tick (500 ms, seperti
            // semula); antar poll, posisi diinterpolasi halus hanya
            // saat sesi ada dan status Playing.
            std::thread::sleep(Duration::from_millis(100));
            tick += 1;
            let mut current = shared.lock().unwrap();

            if tick % 5 == 0 {
                match poll_windows_media(&mut thumb_cache) {
                    Ok(Some(mut track)) => {
                        seq = seq.wrapping_add(1);
                        track.update_seq = seq;
                        track.has_session = true;
                        *current = track;
                        simulated_ms = current.position_ms;
                        last_real_update = std::time::Instant::now();
                        session_alive = true;
                    }
                    Ok(None) => {
                        // Sesi HILANG: jangan simulasi jalan terus.
                        // Dulu cabang ini memajukan posisi selama
                        // state terakhir playing, sehingga bar musik
                        // jalan tanpa musik dan lirik ikut highlight.
                        // Sekarang: bekukan posisi, tandai berhenti.
                        session_alive = false;
                        current.is_playing = false;
                        current.has_session = false;
                        current.title = "Tidak ada media aktif".to_string();
                    }
                    Err(_) => {
                        // Poll gagal: jangan interpolasi buta.
                        session_alive = false;
                        current.has_session = false;
                    }
                }
            } else if session_alive && current.is_playing {
                let elapsed = last_real_update.elapsed().as_millis() as u64;
                if elapsed > 0 {
                    simulated_ms = simulated_ms.saturating_add(elapsed);
                    // Mentok di durasi, jangan wrap ke 0 (poll asli
                    // berikutnya mengoreksi bila lagu lanjut / ganti).
                    if current.duration_ms > 0 && simulated_ms > current.duration_ms {
                        simulated_ms = current.duration_ms;
                    }
                    current.position_ms = simulated_ms;
                    last_real_update = std::time::Instant::now();
                }
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

fn poll_windows_media(
    thumb_cache: &mut std::collections::HashMap<String, Option<Arc<egui::ColorImage>>>,
) -> Result<Option<MediaTrack>, Box<dyn std::error::Error>> {
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
        source_id.clone()
    };

    let media_props = session.TryGetMediaPropertiesAsync()?.get()?;
    let title = media_props.Title()?.to_string();
    let artist = media_props.Artist()?.to_string();

    if title.trim().is_empty() {
        return Ok(None);
    }

    let song_key = format!("{} - {}", artist.trim(), title.trim());
    let thumbnail = if let Some(cached) = thumb_cache.get(&song_key) {
        cached.clone()
    } else {
        let decoded = match media_props.Thumbnail() {
            Ok(thumb_ref) => match thumb_ref.OpenReadAsync() {
                Ok(op) => match op.get() {
                    Ok(stream) => match read_stream_bytes(&stream) {
                        Ok(bytes) if !bytes.is_empty() => {
                            decode_thumbnail(&bytes).map(Arc::new)
                        }
                        _ => None,
                    },
                    _ => None,
                },
                _ => None,
            },
            _ => None,
        };
        if thumb_cache.len() >= 32 {
            thumb_cache.clear();
        }
        thumb_cache.insert(song_key, decoded.clone());
        decoded
    };

    let playback_info = session.GetPlaybackInfo()?;
    let is_playing = playback_info.PlaybackStatus()? == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing;

    let mut playback_rate = 1.0;
    if let Ok(rate_ref) = playback_info.PlaybackRate() {
        if let Ok(rate_val) = rate_ref.Value() {
            if rate_val > 0.0 {
                playback_rate = rate_val;
            }
        }
    }

    let mut position_ms = 0;
    let mut duration_ms = 0;
    let mut timeline_pos_secs = 0.0;
    let mut timeline_age_secs = 0.0;
    if let Ok(timeline) = session.GetTimelineProperties() {
        if let Ok(pos) = timeline.Position() {
            position_ms = (pos.Duration / 10_000) as u64; // 100ns units to ms
            timeline_pos_secs = pos.Duration as f64 / 10_000_000.0;
        }
        if let Ok(end) = timeline.EndTime() {
            duration_ms = (end.Duration / 10_000) as u64;
        }
        if let Ok(lut) = timeline.LastUpdatedTime() {
            if lut.UniversalTime > 0 {
                let now_filetime = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| (d.as_nanos() / 100) as i64 + 116_444_736_000_000_000)
                    .unwrap_or(0);
                if now_filetime >= lut.UniversalTime {
                    timeline_age_secs =
                        ((now_filetime - lut.UniversalTime) as f64 / 10_000_000.0).clamp(0.0, 10.0);
                }
            }
        }
    }

    Ok(Some(MediaTrack {
        title,
        artist,
        is_playing,
        position_ms,
        duration_ms,
        source_app: app_name,
        app_id: source_id,
        thumbnail,
        shuffle: false,
        favorite: false,
        timeline_pos_secs,
        timeline_age_secs,
        playback_rate,
        update_seq: 0,
        has_session: true,
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

use windows::Storage::Streams::{DataReader, IRandomAccessStreamWithContentType};

fn read_stream_bytes(stream: &IRandomAccessStreamWithContentType) -> windows::core::Result<Vec<u8>> {
    let size = stream.Size()? as u32;
    if size == 0 {
        return Ok(Vec::new());
    }
    let reader = DataReader::CreateDataReader(stream)?;
    reader.LoadAsync(size)?.get()?;
    let mut bytes = vec![0u8; size as usize];
    reader.ReadBytes(&mut bytes)?;
    Ok(bytes)
}

fn decode_thumbnail(bytes: &[u8]) -> Option<egui::ColorImage> {
    let img = image::load_from_memory(bytes).ok()?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    Some(egui::ColorImage::from_rgba_unmultiplied(
        [w as usize, h as usize],
        rgba.as_raw(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_media_track_has_media() {
        let mut track = MediaTrack::default();
        assert!(!track.has_media());

        track.has_session = true;
        track.title = "Song Title".to_string();
        assert!(track.has_media());

        track.title = "Tidak ada media aktif".to_string();
        assert!(!track.has_media());

        track.title = "   ".to_string();
        assert!(!track.has_media());
    }

    #[test]
    fn test_stream_fn() {
        let _ = read_stream_bytes;
        let _ = decode_thumbnail;
    }

    #[test]
    fn test_decode_thumbnail() {
        assert!(decode_thumbnail(&[]).is_none());
        assert!(decode_thumbnail(b"invalid image bytes").is_none());

        let img = image::RgbaImage::new(2, 2);
        let mut bytes = std::io::Cursor::new(Vec::new());
        img.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        let decoded = decode_thumbnail(bytes.get_ref());
        assert!(decoded.is_some());
        let color_img = decoded.unwrap();
        assert_eq!(color_img.size, [2, 2]);
    }

    #[test]
    fn test_live_smtc_session_query() {
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        let mut thumb_cache = HashMap::new();
        match poll_windows_media(&mut thumb_cache) {
            Ok(Some(track)) => {
                println!(
                    "Live SMTC found: app_id='{}', source_app='{}', title='{}', artist='{}', pos={:.2}s, age={:.3}s, rate={:.2}, playing={}",
                    track.app_id, track.source_app, track.title, track.artist, track.timeline_pos_secs, track.timeline_age_secs, track.playback_rate, track.is_playing
                );
            }
            Ok(None) => {
                println!("Live SMTC: No active session or track empty");
            }
            Err(e) => {
                println!("Live SMTC err: {:?}", e);
            }
        }
    }

    #[test]
    fn test_compare_all_smtc_sessions() {
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if let Ok(op) = GlobalSystemMediaTransportControlsSessionManager::RequestAsync() {
            if let Ok(manager) = op.get() {
                if let Ok(sessions) = manager.GetSessions() {
                    for i in 0..sessions.Size().unwrap_or(0) {
                        if let Ok(session) = sessions.GetAt(i) {
                            let app_id = session.SourceAppUserModelId().unwrap_or_default().to_string();
                            let tl = session.GetTimelineProperties().ok();
                            let pos = tl.as_ref().and_then(|t| t.Position().ok()).map(|p| p.Duration).unwrap_or(0);
                            let lut = tl.as_ref().and_then(|t| t.LastUpdatedTime().ok()).map(|l| l.UniversalTime).unwrap_or(0);
                            let pi = session.GetPlaybackInfo().ok();
                            let status = pi.as_ref().and_then(|p| p.PlaybackStatus().ok());
                            let rate = pi.as_ref().and_then(|p| p.PlaybackRate().ok()).and_then(|r| r.Value().ok());
                            let props = session.TryGetMediaPropertiesAsync().ok().and_then(|op| op.get().ok());
                            let title = props.as_ref().map(|p| p.Title().unwrap_or_default().to_string()).unwrap_or_default();
                            println!("Session #{}: app_id='{}', title='{}', pos_ms={}, lut={}, status={:?}, rate={:?}", i, app_id, title, pos / 10_000, lut, status, rate);
                        }
                    }
                }
            }
        }
    }
}
