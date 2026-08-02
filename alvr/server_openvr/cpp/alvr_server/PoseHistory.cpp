#include "PoseHistory.h"
#include "Logger.h"
#include "Utils.h"
#include "include/openvr_math.h"
#include <cmath>
#include <cstdint>
#include <mutex>
#include <optional>

namespace {

float rotation_distance_sq(const vr::HmdMatrix34_t& a, const vr::HmdMatrix34_t& b) {
    float distance = 0.f;
    for (int i = 0; i < 3; i++) {
        for (int j = 0; j < 3; j++) {
            float d = a.m[j][i] - b.m[j][i];
            distance += d * d;
        }
    }
    return distance;
}

float position_distance_sq(const float hist_pos[3], const vr::HmdMatrix34_t& compositor_pose) {
    float distance = 0.f;
    for (int i = 0; i < 3; i++) {
        float d = hist_pos[i] - compositor_pose.m[i][3];
        distance += d * d;
    }
    return distance;
}

} // namespace

const char* PoseHistory::StampMethodName(StampMethod m) {
    switch (m) {
    case StampMethod::MatrixGood:
        return "matrix";
    case StampMethod::TimeLag:
        return "time_lag";
    case StampMethod::Latest:
        return "latest";
    }
    return "unknown";
}

void PoseHistory::OnPoseUpdated(uint64_t targetTimestampNs, FfiDeviceMotion motion) {
    TrackingHistoryFrame history;
    history.targetTimestampNs = targetTimestampNs;
    history.motion = motion;

    HmdMatrix_QuatToMat(
        motion.orientation.w,
        motion.orientation.x,
        motion.orientation.y,
        motion.orientation.z,
        &history.rotationMatrix
    );

    std::unique_lock<std::mutex> lock(m_mutex);
    if (!m_transformIdentity) {
        vr::HmdMatrix34_t rotation = vrmath::matMul33(m_transform, history.rotationMatrix);
        history.rotationMatrix = rotation;
    }

    if (m_poseBuffer.size() == 0) {
        m_poseBuffer.push_back(history);
    } else {
        if (m_poseBuffer.back().targetTimestampNs != targetTimestampNs) {
            m_poseBuffer.push_back(history);
        }
    }
    if (m_poseBuffer.size() > 120 * 3) {
        m_poseBuffer.pop_front();
    }

    // P3: lock-free read for encode path / diagnostics
    m_lastTrackingTimestampNs.store(targetTimestampNs, std::memory_order_relaxed);
}

std::optional<PoseHistory::TrackingHistoryFrame> PoseHistory::GetLatest() const {
    std::unique_lock<std::mutex> lock(m_mutex);
    if (m_poseBuffer.empty()) {
        return std::nullopt;
    }
    return m_poseBuffer.back();
}

std::optional<PoseHistory::TrackingHistoryFrame>
PoseHistory::GetPoseNearAge(uint64_t age_ns) const {
    std::unique_lock<std::mutex> lock(m_mutex);
    if (m_poseBuffer.empty()) {
        return std::nullopt;
    }

    const uint64_t latest_ts = m_poseBuffer.back().targetTimestampNs;
    const uint64_t target_ts = (latest_ts > age_ns) ? (latest_ts - age_ns) : 0;

    // History is time-ordered (oldest front, newest back). Walk reverse from
    // newest until we pass target_ts — O(samples in lag window), not O(full buffer).
    // Pick closer of first-at-or-before-target and the sample just newer.
    auto rit = m_poseBuffer.rbegin();
    const TrackingHistoryFrame* newer = nullptr;
    while (rit != m_poseBuffer.rend() && rit->targetTimestampNs > target_ts) {
        newer = &(*rit);
        ++rit;
    }

    if (rit == m_poseBuffer.rend()) {
        // All samples newer than target (tiny buffer / large age): use oldest.
        return m_poseBuffer.front();
    }

    const TrackingHistoryFrame* at_or_before = &(*rit);
    if (!newer) {
        return *at_or_before;
    }

    const uint64_t err_old = target_ts - at_or_before->targetTimestampNs;
    const uint64_t err_new = newer->targetTimestampNs - target_ts;
    // Prefer older-or-equal on ties (less LSW over-warp risk).
    if (err_new < err_old) {
        return *newer;
    }
    return *at_or_before;
}

std::optional<PoseHistory::PoseMatch>
PoseHistory::GetBestPoseMatch(const vr::HmdMatrix34_t& pose) const {
    std::unique_lock<std::mutex> lock(m_mutex);
    if (m_poseBuffer.empty()) {
        Debug("PoseHistory::GetBestPoseMatch: empty buffer.");
        return std::nullopt;
    }

    float minDiff = 1e12f;
    auto minIt = m_poseBuffer.rbegin();
    bool found = false;

    for (auto it = m_poseBuffer.rbegin(); it != m_poseBuffer.rend(); ++it) {
        float distance = rotation_distance_sq(it->rotationMatrix, pose);
        if (!found || distance < minDiff) {
            minDiff = distance;
            minIt = it;
            found = true;
        }
    }

    if (!found) {
        return std::nullopt;
    }

    const auto& latest = m_poseBuffer.back();
    PoseMatch result;
    result.frame = *minIt;
    result.latest = latest;
    result.rotDistanceSq = minDiff;
    result.historySize = m_poseBuffer.size();
    result.usedLatestFallback = false;
    result.compositorPos[0] = pose.m[0][3];
    result.compositorPos[1] = pose.m[1][3];
    result.compositorPos[2] = pose.m[2][3];

    result.posDistanceSq = position_distance_sq(result.frame.motion.position, pose);
    result.posDistanceSqVsLatest = position_distance_sq(latest.motion.position, pose);
    result.rotDistanceSqVsLatest = rotation_distance_sq(latest.rotationMatrix, pose);

    if (latest.targetTimestampNs >= result.frame.targetTimestampNs) {
        result.ageVsLatestNs = latest.targetTimestampNs - result.frame.targetTimestampNs;
    } else {
        result.ageVsLatestNs = 0;
    }

    return result;
}

namespace {

// Stack-scan in the capture layer often yields origin / unusable pose on Linux.
// Full-history matrix match is wasted work in that case — go straight to time-lag.
bool compositor_pose_usable_for_matrix(const vr::HmdMatrix34_t& pose) {
    const float x = pose.m[0][3];
    const float y = pose.m[1][3];
    const float z = pose.m[2][3];
    const float r2 = x * x + y * y + z * z;
    if (r2 < 1e-6f) {
        return false;
    }
    // Reject near-identity rotation with tiny translation noise (common fail mode).
    // Cheap check: diagonal of R roughly 1 (not a full orthonormal test).
    float diag = 0.f;
    for (int i = 0; i < 3; i++) {
        diag += pose.m[i][i];
    }
    // Identity rotation → trace ≈ 3; garbage/zero → ~0
    if (diag < 1.5f) {
        return false;
    }
    return true;
}

PoseHistory::PoseMatch match_from_lagged(
    const PoseHistory::TrackingHistoryFrame& lagged,
    const PoseHistory::TrackingHistoryFrame& latest,
    const vr::HmdMatrix34_t& compositor_pose,
    size_t history_size
) {
    PoseHistory::PoseMatch m;
    m.frame = lagged;
    m.latest = latest;
    m.historySize = history_size;
    m.usedLatestFallback = false;
    m.compositorPos[0] = compositor_pose.m[0][3];
    m.compositorPos[1] = compositor_pose.m[1][3];
    m.compositorPos[2] = compositor_pose.m[2][3];
    m.rotDistanceSq = 1e6f; // sentinel: matrix path skipped / not good
    m.posDistanceSq = position_distance_sq(lagged.motion.position, compositor_pose);
    m.posDistanceSqVsLatest = position_distance_sq(latest.motion.position, compositor_pose);
    m.rotDistanceSqVsLatest = rotation_distance_sq(latest.rotationMatrix, compositor_pose);
    if (latest.targetTimestampNs >= lagged.targetTimestampNs) {
        m.ageVsLatestNs = latest.targetTimestampNs - lagged.targetTimestampNs;
    }
    return m;
}

} // namespace

std::optional<PoseHistory::EncodeStamp> PoseHistory::PickEncodeStamp(
    const vr::HmdMatrix34_t& compositor_pose, uint64_t target_age_ns
) const {
    EncodeStamp out;
    out.targetAgeNs = target_age_ns;

    // Thresholds from measurement goals:
    // - rot_dist << 1 means compositor rotation agrees with some history sample
    // - age within ~50ms means that sample is recent enough to be the render pose
    constexpr float kGoodRotDist = 0.02f;
    constexpr uint64_t kMaxMatrixAgeNs = 50'000'000ull; // 50ms

    // Linux: stack-scanned compositor pose rarely matches tracking history.
    // Skip O(N) matrix scan when pose is unusable; use time-lag (O(lag window)).
    const bool try_matrix =
#ifndef __linux__
        true
#else
        compositor_pose_usable_for_matrix(compositor_pose)
#endif
        ;

    if (try_matrix) {
        auto matrix = GetBestPoseMatch(compositor_pose);
        if (!matrix) {
            return std::nullopt;
        }

        out.matrixMatch = *matrix;
        out.latest = matrix->latest;

        if (matrix->rotDistanceSq <= kGoodRotDist && matrix->ageVsLatestNs <= kMaxMatrixAgeNs) {
            out.frame = matrix->frame;
            out.method = StampMethod::MatrixGood;
            out.actualAgeNs = matrix->ageVsLatestNs;
            return out;
        }
    }

    // Stamp a pose consistently ~target_age behind "now" so:
    // - timestamps advance every tracking sample (LSW not frozen)
    // - stamp is NOT "latest" (LSW does not over-warp / fight the image)
    // Single lock for lag pick + latest + hist size (avoid lock thrash on encode path).
    {
        std::unique_lock<std::mutex> lock(m_mutex);
        if (m_poseBuffer.empty()) {
            return std::nullopt;
        }

        const TrackingHistoryFrame& latest_ref = m_poseBuffer.back();
        out.latest = latest_ref;
        const size_t hist_size = m_poseBuffer.size();

        const uint64_t latest_ts = latest_ref.targetTimestampNs;
        const uint64_t target_ts = (latest_ts > target_age_ns) ? (latest_ts - target_age_ns) : 0;

        auto rit = m_poseBuffer.rbegin();
        const TrackingHistoryFrame* newer = nullptr;
        while (rit != m_poseBuffer.rend() && rit->targetTimestampNs > target_ts) {
            newer = &(*rit);
            ++rit;
        }

        const TrackingHistoryFrame* chosen = nullptr;
        if (rit == m_poseBuffer.rend()) {
            chosen = &m_poseBuffer.front();
        } else {
            const TrackingHistoryFrame* at_or_before = &(*rit);
            if (!newer) {
                chosen = at_or_before;
            } else {
                const uint64_t err_old = target_ts - at_or_before->targetTimestampNs;
                const uint64_t err_new = newer->targetTimestampNs - target_ts;
                chosen = (err_new < err_old) ? newer : at_or_before;
            }
        }

        out.frame = *chosen;
        out.method = StampMethod::TimeLag;
        if (out.latest.targetTimestampNs >= out.frame.targetTimestampNs) {
            out.actualAgeNs = out.latest.targetTimestampNs - out.frame.targetTimestampNs;
        }
        out.matrixMatch
            = match_from_lagged(out.frame, out.latest, compositor_pose, hist_size);
        return out;
    }
}

std::optional<PoseHistory::TrackingHistoryFrame> PoseHistory::GetPoseAt(uint64_t timestampNs
) const {
    std::unique_lock<std::mutex> lock(m_mutex);
    for (auto it = m_poseBuffer.rbegin(), end = m_poseBuffer.rend(); it != end; ++it) {
        if (it->targetTimestampNs == timestampNs)
            return *it;
    }

    Debug("PoseHistory::GetPoseAt: No pose matched.");
    return {};
}

void PoseHistory::SetTransform(const vr::HmdMatrix34_t& transform) {
    std::unique_lock<std::mutex> lock(m_mutex);
    m_transform = transform;

    for (int i = 0; i < 3; ++i) {
        for (int j = 0; j < 3; ++j) {
            if (transform.m[i][j] != (i == j ? 1 : 0)) {
                m_transformIdentity = false;
                return;
            }
        }
    }
    m_transformIdentity = true;
}
