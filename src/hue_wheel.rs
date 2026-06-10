//! HUE 色相環（雙 zone：外環=hue / 中央=saturation）。
//!
//! 完整複製自 VisionMod `core/src/ui/hue_wheel.rs`（邏輯一字不改）。唯一差異：
//! - `super::theme::{...}` → `crate::theme::{...}`（MADO 對應路徑）
//! - popup egui::Id `"visionmod_hue_wheel_popup"` → `"mado_hue_wheel_popup"`
//! - texture key `"visionmod_hue_wheel"` → `"mado_hue_wheel"`
//!
//! 強制來源（global memory `feedback_egui_hue_wheel`）。規則（禁制清單）：
//! - 必 Texture 渲染（禁 mesh / Path arc）
//! - Marker = 白色實心圓 + 黑色陰影圈
//! - hue 範圍 0..1（禁 0..360）；角度 +π/2 偏移
//! - 中央 saturation = 垂直拖曳 + chevron 上下箭頭 + "SATURATION" 全大寫

use eframe::egui;

use crate::theme::{
    self, hsb_to_color32, BG_PANEL, STROKE_HAIRLINE,
};

/// HUE popup 內部固定字級（14pt）。
const HUE_POPUP_LABEL_SIZE: f32 = 14.0;

/// HUE 拖曳 zone（drag-start 鎖定，整個 gesture 不切換）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum HueDragZone {
    #[default]
    None,
    Ring,
    Center,
}

/// HUE popup 當前 hover 的互動區域（呼叫端據此寫說明）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum HueHoverZone {
    #[default]
    None,
    /// 外環（hue）
    Ring,
    /// 中央 saturation chevron
    Center,
    /// 4 預設色 swatch
    Palette,
    /// 5 衍生相位色 swatch（純展示）
    Phase,
}

/// HUE wheel 互動 state（屬於 AppState 內一段）。
#[derive(Default)]
pub struct HueWheelState {
    pub show: bool,
    pub texture: Option<egui::TextureHandle>,
    pub dragging: bool,
    pub drag_zone: HueDragZone,
    /// 同一幀打開 popup 的 latch，避免「click 開」與「click outside 關」同幀 cancel。
    pub just_toggled: bool,
}

/// Generate hue wheel texture with smoothstep anti-aliasing。
pub fn generate_hue_wheel_texture(
    size: usize,
    outer_r_frac: f32,
    inner_r_frac: f32,
) -> egui::ColorImage {
    let mut pixels = vec![egui::Color32::TRANSPARENT; size * size];
    let center = size as f32 / 2.0;
    let outer_r = center * outer_r_frac;
    let inner_r = center * inner_r_frac;

    fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
        let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    }

    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 + 0.5 - center;
            let dy = y as f32 + 0.5 - center;
            let dist = (dx * dx + dy * dy).sqrt();

            let alpha = smoothstep(outer_r + 1.5, outer_r - 0.5, dist)
                * smoothstep(inner_r - 1.5, inner_r + 0.5, dist);

            if alpha > 0.0 {
                let mut angle = dy.atan2(dx) + std::f32::consts::FRAC_PI_2;
                if angle < 0.0 {
                    angle += std::f32::consts::TAU;
                }
                let hue = angle / std::f32::consts::TAU;
                let color = hsb_to_color32(hue, 0.30, 0.92);
                let a = (alpha * 255.0) as u8;
                pixels[y * size + x] = egui::Color32::from_rgba_unmultiplied(
                    color.r(),
                    color.g(),
                    color.b(),
                    a,
                );
            }
        }
    }
    egui::ColorImage {
        size: [size, size],
        pixels,
    }
}

/// 切換按鈕：圓形 swatch（accent 填色），點擊開關 popup。
pub fn toggle_button(
    ui: &mut egui::Ui,
    accent: egui::Color32,
    state: &mut HueWheelState,
) -> egui::Response {
    let size = egui::vec2(18.0, 18.0);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    let p = ui.painter();
    p.circle_filled(rect.center(), 8.0, accent);
    p.circle_stroke(
        rect.center(),
        8.0,
        egui::Stroke::new(
            1.0,
            if state.show || resp.hovered() {
                accent
            } else {
                STROKE_HAIRLINE
            },
        ),
    );
    if resp.clicked() {
        state.show = !state.show;
        state.just_toggled = true;
    }
    resp
}

/// 繪製 HUE wheel popup（必須在每幀 `update` 尾端、`CentralPanel` 之後呼叫）。
pub fn draw(
    ctx: &egui::Context,
    accent_hue: &mut f32,
    accent_saturation: &mut f32,
    _accent_palette_idx: &mut u8,
    state: &mut HueWheelState,
    accent: egui::Color32,
) -> HueHoverZone {
    if !state.show {
        return HueHoverZone::None;
    }
    // y offset = overlay 工具列高 56px（main.rs draw_overlay bar_h）+ 8px 間距 = 64px。
    // 與 Settings panel 同一避讓基準，確保 HUE 圈上緣完全落在工具列下方不被遮。
    let area_resp = egui::Area::new(egui::Id::new("mado_hue_wheel_popup"))
        .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-12.0, 64.0))
        .show(ctx, |ui| {
            egui::Frame::NONE
                .inner_margin(egui::Margin::same(16))
                .fill(BG_PANEL)
                .stroke(egui::Stroke::new(1.0, STROKE_HAIRLINE))
                .corner_radius(egui::CornerRadius::same(8))
                .show(ui, |ui| {
                    let mut hover_zone = HueHoverZone::None;
                    let wheel_size: f32 = 200.0;
                    let ring_width: f32 = 30.0;
                    let outer_radius: f32 = wheel_size / 2.0 - 8.0;
                    let inner_radius: f32 = outer_radius - ring_width;
                    let center_circle_r: f32 =
                        (wheel_size - ring_width * 2.0 - 24.0) / 2.0;

                    let ppp = ui.ctx().pixels_per_point();
                    let tex_size = (wheel_size * ppp) as usize;
                    if state.texture.is_none() {
                        let outer_frac = outer_radius / (wheel_size / 2.0);
                        let inner_frac = inner_radius / (wheel_size / 2.0);
                        let img = generate_hue_wheel_texture(
                            tex_size, outer_frac, inner_frac,
                        );
                        state.texture = Some(ui.ctx().load_texture(
                            "mado_hue_wheel",
                            img,
                            egui::TextureOptions::LINEAR,
                        ));
                    }

                    let (rect, response) = ui.allocate_exact_size(
                        egui::vec2(wheel_size, wheel_size),
                        egui::Sense::click_and_drag(),
                    );
                    let center = rect.center();
                    let painter = ui.painter();

                    if let Some(tex) = &state.texture {
                        painter.image(
                            tex.id(),
                            rect,
                            egui::Rect::from_min_max(
                                egui::pos2(0.0, 0.0),
                                egui::pos2(1.0, 1.0),
                            ),
                            egui::Color32::WHITE,
                        );
                    }
                    painter.circle_filled(center, center_circle_r, accent);

                    // Centre disc: chevron ^ / v + "SATURATION" caption
                    let label_color = egui::Color32::from_black_alpha(180);
                    let chevron_stroke = egui::Stroke::new(2.0, label_color);
                    let arrow_offset = 22.0;
                    let arrow_half_w = 8.0;
                    let arrow_h = 8.0;
                    let up_tip =
                        egui::pos2(center.x, center.y - arrow_offset - arrow_h);
                    let up_left = egui::pos2(
                        center.x - arrow_half_w,
                        center.y - arrow_offset,
                    );
                    let up_right = egui::pos2(
                        center.x + arrow_half_w,
                        center.y - arrow_offset,
                    );
                    painter.line_segment([up_left, up_tip], chevron_stroke);
                    painter.line_segment([up_tip, up_right], chevron_stroke);
                    let down_tip =
                        egui::pos2(center.x, center.y + arrow_offset + arrow_h);
                    let down_left = egui::pos2(
                        center.x - arrow_half_w,
                        center.y + arrow_offset,
                    );
                    let down_right = egui::pos2(
                        center.x + arrow_half_w,
                        center.y + arrow_offset,
                    );
                    painter.line_segment([down_left, down_tip], chevron_stroke);
                    painter.line_segment([down_tip, down_right], chevron_stroke);
                    painter.text(
                        center,
                        egui::Align2::CENTER_CENTER,
                        "SATURATION",
                        egui::FontId::new(HUE_POPUP_LABEL_SIZE, egui::FontFamily::Proportional),
                        label_color,
                    );

                    // Outer-ring marker
                    let marker_angle =
                        *accent_hue * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
                    let marker_radius = outer_radius - ring_width / 2.0;
                    let marker_pos = center
                        + egui::vec2(
                            marker_angle.cos() * marker_radius,
                            marker_angle.sin() * marker_radius,
                        );
                    painter.circle_filled(
                        marker_pos,
                        6.0,
                        egui::Color32::from_black_alpha(50),
                    );
                    painter.circle_filled(marker_pos, 5.0, egui::Color32::WHITE);

                    // Two-zone drag interaction
                    if response.drag_started() {
                        if let Some(pt) = response.interact_pointer_pos() {
                            let dx = pt.x - center.x;
                            let dy = pt.y - center.y;
                            let dist = (dx * dx + dy * dy).sqrt();
                            state.drag_zone = if dist <= center_circle_r + 1.0 {
                                HueDragZone::Center
                            } else if dist <= outer_radius + 4.0 {
                                HueDragZone::Ring
                            } else {
                                HueDragZone::None
                            };
                            state.dragging =
                                state.drag_zone != HueDragZone::None;
                        }
                    }
                    if response.dragged() {
                        match state.drag_zone {
                            HueDragZone::Ring => {
                                if let Some(pt) = response.interact_pointer_pos() {
                                    let dx = pt.x - center.x;
                                    let dy = pt.y - center.y;
                                    let mut angle = dy.atan2(dx)
                                        + std::f32::consts::FRAC_PI_2;
                                    if angle < 0.0 {
                                        angle += std::f32::consts::TAU;
                                    }
                                    *accent_hue = angle / std::f32::consts::TAU;
                                }
                            }
                            HueDragZone::Center => {
                                let dy = response.drag_delta().y;
                                *accent_saturation =
                                    (*accent_saturation - dy * 0.01).clamp(0.0, 2.0);
                            }
                            HueDragZone::None => {}
                        }
                    }
                    if response.drag_stopped() {
                        match state.drag_zone {
                            HueDragZone::Ring => theme::save_hue(*accent_hue),
                            HueDragZone::Center => {
                                theme::save_saturation(*accent_saturation)
                            }
                            HueDragZone::None => {}
                        }
                        state.drag_zone = HueDragZone::None;
                    }
                    if !response.dragged() {
                        state.dragging = false;
                    }

                    // hover 判定（wheel）：圓心區 = saturation、外環 = hue。
                    if let Some(pt) = response.hover_pos() {
                        let dx = pt.x - center.x;
                        let dy = pt.y - center.y;
                        let dist = (dx * dx + dy * dy).sqrt();
                        if dist <= center_circle_r + 1.0 {
                            hover_zone = HueHoverZone::Center;
                        } else if dist <= outer_radius + 4.0 {
                            hover_zone = HueHoverZone::Ring;
                        }
                    }

                    hover_zone
                })
        });
    // Area 回傳的 response.rect = popup 實際螢幕矩形（用於點外關閉幾何判定）。
    let popup_rect = area_resp.response.rect;
    let hover_zone = area_resp.inner.inner;

    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        state.show = false;
    }
    // 點 popup 外部關閉：幾何 hit-test。本幀有 pointer press 且座標落在
    // popup_rect 之外（且非剛 toggle 開啟同幀、非拖曳中）→ 關閉。
    // 取代依賴 is_pointer_over_area()（MADO 主畫面 overlay Area 使其恆 true）。
    if !state.just_toggled && !state.dragging {
        let pressed_outside = ctx.input(|i| {
            i.pointer.any_pressed()
                && i.pointer
                    .interact_pos()
                    .map(|p| !popup_rect.contains(p))
                    .unwrap_or(false)
        });
        if pressed_outside {
            state.show = false;
        }
    }
    state.just_toggled = false;
    hover_zone
}
