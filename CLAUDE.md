# MADO

**版本**：0.1.0（2026-06-08）
**著作權**：MADZINE（鐘柏勳 / Pohsun Chung）
**位置**：`/Users/madzine/MADZINE/Documents/FeatureRef/MADO/`

---

## 定位

macOS 專用無邊框 USB camera 預覽小工具。

| 功能 | 實作 |
|------|------|
| 無邊框視窗 | `ViewportBuilder::with_decorations(false)` |
| 上下/左右翻轉 | overlay 按鈕 → UV flip（GPU 端） |
| 永遠置頂 | `ViewportCommand::WindowLevel(AlwaysOnTop/Normal)` toggle |
| 雙擊全螢幕 | `response.double_clicked()` → `ViewportCommand::Fullscreen` |
| Hover overlay | 滑鼠進視窗才出現控制列（相機 ComboBox / Flip H / Flip V / Always On Top / Close），移出立即消失 |
| 相機切換 | ComboBox（硬體裝置例外允許下拉），spcamera_unique-id 識別 |
| 全畫面拖曳 | `response.drag_started()` → `ViewportCommand::StartDrag` |
| 右下角 resize | 16×16 hit-test → `BeginResize(SouthEast)` |

---

## 技術棧

- **Rust** + **egui/eframe 0.31** + **wgpu 0.31**（與 VisionMod 對齊）
- **Python camera service**（venv，AVFoundation backend）
- **IPC**：48B BinHeaderV1 + `frame.bin`（macOS InPlaceFrameWriter，零 rename）
- macOS-only

---

## 來源對照（複製自 VisionMod）

| MADO 檔案 | VisionMod 來源 |
|---|---|
| `scripts/frame_mmap.py` | `core/scripts/frame_mmap.py`（byte-identical） |
| `scripts/camera_service_mac.py` | `core/scripts/camera_service_mac.py`（精簡） |
| `src/camera.rs::list_cameras` | `core/src/camera/capture.rs::list_cameras_macos` |
| `src/camera.rs::BinHeaderV1` | `core/src/ipc.rs::read_bin_header_v1` |
| `src/main.rs` 無邊框/StartDrag/雙擊全螢幕/BeginResize | `core/src/ui/output_window.rs`（2026-06-08 變更） |
| `src/main.rs::install_fonts` | `core/src/ui/theme.rs::install_fonts`（Barlow + NotoTC + NotoJP） |
| `packaging/build_dmg.sh` | `packaging/build_dmg.sh`（精簡：移除 SD 模型 / FFmpeg / audio entitlement / DMG 背景圖） |
| `packaging/entitlements.plist` | `packaging/entitlements.plist`（移除 audio-input） |

---

## 結構

```
MADO/
├── Cargo.toml
├── README.md
├── CLAUDE.md                       # 本檔
├── MADO_v0.1.0.dmg                 # 自包含安裝檔（52MB，notarized + stapled）
├── src/
│   ├── main.rs                     # MadoApp + 無邊框/拖曳/全螢幕/overlay/翻轉 + install_fonts
│   └── camera.rs                   # list_cameras + BinHeaderV1 reader + service spawn
├── scripts/
│   ├── camera_service_mac.py       # AVFoundation + delegate + spcamera_unique-id
│   └── frame_mmap.py               # InPlaceFrameWriter（byte-identical 自 VisionMod）
├── assets/
│   ├── fonts/                      # BarlowCondensed-Light + NotoSansTC-Light + NotoSansJP-Light
│   └── icons/                      # icon_1024.png + icon_256.png + gen_icon.py
├── packaging/
│   ├── AppIcon.icns                # 白底 squircle + 珊瑚粉圓環（STROKE=6@1024）+ 漢字「窓」
│   ├── Info.plist
│   ├── entitlements.plist
│   └── build_dmg.sh                # 自包含 DMG 打包腳本
└── .venv/                          # Python venv（pyobjc + numpy，散布時 py-app-standalone 重定位）
```

---

## Icon 設計

- **結構**：白 squircle（macOS Big Sur+ 22.37% 圓角）+ 珊瑚粉圓環 `#FF6C47`（STROKE=6 @ 1024，絕對 pixel）+ 圈內漢字「窓」（Noto Sans JP Light，同色）
- **品牌脈絡**：與 VisionMod icon 陰陽呼應（VisionMod 是橘環+粉字，MADO 是白底+粉環粉字）
- **MADZINE 強規則**：icon 必須含圓環（memory `feedback_icon_must_have_ring.md`）

---

## DMG 打包

| 項目 | 值 |
|---|---|
| 檔案 | `MADO_v0.1.0.dmg`（52MB） |
| Bundle | `MADO.app` + Applications 軟連結 |
| 內嵌 | `.venv/`（140MB，py-app-standalone 可重定位 Python）+ `scripts/` + `AppIcon.icns` |
| Bundle ID | `com.madzine.mado` |
| 簽署 | Developer ID Application: Pohsun Chung (W89J6VDBML)，hardened runtime |
| Entitlements | `cs.disable-library-validation` / `allow-jit` / `allow-unsigned-executable-memory` / `device.camera` |
| Notarize | Accepted（submission id `63f2cd47-fc25-4d62-bf33-e32bd1b58515`） |
| Staple | `stapler validate` worked |
| Gatekeeper | `spctl accepted, source=Notarized Developer ID` |
| DMG layout | 視窗 640×360 / icon size 96 / MADO.app `{160,180}` / Applications `{480,180}` |

vs VisionMod 3.6GB 精簡：移除 SD 模型 / FFmpeg / audio entitlement / DMG 背景圖；venv 只留 pyobjc + numpy。

---

## 開發過程教訓（已寫入 agent 定義）

### icon-design agent 失誤（已修補）

1. **MADZINE icon 必須有圓環**（最高優先）——首版設計成「白底 squircle + 純漢字」沒圓環被打回。Memory：`feedback_icon_must_have_ring.md`
2. **線寬必須用絕對 pixel**——首版環粗寫「畫布 6-8%」（@1024=72px）比 VisionMod 範本 `STROKE=6` 粗 12 倍。修正後對齊 VisionMod 細線美學
3. **通用規範 vs 專案規格衝突先列清單再執行**——agent 自帶「圓形/Avenir-Light/無圖形元素」與本案 squircle/Barlow/漢字衝突，正確做法是列衝突清單後直接執行專案規格

### dmg-builder agent 失誤（已修補）

1. **DMG osascript layout 不可省略**——首版「極簡 DMG」砍掉視窗 bounds + icon position，掛載後 layout 歪（icon 被切到視窗外）
2. **Notarize Accepted ≠ Staple 完成**——首版回報「staple validate worked」造假，notarize 過了但沒實際 `stapler staple`，離線會卡 Gatekeeper
3. **報告的驗證指令必須親自跑 + 貼真實輸出**——主 session 會二次驗證，造假會被抓
4. **新 DMG hash ≠ 舊 ticket**——任何重建 DMG 都要重 notarize 拿新 ticket

教訓已寫入 `~/.claude/agents/icon-design.md` 與 `~/.claude/agents/dmg-builder.md` 文末。

---

## 變更紀錄

### 2026-06-08 v0.1.0 — 首版

- Rust + egui + Python camera service 架構（複製 VisionMod 經驗）
- 無邊框視窗 + hover overlay + Flip H/V + Always On Top + 雙擊全螢幕 + StartDrag + 右下 resize handle
- 相機 ComboBox 切換（spcamera_unique-id 持久化識別）
- 三套字體 install_fonts（Barlow Latin + NotoTC 繁中 + NotoJP 日文，全文 CJK fallback）
- 白 squircle + 珊瑚粉圓環 + 漢字「窓」icon（與 VisionMod 陰陽呼應）
- 自包含 DMG 52MB（內嵌可重定位 venv + scripts）
- Notarized + Stapled，Gatekeeper accepted
