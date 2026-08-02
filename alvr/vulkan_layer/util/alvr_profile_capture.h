// Standalone capture-process profiler (vrcompositor / VK_LAYER_ALVR_capture).
// Does not link Rust — writes JSONL when ALVR_PROFILE is set.
// Timestamps use CLOCK_MONOTONIC (same domain as driver on Linux).

#pragma once

#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <ctime>
#include <mutex>
#include <string>
#include <unistd.h>

namespace alvr_profile_capture {

inline uint64_t now_ns() {
    timespec ts {};
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (uint64_t)ts.tv_sec * 1000000000ull + (uint64_t)ts.tv_nsec;
}

inline bool enabled() {
    static int cached = -1;
    if (cached < 0) {
        const char* e = std::getenv("ALVR_PROFILE");
        if (!e || !e[0] || e[0] == '0'
            || strcmp(e, "off") == 0 || strcmp(e, "false") == 0) {
            cached = 0;
        } else {
            cached = 1;
        }
    }
    return cached == 1;
}

inline bool is_frame_level() {
    static int cached = -1;
    if (cached < 0) {
        const char* e = std::getenv("ALVR_PROFILE");
        if (!e) {
            cached = 0;
        } else if (strcmp(e, "frame") == 0 || strcmp(e, "2") == 0
                   || strcmp(e, "detail") == 0 || strcmp(e, "3") == 0) {
            cached = 1;
        } else {
            cached = 0; // summary: only aggregate via driver; capture still stamps submit_ns
        }
    }
    return cached == 1;
}

inline void write_span(
    const char* stage, uint64_t frame_id, uint64_t start_ns, uint64_t end_ns, uint64_t extra
) {
    if (!enabled() || !is_frame_level())
        return;
    static std::mutex mu;
    static FILE* f = nullptr;
    static bool tried = false;
    std::lock_guard<std::mutex> lock(mu);
    if (!tried) {
        tried = true;
        const char* path = std::getenv("ALVR_PROFILE_PATH");
        std::string p = path && path[0] ? path : "/tmp/alvr-profile-capture.jsonl";
        // Capture process uses a sibling file unless path ends with .jsonl — append -capture
        if (!path || !path[0]) {
            p = "/tmp/alvr-profile-capture.jsonl";
        }
        f = fopen(p.c_str(), "a");
        if (f) {
            fprintf(
                f,
                "{\"type\":\"header\",\"proc\":\"capture\",\"pid\":%d,\"ts_ns\":%llu}\n",
                (int)getpid(),
                (unsigned long long)now_ns()
            );
            fflush(f);
        }
    }
    if (!f)
        return;
    uint64_t dur_us = (end_ns - start_ns) / 1000ull;
    fprintf(
        f,
        "{\"type\":\"span\",\"stage\":\"%s\",\"frame_id\":%llu,\"start_ns\":%llu,"
        "\"end_ns\":%llu,\"dur_us\":%llu,\"extra\":%llu}\n",
        stage,
        (unsigned long long)frame_id,
        (unsigned long long)start_ns,
        (unsigned long long)end_ns,
        (unsigned long long)dur_us,
        (unsigned long long)extra
    );
}

} // namespace alvr_profile_capture
