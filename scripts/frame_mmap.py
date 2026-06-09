#!/usr/bin/env python3
"""macOS frame IPC 共用寫入 helper（複製自 VisionMod core/scripts/frame_mmap.py）。

固定檔原地覆寫（in-place），消除 APFS 鎖競爭。header schema 對齊 VisionMod
BinHeaderV1（48B `<IIQQIIIIII>`），reserved 欄位寫成 frame_id 做 torn-read seq。
"""

import os
import struct

BIN_MAGIC = 0x564D_4442
BIN_VERSION = 1
BIN_HEADER_SIZE = 48
BIN_HEADER_FMT = "<IIQQIIIIII"
assert struct.calcsize(BIN_HEADER_FMT) == BIN_HEADER_SIZE


class InPlaceFrameWriter:
    __slots__ = ("path", "_fd", "_last_total_len")

    def __init__(self, path: str):
        self.path = str(path)
        self._fd = os.open(self.path, os.O_WRONLY | os.O_CREAT, 0o644)
        self._last_total_len = -1

    def _write_all(self, offset: int, data) -> None:
        os.lseek(self._fd, offset, os.SEEK_SET)
        view = memoryview(data)
        total = len(view)
        off = 0
        while off < total:
            n = os.write(self._fd, view[off:])
            if n <= 0:
                raise OSError("os.write returned 0")
            off += n

    def write(self, header: bytes, payload: bytes, frame_id: int) -> None:
        if len(header) != BIN_HEADER_SIZE:
            raise ValueError(f"header must be {BIN_HEADER_SIZE}B, got {len(header)}")
        seq = frame_id & 0xFFFFFFFF
        header = header[:44] + struct.pack("<I", seq)
        total = BIN_HEADER_SIZE + len(payload)
        if total != self._last_total_len:
            os.ftruncate(self._fd, total)
            self._last_total_len = total
        self._write_all(BIN_HEADER_SIZE, payload)
        self._write_all(0, header)

    def close(self) -> None:
        if self._fd is not None and self._fd >= 0:
            try:
                os.close(self._fd)
            finally:
                self._fd = -1

    def __del__(self):
        try:
            self.close()
        except Exception:
            pass
