use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
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
#[allow(dead_code)]
pub type LyricsCache = Arc<Mutex<HashMap<String, Vec<LyricLine>>>>;

#[derive(Deserialize, Debug, Clone)]
pub struct LrclibResponse {
    #[serde(rename = "syncedLyrics")]
    pub synced_lyrics: Option<String>,
    #[serde(rename = "plainLyrics")]
    pub plain_lyrics: Option<String>,
    pub duration: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct Candidate {
    pub synced: Option<String>,
    pub duration_secs: f64,
}

pub fn pick_best(cands: &[Candidate], want_secs: f64) -> Option<usize> {
    cands
        .iter()
        .enumerate()
        .filter(|(_, c)| c.synced.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false))
        .map(|(i, c)| (i, (c.duration_secs - want_secs).abs()))
        .filter(|(_, d)| *d <= 3.0)
        .min_by(|x, y| x.1.partial_cmp(&y.1).unwrap())
        .map(|(i, _)| i)
}

pub fn accept_lyrics_result(current_key: &str, result_key: &str) -> bool {
    current_key == result_key
}

pub fn line_index(lines: &[LyricLine], pos_secs: f64) -> Option<usize> {
    let pos_ms = (pos_secs * 1000.0).max(0.0) as u64;
    lines.iter().rposition(|l| l.time_ms <= pos_ms)
}

#[derive(Debug, Clone)]
pub struct LyricSync {
    pub anchor_secs: f64,
    pub anchor_at: Instant,
    pub rate: f64,
    pub playing: bool,
    /// Geser manual (config.rs: lyric_offset_ms). Positif = lirik lebih cepat muncul.
    pub offset_ms: i64,
    pub seek_count: u64,
}

impl LyricSync {
    pub fn new(now: Instant) -> Self {
        Self {
            anchor_secs: 0.0,
            anchor_at: now,
            rate: 1.0,
            playing: false,
            offset_ms: 0,
            seek_count: 0,
        }
    }

    /// Panggil saat lagu berganti.
    pub fn reset(&mut self, now: Instant) {
        self.anchor_secs = 0.0;
        self.anchor_at = now;
        self.playing = false;
    }

    fn raw_secs(&self, now: Instant) -> f64 {
        if self.playing {
            self.anchor_secs + now.saturating_duration_since(self.anchor_at).as_secs_f64() * self.rate
        } else {
            self.anchor_secs
        }
    }

    /// Posisi untuk mencari baris lirik (sudah termasuk offset).
    pub fn position_secs(&self, now: Instant) -> f64 {
        (self.raw_secs(now) + self.offset_ms as f64 / 1000.0).max(0.0)
    }

    /// Dipanggil tiap poll/event timeline GSMTC.
    /// `reported_secs` = timeline.Position, `age` = now - timeline.LastUpdatedTime
    /// (jepit ke 0..10 dtk). Mengembalikan (jumped, drift).
    pub fn on_timeline(
        &mut self,
        reported_secs: f64,
        age: Duration,
        playing: bool,
        rate: f64,
        now: Instant,
    ) -> (bool, f64) {
        let age = age.min(Duration::from_secs(10));
        let real = if playing { reported_secs + age.as_secs_f64() * rate } else { reported_secs };
        let predicted = self.raw_secs(now);
        let drift = (real - predicted).abs();
        let jumped = drift > 1.5;
        if jumped {
            self.seek_count = self.seek_count.saturating_add(1);
        }
        // Drift kecil (< 0.25 dtk) diabaikan agar lirik tidak "bergetar".
        if jumped || drift > 0.25 || playing != self.playing {
            self.anchor_secs = real;
            self.anchor_at = now;
        }
        self.playing = playing;
        self.rate = if rate > 0.0 { rate } else { 1.0 };
        (jumped, drift)
    }
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

fn search_lrclib_query(query: &str, want_secs: f64) -> Result<Option<LrclibResponse>, String> {
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
                if want_secs > 0.0 {
                    let candidates: Vec<Candidate> = items
                        .iter()
                        .map(|i| Candidate {
                            synced: i.synced_lyrics.clone(),
                            duration_secs: i.duration.unwrap_or(0.0),
                        })
                        .collect();
                    if let Some(idx) = pick_best(&candidates, want_secs) {
                        return Ok(Some(items[idx].clone()));
                    } else {
                        // Duration mismatch > 3s atau tidak ada synced lyrics
                        return Ok(None);
                    }
                } else {
                    if let Some(synced_item) = items.iter().find(|i| i.synced_lyrics.is_some()) {
                        return Ok(Some(synced_item.clone()));
                    }
                    if let Some(plain_item) = items.iter().find(|i| i.plain_lyrics.is_some()) {
                        return Ok(Some(plain_item.clone()));
                    }
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
            if let Some(resp) = query_lrclib_url(&url).ok().flatten() {
                if let Some(dur) = resp.duration {
                    if duration_secs == 0 || (dur - duration_secs as f64).abs() <= 3.0 {
                        target_res = Some(resp);
                    }
                } else {
                    target_res = Some(resp);
                }
            }
        }

        // Tier 3: /api/search?q= jika Tier 2 nihil (dengan filter durasi <= 3 dtk)
        if target_res.is_none() {
            let search_q = format!("{} {}", clean_a, clean_t);
            target_res = search_lrclib_query(&search_q, duration_secs as f64).ok().flatten();
        }

        let mut state = shared.lock().unwrap();
        if !accept_lyrics_result(&state.current_key, &key) {
            return; // track changed during request, discard stale result
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
                    state.error = Some("Lirik tidak ditemukan".to_string());
                    state.is_loading = false;
                }
            }
            None => {
                state.error = Some("Lirik tidak ditemukan".to_string());
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn interpolates_while_playing() {
        let t0 = Instant::now();
        let mut s = LyricSync::new(t0);
        s.on_timeline(10.0, Duration::from_millis(0), true, 1.0, t0);
        let t1 = t0 + Duration::from_millis(500);
        let pos = s.position_secs(t1);
        assert!((pos - 10.5).abs() < 0.05, "pos = {pos}");
    }

    #[test]
    fn pause_freezes_position() {
        let t0 = Instant::now();
        let mut s = LyricSync::new(t0);
        s.on_timeline(10.0, Duration::from_millis(0), false, 1.0, t0);
        let t1 = t0 + Duration::from_secs(5);
        assert_eq!(s.position_secs(t1), 10.0);
    }

    #[test]
    fn detects_seek_jump() {
        let t0 = Instant::now();
        let mut s = LyricSync::new(t0);
        s.on_timeline(10.0, Duration::from_millis(0), true, 1.0, t0);
        let t1 = t0 + Duration::from_secs(1);
        // User lompat ke detik 50 (drift 39 dtk > 1.5)
        let (jumped, drift) = s.on_timeline(50.0, Duration::from_millis(0), true, 1.0, t1);
        assert!(jumped);
        assert!((drift - 39.0).abs() < 0.1);
        assert_eq!(s.position_secs(t1), 50.0);
    }

    #[test]
    fn small_drift_is_ignored() {
        let t0 = Instant::now();
        let mut s = LyricSync::new(t0);
        s.on_timeline(10.0, Duration::from_millis(0), true, 1.0, t0);
        let t1 = t0 + Duration::from_millis(500);
        // Poller lapor 10.6 padahal prediksi 10.5 (drift 0.1 < 0.25) -> jangan ubah anchor
        let (jumped, drift) = s.on_timeline(10.6, Duration::from_millis(0), true, 1.0, t1);
        assert!(!jumped);
        assert!((drift - 0.1).abs() < 0.01);
        // Anchor tetap di t0 (10.0) -> pada t1 posisi tetap 10.5
        assert!((s.position_secs(t1) - 10.5).abs() < 0.01);
    }

    #[test]
    fn offset_is_applied() {
        let t0 = Instant::now();
        let mut s = LyricSync::new(t0);
        s.on_timeline(10.0, Duration::from_millis(0), true, 1.0, t0);
        s.offset_ms = 500;
        assert_eq!(s.position_secs(t0), 10.5);
    }

    #[test]
    fn picks_closest_duration_with_synced() {
        let cands = vec![
            Candidate { synced: None, duration_secs: 200.0 }, // plain: diskip
            Candidate { synced: Some("a".into()), duration_secs: 208.0 }, // beda 8s > 3s: tolak
            Candidate { synced: Some("b".into()), duration_secs: 201.5 }, // beda 1.5s <= 3s: TERBAIK
            Candidate { synced: Some("c".into()), duration_secs: 198.0 }, // beda 2.0s: kalah dekat
        ];
        assert_eq!(pick_best(&cands, 200.0), Some(2));
    }

    #[test]
    fn late_result_is_rejected() {
        assert!(!accept_lyrics_result("Lagu A", "Lagu B"));
        assert!(accept_lyrics_result("Lagu A", "Lagu A"));
    }
}
