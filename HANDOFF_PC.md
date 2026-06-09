# MADO — PC 端引言

**Repo**：https://github.com/mmmmmmmadman/MADO  （private）
**著作權**：MADZINE（鐘柏勳 / Pohsun Chung）
**最後 macOS commit**：`7d52181` Initial commit（2026-06-09）

---

## 一句話定位

macOS / Windows 無邊框 USB camera 預覽 + 影片播放清單小工具。Rust + egui 0.31 + wgpu 24 + ffmpeg-next 8 + RtAudio。

---

## PC 端首次 setup

```powershell
# 1. clone
git clone https://github.com/mmmmmmmadman/MADO.git
cd MADO

# 2. 系統依賴（vcpkg）
$env:VCPKG_ROOT = "C:\vcpkg"   # 依實際路徑
vcpkg install rtaudio:x64-windows ffmpeg:x64-windows
$env:FFMPEG_DIR = "$env:VCPKG_ROOT\installed\x64-windows"

# 3. build
cargo build --release
```

`build.rs` 已 cfg 分流 macOS / Windows / Linux，PC 端會自動讀 `RTAUDIO_DIR` / `VCPKG_ROOT` 並 link `rtaudio.lib` + `dsound` / `ole32` / `winmm`。

---

## 跨平台狀態

| 模組 | macOS | Windows | 備註 |
|---|---|---|---|
| Camera service | ✅ AVFoundation Python | ❌ **待補** | 參考 `Commercial/VisionMod/core/scripts/camera_service.py`（cv2.VideoCapture），抄過來改成 MADO 單視窗用法 |
| Video decode | ✅ ffmpeg-next | ✅ ffmpeg-next | 跨平台共用 `src/video.rs`，不用改 |
| Audio decode | ✅ ffmpeg-next | ✅ ffmpeg-next | 同上，含 .mov lpcm channel_layout=0 fix |
| Audio output | ✅ RtAudio + CoreAudio | ✅ RtAudio + DirectSound | 跨平台共用 `src/audio_output.rs`，不用改 |
| UI / 字型 / 設定 | ✅ | ✅ | egui + 嵌入字型，全跨平台 |

---

## PC 端第一個 TODO

**`scripts/camera_service.py` Windows 版**（從 VisionMod `core/scripts/camera_service.py` 抄）：

1. 沿用相同的 48B `BinHeaderV1` + `frame.bin` IPC（macOS 與 Windows 共用 `frame_mmap.py`，Windows 走 `_safe_replace` 路徑、reserved=0；macOS 走 `InPlaceFrameWriter`、reserved=frame_id）
2. `src/camera.rs::spawn_camera_service` 已支援傳 `--camera_index`（Windows）+ `--camera_unique_id`（macOS），cfg 分流不動
3. Windows 沒有 spcamera_unique-id，`list_cameras` 走 cv2 index 即可

---

## 程式結構

```
MADO/
├── Cargo.toml              # eframe 0.31 / wgpu 24 / ffmpeg-next 8 / cpal 已退役
├── build.rs                # 編譯 rtaudio_wrapper.cpp + link rtaudio（cfg 分平台）
├── rtaudio_wrapper.cpp     # MAM 抄來的 C++ wrapper（master/secondary callback）
├── scripts/
│   ├── camera_service_mac.py   # macOS 用，PyObjC + AVFoundation
│   ├── camera_service.py       # ★ Windows 待補（cv2）
│   └── frame_mmap.py
├── src/
│   ├── main.rs             # MadoApp + overlay + settings panel + dropped_files
│   ├── camera.rs           # CameraInfo / BinHeaderV1 reader / spawn_camera_service / SharedFrame
│   ├── video.rs            # ffmpeg-next：video frame → SharedFrame，audio sample → AudioRing
│   ├── audio_output.rs     # RtAudio output stream + callback drain ring buffer
│   ├── audio.rs            # RtAudio output device 列舉
│   ├── rtaudio_ffi.rs      # MAM 抄來的 FFI
│   ├── playlist.rs         # PlaylistItem (path + flip_h/v) / LoopMode (Off/One/All)
│   └── settings.rs         # JSON 存檔（audio_device_name + volume）
└── assets/fonts/           # Barlow Condensed Bold + Noto Sans TC/JP Bold
```

---

## 鐵則（違反即重做）

1. **照抄成功案例**：Video / Audio pipeline 來自 Matrix AV Mapper（`OpenSource/MatrixAVMapper/`），動之前先 Read MAM `.claude/CLAUDE.md` 問題 1（cpal 漏裝置）+ 問題 4-5（callback-driven）+ 問題 7（lpcm channel_layout=0）+ 問題 8（DMG @rpath dylib bundle）
2. **camera IPC byte-identical**：`scripts/frame_mmap.py` 從 VisionMod byte-identical 抄來，跨平台分流邏輯不要改（macOS InPlaceFrameWriter + reserved=frame_id；Windows tempfile + os.replace + reserved=0）
3. **不重新發明音訊**：cpal 已退役（macOS 漏裝置），用 RtAudio。AVPlayer 已退役，用 ffmpeg-next + RtAudio 兩段式
4. **UI / 字型 / 配色**：互動元件 ≥16pt、Times New Roman 已退役、用 Barlow Condensed Bold + Noto Sans TC/JP Bold
5. **不 hardcode default**：double-click 歸零的 default 從 Settings::default() 取，不在 widget 內寫死

---

## macOS 端可運作的功能（PC 端目標：全部對齊）

- 拖曳影片到視窗 → 自動進影片模式 + append 到清單（不取代）
- 多檔一次拖、一支一支拖都會累加
- Toolbar（hover 顯示）：⚙ / Camera / Video / Loop Off-One-All / Playlist / ↔ / ↕ / Always On Top / Close
- 工具列空白處可拖視窗、雙擊全螢幕、右下 16×16 resize
- Playlist 面板：每列 ▲ ▼ Remove + per-clip ↔ ↕
- Space 暫停、← 從頭播
- Settings：操作說明 + Audio Output Device 下拉 + Volume 0-1 + Save / Revert
- Settings 存 `~/Library/Application Support/MADO/settings.json`（Windows 端要對應 `%APPDATA%\MADO\settings.json` — `src/settings.rs::config_dir` 已寫 macOS 路徑，PC 端要加 cfg 分流）

---

## 已知雷區（PC 端待踩）

1. `src/settings.rs::config_dir` 寫死 `~/Library/Application Support/MADO/` → Windows 端要改成 `dirs::config_dir() / "MADO"` 之類（加 `dirs` dep 或直接讀 `APPDATA` env）
2. `src/main.rs` `ViewportCommand::StartDrag` 與右下 BeginResize 在 Windows winit 0.30 可能行為不一致，要實測
3. PC 端音訊裝置 `output_channels` 對 WASAPI shared mode 是 2 即可，ASIO 走 `[ASIO] ` prefix（MAM 經驗）— 目前 MADO 只開 stereo 走 default API，PC 端先確認 WASAPI 路徑可用再說
4. ffmpeg-next 8 在 Windows 對 D3D11/DXVA2 硬解需要額外 cfg；先走 software decode 確認能跑

---

## 開機驗證指令

```powershell
cargo build --release
.\target\release\mado.exe
```

預期：camera 預覽不出來（PC 端 camera service 未補），但視窗可開、可拖視窗、`⚙` 可開設定、`Audio Output Device` 下拉應列出 PC 端 WASAPI 裝置（如 Realtek Speakers / 你的耳機）。拖 mp4 進去應能播且有聲。
