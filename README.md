# Notch Island (Windows Dynamic Notch)

Aplikasi desktop Dynamic Notch native Windows 10/11 berbasis **Pure Rust** dan **egui/eframe** dengan integrasi langsung ke **Windows Media Transport Controls (GSMTC)** dan **LRCLIB Synced Lyrics**.

---

## ⚡ Fitur Utama & Keunggulan

- **Ukuran Binary Ringan**: ~6.7 MB (standalone executable native MSVC, tanpa WebView2, Chromium, Electron, atau Tauri runtime).
- **Efisiensi Memori (RAM)**: ~25 - 35 MB saat berjalan aktif.
- **CPU Idle**: ~0.0% - 0.1% (hardware accelerated GPU rendering).
- **Nusa Parity & Perilaku Notch Adaptif**:
  - **Idle Camera Slit**: Berukuran ultra-ringkas (48x8 px) di bibir atas layar saat tidak ada media yang diputar, tembus klik (*click-through*) 100% ke aplikasi di bawahnya.
  - **Playing Minimal Pill**: Saat media berputar, notch berubah menjadi kapsul mini elegan (140x36 px) dengan indikator aktif, jam lokal, dan equalizer mini.
  - **Smooth Hover Expand**: Kursor mouse mendekati slit/pill otomatis memperluas notch menjadi kartu interaktif (460x195 px) dan menghapus sifat tembus klik.
  - **150ms Unhover Debounce**: Notch tidak langsung menutup seketika saat kursor bergeser sedikit, memberikan pengalaman navigasi yang halus.
  - **Pinning Lirik Interaktif**: Klik tombol **Lirik LRCLIB** mengunci notch tetap terbuka menampilkan panel lirik (580x300 px) meskipun kursor mouse berpindah ke luar notch. Saat tombol lirik dimatikan atau tombol **Ciutkan** ditekan, notch kembali ke mode auto-collapse saat kursor pergi.
- **Interactive Seek Bar & Clickable Lyrics Seek**:
  - Scrub bar progres interaktif dengan drag/klik maju-mundur via WinRT `TryChangePlaybackPositionAsync`.
  - Klik langsung baris teks lirik manapun untuk melompat (*seek*) ke timestamp lagu tersebut seketika.
- **Direct WinRT GSMTC & Hardware Fallback**:
  - Kompatibel dengan Spotify, Apple Music, Tidal, YouTube, Chrome, Edge, Firefox, dan VLC.
  - Fallback virtual key hardware Win32 (`keybd_event` untuk `VK_MEDIA_PLAY_PAUSE`, `VK_MEDIA_NEXT_TRACK`, `VK_MEDIA_PREV_TRACK`).
- **Hardware System Volume Slider**:
  - Pengatur volume master audio Windows langsung dari notch via interface Win32 Core Audio `IAudioEndpointVolume` (MMDeviceEnumerator).
- **System Tray & Taskbar Isolation**:
  - Berjalan murni di system tray icon Windows, disembunyikan sepenuhnya dari taskbar (`WS_EX_TOOLWINDOW` & non-`WS_EX_APPWINDOW`).
  - Context menu tray: Buka Notch, Toggle Autostart Windows, Cek Versi, dan Keluar.
- **Native WinRT Toast Notification**:
  - Notifikasi toast native Windows yang mengumumkan versi aplikasi (`Notch Island v0.1.0`) saat startup dan saat diklik dari menu tray.
- **Tab Produktivitas Tambahan (Header Carousel)**:
  - 🎵 **Musik**: Kontrol media GSMTC, seek bar, volume slider, dan tombol lirik.
  - ⏱️ **Pomodoro Timer**: Pengatur fokus kerja dan jeda istirahat dengan auto-transition serta counter sesi harian.
  - 💧 **Water Tracker**: Pengingat minum berkala dengan counter gelas harian yang otomatis di-reset setiap pergantian hari.
  - 📁 **File Shelf**: Rak file serbaguna pendukung drag-and-drop untuk akses cepat file kerja.
  - ✅ **Todo List**: Daftar tugas sederhana dan praktis.
- **Persistensi Konfigurasi**:
  - Seluruh pengaturan tersimpan otomatis di `%APPDATA%\notch-island\config.json`.

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

## ⚙️ Persyaratan Sistem

- **Sistem Operasi**: Windows 10 (versi 1809 / build 17763 ke atas) atau Windows 11.
- **Arsitektur**: x86_64.
- **Toolchain Kompilasi**: Rust 1.85+ (MSVC ABI) & Windows SDK.

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
