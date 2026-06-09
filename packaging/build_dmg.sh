#!/bin/bash
# build_dmg.sh — MADO macOS DMG 打包腳本（自包含 Python runtime）
# 用法：cd /path/to/MADO && bash packaging/build_dmg.sh
#       SKIP_NOTARIZE=1 bash packaging/build_dmg.sh   # 先驗 .app/DMG，不上傳 notarize
#
# 與 VisionMod build_dmg.sh 的差異：
#   - MADO binary 無 Homebrew dylib 依賴 → 省略 dylib 收集 / rpath 修正
#   - 內嵌可重定位 Python（.venv）+ scripts（自包含可散佈）
#   - 不含 SD 模型、不含 FFmpeg、不含 audio-input entitlement
#   - 簽署涵蓋 .venv 內全部 .so/.dylib（inside-out）+ python/主 binary 加 entitlements
#   - 極簡 DMG（無自訂背景圖）
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

APP_NAME="MADO"
BINARY_NAME="mado"
BUNDLE_ID="com.madzine.mado"
VERSION="0.1.0"
DEVELOPER_ID="Developer ID Application: Pohsun Chung (W89J6VDBML)"
NOTARY_PROFILE="anyani-notary"

BINARY_SRC="$PROJECT_DIR/target/release/$BINARY_NAME"
ICNS_SRC="$PROJECT_DIR/packaging/AppIcon.icns"
INFO_PLIST_SRC="$PROJECT_DIR/packaging/Info.plist"
ENTS="$SCRIPT_DIR/entitlements.plist"
# 可重定位 Python 來源（py-app-standalone 產出，已驗證 import）。可由 env 覆寫。
VENV_SRC="${VENV_SRC:-/tmp/mado_py_standalone/cpython-3.14.4-macos-aarch64-none}"
SCRIPTS_SRC="$PROJECT_DIR/scripts"

APP_BUNDLE="$PROJECT_DIR/${APP_NAME}.app"
DMG_NAME="${APP_NAME}_v${VERSION}.dmg"
DMG_PATH="$PROJECT_DIR/$DMG_NAME"

log() { echo "[build_dmg] $*"; }
die() { echo "[build_dmg] ERROR: $*" >&2; exit 1; }

# ── 步驟 0：前置檢查 ──
log "=== MADO DMG 打包 v${VERSION} ==="
[[ -f "$BINARY_SRC" ]] || die "找不到 binary: $BINARY_SRC（先 cargo build --release）"
[[ -f "$ICNS_SRC" ]] || die "找不到 icns: $ICNS_SRC"
[[ -f "$INFO_PLIST_SRC" ]] || die "找不到 Info.plist: $INFO_PLIST_SRC"
[[ -f "$ENTS" ]] || die "找不到 entitlements: $ENTS"
[[ -x "$VENV_SRC/bin/python3.14" ]] || die "找不到可重定位 python: $VENV_SRC/bin/python3.14（先執行 py-app-standalone 建立）"
[[ -d "$SCRIPTS_SRC" ]] || die "找不到 scripts: $SCRIPTS_SRC"
security find-identity -v -p codesigning | grep -q "$DEVELOPER_ID" || die "找不到憑證: $DEVELOPER_ID"

# ── 步驟 1：清除 ──
log "--- 清除舊產出"
rm -rf "$APP_BUNDLE"; rm -f "$DMG_PATH"

# ── 步驟 2：建 .app 骨架 ──
log "--- 建 .app 骨架"
MACOS="$APP_BUNDLE/Contents/MacOS"
RES="$APP_BUNDLE/Contents/Resources"
mkdir -p "$MACOS" "$RES"
cp "$INFO_PLIST_SRC" "$APP_BUNDLE/Contents/Info.plist"
cp "$ICNS_SRC" "$RES/AppIcon.icns"
cp "$BINARY_SRC" "$MACOS/$BINARY_NAME"
chmod +x "$MACOS/$BINARY_NAME"

# ── 步驟 3：內嵌可重定位 Python 為 .venv（放 Resources/，不可放 MacOS/）──
# 資料/runtime 必須在 Contents/Resources/，否則 codesign 簽 .app 時把 script
# 當 code subcomponent → 簽署失敗。Rust detect_python / resolve_script_path
# 已加 ../Resources/ 候選路徑配合此佈局。
log "--- 內嵌 Python runtime（Resources/.venv）"
cp -R "$VENV_SRC" "$RES/.venv"
ln -sf "python3.14" "$RES/.venv/bin/python3"
ln -sf "python3.14" "$RES/.venv/bin/python"

# ── 步驟 4：複製 scripts 到 Resources/ ──
log "--- 複製 scripts（Resources/scripts）"
cp -R "$SCRIPTS_SRC" "$RES/scripts"

# ── 步驟 5：簽署（inside-out）──
CS_LIB=(--force --timestamp --options runtime --sign "$DEVELOPER_ID")
CS_ENT=(--force --timestamp --options runtime --entitlements "$ENTS" --sign "$DEVELOPER_ID")

log "--- 簽署 .venv 內所有 .so/.dylib（批次）"
find "$RES/.venv" -type f \( -name "*.so" -o -name "*.dylib" \) -print0 \
    | xargs -0 -n 20 codesign "${CS_LIB[@]}" >/dev/null 2>&1 || die ".so/.dylib 簽署失敗"

# site-packages 內無副檔名的可執行 Mach-O 也必須簽。
log "--- 簽署 .venv 內其餘可執行 Mach-O"
while IFS= read -r -d '' f; do
    if file "$f" | grep -q "Mach-O"; then
        codesign "${CS_LIB[@]}" "$f" >/dev/null 2>&1 || die "簽署失敗: $f"
    fi
done < <(find "$RES/.venv" -type f -perm +111 ! -name "*.so" ! -name "*.dylib" -print0)

log "--- 簽署 .venv/bin 內 Mach-O 執行檔（python 帶 entitlements）"
for f in "$RES/.venv/bin/"*; do
    [[ -f "$f" ]] || continue
    if file "$f" | grep -q "Mach-O"; then
        codesign "${CS_ENT[@]}" "$f" >/dev/null 2>&1 || die "簽署失敗: $f"
    fi
done

log "--- 簽署主 binary（帶 entitlements）"
codesign "${CS_ENT[@]}" "$MACOS/$BINARY_NAME"

log "--- 簽署 .app bundle（帶 entitlements）"
codesign "${CS_ENT[@]}" "$APP_BUNDLE"

log "--- 驗證簽署"
codesign --verify --strict --verbose=2 "$APP_BUNDLE" || die "codesign verify 失敗"
spctl --assess --verbose=2 --type execute "$APP_BUNDLE" || \
    log "  警告: spctl 評估失敗（notarization 完成前正常）"

# ── 步驟 6：建 DMG（極簡，無自訂背景）──
log "--- 建 DMG"
VOLUME_NAME="${APP_NAME} v${VERSION}"
TMPDIR_DMG="$(mktemp -d)"
TMP_DMG="${PROJECT_DIR}/${APP_NAME}_v${VERSION}_rw.dmg"

cp -R "$APP_BUNDLE" "$TMPDIR_DMG/"
ln -s /Applications "$TMPDIR_DMG/Applications"
cp "$ICNS_SRC" "$TMPDIR_DMG/.VolumeIcon.icns"

log "  建讀寫 DMG"
hdiutil create -srcfolder "${TMPDIR_DMG}" -volname "${VOLUME_NAME}" \
    -fs HFS+ -format UDRW -size 500m "${TMP_DMG}"

log "  掛載並設外觀"
MOUNT_OUTPUT=$(hdiutil attach -readwrite -noverify -noautoopen "${TMP_DMG}")
DEVICE=$(echo "${MOUNT_OUTPUT}" | head -1 | awk '{print $1}')
MOUNT_POINT="/Volumes/${VOLUME_NAME}"
sleep 2
SetFile -c icnC "${MOUNT_POINT}/.VolumeIcon.icns" 2>/dev/null || true
SetFile -a C "${MOUNT_POINT}" 2>/dev/null || true

osascript <<APPLESCRIPT
tell application "Finder"
    tell disk "${VOLUME_NAME}"
        open
        delay 1
        set current view of container window to icon view
        set toolbar visible of container window to false
        set statusbar visible of container window to false
        set the bounds of container window to {100, 150, 740, 510}
        set theViewOptions to icon view options of container window
        set arrangement of theViewOptions to not arranged
        set icon size of theViewOptions to 96
        set text size of theViewOptions to 14
        set position of item "${APP_NAME}.app" of container window to {160, 180}
        set position of item "Applications" of container window to {480, 180}
        close
        open
        delay 1
        close
    end tell
end tell
APPLESCRIPT

sync
hdiutil detach "${MOUNT_POINT}" || hdiutil detach "${DEVICE}"

log "  壓縮為最終 DMG"
hdiutil convert "${TMP_DMG}" -format UDZO -imagekey zlib-level=9 -o "${DMG_PATH}"
rm -rf "${TMPDIR_DMG}"; rm -f "${TMP_DMG}"
hdiutil verify "$DMG_PATH"
log "  DMG: $DMG_PATH"

# ── 步驟 7：Notarization ──
if [[ "${SKIP_NOTARIZE:-0}" == "1" ]]; then
    log "SKIP_NOTARIZE=1 → 跳過 notarize/staple（DMG 已建，供本機驗證）"
    log "=== 完成（未 notarize）: $DMG_PATH ($(ls -lh "$DMG_PATH" | awk '{print $5}')) ==="
    exit 0
fi

log "--- 提交 Notarization"
xcrun notarytool submit "$DMG_PATH" --keychain-profile "$NOTARY_PROFILE" --wait
log "--- Staple"
xcrun stapler staple "$DMG_PATH"
xcrun stapler validate "$DMG_PATH"

# 清掉 repo 根目錄的 staging .app（DMG 內已含 .app）
if [ -d "$APP_BUNDLE" ]; then
    rm -rf "$APP_BUNDLE"
    log "--- 已清 staging .app: $APP_BUNDLE"
fi

log "=== 完成: $DMG_PATH ($(ls -lh "$DMG_PATH" | awk '{print $5}')) ==="
