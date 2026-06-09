//! 音訊輸出裝置列舉（RtAudio backend）。
//! 對齊 Matrix AV Mapper `src/audio_rtaudio.rs::list_audio_devices`：
//! 用 RtAudio 取代 cpal — cpal 在 macOS 漏列 External Headphones / 部分 HDMI 裝置
//! （MAM CLAUDE.md「問題 1」已驗證），改 RtAudio 取得完整裝置清單。
//!
//! 識別字：device name（RtAudio device id 隨 OS 重新分配不穩定，以 name 為使用者識別字）。

use crate::rtaudio_ffi::RtAudio;

#[derive(Debug, Clone)]
pub struct AudioDevice {
    pub id: u32,
    pub name: String,
    pub output_channels: u32,
    pub sample_rate: u32,
}

pub fn list_output_devices() -> Vec<AudioDevice> {
    let rt = match RtAudio::new() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[mado_audio] RtAudio create fail: {}", e);
            return Vec::new();
        }
    };
    let ids = rt.get_device_ids();
    eprintln!("[mado_audio] RtAudio reported {} device(s): {:?}", ids.len(), ids);
    let mut out = Vec::new();
    for id in ids {
        let info = match rt.get_device_info(id) {
            Some(i) => i,
            None => {
                eprintln!("[mado_audio] get_device_info({}) returned None", id);
                continue;
            }
        };
        eprintln!(
            "[mado_audio] dev id={} name={:?} out_ch={} in_ch={} sr={}",
            info.id,
            info.name_str(),
            info.output_channels,
            info.input_channels,
            info.sample_rate
        );
        if info.output_channels == 0 {
            continue;
        }
        out.push(AudioDevice {
            id: info.id,
            name: info.name_str(),
            output_channels: info.output_channels,
            sample_rate: info.sample_rate,
        });
    }
    eprintln!("[mado_audio] filtered to {} output device(s)", out.len());
    out
}

pub fn find_device_id_by_name(name: &str) -> Option<u32> {
    list_output_devices()
        .into_iter()
        .find(|d| d.name == name)
        .map(|d| d.id)
}
