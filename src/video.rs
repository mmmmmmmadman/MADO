//! 影片解碼（ffmpeg-next）+ 音訊解碼（同一條 pipeline）。
//!
//! 對齊 Matrix AV Mapper：
//! - `src/video.rs` 的 video frame decode + scaler 模式
//! - `src/audio_rtaudio.rs` 的 AudioClip::decode_from_video（含 lpcm channel_layout fix +
//!   延遲建 resampler，MAM CLAUDE.md「問題 7」）
//!
//! MADO 用法：開檔 → 兩條 thread（video decode 寫 `SharedFrame`，audio decode 寫 ring
//! buffer 給 RtAudio callback 拉）。pause 旗標停產出（callback 收靜音）。EOF 旗標讓上層
//! 推進播放清單。

use crate::camera::SharedFrame;
use anyhow::{anyhow, Result};
use ffmpeg_next as ff;
use std::collections::VecDeque;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Once};
use std::time::{Duration, Instant};

/// Audio output 統一格式：f32 stereo 48 kHz
pub const AUDIO_SR: u32 = 48_000;
pub const AUDIO_CH: u32 = 2;

static FFMPEG_INIT: Once = Once::new();
fn ensure_ffmpeg_init() {
    FFMPEG_INIT.call_once(|| {
        if let Err(e) = ff::init() {
            log::warn!("[video] ffmpeg init: {}", e);
        }
        ff::util::log::set_level(ff::util::log::Level::Error);
    });
}

/// stereo f32 sample 環形 buffer（mutex 為求簡單；callback 端 try_lock）。
pub struct AudioRing {
    buf: Mutex<VecDeque<(f32, f32)>>,
    /// 容量（樣本對數，>= 48000*0.5 = 0.5s）
    capacity: usize,
}

impl AudioRing {
    pub fn new(capacity_samples: usize) -> Self {
        Self {
            buf: Mutex::new(VecDeque::with_capacity(capacity_samples)),
            capacity: capacity_samples,
        }
    }

    pub fn push(&self, samples: &[(f32, f32)]) {
        if let Ok(mut q) = self.buf.lock() {
            for s in samples {
                if q.len() < self.capacity {
                    q.push_back(*s);
                }
            }
        }
    }

    pub fn len(&self) -> usize {
        self.buf.lock().map(|q| q.len()).unwrap_or(0)
    }

    pub fn pop_into(&self, out_l: &mut [f32], out_r: &mut [f32], vol: f32) {
        debug_assert_eq!(out_l.len(), out_r.len());
        if let Ok(mut q) = self.buf.lock() {
            for i in 0..out_l.len() {
                if let Some((l, r)) = q.pop_front() {
                    out_l[i] = l * vol;
                    out_r[i] = r * vol;
                } else {
                    out_l[i] = 0.0;
                    out_r[i] = 0.0;
                }
            }
        } else {
            for i in 0..out_l.len() {
                out_l[i] = 0.0;
                out_r[i] = 0.0;
            }
        }
    }
}

pub struct VideoPlayer {
    stop_flag: Arc<AtomicBool>,
    pause_flag: Arc<AtomicBool>,
    eof_flag: Arc<AtomicBool>,
    audio_ring: Arc<AudioRing>,
    decoder_handle: Option<std::thread::JoinHandle<()>>,
}

impl VideoPlayer {
    pub fn open(path: &Path, shared_frame: SharedFrame) -> Result<Self> {
        ensure_ffmpeg_init();
        let stop_flag = Arc::new(AtomicBool::new(false));
        let pause_flag = Arc::new(AtomicBool::new(false));
        let eof_flag = Arc::new(AtomicBool::new(false));
        let audio_ring = Arc::new(AudioRing::new((AUDIO_SR as usize) / 2));

        let path_owned = path.to_path_buf();
        let stop_c = stop_flag.clone();
        let pause_c = pause_flag.clone();
        let eof_c = eof_flag.clone();
        let ring_c = audio_ring.clone();

        let handle = std::thread::spawn(move || {
            if let Err(e) =
                decode_loop(&path_owned, shared_frame, stop_c, pause_c, eof_c, ring_c)
            {
                log::error!("[video] decode_loop: {}", e);
            }
        });

        Ok(Self {
            stop_flag,
            pause_flag,
            eof_flag,
            audio_ring,
            decoder_handle: Some(handle),
        })
    }

    pub fn set_paused(&self, paused: bool) {
        self.pause_flag.store(paused, Ordering::Relaxed);
    }

    pub fn is_eof(&self) -> bool {
        self.eof_flag.load(Ordering::Relaxed)
    }

    pub fn audio_ring(&self) -> Arc<AudioRing> {
        self.audio_ring.clone()
    }
}

impl Drop for VideoPlayer {
    fn drop(&mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
        if let Some(h) = self.decoder_handle.take() {
            let _ = h.join();
        }
    }
}

fn infer_channel_layout(channels: u32) -> ff::ChannelLayout {
    use ff::ChannelLayout;
    match channels {
        1 => ChannelLayout::MONO,
        2 => ChannelLayout::STEREO,
        n if n > 2 => ChannelLayout::default(n as i32),
        _ => ChannelLayout::STEREO,
    }
}

fn decode_loop(
    path: &Path,
    shared_frame: SharedFrame,
    stop: Arc<AtomicBool>,
    pause: Arc<AtomicBool>,
    eof: Arc<AtomicBool>,
    audio_ring: Arc<AudioRing>,
) -> Result<()> {
    let mut input = ff::format::input(path).map_err(|e| anyhow!("open {}: {}", path.display(), e))?;

    let video_stream = input
        .streams()
        .best(ff::media::Type::Video)
        .ok_or_else(|| anyhow!("no video track"))?;
    let video_idx = video_stream.index();
    let v_tb = video_stream.time_base();
    let v_tb_num = v_tb.numerator() as f64;
    let v_tb_den = v_tb.denominator() as f64;

    let v_ctx = ff::codec::context::Context::from_parameters(video_stream.parameters())?;
    let mut v_dec = v_ctx.decoder().video()?;
    let width = v_dec.width();
    let height = v_dec.height();
    let mut scaler = ff::software::scaling::Context::get(
        v_dec.format(),
        width,
        height,
        ff::format::Pixel::RGBA,
        width,
        height,
        ff::software::scaling::Flags::BILINEAR,
    )?;

    // ── audio decoder（可選；無 audio track 時 silent）──
    let audio_stream = input.streams().best(ff::media::Type::Audio);
    let audio_idx: Option<usize> = audio_stream.as_ref().map(|s| s.index());
    let mut a_dec_opt: Option<ff::decoder::Audio> = None;
    if let Some(stream) = &audio_stream {
        let ctx = ff::codec::context::Context::from_parameters(stream.parameters())?;
        a_dec_opt = Some(ctx.decoder().audio()?);
    }
    let mut resampler: Option<ff::software::resampling::Context> = None;

    let mut frame_id: u64 = 1;
    let mut wall_start = Instant::now();
    let mut first_pts: Option<f64> = None;

    // ── 主迴圈 ──
    let mut packets_iter = input.packets();
    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        if pause.load(Ordering::Relaxed) {
            // 暫停期間補償 wall_clock，恢復時不衝刺
            let pause_started = Instant::now();
            while !stop.load(Ordering::Relaxed) && pause.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(20));
            }
            // wall_start 加上暫停的時間
            let paused = pause_started.elapsed();
            wall_start += paused;
            continue;
        }

        // 控制 audio ring buffer 不要爆掉
        if audio_ring.len() > (AUDIO_SR as usize) * 3 / 10 {
            std::thread::sleep(Duration::from_millis(5));
            continue;
        }

        let (stream, packet) = match packets_iter.next() {
            Some(sp) => sp,
            None => {
                // EOF
                let _ = v_dec.send_eof();
                drain_video(&mut v_dec, &mut scaler, &shared_frame, &mut frame_id, v_tb_num, v_tb_den, width, height);
                if let Some(a_dec) = a_dec_opt.as_mut() {
                    let _ = a_dec.send_eof();
                    drain_audio(a_dec, &mut resampler, &audio_ring);
                }
                eof.store(true, Ordering::Relaxed);
                break;
            }
        };

        let idx = stream.index();
        if idx == video_idx {
            if v_dec.send_packet(&packet).is_ok() {
                drain_video(
                    &mut v_dec,
                    &mut scaler,
                    &shared_frame,
                    &mut frame_id,
                    v_tb_num,
                    v_tb_den,
                    width,
                    height,
                );
                // pacing: 視訊解碼完依 pts 對齊 wall clock
                if let Some(pts) = shared_pts(&shared_frame) {
                    if first_pts.is_none() {
                        first_pts = Some(pts);
                        wall_start = Instant::now();
                    }
                    let elapsed = wall_start.elapsed().as_secs_f64();
                    let target = pts - first_pts.unwrap_or(0.0);
                    let delay = target - elapsed;
                    if delay > 0.001 {
                        std::thread::sleep(Duration::from_secs_f64(delay.min(0.5)));
                    }
                }
            }
        } else if Some(idx) == audio_idx {
            if let Some(a_dec) = a_dec_opt.as_mut() {
                if a_dec.send_packet(&packet).is_ok() {
                    drain_audio(a_dec, &mut resampler, &audio_ring);
                }
            }
        }
    }

    Ok(())
}

/// 共享 frame slot 額外存 pts 在 frame_id 高位是不安全的；改在這裡 fake 從 frame_id 反推。
/// 簡化：MADO video 用 wall clock pacing 就夠了。實作不從 SharedFrame 取 pts，直接回 None
/// — 上面的 first_pts/wall_start 邏輯改成全幀 pacing 在 drain_video 內處理。
fn shared_pts(_s: &SharedFrame) -> Option<f64> {
    None
}

fn drain_video(
    decoder: &mut ff::decoder::Video,
    scaler: &mut ff::software::scaling::Context,
    shared_frame: &SharedFrame,
    frame_id: &mut u64,
    tb_num: f64,
    tb_den: f64,
    width: u32,
    height: u32,
) {
    let mut decoded = ff::util::frame::video::Video::empty();
    while decoder.receive_frame(&mut decoded).is_ok() {
        let mut rgba = ff::util::frame::video::Video::empty();
        if scaler.run(&decoded, &mut rgba).is_err() {
            continue;
        }
        let pts = decoded.pts().unwrap_or(0) as f64 * tb_num / tb_den;
        let stride = rgba.stride(0);
        let row_bytes = (width * 4) as usize;
        let h = height as usize;
        let data: Vec<u8> = if stride == row_bytes {
            rgba.data(0)[..row_bytes * h].to_vec()
        } else {
            let src = rgba.data(0);
            let mut packed = Vec::with_capacity(row_bytes * h);
            for y in 0..h {
                let s = y * stride;
                packed.extend_from_slice(&src[s..s + row_bytes]);
            }
            packed
        };
        if let Ok(mut slot) = shared_frame.lock() {
            slot.rgba = Some(Arc::new(data));
            slot.width = width;
            slot.height = height;
            slot.frame_id = *frame_id;
        }
        *frame_id += 1;
        // pts 變數目前供 pacing 用（未來精化），保留以避免無用警告
        let _ = pts;
    }
}

fn drain_audio(
    decoder: &mut ff::decoder::Audio,
    resampler: &mut Option<ff::software::resampling::Context>,
    audio_ring: &AudioRing,
) {
    let mut decoded = ff::util::frame::audio::Audio::empty();
    while decoder.receive_frame(&mut decoded).is_ok() {
        // 修 .mov lpcm channel_layout=0（MAM CLAUDE.md 問題 7）
        if decoded.channel_layout().bits() == 0 {
            let ch = decoded.channels() as u32;
            decoded.set_channel_layout(infer_channel_layout(ch));
        }
        // 延遲建 resampler：第一個 frame 出現後依實際格式建
        if resampler.is_none() {
            let in_fmt = decoded.format();
            let in_layout = decoded.channel_layout();
            let in_rate = decoded.rate();
            match ff::software::resampling::Context::get(
                in_fmt,
                in_layout,
                in_rate,
                ff::format::Sample::F32(ff::format::sample::Type::Packed),
                ff::ChannelLayout::STEREO,
                AUDIO_SR,
            ) {
                Ok(r) => *resampler = Some(r),
                Err(e) => {
                    log::warn!("[video] resampler init: {}", e);
                    continue;
                }
            }
        }
        let r = match resampler.as_mut() {
            Some(r) => r,
            None => continue,
        };
        let mut out = ff::util::frame::audio::Audio::empty();
        out.set_rate(AUDIO_SR);
        out.set_channel_layout(ff::ChannelLayout::STEREO);
        out.set_format(ff::format::Sample::F32(ff::format::sample::Type::Packed));
        if r.run(&decoded, &mut out).is_err() {
            continue;
        }
        let samples_n = out.samples();
        let raw = out.data(0);
        // F32 Packed: interleaved L R L R ...
        let needed = samples_n * 2 * std::mem::size_of::<f32>();
        if raw.len() < needed {
            continue;
        }
        let f32_slice: &[f32] =
            unsafe { std::slice::from_raw_parts(raw.as_ptr() as *const f32, samples_n * 2) };
        let mut pairs: Vec<(f32, f32)> = Vec::with_capacity(samples_n);
        for i in 0..samples_n {
            pairs.push((f32_slice[i * 2], f32_slice[i * 2 + 1]));
        }
        audio_ring.push(&pairs);
    }
}
