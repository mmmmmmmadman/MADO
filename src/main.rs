//! MADO：無邊框 USB camera 預覽。
//!
//! 複製自 VisionMod：
//! - 無邊框視窗 + 雙擊全螢幕 + StartDrag + 右下 16×16 BeginResize
//!   → core/src/ui/output_window.rs（2026-06-08 變更）
//! - 相機列舉（system_profiler + spcamera_unique-id）
//!   → core/src/camera/capture.rs::list_cameras_macos
//! - 48B BinHeaderV1 + macOS InPlaceFrameWriter 對應 reader
//!   → core/src/ipc.rs::read_bin_header_v1
//! - camera service AVFoundation backend
//!   → core/scripts/camera_service_mac.py（精簡）
//! - frame_mmap.py InPlaceFrameWriter（byte-identical）
//!
//! UI：滑鼠進入視窗 → overlay 控制列（相機 ComboBox、上下/左右翻、置頂、關閉）。

mod audio;
mod audio_output;
mod camera;
mod playlist;
mod rtaudio_ffi;
mod settings;
mod video;

use eframe::egui;
use playlist::{is_video_file, LoopMode, Playlist};
use settings::Settings;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

const DEFAULT_W: f32 = 960.0;
const DEFAULT_H: f32 = 540.0;

const MANUAL_TEXT: &str = "\
Drag and drop video files onto the window to load them into the playlist.\n\
Drop multiple files at once — or one at a time — they all append to the playlist.\n\
\n\
Toolbar (hover the window to show)\n\
  ⚙          Open this Settings panel\n\
  Camera     Switch to live webcam source\n\
  Video      Switch to playlist playback\n\
  Loop Off   Play through, stop at end of list\n\
  Loop One   Repeat current video\n\
  Loop All   Repeat the whole playlist\n\
  Playlist   Open the playlist drawer (reorder / remove / per-clip flip)\n\
  ↔  ↕       Flip horizontally / vertically (per-clip in Video mode)\n\
\n\
Window\n\
  Drag the toolbar (or the picture) to move the window.\n\
  Double-click the picture to toggle fullscreen.\n\
  Drag the bottom-right corner to resize.\n\
\n\
Keyboard\n\
  ↓       Blackout — fill the window black (camera / video keep running)\n\
  ↑       Restore the live picture\n\
  Space   Pause / resume (Video mode)\n\
  ←       Return to the start, keeping the current play / pause state (Video mode)\n\
";

/// MADO 全域深色主題（在 app 啟動時設定一次）。
///
/// 根因：app 原本用 egui 預設淺色 `Visuals`，只在個別面板 closure 用
/// `ui.style_mut()` 改 `fg_stroke`。ComboBox 下拉 popup 畫在獨立的
/// Foreground layer，吃的是 ctx 全域 visuals → 維持白底；Slider 滑軌 /
/// DragValue / 捲軸的 `extreme_bg_color` / `bg_fill` / `weak_bg_fill`
/// 也來自全域 visuals → 白底。治本：以 `Visuals::dark()` 為基底，
/// 覆寫成 MADO 配色語言（深色背景 18,18,22 系、亮灰 gray230、珊瑚粉
/// 0xFF6C47），所有 widget（含 popup）一律深色。
fn install_visuals(ctx: &egui::Context) {
    const PANEL: egui::Color32 = egui::Color32::from_rgb(18, 18, 22);
    const ELEVATED: egui::Color32 = egui::Color32::from_rgb(28, 28, 34);
    const EXTREME: egui::Color32 = egui::Color32::from_rgb(10, 10, 14);
    const HAIRLINE: egui::Color32 = egui::Color32::from_gray(60);
    const FG_INACTIVE: egui::Color32 = egui::Color32::from_gray(230);
    const FG_DIM: egui::Color32 = egui::Color32::from_gray(160);
    const CORAL: egui::Color32 = egui::Color32::from_rgb(0xFF, 0x6C, 0x47);

    let mut visuals = egui::Visuals::dark();

    // ── 視窗 / 面板 / popup 背景（ComboBox popup 用 window_fill）──
    visuals.window_fill = PANEL;
    visuals.panel_fill = PANEL;
    visuals.window_stroke = egui::Stroke::new(1.0, HAIRLINE);
    // Slider 滑軌 / DragValue 數值框 / TextEdit 背景（治 Slider 白底）
    visuals.extreme_bg_color = EXTREME;
    visuals.faint_bg_color = ELEVATED;
    visuals.code_bg_color = EXTREME;
    // 選取高亮（popup 選中項 / 文字選取）用珊瑚粉低透明
    visuals.selection.bg_fill = egui::Color32::from_rgba_unmultiplied(0xFF, 0x6C, 0x47, 70);
    visuals.selection.stroke = egui::Stroke::new(1.0, CORAL);
    visuals.hyperlink_color = CORAL;
    visuals.warn_fg_color = CORAL;
    // 視窗陰影壓低（borderless overlay 上不需強陰影）
    visuals.popup_shadow = egui::epaint::Shadow {
        offset: [0, 2],
        blur: 8,
        spread: 0,
        color: egui::Color32::from_black_alpha(96),
    };
    visuals.window_shadow = visuals.popup_shadow;

    // ── 各 widget 狀態：底框深色（絕不白底）、文字亮灰 / hover-active 珊瑚粉 ──
    let w = &mut visuals.widgets;

    // noninteractive：label / heading / separator
    w.noninteractive.bg_fill = PANEL;
    w.noninteractive.weak_bg_fill = PANEL;
    w.noninteractive.bg_stroke = egui::Stroke::new(1.0, HAIRLINE); // separator 線
    w.noninteractive.fg_stroke = egui::Stroke::new(1.0, FG_INACTIVE);

    // inactive：未 hover 的 button / combobox 收合態 / slider 軌
    w.inactive.bg_fill = ELEVATED;
    w.inactive.weak_bg_fill = ELEVATED;
    w.inactive.bg_stroke = egui::Stroke::new(1.0, HAIRLINE);
    w.inactive.fg_stroke = egui::Stroke::new(1.0, FG_INACTIVE);

    // hovered：滑入回饋（底框稍亮深色、文字珊瑚粉）
    w.hovered.bg_fill = egui::Color32::from_rgb(40, 40, 48);
    w.hovered.weak_bg_fill = egui::Color32::from_rgb(40, 40, 48);
    w.hovered.bg_stroke = egui::Stroke::new(1.0, CORAL);
    w.hovered.fg_stroke = egui::Stroke::new(1.5, CORAL);

    // active：按下 / 開啟態
    w.active.bg_fill = egui::Color32::from_rgb(48, 48, 56);
    w.active.weak_bg_fill = egui::Color32::from_rgb(48, 48, 56);
    w.active.bg_stroke = egui::Stroke::new(1.0, CORAL);
    w.active.fg_stroke = egui::Stroke::new(1.5, CORAL);

    // open：ComboBox 展開時的收合鈕底框（popup 開著時）
    w.open.bg_fill = ELEVATED;
    w.open.weak_bg_fill = ELEVATED;
    w.open.bg_stroke = egui::Stroke::new(1.0, CORAL);
    w.open.fg_stroke = egui::Stroke::new(1.0, FG_INACTIVE);

    // disabled 文字（egui 另以 fade 降透明；給可辨灰）
    visuals.widgets.noninteractive.fg_stroke.color = FG_INACTIVE;
    // window_fill 已設；確保 widget 圓角與 egui dark 一致即可
    let _ = FG_DIM; // 保留語義常數（waiting hint 等其他處仍用 from_gray）

    // eframe 預設 theme_preference = System：系統 theme 變化時 egui 會用硬編碼
    // Visuals::dark()/light() 覆蓋 set_visuals（這正是 ComboBox popup / Slider 仍白底、
    // selection 仍預設藍的原因）。改設 Dark（停止跟隨系統）並把自訂 visuals 放進
    // dark_style，eframe 套用 theme 時就用我們的版本，不再被覆蓋。
    let mut style = (*ctx.style()).clone();
    style.visuals = visuals;
    let style_arc = std::sync::Arc::new(style);
    ctx.options_mut(|opt| {
        opt.theme_preference = egui::ThemePreference::Dark;
        opt.dark_style = style_arc.clone();
    });
    ctx.set_style(style_arc);
}

fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "barlow".to_string(),
        std::sync::Arc::new(
            egui::FontData::from_static(include_bytes!(
                "../assets/fonts/BarlowCondensed-Bold.ttf"
            ))
            .tweak(egui::FontTweak {
                scale: 1.05,
                y_offset_factor: 0.00,
                y_offset: 0.0,
                baseline_offset_factor: 0.0,
            }),
        ),
    );
    fonts.font_data.insert(
        "noto_tc".to_string(),
        std::sync::Arc::new(
            egui::FontData::from_static(include_bytes!(
                "../assets/fonts/NotoSansTC-Bold.ttf"
            ))
            .tweak(egui::FontTweak {
                scale: 0.92,
                y_offset_factor: -0.04,
                y_offset: 0.0,
                baseline_offset_factor: 0.0,
            }),
        ),
    );
    fonts.font_data.insert(
        "noto_jp".to_string(),
        std::sync::Arc::new(
            egui::FontData::from_static(include_bytes!(
                "../assets/fonts/NotoSansJP-Bold.ttf"
            ))
            .tweak(egui::FontTweak {
                scale: 0.92,
                y_offset_factor: -0.04,
                y_offset: 0.0,
                baseline_offset_factor: 0.0,
            }),
        ),
    );
    if let Some(prop) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
        prop.insert(0, "barlow".to_string());
        prop.push("noto_tc".to_string());
        prop.push("noto_jp".to_string());
    }
    if let Some(mono) = fonts.families.get_mut(&egui::FontFamily::Monospace) {
        mono.push("noto_tc".to_string());
        mono.push("noto_jp".to_string());
    }
    ctx.set_fonts(fonts);
}

fn main() -> eframe::Result<()> {
    let viewport = egui::ViewportBuilder::default()
        .with_decorations(false)
        .with_transparent(false)
        .with_resizable(true)
        .with_inner_size([DEFAULT_W, DEFAULT_H])
        .with_min_inner_size([320.0, 180.0])
        .with_title("MADO");
    let opts = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "MADO",
        opts,
        Box::new(|cc| Ok(Box::new(MadoApp::new(cc)))),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Camera,
    Video,
}

struct MadoApp {
    cameras: Vec<camera::CameraInfo>,
    selected_idx: usize, // index into cameras vec
    selected_unique_id: String,
    /// camera 模式的 flip（與 playlist item 各自獨立）
    camera_flip_h: bool,
    camera_flip_v: bool,
    always_on_top: bool,
    is_fullscreen: bool,

    shared_frame: camera::SharedFrame,
    stop_flag: Arc<AtomicBool>,
    service_child: Option<std::process::Child>,
    /// Video mode 時持有的 Rust 端解碼器（取代 Python video_service）
    video_player: Option<video::VideoPlayer>,
    /// Video mode 時持有的 RtAudio output stream
    audio_output: Option<audio_output::AudioOutput>,

    texture: Option<egui::TextureHandle>,
    last_uploaded_frame_id: u64,
    hover_active: bool,

    mode: Mode,
    playlist: Playlist,
    show_playlist: bool,
    /// playlist 剛由工具列按鈕開啟的那一幀，跳過「點外部關閉」判定，
    /// 避免開啟同幀的 pointer press（落在工具列）立即關閉面板。
    playlist_just_opened: bool,
    paused: bool,
    /// 黑屏顯示開關（Down = 全黑、Up = 恢復）。背景 pipeline 不停，僅控制繪製。
    blackout: bool,

    settings: Settings,
    audio_devices: Vec<audio::AudioDevice>,
    show_settings: bool,
}

impl MadoApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        install_fonts(&cc.egui_ctx);
        install_visuals(&cc.egui_ctx);
        let cameras = camera::list_cameras().unwrap_or_default();
        let selected_idx = 0;
        let selected_unique_id = cameras
            .get(0)
            .map(|c| c.unique_id.clone())
            .unwrap_or_default();

        let shared_frame = Arc::new(std::sync::Mutex::new(camera::FrameSlot::default()));
        let stop_flag = Arc::new(AtomicBool::new(false));
        camera::spawn_reader_thread(shared_frame.clone(), stop_flag.clone());

        let mut app = Self {
            cameras,
            selected_idx,
            selected_unique_id,
            camera_flip_h: false,
            camera_flip_v: false,
            always_on_top: false,
            is_fullscreen: false,
            shared_frame,
            stop_flag,
            service_child: None,
            video_player: None,
            audio_output: None,
            texture: None,
            last_uploaded_frame_id: 0,
            hover_active: false,
            mode: Mode::Camera,
            playlist: Playlist::default(),
            show_playlist: false,
            playlist_just_opened: false,
            paused: false,
            blackout: false,
            settings: Settings::load(),
            audio_devices: audio::list_output_devices(),
            show_settings: false,
        };
        app.restart_service();
        app
    }

    fn current_camera(&self) -> Option<&camera::CameraInfo> {
        self.cameras.get(self.selected_idx)
    }

    /// 目前顯示的影像應該套用的翻轉（依模式自動取對應狀態）。
    fn effective_flip(&self) -> (bool, bool) {
        match self.mode {
            Mode::Camera => (self.camera_flip_h, self.camera_flip_v),
            Mode::Video => match self.playlist.current_item() {
                Some(it) => (it.flip_h, it.flip_v),
                None => (false, false),
            },
        }
    }

    /// 切換目前模式對應的 flip_h。
    fn toggle_flip_h(&mut self) {
        match self.mode {
            Mode::Camera => self.camera_flip_h = !self.camera_flip_h,
            Mode::Video => {
                if let Some(it) = self.playlist.current_item_mut() {
                    it.flip_h = !it.flip_h;
                }
            }
        }
    }

    fn toggle_flip_v(&mut self) {
        match self.mode {
            Mode::Camera => self.camera_flip_v = !self.camera_flip_v,
            Mode::Video => {
                if let Some(it) = self.playlist.current_item_mut() {
                    it.flip_v = !it.flip_v;
                }
            }
        }
    }

    fn stop_service(&mut self) {
        if let Some(mut c) = self.service_child.take() {
            camera::request_stop();
            let _ = c.wait();
        }
        // 釋放 video player / audio output（Drop 會停 thread + RtAudio stream）
        self.audio_output.take();
        self.video_player.take();
    }

    fn restart_service(&mut self) {
        // 依 mode 啟動對應的 service。
        self.stop_service();
        // 新 service 一律從未暫停開始
        self.paused = false;
        match self.mode {
            Mode::Camera => {
                let (uid, idx) = match self.current_camera() {
                    Some(c) => (c.unique_id.clone(), c.index),
                    None => (String::new(), 0),
                };
                self.selected_unique_id = uid.clone();
                match camera::spawn_camera_service(&uid, idx, 1280, 720, 30) {
                    Ok(child) => self.service_child = Some(child),
                    Err(e) => eprintln!("[mado] spawn camera service failed: {}", e),
                }
            }
            Mode::Video => {
                let path_opt = self
                    .playlist
                    .current_item()
                    .map(|it| it.path.clone());
                if let Some(path) = path_opt {
                    // 開 Rust ffmpeg 解碼器 → SharedFrame + AudioRing
                    match video::VideoPlayer::open(&path, self.shared_frame.clone()) {
                        Ok(vp) => {
                            // 用 ring buffer 開 RtAudio output stream
                            let ring = vp.audio_ring();
                            self.video_player = Some(vp);
                            match audio_output::AudioOutput::open(
                                self.settings.audio_device_name.as_deref(),
                                self.settings.volume,
                                ring,
                            ) {
                                Ok(out) => self.audio_output = Some(out),
                                Err(e) => {
                                    eprintln!("[mado] audio_output open failed: {}", e);
                                }
                            }
                        }
                        Err(e) => eprintln!("[mado] video open failed: {}", e),
                    }
                } else {
                    // 影片模式但沒清單 → 回 camera
                    self.mode = Mode::Camera;
                    let (uid, idx) = match self.current_camera() {
                        Some(c) => (c.unique_id.clone(), c.index),
                        None => (String::new(), 0),
                    };
                    self.selected_unique_id = uid.clone();
                    let _ = camera::spawn_camera_service(&uid, idx, 1280, 720, 30)
                        .map(|c| self.service_child = Some(c));
                }
            }
        }
        // 重置 frame_id 以便新 service 第一張立即上傳
        self.last_uploaded_frame_id = 0;
    }

    fn switch_to_camera(&mut self) {
        if self.mode != Mode::Camera {
            self.mode = Mode::Camera;
            self.restart_service();
        }
    }

    fn switch_to_video(&mut self) {
        if self.mode != Mode::Video && !self.playlist.is_empty() {
            self.mode = Mode::Video;
            self.restart_service();
        }
    }

    /// egui dropped_files 進來：分離影片檔，append 到 playlist。
    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped: Vec<std::path::PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|f| f.path.clone())
                .filter(|p| is_video_file(p))
                .collect()
        });
        if dropped.is_empty() {
            return;
        }
        let became_nonempty = self.playlist.append(dropped);
        if self.mode != Mode::Video && became_nonempty {
            // 從空變非空 → 自動進影片模式並從第一支播
            self.playlist.current = 0;
            self.mode = Mode::Video;
            self.restart_service();
        }
    }

    /// 鍵盤：Down = 黑屏、Up = 恢復（任何模式皆可，背景 pipeline 不停）。
    /// Video 模式下另有 Space = pause/resume、← = restart 當前影片。
    fn handle_keys(&mut self, ctx: &egui::Context) {
        // 黑屏切換獨立於模式之外，且必須在 early return 之前處理，
        // 否則黑屏（camera 模式時）按 Up 將無法解除。
        let (down, up) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::ArrowDown),
                i.key_pressed(egui::Key::ArrowUp),
            )
        });
        if down {
            self.blackout = true;
        }
        if up {
            self.blackout = false;
        }

        if self.mode != Mode::Video {
            return;
        }
        let (space, left) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::Space),
                i.key_pressed(egui::Key::ArrowLeft),
            )
        });
        if space {
            self.paused = !self.paused;
            if let Some(vp) = &self.video_player {
                vp.set_paused(self.paused);
            }
        }
        if left {
            // 回到開頭，但保留當前 play/pause 狀態：
            // 播放中 → 回頭續播；暫停中 → 回頭仍暫停（不強制播放）。
            // VideoPlayer 無 seek，回頭靠 respawn；restart_service 一律以
            // paused=false 起始，故 respawn 後再還原原本的暫停狀態。
            let was_paused = self.paused;
            self.restart_service();
            if was_paused {
                self.paused = true;
                if let Some(vp) = &self.video_player {
                    vp.set_paused(true);
                }
            }
        }
    }

    /// 影片模式下，每 frame 檢查 eof，並依 LoopMode 推進。
    fn poll_video_eof(&mut self) {
        if self.mode != Mode::Video {
            return;
        }
        let is_eof = self
            .video_player
            .as_ref()
            .map(|v| v.is_eof())
            .unwrap_or(false);
        if !is_eof {
            return;
        }
        match self.playlist.advance() {
            Some(_) => self.restart_service(),
            None => {
                // LoopMode::Off 且最後一支播完：停在最後一幀
                self.stop_service();
            }
        }
    }

    fn upload_frame(&mut self, ctx: &egui::Context) {
        let (rgba, w, h, fid) = {
            let s = match self.shared_frame.lock() {
                Ok(s) => s,
                Err(_) => return,
            };
            if s.frame_id == self.last_uploaded_frame_id {
                return;
            }
            let rgba = match &s.rgba {
                Some(r) => r.clone(),
                None => return,
            };
            (rgba, s.width as usize, s.height as usize, s.frame_id)
        };
        if w == 0 || h == 0 || rgba.len() < w * h * 4 {
            return;
        }
        let image = egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba[..w * h * 4]);
        match &mut self.texture {
            Some(tex) => tex.set(image, egui::TextureOptions::LINEAR),
            None => {
                self.texture = Some(ctx.load_texture(
                    "camera_frame",
                    image,
                    egui::TextureOptions::LINEAR,
                ));
            }
        }
        self.last_uploaded_frame_id = fid;
    }

    fn apply_always_on_top(&self, ctx: &egui::Context) {
        let level = if self.always_on_top {
            egui::WindowLevel::AlwaysOnTop
        } else {
            egui::WindowLevel::Normal
        };
        ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(level));
    }
}

impl eframe::App for MadoApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(std::time::Duration::from_millis(16));
        self.handle_dropped_files(ctx);
        self.handle_keys(ctx);
        self.poll_video_eof();
        self.upload_frame(ctx);

        // hover 偵測：用 pointer.has_pointer + viewport hovered。
        let hover = ctx.input(|i| i.pointer.has_pointer());
        self.hover_active = hover;

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(egui::Color32::from_rgb(8, 8, 12)))
            .show(ctx, |ui| {
                let available = ui.available_size();
                let (rect, response) =
                    ui.allocate_exact_size(available, egui::Sense::click_and_drag());

                // 1. 繪製 camera texture（filling 整個 client area，UV flip 控制翻轉）
                if let Some(tex) = &self.texture {
                    let (eff_h, eff_v) = self.effective_flip();
                    let (uv_min, uv_max) = uv_from_flip(eff_h, eff_v);
                    let mesh = build_textured_mesh(tex.id(), rect, uv_min, uv_max);
                    ui.painter().add(egui::Shape::mesh(mesh));
                } else {
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "waiting for camera…",
                        egui::FontId::proportional(18.0),
                        egui::Color32::from_gray(160),
                    );
                }

                // 2. 互動：右下角 16×16 resize > 雙擊全螢幕 > 全畫面 StartDrag
                let resize_size = 16.0;
                let resize_rect = egui::Rect::from_min_size(
                    rect.max - egui::vec2(resize_size, resize_size),
                    egui::vec2(resize_size, resize_size),
                );
                let resize_response = ui.interact(
                    resize_rect,
                    ui.id().with("mado_resize_se"),
                    egui::Sense::drag(),
                );
                // 視覺：resize handle 小三角
                {
                    let p = ui.painter();
                    let c = egui::Color32::from_white_alpha(if self.hover_active { 120 } else { 40 });
                    let r = resize_rect;
                    p.line_segment(
                        [egui::pos2(r.min.x + 2.0, r.max.y - 2.0), egui::pos2(r.max.x - 2.0, r.min.y + 2.0)],
                        egui::Stroke::new(1.5, c),
                    );
                    p.line_segment(
                        [egui::pos2(r.min.x + 6.0, r.max.y - 2.0), egui::pos2(r.max.x - 2.0, r.min.y + 6.0)],
                        egui::Stroke::new(1.5, c),
                    );
                }

                if resize_response.drag_started() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(
                        egui::viewport::ResizeDirection::SouthEast,
                    ));
                } else if response.double_clicked() {
                    self.is_fullscreen = !self.is_fullscreen;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.is_fullscreen));
                } else if response.drag_started() {
                    // overlay 控制列點擊不應觸發拖移：用 area + interact_pointer 排除。
                    // overlay 區域我們之後 Area 自動消費點擊（egui Area 高於 CentralPanel）。
                    ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }

                // 3. Overlay 控制列（hover 才顯示，或設定畫面開啟時持續顯示）
                if self.hover_active || self.show_settings {
                    self.draw_overlay(ctx, ui, rect);
                    if self.show_playlist && !self.playlist.is_empty() {
                        self.draw_playlist_panel(ctx, rect);
                    }
                }
                if self.show_settings {
                    self.draw_settings_panel(ctx, rect);
                }

                // 4. 沒影像時的 hint（影片清單空 + camera 沒畫面）
                if self.texture.is_none() && self.mode == Mode::Camera {
                    // 既有 "waiting for camera…" 已處理
                }
            });

        // 5. 黑屏覆蓋：蓋住 texture / overlay / settings 所有層。
        //    背景 pipeline 不停（相機/影片/音訊照跑），僅是顯示開關。
        //    用最頂層 Area + 整個 screen_rect 填純黑，Up 後立即接回即時畫面。
        if self.blackout {
            let screen = ctx.screen_rect();
            egui::Area::new(egui::Id::new("mado_blackout"))
                .fixed_pos(screen.min)
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    ui.painter().rect_filled(
                        screen,
                        egui::CornerRadius::ZERO,
                        egui::Color32::BLACK,
                    );
                });
        }
    }

    fn on_exit(&mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
        camera::request_stop();
        if let Some(mut c) = self.service_child.take() {
            let _ = c.wait();
        }
    }
}

impl MadoApp {
    fn draw_overlay(&mut self, ctx: &egui::Context, _ui: &mut egui::Ui, rect: egui::Rect) {
        let bar_h = 56.0;
        let bar_rect = egui::Rect::from_min_size(
            egui::pos2(rect.min.x, rect.min.y),
            egui::vec2(rect.width(), bar_h),
        );
        let area = egui::Area::new(egui::Id::new("mado_overlay_top"))
            .fixed_pos(bar_rect.min)
            .order(egui::Order::Foreground);
        area.show(ctx, |ui| {
            // 背景半透明條
            let painter = ui.painter();
            painter.rect_filled(
                bar_rect,
                egui::CornerRadius::ZERO,
                egui::Color32::from_black_alpha(160),
            );
            // 工具列背景吃 drag（按鈕本身 click 優先級高，不受影響）
            let bar_drag = ui.interact(
                bar_rect,
                ui.id().with("mado_overlay_drag"),
                egui::Sense::drag(),
            );
            if bar_drag.drag_started() {
                ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
            ui.allocate_ui_with_layout(
                egui::vec2(bar_rect.width(), bar_h),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.add_space(12.0);
                    // 字體大小為 per-panel 排版設定（非配色）；配色（fg_stroke /
                    // 底框）一律走全域深色主題 install_visuals，不在此重設。
                    let style = ui.style_mut();
                    style.text_styles.insert(
                        egui::TextStyle::Button,
                        egui::FontId::new(16.0, egui::FontFamily::Proportional),
                    );
                    style.text_styles.insert(
                        egui::TextStyle::Body,
                        egui::FontId::new(16.0, egui::FontFamily::Proportional),
                    );
                    // 工具列文字強制白色（半透明黑條上最清楚，且不依賴全域主題）；
                    // hover 與選中項走珊瑚粉。selectable 選中態文字仍取 inactive 白色，
                    // 背景取 selection.bg_fill 珊瑚粉，故「Video」= 白字珊瑚底（非藍）。
                    let coral = egui::Color32::from_rgb(0xFF, 0x6C, 0x47);
                    style.visuals.widgets.noninteractive.fg_stroke.color = egui::Color32::WHITE;
                    style.visuals.widgets.inactive.fg_stroke.color = egui::Color32::WHITE;
                    style.visuals.widgets.hovered.fg_stroke.color = coral;
                    style.visuals.widgets.active.fg_stroke.color = coral;
                    style.visuals.selection.bg_fill =
                        egui::Color32::from_rgba_unmultiplied(0xFF, 0x6C, 0x47, 70);
                    style.visuals.selection.stroke = egui::Stroke::new(1.0, coral);

                    // ── 齒輪：進入設定 ──
                    if ui
                        .selectable_label(self.show_settings, "⚙")
                        .clicked()
                    {
                        self.show_settings = !self.show_settings;
                        if self.show_settings {
                            // 重新整理 device 清單
                            self.audio_devices =
                                audio::list_output_devices();
                        }
                    }
                    ui.add_space(8.0);

                    // ── Source 切換（Camera ↔ Video）──
                    let camera_active = self.mode == Mode::Camera;
                    let video_active = self.mode == Mode::Video;
                    if ui
                        .selectable_label(camera_active, "Camera")
                        .clicked()
                    {
                        self.switch_to_camera();
                    }
                    let video_btn = ui.add_enabled(
                        !self.playlist.is_empty() || video_active,
                        egui::SelectableLabel::new(video_active, "Video"),
                    );
                    if video_btn.clicked() {
                        self.switch_to_video();
                    }
                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(8.0);

                    // ── 右側固定錨點 cluster（filename 變動不影響此處位置）──
                    // 注意：right_to_left 內最先 add 的元件靠最右邊。
                    ui.with_layout(
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            ui.add_space(12.0);
                            // 與其他工具列項一致用 selectable_label（非 Button），
                            // 確保未 hover 時走相同 inactive.fg_stroke(gray230) 對比，
                            // hover 時走 hovered.fg_stroke 珊瑚粉。
                            if ui.selectable_label(false, "Close").clicked() {
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                            ui.add_space(8.0);
                            if ui
                                .selectable_label(self.always_on_top, "Always On Top")
                                .clicked()
                            {
                                self.always_on_top = !self.always_on_top;
                                self.apply_always_on_top(ctx);
                            }
                            let (eff_h, eff_v) = self.effective_flip();
                            if ui.selectable_label(eff_v, "↕").clicked() {
                                self.toggle_flip_v();
                            }
                            if ui.selectable_label(eff_h, "↔").clicked() {
                                self.toggle_flip_h();
                            }
                            ui.add_space(8.0);
                            // Playlist 開關（清單非空才顯示）
                            if !self.playlist.is_empty() {
                                let pl_label = format!("Playlist ({})", self.playlist.len());
                                if ui
                                    .selectable_label(self.show_playlist, pl_label)
                                    .clicked()
                                {
                                    self.show_playlist = !self.show_playlist;
                                    if self.show_playlist {
                                        // 開啟當幀跳過「點外部關閉」判定（避免本幀
                                        // 落在工具列的 press 立即關閉）。
                                        self.playlist_just_opened = true;
                                    }
                                }
                            }
                            // Loop 三態鈕（等寬，三態無 glyph）
                            let loop_label = self.playlist.loop_mode.label().to_string();
                            let loop_on = self.playlist.loop_mode != LoopMode::Off;
                            let loop_btn = egui::SelectableLabel::new(loop_on, loop_label);
                            if ui
                                .add_sized(egui::vec2(96.0, 26.0), loop_btn)
                                .clicked()
                            {
                                self.playlist.loop_mode = self.playlist.loop_mode.next();
                            }
                            ui.add_space(8.0);

                            // ── 剩餘左側空間：來源 UI（ComboBox or 檔名）──
                            // 用 left_to_right 收回原本流向，並 truncate 檔名避免擠走右側。
                            ui.with_layout(
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    if camera_active {
                                        let current_label = self
                                            .current_camera()
                                            .map(|c| c.name.clone())
                                            .unwrap_or_else(|| "—".to_string());
                                        let mut change_to: Option<usize> = None;
                                        egui::ComboBox::from_id_salt("mado_camera_combo")
                                            .selected_text(current_label)
                                            .width(220.0)
                                            .show_ui(ui, |ui| {
                                                for (i, cam) in self.cameras.iter().enumerate() {
                                                    let selected = i == self.selected_idx;
                                                    if ui
                                                        .selectable_label(selected, &cam.name)
                                                        .clicked()
                                                    {
                                                        change_to = Some(i);
                                                    }
                                                }
                                            });
                                        if let Some(i) = change_to {
                                            if i != self.selected_idx {
                                                self.selected_idx = i;
                                                self.restart_service();
                                            }
                                        }
                                    } else {
                                        let total = self.playlist.len();
                                        let label = match self.playlist.current_item() {
                                            Some(it) => format!(
                                                "{}/{}  {}",
                                                self.playlist.current + 1,
                                                total,
                                                it.path
                                                    .file_name()
                                                    .and_then(|s| s.to_str())
                                                    .unwrap_or("—")
                                            ),
                                            None => "—".to_string(),
                                        };
                                        ui.add(egui::Label::new(label).truncate());
                                    }
                                },
                            );
                        },
                    );
                },
            );
        });
    }

    fn draw_settings_panel(&mut self, ctx: &egui::Context, rect: egui::Rect) {
        // 半透明遮罩鎖住背景互動。
        // 注意：modal 和遮罩都用 Order::Middle，這樣 ComboBox popup（Order::Foreground）
        // 可以浮在 modal 上方（egui::ComboBox 內部 Area 用 Foreground 固定）。
        let area_bg = egui::Area::new(egui::Id::new("mado_settings_bg"))
            .fixed_pos(rect.min)
            .order(egui::Order::Middle)
            .interactable(true);
        let bg_clicked = area_bg
            .show(ctx, |ui| {
                ui.painter().rect_filled(
                    rect,
                    egui::CornerRadius::ZERO,
                    egui::Color32::from_black_alpha(180),
                );
                ui.interact(
                    rect,
                    ui.id().with("mado_settings_block"),
                    egui::Sense::click(),
                )
                .clicked()
            })
            .inner;

        // 中央 modal
        let panel_w = (rect.width() * 0.8).clamp(420.0, 640.0);
        let panel_h = (rect.height() * 0.85).clamp(360.0, 560.0);
        let panel_rect = egui::Rect::from_center_size(
            rect.center(),
            egui::vec2(panel_w, panel_h),
        );
        let area = egui::Area::new(egui::Id::new("mado_settings_panel"))
            .fixed_pos(panel_rect.min)
            .order(egui::Order::Middle);
        area.show(ctx, |ui| {
            ui.painter().rect_filled(
                panel_rect,
                egui::CornerRadius::same(8),
                egui::Color32::from_rgb(18, 18, 22),
            );
            ui.painter().rect_stroke(
                panel_rect,
                egui::CornerRadius::same(8),
                egui::Stroke::new(1.0, egui::Color32::from_gray(60)),
                egui::epaint::StrokeKind::Outside,
            );
            ui.allocate_ui_with_layout(
                egui::vec2(panel_w, panel_h),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    let style = ui.style_mut();
                    style.text_styles.insert(
                        egui::TextStyle::Heading,
                        egui::FontId::new(20.0, egui::FontFamily::Proportional),
                    );
                    style.text_styles.insert(
                        egui::TextStyle::Body,
                        egui::FontId::new(16.0, egui::FontFamily::Proportional),
                    );
                    style.text_styles.insert(
                        egui::TextStyle::Button,
                        egui::FontId::new(16.0, egui::FontFamily::Proportional),
                    );
                    // 配色（fg_stroke / 底框 / popup / slider 軌）一律走全域
                    // 深色主題 install_visuals，不在此重設。

                    ui.add_space(16.0);
                    ui.horizontal(|ui| {
                        ui.add_space(20.0);
                        ui.heading("Settings");
                    });
                    ui.add_space(8.0);
                    ui.separator();

                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.add_space(12.0);

                            // ── Audio Output Device（即時套用 + 自動存檔）──
                            ui.horizontal(|ui| {
                                ui.add_space(20.0);
                                ui.label(egui::RichText::new("Audio Output Device").strong());
                            });
                            ui.add_space(4.0);
                            ui.horizontal(|ui| {
                                ui.add_space(20.0);
                                let current_label = self
                                    .settings
                                    .audio_device_name
                                    .clone()
                                    .unwrap_or_else(|| "System Default".to_string());
                                let mut new_choice: Option<Option<String>> = None;
                                egui::ComboBox::from_id_salt("mado_audio_dev_combo")
                                    .selected_text(current_label)
                                    .width(panel_w - 80.0)
                                    .show_ui(ui, |ui| {
                                        if ui
                                            .selectable_label(
                                                self.settings.audio_device_name.is_none(),
                                                "System Default",
                                            )
                                            .clicked()
                                        {
                                            new_choice = Some(None);
                                        }
                                        for d in &self.audio_devices {
                                            let sel = self
                                                .settings
                                                .audio_device_name
                                                .as_deref()
                                                == Some(d.name.as_str());
                                            if ui
                                                .selectable_label(sel, &d.name)
                                                .clicked()
                                            {
                                                new_choice = Some(Some(d.name.clone()));
                                            }
                                        }
                                    });
                                if let Some(v) = new_choice {
                                    if v != self.settings.audio_device_name {
                                        self.settings.audio_device_name = v;
                                        // 立即存檔
                                        if let Err(e) = self.settings.save() {
                                            eprintln!("[mado] settings save failed: {}", e);
                                        }
                                        // 切換裝置需重開 audio output（僅 Video 模式有 stream）
                                        if self.mode == Mode::Video {
                                            self.restart_service();
                                        }
                                    }
                                }
                            });

                            ui.add_space(12.0);

                            // ── Volume（即時 set_volume + 放開存檔）──
                            ui.horizontal(|ui| {
                                ui.add_space(20.0);
                                ui.label(egui::RichText::new("Volume").strong());
                            });
                            ui.add_space(4.0);
                            ui.horizontal(|ui| {
                                ui.add_space(20.0);
                                let mut v = self.settings.volume;
                                let response = ui.add(
                                    egui::Slider::new(&mut v, 0.0..=1.0)
                                        .show_value(true)
                                        .fixed_decimals(2),
                                );
                                if response.changed() {
                                    self.settings.volume = v;
                                    // 即時改音量（走 Mutex，不重開 stream）
                                    if let Some(out) = &self.audio_output {
                                        out.set_volume(v);
                                    }
                                }
                                // 放開 / 失焦才存檔，避免每幀寫磁碟
                                if response.drag_stopped() || response.lost_focus() {
                                    if let Err(e) = self.settings.save() {
                                        eprintln!("[mado] settings save failed: {}", e);
                                    }
                                }
                            });

                            ui.add_space(20.0);
                            ui.separator();
                            ui.add_space(12.0);

                            // ── Manual（操作說明）──
                            ui.horizontal(|ui| {
                                ui.add_space(20.0);
                                ui.label(egui::RichText::new("Manual").strong());
                            });
                            ui.horizontal(|ui| {
                                ui.add_space(20.0);
                                ui.label(MANUAL_TEXT);
                            });
                            ui.add_space(16.0);
                        });
                },
            );
        });

        // 點面板外部關閉：遮罩 block 被點到（panel 內 widget / ComboBox popup
        // 會優先消費點擊，使遮罩不觸發 clicked），且指標確實落在 panel_rect 之外。
        // 雙重條件（block.clicked + 幾何 hit-test）避免點面板內部誤關。
        if bg_clicked {
            let outside = ctx
                .input(|i| i.pointer.interact_pos())
                .map(|p| !panel_rect.contains(p))
                .unwrap_or(true);
            if outside {
                self.show_settings = false;
            }
        }
    }

    fn draw_playlist_panel(&mut self, ctx: &egui::Context, rect: egui::Rect) {
        let panel_h = (rect.height() * 0.5).clamp(160.0, 360.0);
        let panel_rect = egui::Rect::from_min_size(
            egui::pos2(rect.min.x, rect.max.y - panel_h),
            egui::vec2(rect.width(), panel_h),
        );
        let area = egui::Area::new(egui::Id::new("mado_playlist_panel"))
            .fixed_pos(panel_rect.min)
            .order(egui::Order::Foreground);
        area.show(ctx, |ui| {
            ui.painter().rect_filled(
                panel_rect,
                egui::CornerRadius::ZERO,
                egui::Color32::from_black_alpha(190),
            );
            ui.allocate_ui_with_layout(
                egui::vec2(panel_rect.width(), panel_h),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.add_space(8.0);
                    let style = ui.style_mut();
                    style.text_styles.insert(
                        egui::TextStyle::Button,
                        egui::FontId::new(16.0, egui::FontFamily::Proportional),
                    );
                    style.text_styles.insert(
                        egui::TextStyle::Body,
                        egui::FontId::new(16.0, egui::FontFamily::Proportional),
                    );
                    // 配色 fg_stroke（inactive 亮灰 / hover-active 珊瑚粉）走全域
                    // 深色主題 install_visuals。此面板背景為半透明黑
                    // from_black_alpha(190)（比 PANEL 更暗），button 用全域實心
                    // ELEVATED 底會浮出方塊，破壞此處刻意的扁平風格。故此面板
                    // 維持原扁平覆寫：inactive 透明（只顯示文字）、hover/active
                    // 低透明深色回饋；描邊一律無（不要全域珊瑚粉描邊框）。
                    style.visuals.widgets.inactive.weak_bg_fill =
                        egui::Color32::TRANSPARENT;
                    style.visuals.widgets.inactive.bg_fill =
                        egui::Color32::TRANSPARENT;
                    style.visuals.widgets.inactive.bg_stroke = egui::Stroke::NONE;
                    for w in [
                        &mut style.visuals.widgets.hovered,
                        &mut style.visuals.widgets.active,
                    ] {
                        w.weak_bg_fill = egui::Color32::from_black_alpha(140);
                        w.bg_fill = egui::Color32::from_black_alpha(140);
                        w.bg_stroke = egui::Stroke::NONE;
                    }

                    ui.horizontal(|ui| {
                        ui.add_space(12.0);
                        ui.label(format!("Playlist · {} items", self.playlist.len()));
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                ui.add_space(12.0);
                                if ui.button("Clear All").clicked() {
                                    self.playlist.clear();
                                    self.show_playlist = false;
                                    if self.mode == Mode::Video {
                                        self.switch_to_camera();
                                    }
                                }
                            },
                        );
                    });
                    ui.separator();

                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            let mut play_idx: Option<usize> = None;
                            let mut remove_idx: Option<usize> = None;
                            let mut move_up_idx: Option<usize> = None;
                            let mut move_down_idx: Option<usize> = None;
                            let mut toggle_h_idx: Option<usize> = None;
                            let mut toggle_v_idx: Option<usize> = None;
                            let current = self.playlist.current;
                            let total = self.playlist.items.len();
                            let in_video = self.mode == Mode::Video;
                            for (i, it) in self.playlist.items.iter().enumerate() {
                                let p = &it.path;
                                let item_flip_h = it.flip_h;
                                let item_flip_v = it.flip_v;
                                ui.horizontal(|ui| {
                                    ui.add_space(12.0);
                                    let playing = in_video && i == current;
                                    let marker = if playing { "▶" } else { " " };
                                    let name = p
                                        .file_name()
                                        .and_then(|s| s.to_str())
                                        .unwrap_or("—");
                                    let label = format!("{}  {:>2}. {}", marker, i + 1, name);
                                    // 右側按鈕先預留：Remove / ▼ / ▲（right_to_left 順序）
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            ui.add_space(12.0);
                                            if ui.button("Remove").clicked() {
                                                remove_idx = Some(i);
                                            }
                                            ui.add_space(4.0);
                                            let can_down = i + 1 < total;
                                            if ui
                                                .add_enabled(
                                                    can_down,
                                                    egui::Button::new("▼"),
                                                )
                                                .clicked()
                                            {
                                                move_down_idx = Some(i);
                                            }
                                            let can_up = i > 0;
                                            if ui
                                                .add_enabled(
                                                    can_up,
                                                    egui::Button::new("▲"),
                                                )
                                                .clicked()
                                            {
                                                move_up_idx = Some(i);
                                            }
                                            ui.add_space(4.0);
                                            // 上下 / 左右翻轉圖示（▲▼ 左邊）
                                            if ui
                                                .selectable_label(item_flip_v, "↕")
                                                .clicked()
                                            {
                                                toggle_v_idx = Some(i);
                                            }
                                            if ui
                                                .selectable_label(item_flip_h, "↔")
                                                .clicked()
                                            {
                                                toggle_h_idx = Some(i);
                                            }
                                            ui.add_space(4.0);
                                            // 剩餘空間給檔名（selectable）
                                            ui.with_layout(
                                                egui::Layout::left_to_right(
                                                    egui::Align::Center,
                                                ),
                                                |ui| {
                                                    if ui
                                                        .selectable_label(playing, label)
                                                        .clicked()
                                                    {
                                                        play_idx = Some(i);
                                                    }
                                                },
                                            );
                                        },
                                    );
                                });
                            }
                            if let Some(i) = toggle_h_idx {
                                if let Some(it) = self.playlist.items.get_mut(i) {
                                    it.flip_h = !it.flip_h;
                                }
                            }
                            if let Some(i) = toggle_v_idx {
                                if let Some(it) = self.playlist.items.get_mut(i) {
                                    it.flip_v = !it.flip_v;
                                }
                            }
                            if let Some(i) = move_up_idx {
                                self.playlist.move_up(i);
                            }
                            if let Some(i) = move_down_idx {
                                self.playlist.move_down(i);
                            }
                            if let Some(i) = play_idx {
                                if self.playlist.select(i) {
                                    self.mode = Mode::Video;
                                    self.restart_service();
                                }
                            }
                            if let Some(i) = remove_idx {
                                let was_current = i == self.playlist.current;
                                self.playlist.remove(i);
                                if self.playlist.is_empty() {
                                    self.show_playlist = false;
                                    if self.mode == Mode::Video {
                                        self.switch_to_camera();
                                    }
                                } else if was_current && self.mode == Mode::Video {
                                    self.restart_service();
                                }
                            }
                        });
                },
            );
        });

        // 點面板外部關閉：本幀有 pointer press 且座標落在 panel_rect 之外。
        // panel 內的 ▲▼ / Remove / per-clip flip / Clear All / 捲動，其 press
        // 座標都在 panel_rect 內，幾何 hit-test 保護不誤關。
        // 開啟當幀（playlist_just_opened）跳過判定，避免開啟同幀工具列 press 立即關閉。
        if self.playlist_just_opened {
            self.playlist_just_opened = false;
        } else {
            let pressed_outside = ctx.input(|i| {
                i.pointer.any_pressed()
                    && i
                        .pointer
                        .interact_pos()
                        .map(|p| !panel_rect.contains(p))
                        .unwrap_or(false)
            });
            if pressed_outside {
                self.show_playlist = false;
            }
        }
    }
}

fn uv_from_flip(flip_h: bool, flip_v: bool) -> (egui::Pos2, egui::Pos2) {
    let (u0, u1) = if flip_h { (1.0, 0.0) } else { (0.0, 1.0) };
    let (v0, v1) = if flip_v { (1.0, 0.0) } else { (0.0, 1.0) };
    (egui::pos2(u0, v0), egui::pos2(u1, v1))
}

/// 計算 camera 紋理 aspect-fit 至 rect（letterbox）。
fn build_textured_mesh(
    tex: egui::TextureId,
    rect: egui::Rect,
    uv_min: egui::Pos2,
    uv_max: egui::Pos2,
) -> egui::Mesh {
    let mut mesh = egui::Mesh::with_texture(tex);
    // 不做 letterbox，直接 fill 整個區域（規格：filling 整個 client area）
    let uvs = [
        egui::pos2(uv_min.x, uv_min.y),
        egui::pos2(uv_max.x, uv_min.y),
        egui::pos2(uv_max.x, uv_max.y),
        egui::pos2(uv_min.x, uv_max.y),
    ];
    let pts = [
        rect.left_top(),
        rect.right_top(),
        rect.right_bottom(),
        rect.left_bottom(),
    ];
    let color = egui::Color32::WHITE;
    for (p, uv) in pts.iter().zip(uvs.iter()) {
        mesh.vertices.push(egui::epaint::Vertex {
            pos: *p,
            uv: *uv,
            color,
        });
    }
    mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
    mesh
}
