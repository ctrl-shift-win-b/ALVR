// Lightweight C++ profiling hooks for the ALVR driver process.
// Function pointers are set from Rust (server_openvr). When unset / ALVR_PROFILE off,
// all helpers are no-ops.
//
// Stage IDs must match alvr/profiling/src/lib.rs Stage enum.

#pragma once

#include <cstdint>

enum AlvrProfileStage : uint8_t {
    ALVR_PROF_TRACKING_RX = 0,
    ALVR_PROF_POSE_PUBLISH = 1,
    ALVR_PROF_PRESENT_SUBMIT = 2,
    ALVR_PROF_PRESENT_RECV = 3,
    ALVR_PROF_STAMP_PICK = 4,
    ALVR_PROF_RENDER_GPU = 5,
    ALVR_PROF_ENCODE_PUSH = 6,
    ALVR_PROF_ENCODE_GET = 7,
    ALVR_PROF_NAL_PARSE = 8,
    ALVR_PROF_FFI_COPY = 9,
    ALVR_PROF_CHANNEL_ENQUEUE = 10,
    ALVR_PROF_CHANNEL_DEQUEUE = 11,
    ALVR_PROF_STREAM_COPY = 12,
    ALVR_PROF_TCP_SEND = 13,
    ALVR_PROF_CLIENT_STATS = 14,
    ALVR_PROF_IPC_PRESENT_DELAY = 15,
    ALVR_PROF_POSE_HISTORY_LOCK = 16,
};

extern "C" {
extern unsigned char (*ProfileEnabled)();
extern unsigned long long (*ProfileNowNs)();
extern void (*ProfileSpanBegin)(unsigned int stage, unsigned long long frame_id);
extern void (*ProfileSpanEnd)(unsigned int stage);
extern void (*ProfileRecord)(
    unsigned int stage,
    unsigned long long frame_id,
    unsigned long long start_ns,
    unsigned long long end_ns,
    unsigned long long extra
);
extern void (*ProfileMark)(
    unsigned int stage, unsigned long long frame_id, unsigned long long extra
);
}

namespace alvr_profile {

inline bool enabled() {
    return ProfileEnabled && ProfileEnabled() != 0;
}

inline uint64_t now_ns() {
    if (ProfileNowNs)
        return ProfileNowNs();
    return 0;
}

inline void span_begin(uint8_t stage, uint64_t frame_id) {
    if (ProfileSpanBegin)
        ProfileSpanBegin(stage, frame_id);
}

inline void span_end(uint8_t stage) {
    if (ProfileSpanEnd)
        ProfileSpanEnd(stage);
}

inline void record(
    uint8_t stage, uint64_t frame_id, uint64_t start_ns, uint64_t end_ns, uint64_t extra = 0
) {
    if (ProfileRecord)
        ProfileRecord(stage, frame_id, start_ns, end_ns, extra);
}

inline void mark(uint8_t stage, uint64_t frame_id, uint64_t extra = 0) {
    if (ProfileMark)
        ProfileMark(stage, frame_id, extra);
}

/// RAII span; no-op when profiling disabled.
struct Span {
    uint8_t stage;
    bool active;

    Span(uint8_t stage_, uint64_t frame_id) : stage(stage_), active(false) {
        if (enabled()) {
            span_begin(stage, frame_id);
            active = true;
        }
    }

    ~Span() {
        if (active)
            span_end(stage);
    }

    Span(const Span&) = delete;
    Span& operator=(const Span&) = delete;
};

} // namespace alvr_profile
