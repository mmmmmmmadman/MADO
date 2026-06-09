//! 使用者設定（JSON 存檔於 ~/Library/Application Support/MADO/settings.json）。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    /// 音訊輸出裝置（CoreAudio UID，None = 系統預設）
    pub audio_device_name: Option<String>,
    /// 音量 0.0 ~ 1.0
    pub volume: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            audio_device_name: None,
            volume: 0.8,
        }
    }
}

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

pub fn settings_path() -> PathBuf {
    config_dir().join("settings.json")
}

impl Settings {
    pub fn load() -> Self {
        let path = settings_path();
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(_) => return Self::default(),
        };
        match serde_json::from_slice::<Settings>(&bytes) {
            Ok(mut s) => {
                s.volume = s.volume.clamp(0.0, 1.0);
                s
            }
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) -> Result<(), String> {
        let dir = config_dir();
        std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir: {}", e))?;
        let path = settings_path();
        let json =
            serde_json::to_string_pretty(self).map_err(|e| format!("serialize: {}", e))?;
        std::fs::write(&path, json).map_err(|e| format!("write: {}", e))
    }
}
