# Notch Island (Windows Dynamic Notch)

Aplikasi desktop Dynamic Notch native Windows 10/11 berbasis **Pure Rust** dan **egui/eframe** dengan integrasi langsung ke **Windows Media Transport Controls (GSMTC)** dan **LRCLIB Synced Lyrics**.

---

## 🎯 Spesifikasi & Keunggulan
- **Ukuran Binary Ringan**: ~6.5 MB (standalone executable, tanpa WebView2, Chromium, Electron, atau Tauri runtime).
- **Efisiensi Memori (RAM)**: ~25 - 35 MB saat berjalan aktif.
- **CPU Idle**: ~0.0% - 0.1% (hardware accelerated GPU rendering).
- **Direct WinRT API & Hardware Fallback**:
  - Membaca metadata dan timeline lagu langsung via Windows Runtime GSMTC Session Manager (`windows` crate), kompatibel dengan Spotify, Apple Music, Tidal, YouTube, Chrome, Edge, Firefox, dan VLC.
  - Dilengkapi Win32 hardware virtual key fallback (`keybd_event` untuk `VK_MEDIA_PLAY_PAUSE`, `VK_MEDIA_NEXT_TRACK`, `VK_MEDIA_PREV_TRACK`) saat SMTC async command diabaikan oleh pemutar target.
- **Interactive Seek Bar**: Scroll bar progres hijau interaktif dengan dukungan klik langsung maupun drag/scrub maju-mundur secara mulus via WinRT `TryChangePlaybackPositionAsync`.
- **LRCLIB Synced Lyrics Engine**:
  - Otomatis mencari dan menyinkronkan lirik baris-per-baris dengan lagu yang sedang diputar.
  - Multi-tier fallback pencarian (`/api/get` exact match $\to$ sanitasi judul $\to$ `/api/search` query) dengan custom User-Agent upstream.
- **Dual Display Mode**:
  - `━ Minimal Bar`: Menempel persis di tepi paling atas monitor (`y = 0.0`) dengan sudut atas rata (`radius 0`) dan sudut bawah melengkung halus, tanpa gap bezel. Menampilkan teks lirik lagu aktif secara horizontal.
  - `🏝 Floating Island`: Kapsul dinamis membulat (*pill shape*) di bagian atas tengah layar.

---

## 🚀 Panduan Menjalankan & Membangun

### 1. Menjalankan Binary
Jalankan file executable hasil rilis:
```cmd
.\target\release\notch-island.exe
```
Atau via batch script:
```cmd
.\run.bat
```

### 2. Membangun dari Source
Pastikan toolchain Rust (MSVC) sudah terpasang:
```cmd
# Validasi kode
cargo check

# Build release binary (teroptimasi)
cargo build --release
```

---

## 🐛 Bug & Keterbatasan yang Masih Ada (Known Issues)

1. **Sinkronisasi Multi-Player (Audio Focus Conflict)**:
   - Fallback `keybd_event` mengirimkan sinyal media secara broadcast pada tingkat OS Windows. Jika terdapat lebih dari satu sumber media aktif bersamaan (misalnya Spotify desktop dan video YouTube di Google Chrome), kontrol play/pause dapat mengarah ke pemutar yang terakhir kali memegang fokus audio Windows.
2. **Keterbatasan Seek API pada Player Tertentu**:
   - Fungsi scrub maju-mundur menggunakan WinRT `TryChangePlaybackPositionAsync`. Spotify desktop mendukung penuh, namun beberapa pemutar berbasis web/browser tertentu dapat menolak perintah perubahan posisi jika buffer stream belum selesai dimuat.
3. **Multi-Monitor DPI Hot-Plugging**:
   - Kalkulasi geometri monitor saat ini membaca monitor utama (`SM_CXSCREEN`). Jika window dipindahkan antar monitor dengan scaling DPI berbeda saat aplikasi berjalan, notch memerlukan restart/re-anchoring geometri agar posisinya kembali presisi di tengah layar.
4. **Volume Slider Hardware Belum Terintegrasi di UI**:
   - Kontrol volume audio internal sistem belum memiliki slider visual khusus di antarmuka notch (saat ini masih mengandalkan tombol fisik keyboard atau flyout volume bawaan Windows).
5. **Background Material Blur (Mica / Acrylic)**:
   - Kontainer notch saat ini menggunakan warna solid semi-gelap (`#0A0C10`) dengan border tipis transparan; efek blur kaca DWM Acrylic/Mica di belakang window belum diaktifkan.

---

## 🔮 Rencana Perbaikan Versi Berikutnya (Next Version Roadmap)

- [ ] **Realtime Audio FFT Spectrum (WASAPI Loopback Capture)**: Mengganti simulasi visualizer sinus dengan pembacaan frekuensi audio loopback PCM asli melalui API WASAPI Windows, sehingga equalizer berdenyut akurat mengikuti ritme lagu.
- [ ] **Spring-Physics Fluid Animation (Damped Harmonic Oscillator)**: Menerapkan kurva transisi elastis saat ukuran notch membesar (*expand*) dan mengecil (*collapse*) agar terasa mulus dan responsif.
- [ ] **Windows 11 DWM Mica / Acrylic Surface Blur**: Integrasi `SetWindowCompositionAttribute` / DWM API untuk menghasilkan efek frosted glass di latar belakang notch.
- [ ] **Persistent User Preferences**: Menyimpan preferensi mode tampilan (`Bar` / `Island`), posisi pinned, dan status lirik ke file konfigurasi lokal (`AppData/Roaming/notch-island/config.json`).
- [ ] **Hardware Volume & Device Switcher**: Slider volume master interaktif dan pemilih output audio (Speaker / Headphone).
- [ ] **Local LRC File Drag-and-Drop**: Kemampuan memasukkan file `.lrc` lokal untuk lagu yang tidak tersedia di database LRCLIB.

---

## 📄 Lisensi (MIT License)

Proyek ini dilisensikan di bawah ketentuan **MIT License**.

```text
MIT License

Copyright (c) 2026 Muhammad Adilaga

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```
