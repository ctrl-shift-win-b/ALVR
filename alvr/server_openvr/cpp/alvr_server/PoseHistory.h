#pragma once

#include "ALVR-common/packet_types.h"
#include "openvr_driver_wrap.h"

#include <atomic>
#include <list>
#include <mutex>
#include <optional>

class PoseHistory {
public:
    struct TrackingHistoryFrame {
        uint64_t targetTimestampNs;
        FfiDeviceMotion motion;
        vr::HmdMatrix34_t rotationMatrix;
    };

    struct PoseMatch {
        TrackingHistoryFrame frame;
        TrackingHistoryFrame latest;

        float rotDistanceSq = 0.f;
        float posDistanceSq = 0.f;
        float posDistanceSqVsLatest = 0.f;
        float rotDistanceSqVsLatest = 0.f;

        uint64_t ageVsLatestNs = 0;
        size_t historySize = 0;
        bool usedLatestFallback = false;

        float compositorPos[3] = { 0.f, 0.f, 0.f };
    };

    enum class StampMethod {
        MatrixGood, // rot match tight + age sane (Windows-like)
        TimeLag,    // history sample ~target_age behind latest
        Latest,     // last resort
    };

    struct EncodeStamp {
        TrackingHistoryFrame frame;
        TrackingHistoryFrame latest;
        PoseMatch matrixMatch;
        StampMethod method = StampMethod::Latest;
        uint64_t targetAgeNs = 0;
        uint64_t actualAgeNs = 0;
    };

    void OnPoseUpdated(uint64_t targetTimestampNs, FfiDeviceMotion motion);

    std::optional<PoseMatch> GetBestPoseMatch(const vr::HmdMatrix34_t& pose) const;

    std::optional<TrackingHistoryFrame> GetPoseNearAge(uint64_t age_ns) const;

    // target_age_ns: desired lag of stamp behind latest tracking.
    std::optional<EncodeStamp> PickEncodeStamp(
        const vr::HmdMatrix34_t& compositor_pose, uint64_t target_age_ns
    ) const;

    std::optional<TrackingHistoryFrame> GetLatest() const;

    std::optional<TrackingHistoryFrame> GetPoseAt(uint64_t timestampNs) const;

    // P3: last tracking sample timestamp written by OnPoseUpdated (ns).
    uint64_t GetLastTrackingTimestampNs() const {
        return m_lastTrackingTimestampNs.load(std::memory_order_relaxed);
    }

    void SetTransform(const vr::HmdMatrix34_t& transform);

    static const char* StampMethodName(StampMethod m);

private:
    mutable std::mutex m_mutex;
    std::list<TrackingHistoryFrame> m_poseBuffer;
    std::atomic<uint64_t> m_lastTrackingTimestampNs { 0 };
    vr::HmdMatrix34_t m_transform
        = { { { 1.0, 0.0, 0.0, 0.0 }, { 0.0, 1.0, 0.0, 0.0 }, { 0.0, 0.0, 1.0, 0.0 } } };
    bool m_transformIdentity = true;
};
