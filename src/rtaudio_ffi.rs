//! RtAudio FFI bindings
//! Low-level bindings to RtAudio C wrapper

use std::ffi::CStr;
use std::os::raw::{c_char, c_int, c_uint, c_void};

#[repr(C)]
pub struct CDeviceInfo {
    pub id: c_uint,
    pub name: [c_char; 256],
    pub output_channels: c_uint,
    pub input_channels: c_uint,
    pub sample_rate: c_uint,
    pub is_default: c_int,
}

impl CDeviceInfo {
    pub fn name_str(&self) -> String {
        unsafe {
            CStr::from_ptr(self.name.as_ptr())
                .to_string_lossy()
                .into_owned()
        }
    }
}

pub type RtAudioHandle = *mut c_void;
pub type RtStreamHandle = *mut c_void;

// Process callback: Rust function that generates audio
// Now includes device_id and channel_offset for per-device filtering
pub type ProcessCallback = unsafe extern "C" fn(
    left_out: *mut f32,
    right_out: *mut f32,
    frames: c_uint,
    user_data: *mut c_void,
    device_id: c_uint,
    channel_offset: c_uint,
);

extern "C" {
    pub fn rtaudio_create() -> RtAudioHandle;
    pub fn rtaudio_destroy(handle: RtAudioHandle);

    pub fn rtaudio_get_device_count(handle: RtAudioHandle) -> c_uint;
    pub fn rtaudio_get_device_ids(
        handle: RtAudioHandle,
        ids: *mut c_uint,
        max_count: c_uint,
    ) -> c_int;
    pub fn rtaudio_get_device_info(
        handle: RtAudioHandle,
        device_id: c_uint,
        info: *mut CDeviceInfo,
    ) -> c_int;

    pub fn rtaudio_open_stream(
        handle: RtAudioHandle,
        device_id: c_uint,
        num_channels: c_uint,
        sample_rate: c_uint,
        buffer_size: c_uint,
        channel_offset: c_uint,
        is_master: c_int,
    ) -> RtStreamHandle;

    pub fn rtaudio_close_stream(handle: RtAudioHandle, stream: RtStreamHandle);
    pub fn rtaudio_start_stream(handle: RtAudioHandle, stream: RtStreamHandle) -> c_int;
    pub fn rtaudio_stop_stream(stream: RtStreamHandle) -> c_int;

    pub fn rtaudio_set_process_callback(
        stream: RtStreamHandle,
        callback: ProcessCallback,
        user_data: *mut c_void,
    );

    pub fn rtaudio_add_secondary_stream(
        master_stream: RtStreamHandle,
        secondary_stream: RtStreamHandle,
    );

    pub fn rtaudio_remove_secondary_stream(
        master_stream: RtStreamHandle,
        secondary_stream: RtStreamHandle,
    );
}

/// Safe wrapper around RtAudio handle
pub struct RtAudio {
    handle: RtAudioHandle,
}

impl RtAudio {
    pub fn new() -> Result<Self, String> {
        let handle = unsafe { rtaudio_create() };
        if handle.is_null() {
            Err("Failed to create RtAudio instance".to_string())
        } else {
            Ok(Self { handle })
        }
    }

    pub fn handle(&self) -> RtAudioHandle {
        self.handle
    }

    pub fn get_device_count(&self) -> u32 {
        unsafe { rtaudio_get_device_count(self.handle) }
    }

    pub fn get_device_ids(&self) -> Vec<u32> {
        let count = self.get_device_count();
        if count == 0 {
            return Vec::new();
        }

        let mut ids = vec![0u32; count as usize];
        let result = unsafe {
            rtaudio_get_device_ids(self.handle, ids.as_mut_ptr(), count)
        };

        if result < 0 {
            Vec::new()
        } else {
            ids.truncate(result as usize);
            ids
        }
    }

    pub fn get_device_info(&self, device_id: u32) -> Option<CDeviceInfo> {
        let mut info = CDeviceInfo {
            id: 0,
            name: [0; 256],
            output_channels: 0,
            input_channels: 0,
            sample_rate: 0,
            is_default: 0,
        };

        let result = unsafe {
            rtaudio_get_device_info(self.handle, device_id, &mut info)
        };

        if result == 0 {
            Some(info)
        } else {
            None
        }
    }

    pub fn open_stream(
        &self,
        device_id: u32,
        num_channels: u32,
        sample_rate: u32,
        buffer_size: u32,
        channel_offset: u32,
        is_master: bool,
    ) -> Option<RtStreamHandle> {
        let stream = unsafe {
            rtaudio_open_stream(
                self.handle,
                device_id,
                num_channels,
                sample_rate,
                buffer_size,
                channel_offset,
                if is_master { 1 } else { 0 },
            )
        };

        if stream.is_null() {
            None
        } else {
            Some(stream)
        }
    }

    pub fn close_stream(&self, stream: RtStreamHandle) {
        unsafe { rtaudio_close_stream(self.handle, stream) }
    }

    pub fn start_stream(&self, stream: RtStreamHandle) -> Result<(), String> {
        let result = unsafe { rtaudio_start_stream(self.handle, stream) };
        if result == 0 {
            Ok(())
        } else {
            Err("Failed to start stream".to_string())
        }
    }

    pub fn stop_stream(&self, stream: RtStreamHandle) -> Result<(), String> {
        let result = unsafe { rtaudio_stop_stream(stream) };
        if result == 0 {
            Ok(())
        } else {
            Err("Failed to stop stream".to_string())
        }
    }

    pub fn set_process_callback(
        &self,
        stream: RtStreamHandle,
        callback: ProcessCallback,
        user_data: *mut c_void,
    ) {
        unsafe { rtaudio_set_process_callback(stream, callback, user_data) }
    }

    pub fn add_secondary_stream(&self, master: RtStreamHandle, secondary: RtStreamHandle) {
        unsafe { rtaudio_add_secondary_stream(master, secondary) }
    }

    pub fn remove_secondary_stream(&self, master: RtStreamHandle, secondary: RtStreamHandle) {
        unsafe { rtaudio_remove_secondary_stream(master, secondary) }
    }
}

impl Drop for RtAudio {
    fn drop(&mut self) {
        unsafe { rtaudio_destroy(self.handle) }
    }
}

// SAFETY: RtAudio handle is thread-safe
unsafe impl Send for RtAudio {}
unsafe impl Sync for RtAudio {}
