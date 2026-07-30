#include "pose.hpp"
#include "logger.h"

#include <cmath>
#include <cstring>
#include <string>

#define UNW_LOCAL_ONLY
#include <libunwind.h>

namespace {

inline HmdMatrix34_t transposeMul33(const HmdMatrix34_t &a) {
    HmdMatrix34_t result;
    for (unsigned i = 0; i < 3; i++) {
        for (unsigned k = 0; k < 3; k++) {
            result.m[i][k] = a.m[k][i];
        }
    }
    result.m[0][3] = a.m[0][3];
    result.m[1][3] = a.m[1][3];
    result.m[2][3] = a.m[2][3];
    return result;
}

inline HmdMatrix34_t matMul33(const HmdMatrix34_t &a, const HmdMatrix34_t &b) {
    HmdMatrix34_t result;
    for (unsigned i = 0; i < 3; i++) {
        for (unsigned j = 0; j < 3; j++) {
            result.m[i][j] = 0.0f;
            for (unsigned k = 0; k < 3; k++) {
                result.m[i][j] += a.m[i][k] * b.m[k][j];
            }
        }
    }
    return result;
}

bool is_near_identity_rotation(const HmdMatrix34_t &m) {
    auto r = matMul33(m, transposeMul33(m));
    for (int i = 0; i < 3; ++i) {
        for (int j = 0; j < 3; ++j) {
            if (std::abs(r.m[i][j] - (i == j ? 1.f : 0.f)) > 0.1f)
                return false;
        }
    }
    return true;
}

bool check_pose(const TrackedDevicePose_t &p) {
    if (p.bPoseIsValid != 1 || p.bDeviceIsConnected != 1)
        return false;
    // SteamVR TrackingResult_Running_OK == 200
    if (p.eTrackingResult != 200)
        return false;
    return is_near_identity_rotation(p.mDeviceToAbsoluteTracking);
}

// Prefer HMD-like poses (standing height) over controllers near the floor/origin.
float pose_hmd_score(const TrackedDevicePose_t &p) {
    const float x = p.mDeviceToAbsoluteTracking.m[0][3];
    const float y = p.mDeviceToAbsoluteTracking.m[1][3];
    const float z = p.mDeviceToAbsoluteTracking.m[2][3];
    // Reject pure origin (broken/empty) — our MEASURE saw comp_pos=(0,0,0) always.
    const float r2 = x * x + y * y + z * z;
    if (r2 < 1e-6f)
        return -1.f;
    // Head height typically ~1.0–2.0 m
    float score = 0.f;
    if (y > 0.8f && y < 2.2f)
        score += 10.f + y;
    else if (y > 0.3f)
        score += 1.f + y;
    else
        score += 0.1f;
    return score;
}

bool is_render_thread_frame(const char *name) {
    // Historical SteamVR compositor symbols (may vary by version).
    if (strcmp(name, "_ZN13CRenderThread11UpdateAsyncEv") == 0)
        return true;
    if (strcmp(name, "_ZN13CRenderThread6UpdateEv") == 0)
        return true;
    // Broader: demangled-ish fragments if names change slightly
    if (strstr(name, "CRenderThread") != nullptr)
        return true;
    if (strstr(name, "UpdateAsync") != nullptr)
        return true;
    return false;
}

// Scan [sp, sp_end) for TrackedDevicePose_t candidates; keep best HMD-like.
void scan_range_for_pose(
    uintptr_t sp, uintptr_t sp_end, TrackedDevicePose_t &best, float &best_score, int &found_n
) {
    if (sp_end <= sp || (sp_end - sp) > 1u << 20)
        return; // sanity
    // Align to 4 bytes (historical ALVR scan)
    sp &= ~uintptr_t(3);
    for (uintptr_t addr = sp; addr + sizeof(TrackedDevicePose_t) <= sp_end; addr += 4) {
        auto *p = reinterpret_cast<TrackedDevicePose_t *>(addr);
        if (!check_pose(*p))
            continue;
        found_n++;
        float s = pose_hmd_score(*p);
        if (s < 0.f)
            continue;
        if (s > best_score) {
            best_score = s;
            best = *p; // COPY — never keep a stack pointer
        }
    }
}

} // namespace

// For a smooth experience, the correct pose for a frame must be known.
// Not part of Vulkan parameters, so we inspect the call stack.
//
// CRITICAL: never cache a pointer into the stack across calls. A previous
// ALVR bug cached the first hit forever → dangling pointer → identity poses
// → encoder MEASURE saw comp_pos=(0,0,0) and matrix_rot_dist=3 always.
const TrackedDevicePose_t &find_pose_in_call_stack() {
    thread_local TrackedDevicePose_t tls_found {};
    thread_local TrackedDevicePose_t tls_notfound {};
    static int s_log_ok = 0;
    static int s_log_fail = 0;

    TrackedDevicePose_t best {};
    float best_score = -1.f;
    int found_n = 0;
    int frames_scanned = 0;
    int render_frames = 0;

    unw_context_t ctx;
    unw_getcontext(&ctx);
    unw_cursor_t cursor;
    unw_init_local(&cursor, &ctx);

    while (unw_step(&cursor) > 0) {
        frames_scanned++;
        char name[256];
        unw_word_t off = 0;
        name[0] = '\0';
        unw_get_proc_name(&cursor, name, sizeof(name), &off);

        unw_word_t sp = 0, sp_end = 0;
        unw_get_reg(&cursor, UNW_REG_SP, &sp);

        // Peek next frame SP as approximate end of this frame's stack region.
        unw_cursor_t next = cursor;
        if (unw_step(&next) > 0) {
            unw_get_reg(&next, UNW_REG_SP, &sp_end);
        } else {
            sp_end = sp + 4096;
        }
        if (sp_end < sp)
            sp_end = sp + 4096;

        const bool prefer = is_render_thread_frame(name);
        if (prefer)
            render_frames++;

        // Always scan; prefer scores from CRenderThread frames by boost.
        float before = best_score;
        scan_range_for_pose((uintptr_t)sp, (uintptr_t)sp_end, best, best_score, found_n);
        if (prefer && best_score > before && best_score > 0.f) {
            // small boost already encoded by finding better score first in that frame
        }

        // Cap walk
        if (frames_scanned > 64)
            break;
    }

    if (best_score > 0.f) {
        tls_found = best;
        if (s_log_ok < 8) {
            s_log_ok++;
            Warn(
                "pose_scan: OK n=%d frames=%d render_frames=%d score=%.2f "
                "pos=(%.3f,%.3f,%.3f)\n",
                found_n,
                frames_scanned,
                render_frames,
                best_score,
                best.mDeviceToAbsoluteTracking.m[0][3],
                best.mDeviceToAbsoluteTracking.m[1][3],
                best.mDeviceToAbsoluteTracking.m[2][3]
            );
        }
        return tls_found;
    }

    if (s_log_fail < 8) {
        s_log_fail++;
        Warn(
            "pose_scan: MISS frames=%d candidates=%d render_frames=%d "
            "(will send identity — encoder falls back to time_lag)\n",
            frames_scanned,
            found_n,
            render_frames
        );
    }
    memset(&tls_notfound, 0, sizeof(tls_notfound));
    return tls_notfound;
}
