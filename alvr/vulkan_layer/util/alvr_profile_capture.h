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

// Read ALVR_PROFILE* from env, else ~/.config/alvr/profile.env (Steam-safe).
inline std::string profile_var(const char* key) {
    if (const char* e = std::getenv(key)) {
        return std::string(e);
    }
    static std::string file_cache;
    static bool file_loaded = false;
    if (!file_loaded) {
        file_loaded = true;
        std::string path;
        if (const char* custom = std::getenv("ALVR_PROFILE_ENV_FILE")) {
            path = custom;
        } else if (const char* xdg = std::getenv("XDG_CONFIG_HOME")) {
            path = std::string(xdg) + "/alvr/profile.env";
        } else if (const char* home = std::getenv("HOME")) {
            path = std::string(home) + "/.config/alvr/profile.env";
        }
        if (!path.empty()) {
            if (FILE* f = fopen(path.c_str(), "r")) {
                char buf[512];
                while (fgets(buf, sizeof(buf), f)) {
                    // strip comments / newline
                    char* hash = strchr(buf, '#');
                    if (hash)
                        *hash = 0;
                    size_t n = strlen(buf);
                    while (n && (buf[n - 1] == '\n' || buf[n - 1] == '\r' || buf[n - 1] == ' '))
                        buf[--n] = 0;
                    char* eq = strchr(buf, '=');
                    if (!eq)
                        continue;
                    *eq = 0;
                    // trim key
                    char* k = buf;
                    while (*k == ' ' || *k == '\t')
                        k++;
                    char* kend = k + strlen(k);
                    while (kend > k && (kend[-1] == ' ' || kend[-1] == '\t'))
                        *--kend = 0;
                    if (strncmp(k, "ALVR_PROFILE", 12) != 0)
                        continue;
                    char* v = eq + 1;
                    while (*v == ' ' || *v == '\t')
                        v++;
                    // store as "KEY=VAL\n" lines in cache
                    file_cache += k;
                    file_cache += '=';
                    file_cache += v;
                    file_cache += '\n';
                }
                fclose(f);
            }
        }
    }
    // lookup in cache
    std::string prefix = std::string(key) + "=";
    size_t pos = 0;
    while (pos < file_cache.size()) {
        size_t end = file_cache.find('\n', pos);
        if (end == std::string::npos)
            end = file_cache.size();
        if (file_cache.compare(pos, prefix.size(), prefix) == 0) {
            return file_cache.substr(pos + prefix.size(), end - pos - prefix.size());
        }
        pos = end + 1;
    }
    return {};
}

inline bool enabled() {
    static int cached = -1;
    if (cached < 0) {
        std::string e = profile_var("ALVR_PROFILE");
        if (e.empty() || e[0] == '0' || e == "off" || e == "false") {
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
        std::string e = profile_var("ALVR_PROFILE");
        if (e.empty()) {
            cached = 0;
        } else if (e == "frame" || e == "2" || e == "detail" || e == "3") {
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
        std::string configured = profile_var("ALVR_PROFILE_PATH");
        // Capture always uses a sibling file so it does not interleave with driver JSONL.
        std::string p = "/tmp/alvr-profile-capture.jsonl";
        if (!configured.empty()) {
            // If driver path is /tmp/alvr-profile.jsonl → /tmp/alvr-profile-capture.jsonl
            if (configured.size() > 6
                && configured.compare(configured.size() - 6, 6, ".jsonl") == 0) {
                p = configured.substr(0, configured.size() - 6) + "-capture.jsonl";
            } else {
                p = configured + "-capture.jsonl";
            }
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
