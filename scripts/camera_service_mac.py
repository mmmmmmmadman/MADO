#!/usr/bin/env python3
"""MADO 相機 service（macOS / AVFoundation backend）。

複製自 VisionMod core/scripts/camera_service_mac.py（精簡為 MADO 單一視窗用途）。
IPC：寫 `<output_dir>/frame.bin`，48B BinHeaderV1 + RGBA payload。
停止：`<output_dir>/stop.txt` 出現，或 SIGTERM。

CLI：
    --camera_unique_id <UUID>   優先（AVCaptureDevice.uniqueID，穩定識別）
    --camera_index <N>          fallback
    --output_dir <path>
    --frame_width / --frame_height / --fps
"""

import argparse
import os
import signal
import struct
import sys
import tempfile
import threading
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from frame_mmap import InPlaceFrameWriter  # noqa: E402

_REQUIRED_PACKAGES = [
    ("objc", "pyobjc-core"),
    ("AVFoundation", "pyobjc-framework-AVFoundation"),
    ("CoreMedia", "pyobjc-framework-CoreMedia"),
    ("Quartz", "pyobjc-framework-Quartz"),
    ("numpy", "numpy"),
]
_missing = []
for _modname, _pkgname in _REQUIRED_PACKAGES:
    try:
        __import__(_modname)
    except ImportError:
        _missing.append(_pkgname)
if _missing:
    print(f"[mado_camera] FATAL missing: {', '.join(_missing)}", file=sys.stderr, flush=True)
    print(f"[mado_camera] pip install {' '.join(_missing)}", file=sys.stderr, flush=True)
    sys.exit(2)

import numpy as np
import objc
from AVFoundation import (
    AVCaptureSession,
    AVCaptureDevice,
    AVCaptureDeviceInput,
    AVCaptureVideoDataOutput,
    AVMediaTypeVideo,
    AVCaptureSessionPreset1280x720,
    AVCaptureSessionPreset1920x1080,
    AVCaptureSessionPreset640x480,
)
from CoreMedia import CMSampleBufferGetImageBuffer, CMTimeMake
from Quartz import (
    CVPixelBufferLockBaseAddress,
    CVPixelBufferUnlockBaseAddress,
    CVPixelBufferGetBaseAddress,
    CVPixelBufferGetBytesPerRow,
    CVPixelBufferGetWidth,
    CVPixelBufferGetHeight,
    CVPixelBufferGetDataSize,
    CVPixelBufferRetain,
    CVPixelBufferRelease,
    kCVPixelFormatType_32BGRA,
)
import contextlib
from Foundation import NSObject
import ctypes
import ctypes.util

_libdispatch = ctypes.CDLL(ctypes.util.find_library("System"))
_dispatch_queue_create = _libdispatch.dispatch_queue_create
_dispatch_queue_create.restype = ctypes.c_void_p
_dispatch_queue_create.argtypes = [ctypes.c_char_p, ctypes.c_void_p]

BIN_MAGIC = 0x564D_4442
BIN_VERSION = 1
BIN_HEADER_SIZE = 48
BIN_HEADER_FMT = "<IIQQIIIIII"
PAYLOAD_FORMAT_RAW_PIXELS = 0
SOURCE_KIND_CAMERA_VIDEO_RGBA = 1
assert struct.calcsize(BIN_HEADER_FMT) == BIN_HEADER_SIZE

_running = True


def _log(msg: str) -> None:
    print(f"[mado_camera] {msg}", file=sys.stderr, flush=True)


def _handle_signal(signum, _frame):
    global _running
    _log(f"signal {signum}, stopping")
    _running = False


def _pack_header(frame_id: int, w: int, h: int) -> bytes:
    if hasattr(time, "clock_gettime_ns"):
        wall_ns = time.clock_gettime_ns(time.CLOCK_REALTIME)
    else:
        wall_ns = int(time.time() * 1_000_000_000)
    payload_count = w * h * 4
    return struct.pack(
        BIN_HEADER_FMT,
        BIN_MAGIC,
        BIN_VERSION,
        frame_id & 0xFFFFFFFFFFFFFFFF,
        wall_ns & 0xFFFFFFFFFFFFFFFF,
        SOURCE_KIND_CAMERA_VIDEO_RGBA,
        PAYLOAD_FORMAT_RAW_PIXELS,
        payload_count & 0xFFFFFFFF,
        w & 0xFFFFFFFF,
        h & 0xFFFFFFFF,
        0,
    )


def _pick_session_preset(want_w: int, want_h: int) -> str:
    if want_w >= 1920 or want_h >= 1080:
        return AVCaptureSessionPreset1920x1080
    if want_w >= 1280 or want_h >= 720:
        return AVCaptureSessionPreset1280x720
    return AVCaptureSessionPreset640x480


class _FrameSlot:
    def __init__(self):
        self.lock = threading.Lock()
        self.rgba: bytes | None = None
        self.w: int = 0
        self.h: int = 0
        self.frame_id: int = 0
        self.last_written: int = 0


_slot = _FrameSlot()


class CameraDelegate(NSObject):
    pass


@contextlib.contextmanager
def _locked_pixel_buffer(pixel_buffer):
    CVPixelBufferLockBaseAddress(pixel_buffer, 0)
    CVPixelBufferRetain(pixel_buffer)
    try:
        yield pixel_buffer
    finally:
        CVPixelBufferRelease(pixel_buffer)
        CVPixelBufferUnlockBaseAddress(pixel_buffer, 0)


def _on_frame(self, _output, sample_buffer, _connection):
    pixel_buffer = CMSampleBufferGetImageBuffer(sample_buffer)
    if pixel_buffer is None:
        return
    with _locked_pixel_buffer(pixel_buffer):
        w = int(CVPixelBufferGetWidth(pixel_buffer))
        h = int(CVPixelBufferGetHeight(pixel_buffer))
        bpr = int(CVPixelBufferGetBytesPerRow(pixel_buffer))
        data_size = int(CVPixelBufferGetDataSize(pixel_buffer))
        safe_size = min(data_size, bpr * h)
        base = CVPixelBufferGetBaseAddress(pixel_buffer)
        arr = np.frombuffer(base.as_buffer(safe_size), dtype=np.uint8)
        if arr.size < bpr * h:
            return
        arr = arr[: bpr * h].reshape(h, bpr)
        bgra = arr[:, : w * 4].reshape(h, w, 4)
        rgba = bgra[:, :, [2, 1, 0, 3]]
        # 不做水平鏡像 — 翻轉由 MADO Rust 端 UV 控制（使用者按鈕）。
        rgba_bytes = np.ascontiguousarray(rgba).tobytes()

    with _slot.lock:
        _slot.rgba = rgba_bytes
        _slot.w = w
        _slot.h = h
        _slot.frame_id += 1


objc.classAddMethods(CameraDelegate, [
    objc.selector(
        _on_frame,
        selector=b"captureOutput:didOutputSampleBuffer:fromConnection:",
        signature=b"v@:@@@",
    ),
])


def writer_loop(output_dir: Path, log_every: int = 240):
    frame_path = output_dir / "frame.bin"
    writer = InPlaceFrameWriter(str(frame_path))
    last_logged = 0
    while _running:
        with _slot.lock:
            if _slot.frame_id == _slot.last_written or _slot.rgba is None:
                rgba = None
            else:
                rgba = _slot.rgba
                w = _slot.w
                h = _slot.h
                fid = _slot.frame_id
                _slot.last_written = fid
        if rgba is None:
            time.sleep(0.001)
            continue
        header = _pack_header(fid, w, h)
        try:
            writer.write(header, rgba, fid)
        except OSError as exc:
            _log(f"write fail: {exc}")
            continue
        if fid - last_logged >= log_every:
            _log(f"rgba {fid} {w}x{h}")
            last_logged = fid
    writer.close()


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--camera_index", type=int, default=0)
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

    signal.signal(signal.SIGINT, _handle_signal)
    if hasattr(signal, "SIGTERM"):
        signal.signal(signal.SIGTERM, _handle_signal)

    output_dir = Path(args.output_dir)
    output_dir.mkdir(parents=True, exist_ok=True)
    stop_path = output_dir / "stop.txt"
    try:
        stop_path.unlink()
    except OSError:
        pass

    device = None
    if args.camera_unique_id:
        device = AVCaptureDevice.deviceWithUniqueID_(args.camera_unique_id)
        if device is None:
            _log(f"FATAL: unique_id={args.camera_unique_id!r} not found")
            return 2
        _log(f"open {device.localizedName()} (uid)")
    else:
        devices = AVCaptureDevice.devicesWithMediaType_(AVMediaTypeVideo)
        if not devices or args.camera_index >= len(devices):
            _log(f"FATAL: index={args.camera_index} out of range ({len(devices) if devices else 0})")
            return 2
        device = devices[args.camera_index]
        _log(f"open {device.localizedName()} (idx={args.camera_index})")

    session = AVCaptureSession.alloc().init()
    session.setSessionPreset_(_pick_session_preset(args.frame_width, args.frame_height))

    device_input, err = AVCaptureDeviceInput.deviceInputWithDevice_error_(device, None)
    if device_input is None:
        _log(f"AVCaptureDeviceInput fail: {err}")
        return 1
    if not session.canAddInput_(device_input):
        _log("session reject input")
        return 1
    session.addInput_(device_input)

    output = AVCaptureVideoDataOutput.alloc().init()
    output.setAlwaysDiscardsLateVideoFrames_(True)
    output.setVideoSettings_({"PixelFormatType": kCVPixelFormatType_32BGRA})

    delegate = CameraDelegate.alloc().init()
    cb_queue_ptr = _dispatch_queue_create(b"mado.camera_service_mac", None)
    if not cb_queue_ptr:
        _log("FATAL: dispatch_queue_create")
        return 2
    cb_queue = objc.objc_object(c_void_p=cb_queue_ptr)
    output.setSampleBufferDelegate_queue_(delegate, cb_queue)

    if not session.canAddOutput_(output):
        _log("FATAL: session reject output")
        return 2
    session.addOutput_(output)

    try:
        if device.lockForConfiguration_(None):
            try:
                duration = CMTimeMake(1, max(1, int(args.fps)))
                device.setActiveVideoMinFrameDuration_(duration)
                device.setActiveVideoMaxFrameDuration_(duration)
            finally:
                device.unlockForConfiguration()
            _log(f"fps={args.fps}")
    except Exception as exc:
        _log(f"fps set fail: {exc}")

    session.startRunning()
    _log("AVCaptureSession started")

    writer = threading.Thread(target=writer_loop, args=(output_dir,), daemon=True)
    writer.start()

    while _running:
        if stop_path.exists():
            _log("stop.txt detected")
            break
        time.sleep(0.05)

    session.stopRunning()
    _log("stopped")
    return 0


if __name__ == "__main__":
    sys.exit(main())
