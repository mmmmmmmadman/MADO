/*
 * WAAASAABIII - RtAudio C Wrapper
 * Wraps RtAudio C++ API for Rust FFI
 */

#include <rtaudio/RtAudio.h>
#include <string.h>
#include <vector>
#include <map>
#include <mutex>
#include <atomic>

extern "C" {

// Opaque handle types
typedef void* RtAudioHandle;
typedef void* RtStreamHandle;

// Device info struct (C-compatible)
struct CDeviceInfo {
    unsigned int id;
    char name[256];
    unsigned int output_channels;
    unsigned int input_channels;
    unsigned int sample_rate;
    int is_default;
};

// Audio process callback: Rust function that generates audio
// Now includes device_id and channel_offset for per-device filtering
typedef void (*ProcessCallback)(float* left_out, float* right_out, unsigned int frames, void* user_data, unsigned int device_id, unsigned int channel_offset);

// Stream type (master vs secondary)
enum StreamType {
    STREAM_MASTER,
    STREAM_SECONDARY
};

// Secondary stream list (for master callback to write to)
struct SecondaryStreamNode {
    struct StreamData* stream;
    SecondaryStreamNode* next;
    std::atomic<bool> removed{false};  // Mark for safe removal (avoid dangling pointer)
};

// Stream data (includes its own RtAudio instance)
struct StreamData {
    RtAudio* rtaudio;  // Each stream has its own RtAudio instance
    StreamType stream_type;
    ProcessCallback process_callback;  // Rust audio processor (master only)
    void* user_data;

    // Lock-free ring buffer (secondary streams only)
    std::vector<float> ring_buffer;
    std::atomic<size_t> write_pos;
    std::atomic<size_t> read_pos;
    size_t ring_buffer_size;
    unsigned int num_channels;
    unsigned int device_channels;  // Total channels of device
    unsigned int channel_offset;   // Starting channel offset
    unsigned int buffer_size;      // Store original buffer size
    unsigned int device_id;        // Device ID for filtering tracks

    // Fade-in
    std::atomic<size_t> fade_in_remaining;

    // Pre-allocated temp buffers (master only)
    std::vector<float> temp_left;
    std::vector<float> temp_right;

    // Linked list of secondary streams (master only)
    std::atomic<SecondaryStreamNode*> secondary_streams;

    StreamData(unsigned int buf_size, unsigned int channels, unsigned int dev_channels,
               unsigned int offset, unsigned int dev_id, StreamType type)
        : rtaudio(nullptr)
        , stream_type(type)
        , process_callback(nullptr)
        , user_data(nullptr)
        , write_pos(0)
        , read_pos(0)
        , num_channels(channels)
        , device_channels(dev_channels)
        , channel_offset(offset)
        , buffer_size(buf_size)
        , device_id(dev_id)
        , fade_in_remaining(4)
        , secondary_streams(nullptr)
    {
        if (type == STREAM_MASTER) {
            // Master: allocate temp buffers for audio processing
            temp_left.resize(buf_size, 0.0f);
            temp_right.resize(buf_size, 0.0f);
        } else {
            // Secondary: allocate ring buffer AND temp buffers for processing
            ring_buffer_size = buf_size * channels * 8;  // 8x like KousatenMixer
            ring_buffer.resize(ring_buffer_size, 0.0f);
            write_pos.store(buf_size * channels * 2);  // Pre-fill 2 buffers
            temp_left.resize(buf_size, 0.0f);
            temp_right.resize(buf_size, 0.0f);
        }
    }

    ~StreamData() {
        // Clean up secondary streams list
        SecondaryStreamNode* node = secondary_streams.load();
        while (node) {
            SecondaryStreamNode* next = node->next;
            delete node;
            node = next;
        }

        if (rtaudio) {
            try {
                if (rtaudio->isStreamOpen()) {
                    if (rtaudio->isStreamRunning()) {
                        rtaudio->stopStream();
                    }
                    rtaudio->closeStream();
                }
            } catch (...) {}
            delete rtaudio;
        }
    }
};

// Master stream callback: calls Rust, writes to output and secondary streams
int rtaudio_master_callback(void* output_buffer, void* /*input_buffer*/,
                            unsigned int n_frames, double /*stream_time*/,
                            RtAudioStreamStatus /*status*/, void* user_data)
{
    auto* stream_data = static_cast<StreamData*>(user_data);
    auto* out = static_cast<float*>(output_buffer);

    // Clear entire output buffer first
    memset(out, 0, n_frames * stream_data->device_channels * sizeof(float));

    // Fade-in: gradually increase volume over first few callbacks
    size_t fade = stream_data->fade_in_remaining.load(std::memory_order_acquire);
    float fade_multiplier = 1.0f;
    if (fade > 0) {
        stream_data->fade_in_remaining.store(fade - 1, std::memory_order_release);
        fade_multiplier = (4.0f - fade) / 4.0f;  // 0.25, 0.5, 0.75, 1.0
    }

    // Call Rust audio processor to generate audio for THIS device
    if (stream_data->process_callback) {
        stream_data->process_callback(
            stream_data->temp_left.data(),
            stream_data->temp_right.data(),
            n_frames,
            stream_data->user_data,
            stream_data->device_id,
            stream_data->channel_offset
        );
    }

    // Write to master output buffer (with channel offset and fade)
    for (unsigned int frame = 0; frame < n_frames; ++frame) {
        unsigned int left_ch = stream_data->channel_offset;
        unsigned int right_ch = stream_data->channel_offset + 1;

        if (left_ch < stream_data->device_channels) {
            out[frame * stream_data->device_channels + left_ch] = stream_data->temp_left[frame] * fade_multiplier;
        }
        if (right_ch < stream_data->device_channels) {
            out[frame * stream_data->device_channels + right_ch] = stream_data->temp_right[frame] * fade_multiplier;
        }
    }

    // Write to all secondary streams (each gets its own filtered mix)
    SecondaryStreamNode* node = stream_data->secondary_streams.load(std::memory_order_acquire);
    while (node) {
        // Skip removed nodes (safe removal pattern)
        if (node->removed.load(std::memory_order_acquire)) {
            node = node->next;
            continue;
        }
        StreamData* secondary = node->stream;

        // Clear secondary temp buffers
        std::fill(secondary->temp_left.begin(), secondary->temp_left.end(), 0.0f);
        std::fill(secondary->temp_right.begin(), secondary->temp_right.end(), 0.0f);

        // Call Rust processor with THIS secondary device's ID and offset
        if (stream_data->process_callback) {
            stream_data->process_callback(
                secondary->temp_left.data(),
                secondary->temp_right.data(),
                n_frames,
                stream_data->user_data,
                secondary->device_id,
                secondary->channel_offset
            );
        }

        size_t wp = secondary->write_pos.load(std::memory_order_acquire);

        // Interleave L/R into secondary ring buffer
        for (unsigned int i = 0; i < n_frames; ++i) {
            secondary->ring_buffer[wp % secondary->ring_buffer_size] = secondary->temp_left[i];
            wp = (wp + 1) % secondary->ring_buffer_size;

            secondary->ring_buffer[wp % secondary->ring_buffer_size] = secondary->temp_right[i];
            wp = (wp + 1) % secondary->ring_buffer_size;
        }

        secondary->write_pos.store(wp, std::memory_order_release);
        node = node->next;
    }

    return 0;
}

// Secondary stream callback: reads from ring buffer
int rtaudio_secondary_callback(void* output_buffer, void* /*input_buffer*/,
                               unsigned int n_frames, double /*stream_time*/,
                               RtAudioStreamStatus /*status*/, void* user_data)
{
    auto* stream_data = static_cast<StreamData*>(user_data);
    auto* out = static_cast<float*>(output_buffer);

    // Clear entire output buffer first
    memset(out, 0, n_frames * stream_data->device_channels * sizeof(float));

    // Fade-in: gradually increase volume
    size_t fade = stream_data->fade_in_remaining.load(std::memory_order_acquire);
    float fade_multiplier = 1.0f;
    if (fade > 0) {
        stream_data->fade_in_remaining.store(fade - 1, std::memory_order_release);
        fade_multiplier = (4.0f - fade) / 4.0f;
    }

    // Lock-free read from ring buffer
    size_t rp = stream_data->read_pos.load(std::memory_order_acquire);

    // Read stereo samples and write to correct channel offset (with fade)
    for (unsigned int frame = 0; frame < n_frames; ++frame) {
        for (unsigned int ch = 0; ch < stream_data->num_channels; ++ch) {
            size_t pos = rp % stream_data->ring_buffer_size;
            float sample = stream_data->ring_buffer[pos];
            stream_data->ring_buffer[pos] = 0.0f;  // Clear after reading

            unsigned int out_ch = stream_data->channel_offset + ch;
            if (out_ch < stream_data->device_channels) {
                out[frame * stream_data->device_channels + out_ch] = sample * fade_multiplier;
            }

            rp = (rp + 1) % stream_data->ring_buffer_size;
        }
    }

    stream_data->read_pos.store(rp, std::memory_order_release);
    return 0;
}

// Create RtAudio instance
RtAudioHandle rtaudio_create() {
    try {
        return new RtAudio();
    } catch (...) {
        return nullptr;
    }
}

// Destroy RtAudio instance
void rtaudio_destroy(RtAudioHandle handle) {
    if (handle) {
        delete static_cast<RtAudio*>(handle);
    }
}

// Get device count
unsigned int rtaudio_get_device_count(RtAudioHandle handle) {
    if (!handle) return 0;
    try {
        auto* rt = static_cast<RtAudio*>(handle);
        return rt->getDeviceIds().size();
    } catch (...) {
        return 0;
    }
}

// Get device IDs
int rtaudio_get_device_ids(RtAudioHandle handle, unsigned int* ids, unsigned int max_count) {
    if (!handle || !ids) return -1;

    try {
        auto* rt = static_cast<RtAudio*>(handle);
        std::vector<unsigned int> device_ids = rt->getDeviceIds();

        unsigned int count = device_ids.size();
        if (count > max_count) count = max_count;

        for (unsigned int i = 0; i < count; ++i) {
            ids[i] = device_ids[i];
        }

        return count;
    } catch (...) {
        return -1;
    }
}

// Get device info
int rtaudio_get_device_info(RtAudioHandle handle, unsigned int device_id, CDeviceInfo* info) {
    if (!handle || !info) return -1;

    try {
        auto* rt = static_cast<RtAudio*>(handle);
        RtAudio::DeviceInfo device_info = rt->getDeviceInfo(device_id);

        info->id = device_id;
        strncpy(info->name, device_info.name.c_str(), 255);
        info->name[255] = '\0';
        info->output_channels = device_info.outputChannels;
        info->input_channels = device_info.inputChannels;
        info->is_default = device_info.isDefaultOutput ? 1 : 0;

        // Get first supported sample rate
        if (!device_info.sampleRates.empty()) {
            info->sample_rate = device_info.sampleRates[0];
        } else {
            info->sample_rate = 48000;
        }

        return 0;
    } catch (...) {
        return -1;
    }
}

// Open stream (master or secondary)
RtStreamHandle rtaudio_open_stream(RtAudioHandle /*handle*/,
                                   unsigned int device_id,
                                   unsigned int num_channels,
                                   unsigned int sample_rate,
                                   unsigned int buffer_size,
                                   unsigned int channel_offset,
                                   int is_master) {
    try {
        // Get device info to determine total channels
        auto* query_rtaudio = new RtAudio();
        RtAudio::DeviceInfo device_info = query_rtaudio->getDeviceInfo(device_id);
        unsigned int device_channels = device_info.outputChannels;
        delete query_rtaudio;

        StreamType stream_type = is_master ? STREAM_MASTER : STREAM_SECONDARY;
        auto* stream_data = new StreamData(buffer_size, num_channels, device_channels,
                                          channel_offset, device_id, stream_type);
        stream_data->rtaudio = new RtAudio();

        RtAudio::StreamParameters output_params;
        output_params.deviceId = device_id;
        output_params.nChannels = device_channels;  // Open all device channels
        output_params.firstChannel = 0;

        RtAudio::StreamOptions options;
        options.flags = RTAUDIO_SCHEDULE_REALTIME;
        options.numberOfBuffers = 2;

        unsigned int frames = buffer_size;

        // Use different callback based on stream type
        RtAudioCallback callback = is_master ? rtaudio_master_callback : rtaudio_secondary_callback;

        stream_data->rtaudio->openStream(&output_params, nullptr, RTAUDIO_FLOAT32,
                      sample_rate, &frames, callback,
                      stream_data, &options);

        return stream_data;
    } catch (...) {
        return nullptr;
    }
}

// Close stream
void rtaudio_close_stream(RtAudioHandle /*handle*/, RtStreamHandle stream) {
    if (!stream) return;

    try {
        delete static_cast<StreamData*>(stream);
    } catch (...) {
        // Ignore errors
    }
}

// Start stream
int rtaudio_start_stream(RtAudioHandle /*handle*/, RtStreamHandle stream) {
    if (!stream) return -1;

    try {
        auto* stream_data = static_cast<StreamData*>(stream);

        // Clear ring buffer (safe before stream starts)
        std::fill(stream_data->ring_buffer.begin(), stream_data->ring_buffer.end(), 0.0f);

        // Reset read position to 0
        stream_data->read_pos.store(0, std::memory_order_release);

        // Pre-fill 2 buffers worth of silence for safety margin
        size_t prefill_samples = stream_data->buffer_size * stream_data->num_channels * 2;
        stream_data->write_pos.store(prefill_samples, std::memory_order_release);

        // Enable fade-in to let hardware settle
        stream_data->fade_in_remaining.store(4, std::memory_order_release);

        stream_data->rtaudio->startStream();
        return 0;
    } catch (...) {
        return -1;
    }
}

// Stop stream (now takes stream handle instead of RtAudio handle)
int rtaudio_stop_stream(RtStreamHandle stream) {
    if (!stream) return -1;

    try {
        auto* stream_data = static_cast<StreamData*>(stream);
        if (stream_data->rtaudio && stream_data->rtaudio->isStreamRunning()) {
            stream_data->rtaudio->stopStream();
        }
        return 0;
    } catch (...) {
        return -1;
    }
}

// Set process callback (master stream only)
void rtaudio_set_process_callback(RtStreamHandle stream, ProcessCallback callback, void* user_data) {
    if (!stream) return;

    auto* stream_data = static_cast<StreamData*>(stream);
    if (stream_data->stream_type == STREAM_MASTER) {
        stream_data->process_callback = callback;
        stream_data->user_data = user_data;
        // Debug: verify callback is set
        if (callback && user_data) {
            // OK - will be logged from Rust side
        }
    }
}

// Add secondary stream to master (master stream only)
void rtaudio_add_secondary_stream(RtStreamHandle master_stream, RtStreamHandle secondary_stream) {
    if (!master_stream || !secondary_stream) return;

    auto* master = static_cast<StreamData*>(master_stream);
    auto* secondary = static_cast<StreamData*>(secondary_stream);

    if (master->stream_type != STREAM_MASTER || secondary->stream_type != STREAM_SECONDARY) {
        return;
    }

    // Add to linked list (lock-free prepend)
    auto* new_node = new SecondaryStreamNode;
    new_node->stream = secondary;

    SecondaryStreamNode* old_head = master->secondary_streams.load(std::memory_order_acquire);
    do {
        new_node->next = old_head;
    } while (!master->secondary_streams.compare_exchange_weak(old_head, new_node,
                                                              std::memory_order_release,
                                                              std::memory_order_acquire));
}

// Remove secondary stream from master (master stream only)
// Uses mark-for-removal pattern to avoid dangling pointer in callback
void rtaudio_remove_secondary_stream(RtStreamHandle master_stream, RtStreamHandle secondary_stream) {
    if (!master_stream || !secondary_stream) return;

    auto* master = static_cast<StreamData*>(master_stream);
    auto* secondary = static_cast<StreamData*>(secondary_stream);

    // Mark node as removed (callback will skip it)
    // Actual memory cleanup happens later or on stream close
    SecondaryStreamNode* current = master->secondary_streams.load(std::memory_order_acquire);

    while (current) {
        if (current->stream == secondary) {
            // Mark as removed - callback will skip this node
            current->removed.store(true, std::memory_order_release);
            return;
        }
        current = current->next;
    }
}

} // extern "C"
