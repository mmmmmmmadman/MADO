//! 相機列舉（system_profiler，複製自 VisionMod core/src/camera/capture.rs::list_cameras_macos）
//! 與 Python service 啟動／IPC frame 讀取（複製自 VisionMod core/src/ipc.rs BinHeaderV1 格式）。

use anyhow::{anyhow, Result};
use std::io::Read;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct CameraInfo {
    pub index: u32,
    pub name: String,
    pub unique_id: String,
}

const SYSTEM_PROFILER_TIMEOUT: Duration = Duration::from_secs(5);

fn run_with_timeout(program: &str, args: &[&str], timeout: Duration) -> Result<Vec<u8>> {
    use wait_timeout::ChildExt;
    let mut child = Command::new(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| anyhow!("{} spawn fail: {}", program, e))?;
    match child.wait_timeout(timeout)? {
        Some(status) => {
            if !status.success() {
                return Err(anyhow!("{} exit {}", program, status));
            }
            let mut buf = Vec::new();
            if let Some(mut out) = child.stdout.take() {
                out.read_to_end(&mut buf)?;
            }
            Ok(buf)
        }
        None => {
            let _ = child.kill();
            let _ = child.wait();
            Err(anyhow!("{} timeout", program))
        }
    }
}

#[cfg(target_os = "macos")]
pub fn list_cameras() -> Result<Vec<CameraInfo>> {
    let stdout = run_with_timeout(
        "system_profiler",
        &["SPCameraDataType", "-json"],
        SYSTEM_PROFILER_TIMEOUT,
    )?;
    let json: serde_json::Value = serde_json::from_slice(&stdout)
        .map_err(|e| anyhow!("system_profiler JSON: {}", e))?;
    let arr = json
        .get("SPCameraDataType")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow!("no SPCameraDataType array"))?;
    let cameras: Vec<CameraInfo> = arr
        .iter()
        .enumerate()
        .map(|(idx, cam)| {
            let name = cam
                .get("_name")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("Camera {}", idx));
            let unique_id = cam
                .get("spcamera_unique-id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_default();
            CameraInfo {
                index: idx as u32,
                name,
                unique_id,
            }
        })
        .collect();
    if cameras.is_empty() {
        Ok(vec![CameraInfo {
            index: 0,
            name: "Camera 0".to_string(),
            unique_id: String::new(),
        }])
    } else {
        Ok(cameras)
    }
}

#[cfg(not(target_os = "macos"))]
pub fn list_cameras() -> Result<Vec<CameraInfo>> {
    Ok(vec![CameraInfo {
        index: 0,
        name: "Camera 0".to_string(),
        unique_id: String::new(),
    }])
}

// ── BinHeaderV1（複製自 VisionMod core/src/ipc.rs）──
pub const BIN_MAGIC: u32 = 0x564D_4442;
pub const BIN_VERSION_V1: u32 = 1;
pub const BIN_HEADER_V1_SIZE: usize = 48;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct BinHeaderV1 {
    pub magic: u32,
    pub version: u32,
    pub frame_id: u64,
    pub wall_ns: u64,
    pub source_kind: u32,
    pub payload_format: u32,
    pub payload_count: u32,
    pub width: u32,
    pub height: u32,
    pub reserved: u32,
}

pub fn read_bin_header_v1(bytes: &[u8]) -> Option<BinHeaderV1> {
    if bytes.len() < BIN_HEADER_V1_SIZE {
        return None;
    }
    let head: BinHeaderV1 = bytemuck::pod_read_unaligned(&bytes[..BIN_HEADER_V1_SIZE]);
    if head.magic != BIN_MAGIC || head.version != BIN_VERSION_V1 {
        return None;
    }
    #[cfg(target_os = "macos")]
    {
        if head.reserved != (head.frame_id as u32) {
            return None;
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        if head.reserved != 0 {
            return None;
        }
    }
    Some(head)
}

/// 最新 frame 共享 slot（producer thread 寫，UI thread 讀）。
#[derive(Default)]
pub struct FrameSlot {
    pub rgba: Option<Arc<Vec<u8>>>,
    pub width: u32,
    pub height: u32,
    pub frame_id: u64,
}

pub type SharedFrame = Arc<Mutex<FrameSlot>>;

/// 找可重定位 venv Python 直譯器，比照 VisionMod service.rs::detect_venv_python。
///
/// 搜尋順序：
///   1. exe 同層 .venv/bin/python（開發期 binary 旁）
///   2. ../Resources/.venv/bin/python（macOS .app bundle：exe = Contents/MacOS/mado）
///   3. exe 父層 .venv/bin/python
///   4. exe 祖父層 .venv/bin/python
///   5. CARGO_MANIFEST_DIR/.venv/bin/python（cargo run 開發期）
///   6. 系統 python3 fallback
fn detect_python() -> Result<PathBuf> {
    let rel = ".venv/bin/python";
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            // exe 同層（開發期）
            candidates.push(dir.join(rel));
            if let Some(parent) = dir.parent() {
                // macOS .app：Contents/MacOS/ → Contents/Resources/
                candidates.push(parent.join("Resources").join(rel));
                candidates.push(parent.join(rel));
                if let Some(grand) = parent.parent() {
                    candidates.push(grand.join(rel));
                }
            }
        }
    }
    candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel));
    for c in &candidates {
        if c.exists() {
            return Ok(c.clone());
        }
    }
    // 系統 python3 fallback（最後手段）
    Ok(PathBuf::from("python3"))
}

/// 解析 Python 腳本絕對路徑，比照 VisionMod service.rs::resolve_script_path。
///
/// 搜尋順序（腳本名為相對路徑時）：
///   1. exe 同層 scripts/<name>
///   2. ../Resources/scripts/<name>（macOS .app bundle）
///   3. CARGO_MANIFEST_DIR/scripts/<name>（cargo run 開發期）
fn resolve_script_path(script_name: &str) -> PathBuf {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("scripts").join(script_name));
            if let Some(parent) = dir.parent() {
                // macOS .app：Contents/MacOS/ → Contents/Resources/scripts/
                candidates.push(parent.join("Resources").join("scripts").join(script_name));
            }
        }
    }
    candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts").join(script_name));
    for c in &candidates {
        if c.exists() {
            return c.clone();
        }
    }
    // 找不到就回 CARGO_MANIFEST_DIR 路徑，讓 spawn 失敗暴露問題
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts").join(script_name)
}

pub fn frame_dir() -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push("mado");
    p.push("camera");
    p
}

/// spawn camera_service_mac.py，回傳 Child handle。
pub fn spawn_camera_service(
    unique_id: &str,
    fallback_index: u32,
    width: u32,
    height: u32,
    fps: u32,
) -> Result<Child> {
    let py = detect_python()?;
    let script = resolve_script_path("camera_service_mac.py");
    if !script.exists() {
        return Err(anyhow!("script not found: {}", script.display()));
    }
    let out_dir = frame_dir();
    std::fs::create_dir_all(&out_dir).ok();
    // 清舊 stop.txt
    let stop = out_dir.join("stop.txt");
    let _ = std::fs::remove_file(&stop);

    let mut cmd = Command::new(&py);
    cmd.arg(&script)
        .arg("--output_dir")
        .arg(&out_dir)
        .arg("--frame_width")
        .arg(width.to_string())
        .arg("--frame_height")
        .arg(height.to_string())
        .arg("--fps")
        .arg(fps.to_string());
    if !unique_id.is_empty() {
        cmd.arg("--camera_unique_id").arg(unique_id);
    } else {
        cmd.arg("--camera_index").arg(fallback_index.to_string());
    }
    cmd.stdout(Stdio::null()).stderr(Stdio::inherit());
    let child = cmd.spawn().map_err(|e| anyhow!("spawn py: {}", e))?;
    Ok(child)
}

pub fn request_stop() {
    let stop = frame_dir().join("stop.txt");
    let _ = std::fs::write(&stop, b"");
}

// video_service Python 端已於 2026-06-09 退役，改 Rust 端 ffmpeg-next + RtAudio。
// 此檔保留 camera service（相機預覽）spawn_camera_service / request_stop / SharedFrame。

/// reader thread：每 ~16ms 讀 frame.bin，比 frame_id 去重後寫 SharedFrame。
pub fn spawn_reader_thread(shared: SharedFrame, stop_flag: Arc<std::sync::atomic::AtomicBool>) {
    use std::sync::atomic::Ordering;
    std::thread::spawn(move || {
        let path = frame_dir().join("frame.bin");
        let mut last_id: u64 = 0;
        while !stop_flag.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(16));
            let bytes = match std::fs::read(&path) {
                Ok(b) => b,
                Err(_) => continue,
            };
            let head = match read_bin_header_v1(&bytes) {
                Some(h) => h,
                None => continue,
            };
            if head.frame_id == last_id {
                continue;
            }
            let payload_count = head.payload_count as usize;
            if bytes.len() < BIN_HEADER_V1_SIZE + payload_count {
                continue;
            }
            let payload = &bytes[BIN_HEADER_V1_SIZE..BIN_HEADER_V1_SIZE + payload_count];
            let rgba = Arc::new(payload.to_vec());
            last_id = head.frame_id;
            if let Ok(mut s) = shared.lock() {
                s.rgba = Some(rgba);
                s.width = head.width;
                s.height = head.height;
                s.frame_id = head.frame_id;
            }
        }
    });
}
