use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use serde::Deserialize;

#[derive(Debug, Clone)]
pub struct LyricLine {
    pub time_ms: u64,
    pub text: String,
}

#[derive(Default, Clone)]
pub struct LyricsState {
    pub current_key: String,
    pub lines: Vec<LyricLine>,
    pub is_loading: bool,
    pub error: Option<String>,
}

pub type SharedLyrics = Arc<Mutex<LyricsState>>;

#[derive(Deserialize, Debug, Clone)]
struct LrclibResponse {
    #[serde(rename = "syncedLyrics")]
    synced_lyrics: Option<String>,
    #[serde(rename = "plainLyrics")]
    plain_lyrics: Option<String>,
    #[allow(dead_code)]
    duration: Option<f64>,
}

pub fn parse_lrc(lrc_text: &str) -> Vec<LyricLine> {
    let mut lines = Vec::new();
    for line in lrc_text.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with('[') {
            continue;
        }
        if let Some(close_bracket) = trimmed.find(']') {
            let time_str = &trimmed[1..close_bracket];
            let text = trimmed[close_bracket + 1..].trim().to_string();
            
            // Format mm:ss.xx or mm:ss:xx
            let parts: Vec<&str> = time_str.split(':').collect();
            if parts.len() >= 2 {
                if let (Ok(min), Ok(sec)) = (parts[0].parse::<u64>(), parts[1].parse::<f64>()) {
                    let ms = min * 60_000 + (sec * 1000.0) as u64;
                    lines.push(LyricLine { time_ms: ms, text });
                }
            }
        }
    }
    lines.sort_by_key(|l| l.time_ms);
    lines
}

fn clean_track_title(title: &str) -> String {
    let mut s = title.trim();
    let suffixes = [
        " (feat.", " (Feat.", " (with ", " (With ",
        " (Official", " [Official", " (Lyric", " [Lyric",
        " (Visualizer", " [Visualizer", " (Audio)", " [Audio]",
        " (Remaster", " [Remaster", " - Remaster", " - Live",
        " - Single Version", " (Deluxe", " [Deluxe",
    ];
    for suf in suffixes {
        if let Some(pos) = s.to_lowercase().find(&suf.to_lowercase()) {
            s = &s[..pos];
        }
    }
    s.trim().to_string()
}

fn clean_artist_name(artist: &str) -> String {
    let mut s = artist.trim();
    if s.to_lowercase().ends_with(" - topic") {
        s = &s[..s.len() - 8];
    }
    if s.to_lowercase().ends_with("vevo") {
        s = &s[..s.len() - 4];
    }
    s.trim().to_string()
}

fn query_lrclib_url(url: &str) -> Result<Option<LrclibResponse>, String> {
    const UA: &str = "Gili/4.2.2 (https://github.com/faiz)";

    for attempt in 0..2 {
        if attempt > 0 {
            std::thread::sleep(Duration::from_millis(350));
        }

        let resp_result = ureq::get(url)
            .timeout(Duration::from_secs(5))
            .set("User-Agent", UA)
            .set("Accept", "application/json")
            .call();

        match resp_result {
            Ok(resp) => {
                if let Ok(data) = serde_json::from_reader::<_, LrclibResponse>(resp.into_reader()) {
                    return Ok(Some(data));
                } else {
                    return Ok(None);
                }
            }
            Err(ureq::Error::Status(503, _)) => {
                continue;
            }
            Err(ureq::Error::Status(404, _)) => {
                return Ok(None);
            }
            Err(e) => {
                if attempt == 1 {
                    return Err(e.to_string());
                }
            }
        }
    }
    Ok(None)
}

fn search_lrclib_query(query: &str) -> Result<Option<LrclibResponse>, String> {
    const UA: &str = "Gili/4.2.2 (https://github.com/faiz)";
    let url = format!("https://lrclib.net/api/search?q={}", urlencoding_simple(query));

    let resp_result = ureq::get(&url)
        .timeout(Duration::from_secs(5))
        .set("User-Agent", UA)
        .set("Accept", "application/json")
        .call();

    match resp_result {
        Ok(resp) => {
            if let Ok(items) = serde_json::from_reader::<_, Vec<LrclibResponse>>(resp.into_reader()) {
                if let Some(synced_item) = items.iter().find(|i| i.synced_lyrics.is_some()) {
                    return Ok(Some(synced_item.clone()));
                }
                if let Some(plain_item) = items.iter().find(|i| i.plain_lyrics.is_some()) {
                    return Ok(Some(plain_item.clone()));
                }
            }
            Ok(None)
        }
        Err(e) => Err(e.to_string()),
    }
}

pub fn fetch_lyrics_async(
    shared: SharedLyrics,
    cache: Arc<Mutex<HashMap<String, Vec<LyricLine>>>>,
    artist: String,
    title: String,
    duration_secs: u64,
) {
    let key = format!("{} - {}", artist.trim(), title.trim());
    {
        let mut state = shared.lock().unwrap();
        if state.current_key == key && (!state.lines.is_empty() || state.is_loading) {
            return;
        }
        state.current_key = key.clone();
        state.is_loading = true;
        state.error = None;
        state.lines.clear();
    }

    // Check cache first
    {
        let cached = cache.lock().unwrap();
        if let Some(lines) = cached.get(&key) {
            let mut state = shared.lock().unwrap();
            state.lines = lines.clone();
            state.is_loading = false;
            return;
        }
    }

    // Fetch from LRCLIB in background thread
    std::thread::spawn(move || {
        let clean_t = clean_track_title(&title);
        let clean_a = clean_artist_name(&artist);

        let enc_artist = urlencoding_simple(&clean_a);
        let enc_title = urlencoding_simple(&clean_t);

        // Tier 1: /api/get dengan durasi
        let mut target_res = if duration_secs > 0 {
            let url = format!(
                "https://lrclib.net/api/get?artist_name={}&track_name={}&duration={}",
                enc_artist, enc_title, duration_secs
            );
            query_lrclib_url(&url).ok().flatten()
        } else {
            None
        };

        // Tier 2: /api/get tanpa durasi jika Tier 1 nihil
        if target_res.is_none() {
            let url = format!(
                "https://lrclib.net/api/get?artist_name={}&track_name={}",
                enc_artist, enc_title
            );
            target_res = query_lrclib_url(&url).ok().flatten();
        }

        // Tier 3: /api/search?q= jika Tier 2 nihil
        if target_res.is_none() {
            let search_q = format!("{} {}", clean_a, clean_t);
            target_res = search_lrclib_query(&search_q).ok().flatten();
        }

        let mut state = shared.lock().unwrap();
        if state.current_key != key {
            return; // track changed during request
        }

        match target_res {
            Some(resp) => {
                if let Some(synced) = resp.synced_lyrics {
                    let parsed = parse_lrc(&synced);
                    if !parsed.is_empty() {
                        cache.lock().unwrap().insert(key, parsed.clone());
                        state.lines = parsed;
                        state.is_loading = false;
                        state.error = None;
                        return;
                    }
                }
                if let Some(plain) = resp.plain_lyrics {
                    let mut fallback = Vec::new();
                    for (i, line) in plain.lines().enumerate() {
                        fallback.push(LyricLine {
                            time_ms: i as u64 * 3000,
                            text: line.trim().to_string(),
                        });
                    }
                    state.lines = fallback;
                    state.is_loading = false;
                    state.error = None;
                } else {
                    state.error = Some("Lirik tidak tersedia di LRCLIB".to_string());
                    state.is_loading = false;
                }
            }
            None => {
                state.error = Some("Tidak dapat memuat lirik (LRCLIB)".to_string());
                state.is_loading = false;
            }
        }
    });
}

fn urlencoding_simple(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.' || b == b'~' {
            out.push(b as char);
        } else if b == b' ' {
            out.push('+');
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}
