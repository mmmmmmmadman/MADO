# MADO

macOS 專用無邊框 USB camera 預覽小程式。Rust + egui，相機透過 Python + AVFoundation service 餵 RGBA frame。

複製自 VisionMod 既有實作（不重新發明）：

- 無邊框視窗 + 雙擊全螢幕 + StartDrag + 右下 16×16 BeginResize
  → `VisionMod/core/src/ui/output_window.rs`
- 相機列舉（`system_profiler SPCameraDataType -json` + `spcamera_unique-id`）
  → `VisionMod/core/src/camera/capture.rs::list_cameras_macos`
- 48B BinHeaderV1 + macOS reserved=frame_id torn-read guard
  → `VisionMod/core/src/ipc.rs::read_bin_header_v1`
- camera service（AVCaptureSession + delegate + alwaysDiscardsLateVideoFrames）
  → `VisionMod/core/scripts/camera_service_mac.py`（精簡）
- frame_mmap.InPlaceFrameWriter（byte-identical）
  → `VisionMod/core/scripts/frame_mmap.py`

## 一次性 Python 環境

需要 PyObjC + numpy。建議在 `MADO/.venv/` 建獨立 venv：

```bash
cd /Users/madzine/MADZINE/Documents/FeatureRef/MADO
python3 -m venv .venv
.venv/bin/pip install --upgrade pip
.venv/bin/pip install numpy pyobjc-core pyobjc-framework-AVFoundation pyobjc-framework-CoreMedia pyobjc-framework-Quartz
```

Rust 端 `detect_python` 會優先用 `.venv/bin/python3`；若找不到 fallback 系統 `python3`（需自行確保套件已裝）。

## Build & Run

```bash
cd /Users/madzine/MADZINE/Documents/FeatureRef/MADO
cargo build --release
./target/release/mado
```

或 dev：`cargo run`。

## 操作

- 滑鼠移進視窗 → 上方半透明 overlay 控制列出現（相機 ComboBox / 左右翻 / 上下翻 / 永遠置頂 / 關閉）。
- 滑鼠移出 → overlay 立即消失。
- 視窗任意位置拖曳 → 移動 OS 視窗（egui `ViewportCommand::StartDrag`）。
- 雙擊 → 全螢幕 ↔ 視窗。
- 右下角 16×16 區域拖曳 → BeginResize(SouthEast)。
- 翻轉以 texture UV 反向實作（CPU buffer 不動，相機 service 不改）。

## 限制

- macOS-only。`#[cfg(target_os = "macos")]` 之外 `list_cameras` 只回 `Camera 0`、camera service 只在 macOS 有實作。
- 翻轉只影響顯示，不寫回 frame buffer。
- 相機切換靠停舊 service（寫 `stop.txt`）+ spawn 新 service；舊 process 釋放需 AVFoundation handle，失敗交給 spawn fail（無重試循環）。
