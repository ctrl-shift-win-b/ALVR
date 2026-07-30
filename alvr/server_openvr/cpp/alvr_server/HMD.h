#pragma once

#include "ALVR-common/packet_types.h"
#include "TrackedDevice.h"
#include "openvr_driver_wrap.h"
#include <atomic>
#include <memory>
#include <thread>
#ifdef _WIN32
#include "platform/win32/OvrDirectModeComponent.h"
#endif

class Controller;
class Controller;
class ViveTrackerProxy;

class CEncoder;
#ifdef _WIN32
class CD3DRender;
#endif
class PoseHistory;

class Hmd : public TrackedDevice, vr::IVRDisplayComponent {
public:
    std::shared_ptr<PoseHistory> m_poseHistory;
    std::shared_ptr<CEncoder> m_encoder;

    Hmd();
    virtual ~Hmd();
    void OnPoseUpdated(
        uint64_t targetTimestampNs, FfiDeviceMotion motion, float poseTimeOffsetS
    );
    void StartStreaming();
    void StopStreaming();
    void SetViewsConfig(FfiViewsConfig config);

private:
    vr::VRInputComponentHandle_t m_proximity;

    FfiViewsConfig views_config;

    bool m_baseComponentsInitialized;
    bool m_streamComponentsInitialized;

    vr::HmdMatrix34_t m_eyeToHeadLeft;
    vr::HmdMatrix34_t m_eyeToHeadRight;
    vr::HmdRect2_t m_eyeFoVLeft;
    vr::HmdRect2_t m_eyeFoVRight;

    std::wstring m_adapterName;

#ifdef _WIN32
    std::shared_ptr<CD3DRender> m_D3DRender;
#endif

#ifdef _WIN32
    std::shared_ptr<OvrDirectModeComponent> m_directModeComponent;
#endif

    std::shared_ptr<ViveTrackerProxy> m_viveTrackerProxy;

    // Remaining STEREO Get* log lines after each ViewsConfig publish.
    int m_stereoGetLogRemaining = 8;

#ifndef _WIN32
    bool m_refreshRateSet = false;
    // Display timing pump:
    // - Idle: resubmit pose + VsyncEvent so StartVRCompositor does not 303.
    // - Streaming: VsyncEvent only (DriverDirectModeSendsVsyncEvents=true).
    //   Without continuous VsyncEvent after stream start, SteamVR frame pacing
    //   collapses and client late-stage warp sees multi-second pose steps.
    std::atomic_bool m_displayTimingRunning { false };
    // When true, pump only fires VsyncEvent (client tracking owns poses).
    std::atomic_bool m_displayTimingVsyncOnly { false };
    std::thread m_displayTimingThread;
    void start_display_timing_pump(bool vsync_only);
    void stop_display_timing_pump();
#endif

    // TrackedDevice
    virtual bool activate() final;
    virtual void* get_component(const char* component_name_and_version) final;

    // IVRDisplayComponent
    virtual void GetWindowBounds(int32_t* x, int32_t* y, uint32_t* width, uint32_t* height);
    virtual bool IsDisplayOnDesktop() { return false; }
    virtual bool IsDisplayRealDisplay();
    virtual void GetRecommendedRenderTargetSize(uint32_t* width, uint32_t* height);
    virtual void GetEyeOutputViewport(
        vr::EVREye eye, uint32_t* x, uint32_t* y, uint32_t* width, uint32_t* height
    );
    virtual void
    GetProjectionRaw(vr::EVREye eEye, float* pfLeft, float* pfRight, float* pfTop, float* pfBottom);
    virtual vr::DistortionCoordinates_t ComputeDistortion(vr::EVREye eEye, float fU, float fV);
};
