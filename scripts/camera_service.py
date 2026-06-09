#!/usr/bin/env python3
"""MADO 相機 service（Windows / OpenCV cv2.VideoCapture backend）。

複製自 VisionMod core/scripts/camera_service.py（FrameSlot latest-wins +
capture_loop + rgba_loop），精簡為 MADO 單一視窗用途，並對齊 MADO macOS 版
（camera_service_mac.py）的 IPC：

  - 不做水平鏡像（翻轉交給 MADO Rust 端 UV 控制，使用者按鈕）。
  - frame.bin header 48B BinHeaderV1 `<IIQQIIIIII`，source_kind=1（CameraVideoRgba）、
    payload_format=0（raw pixels）、reserved=0。
  - Windows 走 tempfile + os.replace（_safe_replace），reserved 恆 0；
    不用 InPlaceFrameWriter（那會寫 reserved=frame_id，被 Rust non-macos
    reader（ipc.rs reserved != 0 → reject）拒收）。

架構（frame source + consumer pattern）：
  - capture_thread — 持有唯一 cv2.VideoCapture，寫 latest_frame slot
  - rgba_thread    — 取 latest frame → BGR2RGBA（alpha=255）→ atomic-swap frame.bin
  - main thread    — signal handler + stop.txt 輪詢 + 監看 capture 存活

CLI：
    --camera_index <N>          Windows 用此開機（忽略 unique_id）
    --camera_unique_id <UUID>   接受但 Windows 不使用（cv2 無等價穩定識別）
    --output_dir <path>
    --frame_width / --frame_height / --fps

停止：output_dir/stop.txt 出現，或收到 SIGTERM / SIGINT。
開不了相機 → 印 stderr + exit code != 0（讓 Rust monitor 看得到）。
"""

import argparse
import json
import os
import signal
import struct
import sys
import tempfile
import threading
import time
from pathlib import Path

import cv2
import numpy as np

# ─── header 常數，與 src/camera.rs::BinHeaderV1 同步 ──────────────────
BIN_MAGIC = 0x564D_4442
BIN_VERSION = 1
SOURCE_KIND_CAMERA_VIDEO_RGBA = 1
PAYLOAD_FORMAT_RAW_PIXELS = 0
_HEADER_FMT = "<IIQQIIIIII"
assert struct.calcsize(_HEADER_FMT) == 48, "header 必須 48 bytes"

# ─── 停止協調：signal handler 只能裝在 main thread ────────────────────
_running = threading.Event()
_running.set()

# ─── error log rate-limit：write skip 爆量時每 N 次彙整一筆 ───────────
_ERR_LOG_INTERVAL = 300


class _RateLimitedLogger:
    """每 loop 一個 instance；首次必印，後續每 _ERR_LOG_INTERVAL 次合併。"""

    __slots__ = ("tag", "count", "last_logged")

    def __init__(self, tag):
        self.tag = tag
        self.count = 0
        self.last_logged = 0

    def log(self, err):
        self.count += 1
        if self.count == 1 or (self.count - self.last_logged) >= _ERR_LOG_INTERVAL:
            since = self.count - self.last_logged
            print(
                f"[mado_camera] {self.tag} write skipped {since}x"
                f" (total {self.count}); last error: {err!r}",
                file=sys.stderr,
                flush=True,
            )
            self.last_logged = self.count


def _handle_signal(signum, frame):
    _running.clear()


def _pack_header(source_kind, frame_id, payload_format, payload_count, w, h):
    """組 48-byte little-endian header，戳記 CLOCK_REALTIME，reserved=0。

    byte-layout 與 MADO macOS camera_service_mac.py::_pack_header 一致：
    magic / version=1 / frame_id / wall_ns / source_kind=1 / payload_format=0 /
    payload_count=w*h*4 / w / h / reserved=0。
    """
    if hasattr(time, "clock_gettime_ns"):
        wall_ns = time.clock_gettime_ns(time.CLOCK_REALTIME)
    else:
        wall_ns = int(time.time() * 1_000_000_000)
    return struct.pack(
        _HEADER_FMT,
        BIN_MAGIC,
        BIN_VERSION,
        frame_id & 0xFFFFFFFFFFFFFFFF,
        wall_ns & 0xFFFFFFFFFFFFFFFF,
        source_kind & 0xFFFFFFFF,
        payload_format & 0xFFFFFFFF,
        payload_count & 0xFFFFFFFF,
        w & 0xFFFFFFFF,
        h & 0xFFFFFFFF,
        0,
    )


def _safe_replace(src, dst, retries=5):
    """os.replace + Windows 檔鎖 retry。

    Windows 共享鎖（reader open frame.bin 時）會讓 os.replace 拋
    PermissionError；遞增 backoff retry，最後仍失敗就 unlink src 避免
    堆積 .tmp 殘骸，回傳 False 讓呼叫端 rate-limit log。
    """
    for i in range(retries):
        try:
            os.replace(src, dst)
            return True
        except (PermissionError, FileNotFoundError):
            if i < retries - 1:
                time.sleep(0.002 * (i + 1))
    try:
        os.replace(src, dst)
        return True
    except (PermissionError, FileNotFoundError):
        try:
            os.unlink(src)
        except OSError:
            pass
        return False


# =====================================================================
# Frame slot — capture thread 與 consumer 之間唯一交會點。consumer 以自己
# 步調 pull 最新 frame，無 queue、無 back-pressure（丟 frame 是預期行為）。
# =====================================================================

class FrameSlot:
    WAIT_FRAME = "frame"      # 收到第一張 frame
    WAIT_ERROR = "error"      # capture thread 提早設 error
    WAIT_TIMEOUT = "timeout"  # 兩者都沒發生

    def __init__(self):
        self._lock = threading.Lock()
        self._frame = None
        self._frame_id = 0
        self._dims = (0, 0)
        self._has_frame = threading.Event()
        self._error = threading.Event()

    def put(self, frame_bgr):
        with self._lock:
            self._frame = frame_bgr
            self._frame_id += 1
            h, w = frame_bgr.shape[:2]
            self._dims = (w, h)
        self._has_frame.set()

    def get(self, last_seen_id):
        """比 last_seen_id 新則回 (frame_copy, id, w, h)，否則 (None, id, w, h)。"""
        with self._lock:
            if self._frame is None or self._frame_id == last_seen_id:
                return None, self._frame_id, self._dims[0], self._dims[1]
            return (
                self._frame.copy(),
                self._frame_id,
                self._dims[0],
                self._dims[1],
            )

    def signal_error(self):
        self._error.set()

    def wait_first(self, timeout=5.0):
        """同時監看 _has_frame 與 _error，任一觸發即返回。

        回傳：WAIT_FRAME / WAIT_ERROR / WAIT_TIMEOUT 字串常數。
        """
        deadline = time.monotonic() + timeout
        poll_step = 0.05  # 50ms
        while True:
            if self._has_frame.wait(timeout=poll_step):
                return FrameSlot.WAIT_FRAME
            if self._error.is_set():
                return FrameSlot.WAIT_ERROR
            if time.monotonic() >= deadline:
                if self._has_frame.is_set():
                    return FrameSlot.WAIT_FRAME
                if self._error.is_set():
                    return FrameSlot.WAIT_ERROR
                return FrameSlot.WAIT_TIMEOUT


# =====================================================================
# Capture thread — 持有唯一 cv2.VideoCapture，寫 slot（不做水平鏡像）。
# =====================================================================

def capture_loop(camera_index, frame_width, frame_height, fps, slot, status_holder):
    # Windows：MSMF backend 在部分機器無法 by-index 開啟，改用 DSHOW
    # （DirectShow，Windows USB 相機通用）；失敗再 fallback 預設 backend。
    if sys.platform.startswith("win"):
        cap = cv2.VideoCapture(camera_index, cv2.CAP_DSHOW)
        if not cap.isOpened():
            try:
                cap.release()
            except Exception:
                pass
            cap = cv2.VideoCapture(camera_index)
    else:
        cap = cv2.VideoCapture(camera_index)
    if not cap.isOpened():
        print(
            f"[mado_camera] cannot open camera index {camera_index}",
            file=sys.stderr,
            flush=True,
        )
        status_holder["capture_error"] = True
        slot.signal_error()
        _running.clear()
        return

    # cap.set 請求解析度 / fps + buffersize=1（latest-wins）。
    cap.set(cv2.CAP_PROP_FRAME_WIDTH, frame_width)
    cap.set(cv2.CAP_PROP_FRAME_HEIGHT, frame_height)
    cap.set(cv2.CAP_PROP_FPS, fps)
    cap.set(cv2.CAP_PROP_BUFFERSIZE, 1)

    # cap.get 回填實際值並印警告 if mismatch（禁止 silent fallback）。
    actual_w = int(cap.get(cv2.CAP_PROP_FRAME_WIDTH))
    actual_h = int(cap.get(cv2.CAP_PROP_FRAME_HEIGHT))
    actual_fps = cap.get(cv2.CAP_PROP_FPS)
    if actual_w != frame_width or actual_h != frame_height:
        print(
            f"[mado_camera] 解析度不符：請求 {frame_width}x{frame_height}"
            f" → driver 實際 {actual_w}x{actual_h}",
            file=sys.stderr,
            flush=True,
        )
    if int(actual_fps) != fps:
        print(
            f"[mado_camera] fps 不符：請求 {fps} → driver 實際 {actual_fps}",
            file=sys.stderr,
            flush=True,
        )
    print(
        f"[mado_camera] opened camera {camera_index}"
        f" resolution={actual_w}x{actual_h} fps={actual_fps}",
        file=sys.stderr,
        flush=True,
    )

    # cap.read() 連續失敗上限：相機中途拔除時 read 會一直回 False，
    # 150 次 × 5ms ≈ 0.75s 容忍瞬時掉幀；超過視為相機失效。
    _MAX_READ_FAILURES = 150
    read_failures = 0

    try:
        while _running.is_set():
            ok, frame = cap.read()
            if not ok or frame is None:
                read_failures += 1
                if read_failures >= _MAX_READ_FAILURES:
                    print(
                        "[mado_camera] cap.read() 連續失敗"
                        f" {read_failures} 次，相機可能已拔除，capture 退出",
                        file=sys.stderr,
                        flush=True,
                    )
                    status_holder["capture_error"] = True
                    slot.signal_error()
                    _running.clear()
                    return
                time.sleep(0.005)
                continue
            read_failures = 0
            # 不做水平鏡像 — 翻轉由 MADO Rust 端 UV 控制（使用者按鈕）。
            slot.put(frame)
    finally:
        cap.release()
        print("[mado_camera] capture released", file=sys.stderr, flush=True)


# =====================================================================
# RGBA consumer thread — 取最新 camera frame → BGR2RGBA（alpha=255）→
# tempfile + os.replace atomic-swap frame.bin（reserved=0）。
# =====================================================================

def rgba_loop(slot, output_dir):
    frame_path = os.path.join(output_dir, "frame.bin")
    err_logger = _RateLimitedLogger("rgba")

    wait_result = slot.wait_first(timeout=5.0)
    if wait_result != FrameSlot.WAIT_FRAME:
        print(
            f"[mado_camera] rgba thread: wait_first={wait_result}, aborting",
            file=sys.stderr,
            flush=True,
        )
        return

    frame_id = 0
    last_seen_id = 0
    print("[mado_camera] rgba thread running", file=sys.stderr, flush=True)

    while _running.is_set():
        frame_bgr, new_id, _, _ = slot.get(last_seen_id)
        if frame_bgr is None:
            time.sleep(0.005)
            continue
        last_seen_id = new_id

        # BGR → RGBA（R 在前），alpha 全 255。不做翻轉。
        rgba = cv2.cvtColor(frame_bgr, cv2.COLOR_BGR2RGBA)
        h, w = rgba.shape[:2]

        # NamedTemporaryFile：每幀唯一 .tmp 檔名，rename 失敗也不污染下一輪。
        tmp_name = None
        try:
            with tempfile.NamedTemporaryFile(
                dir=output_dir, prefix="frame.", suffix=".tmp", delete=False
            ) as f:
                tmp_name = f.name
                f.write(
                    _pack_header(
                        SOURCE_KIND_CAMERA_VIDEO_RGBA,
                        frame_id,
                        PAYLOAD_FORMAT_RAW_PIXELS,
                        w * h * 4,
                        w,
                        h,
                    )
                )
                f.write(np.ascontiguousarray(rgba).tobytes())
            if not _safe_replace(tmp_name, frame_path):
                err_logger.log("safe_replace failed after retries")
                time.sleep(0.01)
                continue
        except OSError as e:
            err_logger.log(e)
            if tmp_name is not None:
                try:
                    os.unlink(tmp_name)
                except OSError:
                    pass
            time.sleep(0.01)
            continue

        frame_id += 1
        if frame_id % 120 == 0:
            print(
                f"[mado_camera] rgba frame {frame_id} {w}x{h}",
                file=sys.stderr,
                flush=True,
            )

    print("[mado_camera] rgba thread exited", file=sys.stderr, flush=True)


# =====================================================================
# Camera probe — 列舉模式：cv2.VideoCapture probe 0..N，輸出 JSON 到 stdout
# （照抄 VisionMod core/scripts/camera_service.py，Rust list_cameras_windows 解析端）
# =====================================================================

def _dshow_device_names():
    """Windows：用 pygrabber 讀 DirectShow 裝置 FriendlyName list。

    cv2.VideoCapture 在 Windows 拿不到 FriendlyName（只有 index），須改讀
    DirectShow。`FilterGraph().get_input_devices()` 回傳的 list 順序即
    DirectShow 列舉順序，與 `cv2.VideoCapture(i, cv2.CAP_DSHOW)` 的 index i
    對齊，故 names[i] 對應 cv2 index i。

    必須 try/except：pygrabber / comtypes 在部分環境（無 COM、driver 異常）
    可能拋例外，失敗一律回空 list，讓呼叫端 fallback `Camera {i}`，不可讓整個
    列舉掛掉。非 Windows 平台直接回空 list。
    """
    if not sys.platform.startswith("win"):
        return []
    try:
        from pygrabber.dshow_graph import FilterGraph

        return list(FilterGraph().get_input_devices())
    except Exception as e:
        print(
            f"[mado_camera] pygrabber device name 列舉失敗，fallback Camera N: {e!r}",
            file=sys.stderr,
            flush=True,
        )
        return []


def _probe_camera(index, backend):
    """Probe 單一 cv2 裝置 index：嘗試 open → 讀解析度 → release。
    回傳 dict（index/opened/width/height）或 None（無此裝置）。"""
    cap = cv2.VideoCapture(index, backend) if backend is not None else cv2.VideoCapture(index)
    try:
        if not cap.isOpened():
            return None
        # 讀 driver 回報的解析度（不嘗試讀 frame，避免拖慢列舉）。
        w = int(cap.get(cv2.CAP_PROP_FRAME_WIDTH))
        h = int(cap.get(cv2.CAP_PROP_FRAME_HEIGHT))
        return {"index": index, "opened": True, "width": w, "height": h}
    finally:
        # cv2 backend 不主動 release 會殘留 device handle，下一個 spawn 撞鎖。
        try:
            cap.release()
        except Exception:
            pass


def list_cameras_probe(max_probe=10):
    """probe cv2 裝置 index 0..max_probe-1，輸出 JSON list 到 stdout。

    Windows 用 cv2.CAP_DSHOW（與 capture_loop 一致；MSMF by-index 在部分機器
    無效）；DSHOW 失敗再 fallback 預設 backend。其他平台用預設 backend。

    名稱列舉（pygrabber DirectShow FriendlyName）與可開性（cv2 probe）兩者結合：
    pygrabber 取 names，cv2 probe 確認 opened；最終每台輸出
    {"index": i, "name": ..., "opened": bool}。name 有 pygrabber 名稱用
    names[i]，否則 fallback "Camera {i}"。
    輸出：[{"index": 0, "name": "ASUS FHD webcam", "opened": true}, ...]
    僅列「open 成功」的 index；連續 3 次 miss 視為列舉完，避免無限 probe。
    """
    if sys.platform.startswith("win"):
        backend = cv2.CAP_DSHOW
    else:
        backend = None

    # pygrabber DirectShow 名稱（整段失敗 → 空 list → 全部 fallback Camera N）。
    names = _dshow_device_names()

    def _name_for(i):
        if 0 <= i < len(names) and names[i]:
            return names[i]
        return f"Camera {i}"

    results = []
    consecutive_miss = 0
    # cv2 index 在 DSHOW 上可能稀疏（拔掉 index 0 後保留 1）。採「連續 miss 3 次
    # 就停」避免無限 probe，又允許 index 跳號（罕見但存在）。
    # probe 上限取 max_probe 與 names 長度的較大者，確保 pygrabber 列到的每台
    # 都會被 probe 到並帶上對應 name。
    probe_limit = max(max_probe, len(names))
    for i in range(probe_limit):
        info = _probe_camera(i, backend)
        # Windows DSHOW 失敗 → 再試預設 backend，兩種都不行才視為該 index 不可用。
        if info is None and sys.platform.startswith("win"):
            info = _probe_camera(i, None)
        if info is None:
            consecutive_miss += 1
            # 只在已超過 pygrabber 列舉範圍後才允許 early-break，避免
            # names 中間某台暫時開不了就提早中斷後面該列的裝置。
            if consecutive_miss >= 3 and i >= len(names):
                break
            continue
        consecutive_miss = 0
        info["name"] = _name_for(i)
        results.append(info)

    print(json.dumps(results), flush=True)
    return 0


# =====================================================================
# Main
# =====================================================================

def main():
    parser = argparse.ArgumentParser(description="MADO camera service (Windows cv2)")
    parser.add_argument(
        "--list_cameras",
        action="store_true",
        help="cv2 probe 列舉裝置並輸出 JSON 到 stdout，不啟動 capture loop",
    )
    parser.add_argument("--camera_index", type=int, default=0)
    # Windows 不使用 unique_id（cv2 無等價穩定識別），接受以對齊 Rust spawn 參數。
    parser.add_argument("--camera_unique_id", type=str, default="")
    parser.add_argument(
        "--output_dir",
        type=str,
        default=str(Path(tempfile.gettempdir()) / "mado" / "camera"),
    )
    parser.add_argument("--frame_width", type=int, default=1280)
    parser.add_argument("--frame_height", type=int, default=720)
    parser.add_argument("--fps", type=int, default=30)
    args = parser.parse_args()

    # 列舉模式：probe 後直接 exit 0，不啟動 capture loop。
    if args.list_cameras:
        return list_cameras_probe()

    # main thread 裝 signal handler。
    signal.signal(signal.SIGINT, _handle_signal)
    if hasattr(signal, "SIGTERM"):
        signal.signal(signal.SIGTERM, _handle_signal)

    os.makedirs(args.output_dir, exist_ok=True)
    stop_path = os.path.join(args.output_dir, "stop.txt")
    if os.path.exists(stop_path):
        try:
            os.remove(stop_path)
        except OSError:
            pass

    slot = FrameSlot()
    status_holder = {"capture_error": False}

    cap_thread = threading.Thread(
        target=capture_loop,
        args=(
            args.camera_index,
            args.frame_width,
            args.frame_height,
            args.fps,
            slot,
            status_holder,
        ),
        name="capture",
        daemon=True,
    )
    cap_thread.start()

    # wait_first 同時監看 frame + error event：cap.isOpened() 失敗時 capture
    # thread 立即 signal_error()，主 thread 立刻拿到 WAIT_ERROR 而非等滿 5s。
    wait_result = slot.wait_first(timeout=5.0)
    if wait_result == FrameSlot.WAIT_ERROR:
        print(
            "[mado_camera] capture failed to open camera",
            file=sys.stderr,
            flush=True,
        )
        _running.clear()
        return 3
    if wait_result == FrameSlot.WAIT_TIMEOUT:
        print(
            "[mado_camera] capture produced no frame within 5s",
            file=sys.stderr,
            flush=True,
        )
        _running.clear()
        return 3

    t_rgba = threading.Thread(
        target=rgba_loop,
        args=(slot, args.output_dir),
        name="rgba",
        daemon=True,
    )
    t_rgba.start()

    # main thread 輪詢 stop.txt + 監看 capture 存活。
    try:
        while _running.is_set():
            if os.path.exists(stop_path):
                print("[mado_camera] stop signal", file=sys.stderr, flush=True)
                _running.clear()
                break
            if not cap_thread.is_alive():
                _running.clear()
                break
            time.sleep(0.1)
    except KeyboardInterrupt:
        _running.clear()

    t_rgba.join(timeout=2.0)

    print("[mado_camera] shutdown complete", file=sys.stderr, flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
