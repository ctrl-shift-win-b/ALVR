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

    // Prefer sample at or before target_ts with smallest lag; else closest after.
    const TrackingHistoryFrame* best = nullptr;
    uint64_t best_err = UINT64_MAX;

    for (const auto& f : m_poseBuffer) {
        uint64_t err = (f.targetTimestampNs >= target_ts)
            ? (f.targetTimestampNs - target_ts)
            : (target_ts - f.targetTimestampNs);
        // Prefer older-or-equal to target when errors equal (less LSW over-warp risk).
        if (err < best_err
            || (err == best_err && best
                && f.targetTimestampNs < best->targetTimestampNs)) {
            best_err = err;
            best = &f;
        }
    }

    if (!best) {
        return std::nullopt;
    }
    return *best;
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

std::optional<PoseHistory::EncodeStamp> PoseHistory::PickEncodeStamp(
    const vr::HmdMatrix34_t& compositor_pose, uint64_t target_age_ns
) const {
    auto matrix = GetBestPoseMatch(compositor_pose);
    if (!matrix) {
        return std::nullopt;
    }

    EncodeStamp out;
    out.matrixMatch = *matrix;
    out.latest = matrix->latest;
    out.targetAgeNs = target_age_ns;

    // Thresholds from measurement goals:
    // - rot_dist << 1 means compositor rotation agrees with some history sample
    // - age within ~50ms means that sample is recent enough to be the render pose
    constexpr float kGoodRotDist = 0.02f;
    constexpr uint64_t kMaxMatrixAgeNs = 50'000'000ull; // 50ms

    if (matrix->rotDistanceSq <= kGoodRotDist && matrix->ageVsLatestNs <= kMaxMatrixAgeNs) {
        out.frame = matrix->frame;
        out.method = StampMethod::MatrixGood;
        out.actualAgeNs = matrix->ageVsLatestNs;
        return out;
    }

    // Linux typical path: stack-scanned pose does not match history well.
    // Stamp a pose that is consistently ~target_age behind "now" so:
    // - timestamps advance every tracking sample (LSW not frozen)
    // - stamp is NOT "latest" (LSW does not over-warp / fight the image)
    auto lagged = GetPoseNearAge(target_age_ns);
    if (lagged) {
        out.frame = *lagged;
        out.method = StampMethod::TimeLag;
        if (out.latest.targetTimestampNs >= out.frame.targetTimestampNs) {
            out.actualAgeNs = out.latest.targetTimestampNs - out.frame.targetTimestampNs;
        }
        return out;
    }

    out.frame = matrix->latest;
    out.method = StampMethod::Latest;
    out.actualAgeNs = 0;
    return out;
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
