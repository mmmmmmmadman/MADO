#!/bin/bash
# build_dmg.sh — MADO macOS DMG 打包腳本
# 用法：cd /path/to/MADO && bash packaging/build_dmg.sh
#       SKIP_NOTARIZE=1 bash packaging/build_dmg.sh   # 先驗 .app/DMG 不上傳
#
# 合併兩條成功案例：
#   - VisionMod packaging/build_dmg.sh：
#       py-app-standalone 可重定位 Python + inside-out 簽署 + DMG layout + notarize/staple
#   - Matrix AV Mapper packaging/mac/build_dmg.sh：
#       resolve_rpath() + bundle_dylib() 遞迴收 Homebrew dylib +
#       install_name_tool 改寫 @rpath → @executable_path/../Frameworks/
#     （MADO binary 連 librtaudio.7.dylib + libav*.dylib，target 機無 Homebrew，
#      必須打包到 .app/Contents/Frameworks/ 才能執行 — MAM CLAUDE.md 問題 8）
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

APP_NAME="MADO"
BINARY_NAME="mado"
BUNDLE_ID="com.madzine.mado"
VERSION="0.2.0"
DEVELOPER_ID="Developer ID Application: Pohsun Chung (W89J6VDBML)"
NOTARY_PROFILE="anyani-notary"

BINARY_SRC="$PROJECT_DIR/target/release/$BINARY_NAME"
ICNS_SRC="$PROJECT_DIR/packaging/AppIcon.icns"
INFO_PLIST_SRC="$PROJECT_DIR/packaging/Info.plist"
ENTS="$SCRIPT_DIR/entitlements.plist"
VENV_SRC="${VENV_SRC:-/private/tmp/mado_py_standalone/cpython-3.14-macos-aarch64-none}"
SCRIPTS_SRC="$PROJECT_DIR/scripts"

APP_BUNDLE="$PROJECT_DIR/${APP_NAME}.app"
DMG_NAME="MADO_v${VERSION}.dmg"
DMG_PATH="$PROJECT_DIR/$DMG_NAME"

HOMEBREW_LIB_PATHS=(
    "/opt/homebrew/lib"
    "/opt/homebrew/opt/ffmpeg/lib"
    "/opt/homebrew/opt/rtaudio/lib"
    "/usr/local/lib"
)

log() { echo "[build_dmg] $*"; }
die() { echo "[build_dmg] ERROR: $*" >&2; exit 1; }

log "=== MADO DMG 打包 v${VERSION} ==="
[[ -f "$BINARY_SRC" ]] || die "找不到 binary: $BINARY_SRC（先 cargo build --release）"
[[ -f "$ICNS_SRC" ]] || die "找不到 icns: $ICNS_SRC"
[[ -f "$INFO_PLIST_SRC" ]] || die "找不到 Info.plist: $INFO_PLIST_SRC"
[[ -f "$ENTS" ]] || die "找不到 entitlements: $ENTS"
[[ -x "$VENV_SRC/bin/python3.14" ]] || die "找不到可重定位 python: $VENV_SRC/bin/python3.14"
[[ -d "$SCRIPTS_SRC" ]] || die "找不到 scripts: $SCRIPTS_SRC"
security find-identity -v -p codesigning | grep -q "$DEVELOPER_ID" || die "找不到憑證: $DEVELOPER_ID"

log "--- 清除舊產出"
rm -rf "$APP_BUNDLE"; rm -f "$DMG_PATH"

log "--- 建 .app 骨架"
MACOS="$APP_BUNDLE/Contents/MacOS"
RES="$APP_BUNDLE/Contents/Resources"
FRAMEWORKS_DIR="$APP_BUNDLE/Contents/Frameworks"
mkdir -p "$MACOS" "$RES" "$FRAMEWORKS_DIR"
cp "$INFO_PLIST_SRC" "$APP_BUNDLE/Contents/Info.plist"
cp "$ICNS_SRC" "$RES/AppIcon.icns"
cp "$BINARY_SRC" "$MACOS/$BINARY_NAME"
chmod +x "$MACOS/$BINARY_NAME"
EXECUTABLE="$MACOS/$BINARY_NAME"

# ── resolve_rpath / bundle_dylib（MAM 模式）──
resolve_rpath() {
    local rpath_ref="$1"
    local lib_name
    lib_name=$(basename "$rpath_ref")
    for search_dir in "${HOMEBREW_LIB_PATHS[@]}"; do
        if [[ -f "${search_dir}/${lib_name}" ]]; then
            echo "${search_dir}/${lib_name}"
            return 0
        fi
    done
    return 1
}

bundle_dylib() {
    local lib_path="$1"
    local lib_name
    lib_name=$(basename "$lib_path")
    if [[ "$lib_path" == /System/* ]] || [[ "$lib_path" == /usr/lib/* ]]; then
        return
    fi
    if [[ -f "${FRAMEWORKS_DIR}/${lib_name}" ]]; then
        return
    fi
    local real_path
    real_path=$(realpath "$lib_path" 2>/dev/null || echo "$lib_path")
    [[ -f "$real_path" ]] || { log "  WARN: $lib_path 不存在，跳過"; return; }
    log "  bundle: $lib_name"
    cp "$real_path" "${FRAMEWORKS_DIR}/${lib_name}"
    chmod 755 "${FRAMEWORKS_DIR}/${lib_name}"
    install_name_tool -id "@executable_path/../Frameworks/${lib_name}" \
        "${FRAMEWORKS_DIR}/${lib_name}" 2>/dev/null || true

    otool -L "${FRAMEWORKS_DIR}/${lib_name}" | tail -n +2 | awk '{print $1}' | while read -r dep; do
        if [[ "$dep" == /System/* ]] || [[ "$dep" == /usr/lib/* ]]; then
            continue
        fi
        local dep_resolved="$dep"
        if [[ "$dep" == @rpath/* ]]; then
            dep_resolved=$(resolve_rpath "$dep") || { log "    WARN: 無法解 ${dep}"; continue; }
        elif [[ "$dep" == @executable_path/* ]] || [[ "$dep" == @loader_path/* ]]; then
            continue
        fi
        bundle_dylib "$dep_resolved"
        local dep_name
        dep_name=$(basename "$dep")
        install_name_tool -change "$dep" "@executable_path/../Frameworks/${dep_name}" \
            "${FRAMEWORKS_DIR}/${lib_name}" 2>/dev/null || true
    done
}

log "--- 收主 binary 的 Homebrew dylib 依賴（遞迴）"
otool -L "$EXECUTABLE" | tail -n +2 | awk '{print $1}' | while read -r lib; do
    if [[ "$lib" == /System/* ]] || [[ "$lib" == /usr/lib/* ]]; then
        continue
    fi
    local_lib="$lib"
    if [[ "$lib" == @rpath/* ]]; then
        local_lib=$(resolve_rpath "$lib") || { log "  WARN: @rpath 解不到 ${lib}"; continue; }
    fi
    bundle_dylib "$local_lib"
done

log "--- 改寫主 binary 的 dylib 參考 → @executable_path/../Frameworks/"
otool -L "$EXECUTABLE" | tail -n +2 | awk '{print $1}' | while read -r lib; do
    if [[ "$lib" == /System/* ]] || [[ "$lib" == /usr/lib/* ]]; then
        continue
    fi
    if [[ "$lib" == @executable_path/* ]] || [[ "$lib" == @loader_path/* ]]; then
        continue
    fi
    lib_name=$(basename "$lib")
    install_name_tool -change "$lib" "@executable_path/../Frameworks/${lib_name}" \
        "$EXECUTABLE" 2>/dev/null || true
done

log "--- 改寫 Frameworks/ 內 dylib 彼此的參考"
for fw in "${FRAMEWORKS_DIR}"/*.dylib; do
    [ -f "$fw" ] || continue
    otool -L "$fw" | tail -n +2 | awk '{print $1}' | while read -r dep; do
        if [[ "$dep" == /System/* ]] || [[ "$dep" == /usr/lib/* ]]; then continue; fi
        if [[ "$dep" == @executable_path/* ]]; then continue; fi
        dep_name=$(basename "$dep")
        if [[ -f "${FRAMEWORKS_DIR}/${dep_name}" ]]; then
            install_name_tool -change "$dep" "@executable_path/../Frameworks/${dep_name}" \
                "$fw" 2>/dev/null || true
        fi
    done
done

# ── 內嵌可重定位 Python ──
log "--- 內嵌 Python runtime（Resources/.venv）"
# -L follow symlinks：VENV_SRC 本身常為 symlink（uv 對 cpython-3.14 指向 cpython-3.14.4），
# 不 follow 會 bundle 出 symlink → codesign 拒「invalid destination for symbolic link in bundle」
cp -RL "$VENV_SRC" "$RES/.venv"
ln -sf "python3.14" "$RES/.venv/bin/python"

log "--- 複製 scripts（Resources/scripts）"
cp -R "$SCRIPTS_SRC" "$RES/scripts"

# ── 簽署（inside-out）──
CS_LIB=(--force --timestamp --options runtime --sign "$DEVELOPER_ID")
CS_ENT=(--force --timestamp --options runtime --entitlements "$ENTS" --sign "$DEVELOPER_ID")

log "--- 簽署 Frameworks/ 內所有 dylib"
find "$FRAMEWORKS_DIR" -type f -name "*.dylib" -print0 \
    | xargs -0 -n 20 codesign "${CS_LIB[@]}" >/dev/null 2>&1 || die "Frameworks dylib 簽署失敗"

log "--- 簽署 .venv 內所有 .so/.dylib（批次）"
find "$RES/.venv" -type f \( -name "*.so" -o -name "*.dylib" \) -print0 \
    | xargs -0 -n 20 codesign "${CS_LIB[@]}" >/dev/null 2>&1 || die ".venv .so/.dylib 簽署失敗"

log "--- 簽署 .venv 內其餘可執行 Mach-O"
while IFS= read -r -d '' f; do
    if file "$f" | grep -q "Mach-O"; then
        codesign "${CS_LIB[@]}" "$f" >/dev/null 2>&1 || die "簽署失敗: $f"
    fi
done < <(find "$RES/.venv" -type f -perm +111 ! -name "*.so" ! -name "*.dylib" -print0)

log "--- 簽署 .venv/bin Mach-O 執行檔（python 帶 entitlements）"
for f in "$RES/.venv/bin/"*; do
    [[ -f "$f" ]] || continue
    if file "$f" | grep -q "Mach-O"; then
        codesign "${CS_ENT[@]}" "$f" >/dev/null 2>&1 || die "簽署失敗: $f"
    fi
done

log "--- 簽署主 binary（帶 entitlements）"
codesign "${CS_ENT[@]}" "$EXECUTABLE"

log "--- 簽署 .app bundle（帶 entitlements）"
codesign "${CS_ENT[@]}" "$APP_BUNDLE"

log "--- 驗證簽署"
codesign --verify --strict --verbose=2 "$APP_BUNDLE" || die "codesign verify 失敗"
spctl --assess --verbose=2 --type execute "$APP_BUNDLE" || \
    log "  警告: spctl 評估失敗（notarization 完成前正常）"

# ── 建 DMG ──
log "--- 建 DMG"
VOLUME_NAME="${APP_NAME} v${VERSION}"
WIN_W=640; WIN_H=360
TMPDIR_DMG="$(mktemp -d)"
TMP_DMG="${PROJECT_DIR}/MADO_v${VERSION}_rw.dmg"

cp -R "$APP_BUNDLE" "$TMPDIR_DMG/"
ln -s /Applications "$TMPDIR_DMG/Applications"
cp "$ICNS_SRC" "$TMPDIR_DMG/.VolumeIcon.icns"

log "  建讀寫 DMG"
hdiutil create -srcfolder "${TMPDIR_DMG}" -volname "${VOLUME_NAME}" \
    -fs HFS+ -format UDRW -size 400m "${TMP_DMG}"

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
        set the bounds of container window to {200, 200, $((200 + WIN_W)), $((200 + WIN_H))}
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

# ── Notarization ──
if [[ "${SKIP_NOTARIZE:-0}" == "1" ]]; then
    log "SKIP_NOTARIZE=1 → 跳過 notarize/staple"
    log "=== 完成（未 notarize）: $DMG_PATH ($(ls -lh "$DMG_PATH" | awk '{print $5}')) ==="
    exit 0
fi

log "--- 提交 Notarization"
xcrun notarytool submit "$DMG_PATH" --keychain-profile "$NOTARY_PROFILE" --wait
log "--- Staple"
xcrun stapler staple "$DMG_PATH"
xcrun stapler validate "$DMG_PATH"

if [ -d "$APP_BUNDLE" ]; then
    rm -rf "$APP_BUNDLE"
    log "--- 已清 staging .app: $APP_BUNDLE"
fi

log "=== 完成: $DMG_PATH ($(ls -lh "$DMG_PATH" | awk '{print $5}')) ==="
