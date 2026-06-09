//! RtAudio 輸出 stream — 從 VideoPlayer 的 AudioRing 拉 stereo f32 samples 推到指定裝置。
//!
//! 對齊 Matrix AV Mapper callback-driven 架構（MAM CLAUDE.md「問題 4/5」）：
//! callback 直接呼叫 Rust extern "C" fn，不靠 sleep-based master thread。
//!
//! MADO 單 stream 用途（不需 MAM 的 master/secondary 多裝置混音架構），開一個 stream
//! 對到 settings 選的裝置 + 音量。

use crate::rtaudio_ffi::{RtAudio, RtStreamHandle};
use crate::video::{AudioRing, AUDIO_CH, AUDIO_SR};
use std::os::raw::{c_uint, c_void};
use std::sync::Arc;
use std::sync::Mutex;

const BUFFER_SIZE: u32 = 512;

struct CallbackCtx {
    ring: Arc<AudioRing>,
    volume: Mutex<f32>,
}

pub struct AudioOutput {
    rt: Arc<RtAudio>,
    stream: RtStreamHandle,
    ctx: *mut CallbackCtx,
}

// SAFETY: RtAudio handle + ctx pointer 只在 callback / drop 使用；callback 由 hardware
// thread 呼叫不會與 owner 衝突，drop 時先 stop 再 free。
unsafe impl Send for AudioOutput {}
unsafe impl Sync for AudioOutput {}

unsafe extern "C" fn process_cb(
    left_out: *mut f32,
    right_out: *mut f32,
    frames: c_uint,
    user_data: *mut c_void,
    _device_id: c_uint,
    _channel_offset: c_uint,
) {
    if user_data.is_null() {
        return;
    }
    let ctx = &*(user_data as *const CallbackCtx);
    let n = frames as usize;
    let l = std::slice::from_raw_parts_mut(left_out, n);
    let r = std::slice::from_raw_parts_mut(right_out, n);
    let vol = *ctx.volume.lock().unwrap_or_else(|p| p.into_inner());
    ctx.ring.pop_into(l, r, vol);
}

impl AudioOutput {
    /// 開 RtAudio output stream 到指定 device_id（None = default device）。
    pub fn open(
        device_name: Option<&str>,
        volume: f32,
        ring: Arc<AudioRing>,
    ) -> Result<Self, String> {
        let rt = Arc::new(RtAudio::new()?);
        let device_id = match device_name {
            Some(n) if !n.is_empty() => crate::audio::find_device_id_by_name(n).ok_or_else(
                || format!("device not found: {}", n),
            )?,
            _ => {
                // 找 default output
                let devs = crate::audio::list_output_devices();
                devs.into_iter()
                    .next()
                    .map(|d| d.id)
                    .ok_or_else(|| "no output device".to_string())?
            }
        };

        let stream = rt
            .open_stream(
                device_id,
                AUDIO_CH,
                AUDIO_SR,
                BUFFER_SIZE,
                0,
                true,
            )
            .ok_or_else(|| format!("open_stream device_id={}", device_id))?;

        let ctx = Box::into_raw(Box::new(CallbackCtx {
            ring,
            volume: Mutex::new(volume.clamp(0.0, 1.0)),
        }));

        rt.set_process_callback(stream, process_cb, ctx as *mut c_void);
        rt.start_stream(stream)?;

        log::info!(
            "[audio_out] opened device_id={} ({}) sr={} ch={}",
            device_id,
            device_name.unwrap_or("<default>"),
            AUDIO_SR,
            AUDIO_CH
        );

        Ok(Self { rt, stream, ctx })
    }

    pub fn set_volume(&self, v: f32) {
        unsafe {
            if !self.ctx.is_null() {
                let ctx = &*self.ctx;
                if let Ok(mut g) = ctx.volume.lock() {
                    *g = v.clamp(0.0, 1.0);
                }
            }
        }
    }
}

impl Drop for AudioOutput {
    fn drop(&mut self) {
        let _ = self.rt.stop_stream(self.stream);
        self.rt.close_stream(self.stream);
        if !self.ctx.is_null() {
            unsafe {
                drop(Box::from_raw(self.ctx));
            }
            self.ctx = std::ptr::null_mut();
        }
    }
}
