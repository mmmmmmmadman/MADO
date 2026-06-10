//! Accent / HUE 純函式（完整移植自 VisionMod core/src/ui/theme.rs）。
//!
//! 強制來源（global memory `feedback_egui_hue_wheel`）：
//! - VisionMod `core/src/ui/theme.rs`：hsb_to_color32 / compute_accent_colors /
//!   AccentColors / ACCENT_PALETTE / BG_PANEL / FG_DIM / STROKE_HAIRLINE /
//!   save_hue / load_hue / save_saturation / load_saturation
//!
//! 與 VisionMod 唯一差異：HUE 持久化路徑改寫到 MADO 自己的 config dir
//! （`~/Library/Application Support/MADO/`，沿用 `settings.rs` config_dir），
//! 檔名 `hue.txt` / `saturation.txt`。不寫 `.visionmod`。

use eframe::egui;
use std::path::PathBuf;

// ─────────────────────────────────────────────────────────────────────────────
// 配色常數（移植自 VisionMod theme.rs；MADO 全域深色主題已用 18,18,22 系，
// 但 HUE popup widget 需用與 VisionMod 一致的 BG_PANEL / FG_DIM / STROKE_HAIRLINE
// 才能 byte-identical 複製 hue_wheel.rs，故此處原樣移植 VisionMod 常數）。
// ─────────────────────────────────────────────────────────────────────────────

pub const BG_PANEL: egui::Color32 = egui::Color32::from_rgb(44, 44, 44);
pub const FG_DIM: egui::Color32 = egui::Color32::from_rgb(108, 108, 108);
pub const SEPARATOR: egui::Color32 = egui::Color32::from_rgb(60, 60, 60);
pub const STROKE_HAIRLINE: egui::Color32 = SEPARATOR;

// ─────────────────────────────────────────────────────────────────────────────
// 預設 HUE（MADZINE 品牌色 = 珊瑚粉，HSB H = 12°，hue = 12/360 ≈ 0.033）
// ─────────────────────────────────────────────────────────────────────────────

/// 珊瑚粉預設 hue（global memory `user_brand_color`）。
pub const DEFAULT_ACCENT_HUE: f32 = 12.0 / 360.0;
/// 預設 saturation multiplier（1.0 = AnyAni / Fluvius 標準粉彩 S=0.30）。
pub const DEFAULT_ACCENT_SATURATION: f32 = 1.0;

/// HUE wheel 內建 4 預設色相（使用者快速切換 swatch）：
/// 0 = 珊瑚粉 (H=12°)、1 = 薄荷綠 (H=160°)、2 = 天藍 (H=210°)、3 = 紫羅蘭 (H=280°)。
pub const ACCENT_PALETTE: [f32; 4] = [
    12.0 / 360.0,
    160.0 / 360.0,
    210.0 / 360.0,
    280.0 / 360.0,
];

// ─────────────────────────────────────────────────────────────────────────────
// HSB pastel accent system（移植自 VisionMod theme.rs）
// ─────────────────────────────────────────────────────────────────────────────

/// HSB → sRGB Color32（直接轉換，對應 SwiftUI Color(hue:saturation:brightness:)）。
pub fn hsb_to_color32(h: f32, s: f32, b: f32) -> egui::Color32 {
    let h = ((h % 1.0) + 1.0) % 1.0 * 360.0;
    let c = b * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = b - c;
    let (r1, g1, b1) = if h < 60.0 {
        (c, x, 0.0)
    } else if h < 120.0 {
        (x, c, 0.0)
    } else if h < 180.0 {
        (0.0, c, x)
    } else if h < 240.0 {
        (0.0, x, c)
    } else if h < 300.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };
    egui::Color32::from_rgb(
        ((r1 + m) * 255.0) as u8,
        ((g1 + m) * 255.0) as u8,
        ((b1 + m) * 255.0) as u8,
    )
}

/// 全域 accent 衍生色（同相位三聯 + 5 個固定相位衍生）。
#[allow(dead_code)] // 多個衍生色保留給後續視窗 / 訊號 lane / 警告色使用
pub struct AccentColors {
    pub accent: egui::Color32,
    pub accent_dim: egui::Color32,
    pub accent_glow: egui::Color32,
    pub accent_complementary: egui::Color32,
    pub accent_triadic_a: egui::Color32,
    pub accent_triadic_b: egui::Color32,
    pub accent_analogous_a: egui::Color32,
    pub accent_analogous_b: egui::Color32,
}

/// 從單一 hue (0..1) + saturation multiplier 推導所有衍生色。
///
/// `sat_mult`：0 = 全灰、1 = AnyAni 標準 (S=0.30/0.15/0.40)、2 = 雙倍 chroma。
/// 內部對每個 S 乘以 `sat_mult` 並 clamp 到 [0,1]。
pub fn compute_accent_colors(hue: f32, sat_mult: f32) -> AccentColors {
    let m = sat_mult.max(0.0);
    let s_accent = (0.30 * m).clamp(0.0, 1.0);
    let s_dim = (0.15 * m).clamp(0.0, 1.0);
    let s_glow = (0.40 * m).clamp(0.0, 1.0);
    AccentColors {
        accent: hsb_to_color32(hue, s_accent, 0.92),
        accent_dim: hsb_to_color32(hue, s_dim, 0.60),
        accent_glow: hsb_to_color32(hue, s_glow, 1.0),
        accent_complementary: hsb_to_color32(hue + 0.5, s_accent, 0.92),
        accent_triadic_a: hsb_to_color32(hue + 0.333, s_accent, 0.92),
        accent_triadic_b: hsb_to_color32(hue - 0.333, s_accent, 0.92),
        accent_analogous_a: hsb_to_color32(hue + 0.083, s_accent, 0.92),
        accent_analogous_b: hsb_to_color32(hue - 0.083, s_accent, 0.92),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// HUE 持久化：~/Library/Application Support/MADO/{hue,saturation}.txt
// （沿用 settings.rs 的 config_dir；MADO 不寫 .visionmod）
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
fn config_dir() -> PathBuf {
    let mut p = match std::env::var_os("HOME") {
        Some(h) => PathBuf::from(h),
        None => PathBuf::from("."),
    };
    p.push("Library");
    p.push("Application Support");
    p.push("MADO");
    p
}

#[cfg(target_os = "windows")]
fn config_dir() -> PathBuf {
    let mut p = match std::env::var_os("APPDATA") {
        Some(h) => PathBuf::from(h),
        None => PathBuf::from("."),
    };
    p.push("MADO");
    p
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn config_dir() -> PathBuf {
    PathBuf::from(".")
}

pub fn load_hue() -> f32 {
    let path = config_dir().join("hue.txt");
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(DEFAULT_ACCENT_HUE)
}

pub fn save_hue(hue: f32) {
    let dir = config_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        log::warn!("[theme] save_hue create_dir failed: {e}");
        return;
    }
    let path = dir.join("hue.txt");
    if let Err(e) = std::fs::write(&path, format!("{}", hue)) {
        log::warn!("[theme] save_hue write {} failed: {e}", path.display());
    }
}

pub fn load_saturation() -> f32 {
    let path = config_dir().join("saturation.txt");
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(DEFAULT_ACCENT_SATURATION)
}

pub fn save_saturation(sat: f32) {
    let dir = config_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        log::warn!("[theme] save_saturation create_dir failed: {e}");
        return;
    }
    let path = dir.join("saturation.txt");
    if let Err(e) = std::fs::write(&path, format!("{}", sat)) {
        log::warn!("[theme] save_saturation write {} failed: {e}", path.display());
    }
}
