#include "HMD.h"

#include "Controller.h"
#include "Logger.h"
#include "Paths.h"
#include "PoseHistory.h"
#include "Settings.h"
#include "Utils.h"
#include "ViveTrackerProxy.h"
#include "bindings.h"
#include <cfloat>
#include <chrono>
#include <cmath>

#ifdef _WIN32
#include "platform/win32/CEncoder.h"
#elif __APPLE__
#include "platform/macos/CEncoder.h"
#else
#include "platform/linux/CEncoder.h"
#endif

const vr::HmdMatrix34_t MATRIX_IDENTITY
    = { { { 1.0, 0.0, 0.0, 0.0 }, { 0.0, 1.0, 0.0, 0.0 }, { 0.0, 0.0, 1.0, 0.0 } } };

vr::HmdRect2_t fov_to_projection(FfiFov fov) {
    auto proj_bounds = vr::HmdRect2_t {};
    // OpenVR GetProjectionRaw expects tan(angle) frustum edges (not angles).
    proj_bounds.vTopLeft.v[0] = tanf(fov.left);
    proj_bounds.vBottomRight.v[0] = tanf(fov.right);
    // Sample OpenVR driver uses top=-1, bottom=+1 (Y sign opposite to OpenXR).
    proj_bounds.vTopLeft.v[1] = tanf(fov.down);
    proj_bounds.vBottomRight.v[1] = tanf(fov.up);

    return proj_bounds;
}

namespace {

// Diagnostics: HMD poses submitted to SteamVR + frame-to-frame step stats.
void log_steamvr_hmd_pose(
    const char* source,
    vr::TrackedDeviceIndex_t object_id,
    const vr::DriverPose_t& pose,
    uint64_t tracking_ts_ns
) {
    static auto last_log = std::chrono::steady_clock::now();
    static uint64_t count = 0;
    static vr::DriverPose_t prev {};
    static bool has_prev = false;
    static uint64_t prev_ts = 0;
    static uint64_t stuck_count = 0;

    // Per-sample step stats within the 2s window (measurement, not 2s span).
    static double sum_step_m = 0;
    static double sum_step_deg = 0;
    static double max_step_m = 0;
    static double max_step_deg = 0;
    static uint64_t step_n = 0;
    static double prev_dx = 0, prev_dy = 0, prev_dz = 0;
    static bool has_prev_step = false;
    static uint64_t reverse_count = 0; // consecutive Δpos dot product < 0
    static double sum_ts_step_ms = 0;
    static double max_ts_step_ms = 0;

    count++;
    if (has_prev) {
        double dx = pose.vecPosition[0] - prev.vecPosition[0];
        double dy = pose.vecPosition[1] - prev.vecPosition[1];
        double dz = pose.vecPosition[2] - prev.vecPosition[2];
        double dpos = std::sqrt(dx * dx + dy * dy + dz * dz);
        double dot = pose.qRotation.w * prev.qRotation.w + pose.qRotation.x * prev.qRotation.x
            + pose.qRotation.y * prev.qRotation.y + pose.qRotation.z * prev.qRotation.z;
        if (dot > 1.0)
            dot = 1.0;
        if (dot < -1.0)
            dot = -1.0;
        double dang = 2.0 * std::acos(std::fabs(dot));
        double dang_deg = dang * (180.0 / 3.14159265358979323846);

        // ~0.05° or 0.1mm threshold
        if (!((dpos * dpos > 1e-8) || (dang > 0.001)))
            stuck_count++;

        sum_step_m += dpos;
        sum_step_deg += dang_deg;
        if (dpos > max_step_m)
            max_step_m = dpos;
        if (dang_deg > max_step_deg)
            max_step_deg = dang_deg;
        step_n++;

        if (has_prev_step) {
            double rev = dx * prev_dx + dy * prev_dy + dz * prev_dz;
            if (rev < 0.0 && dpos > 1e-4)
                reverse_count++;
        }
        prev_dx = dx;
        prev_dy = dy;
        prev_dz = dz;
        has_prev_step = true;

        if (prev_ts) {
            double ts_ms = (double)(tracking_ts_ns - prev_ts) / 1e6;
            // Only count forward steps (ignore reorder/wrap).
            if (ts_ms > 0.0 && ts_ms < 100.0) {
                sum_ts_step_ms += ts_ms;
                if (ts_ms > max_ts_step_ms)
                    max_ts_step_ms = ts_ms;
            }
        }
    }

    auto now = std::chrono::steady_clock::now();
    const double elapsed = std::chrono::duration<double>(now - last_log).count();
    if (elapsed < 2.0) {
        prev = pose;
        has_prev = true;
        prev_ts = tracking_ts_ns;
        return;
    }

    double hz = count / elapsed;
    double mean_step_m = step_n ? (sum_step_m / step_n) : 0.0;
    double mean_step_deg = step_n ? (sum_step_deg / step_n) : 0.0;
    double mean_ts_ms = step_n ? (sum_ts_step_ms / step_n) : 0.0;

    Warn(
        "MEASURE/HMD submit via %s: rate=%.1fHz n=%llu offset=%.1fms "
        "vel=(%.3f,%.3f,%.3f) avel=(%.3f,%.3f,%.3f) "
        "step_mean=%.5fm/%.3fdeg step_max=%.5fm/%.3fdeg "
        "ts_step_mean=%.2fms ts_step_max=%.2fms "
        "reverse_steps=%llu/%llu stuck=%llu/%llu "
        "pos=(%.3f,%.3f,%.3f) — reverse_steps high => frame-to-frame overshoot",
        source,
        hz,
        (unsigned long long)count,
        pose.poseTimeOffset * 1000.0,
        pose.vecVelocity[0],
        pose.vecVelocity[1],
        pose.vecVelocity[2],
        pose.vecAngularVelocity[0],
        pose.vecAngularVelocity[1],
        pose.vecAngularVelocity[2],
        mean_step_m,
        mean_step_deg,
        max_step_m,
        max_step_deg,
        mean_ts_ms,
        max_ts_step_ms,
        (unsigned long long)reverse_count,
        (unsigned long long)(step_n > 0 ? step_n : 1),
        (unsigned long long)stuck_count,
        (unsigned long long)count,
        pose.vecPosition[0],
        pose.vecPosition[1],
        pose.vecPosition[2]
    );

    count = 0;
    stuck_count = 0;
    sum_step_m = sum_step_deg = 0;
    max_step_m = max_step_deg = 0;
    step_n = 0;
    reverse_count = 0;
    sum_ts_step_ms = max_ts_step_ms = 0;
    has_prev_step = false;
    last_log = now;
    prev = pose;
    has_prev = true;
    prev_ts = tracking_ts_ns;
}

} // namespace

Hmd::Hmd()
    : TrackedDevice(
          HEAD_ID,
          Settings::Instance().m_TrackingRefOnly ? vr::TrackedDeviceClass_TrackingReference
                                                 : vr::TrackedDeviceClass_HMD
      )
    , m_baseComponentsInitialized(false)
    , m_streamComponentsInitialized(false) {
    Debug("Hmd::constructor");

    // Stereo geometry is chosen in Hard Config (AVP vs Quest 3) and baked into
    // openvr_config before SteamVR starts. SteamVR snapshots GetProjectionRaw at
    // activate, so this must already match the headset that will connect.
    // Invalid/missing values fall back to the AVP frustum this branch was
    // calibrated against (do not use symmetric ±1 rad placeholders).
    this->views_config = FfiViewsConfig {};
    auto& st = Settings::Instance();
    this->views_config.ipd_m = st.m_defaultIpdM;
    this->views_config.fov[0] = FfiFov {
        st.m_defaultFovLLeft,
        st.m_defaultFovLRight,
        st.m_defaultFovLUp,
        st.m_defaultFovLDown
    };
    this->views_config.fov[1] = FfiFov {
        st.m_defaultFovRLeft,
        st.m_defaultFovRRight,
        st.m_defaultFovRUp,
        st.m_defaultFovRDown
    };
    auto fov_ok = [](const FfiFov& f) {
        return std::isfinite(f.left) && std::isfinite(f.right) && std::isfinite(f.up)
            && std::isfinite(f.down) && std::fabs(f.left) < 2.5f && std::fabs(f.right) < 2.5f
            && std::fabs(f.up) < 2.5f && std::fabs(f.down) < 2.5f && f.left < f.right
            && f.down < f.up;
    };
    if (!(this->views_config.ipd_m > 0.04f && this->views_config.ipd_m < 0.10f)
        || !fov_ok(this->views_config.fov[0]) || !fov_ok(this->views_config.fov[1])) {
        Warn("Hmd::constructor: baked stereo invalid — falling back to AVP frustum");
        this->views_config.ipd_m = 0.063f;
        this->views_config.fov[0] = FfiFov { -1.054f, 0.791f, 0.878f, -0.791f };
        this->views_config.fov[1] = FfiFov { -0.793f, 1.057f, 0.881f, -0.793f };
    }
    Warn(
        "Hmd::constructor baked stereo ipd=%.4f fovL=[%.3f,%.3f,%.3f,%.3f] "
        "fovR=[%.3f,%.3f,%.3f,%.3f]",
        this->views_config.ipd_m,
        this->views_config.fov[0].left,
        this->views_config.fov[0].right,
        this->views_config.fov[0].up,
        this->views_config.fov[0].down,
        this->views_config.fov[1].left,
        this->views_config.fov[1].right,
        this->views_config.fov[1].up,
        this->views_config.fov[1].down
    );

    m_poseHistory = std::make_shared<PoseHistory>();

    if (Settings::Instance().m_enableViveTrackerProxy) {
        m_viveTrackerProxy = std::make_unique<ViveTrackerProxy>(*this);
        if (!vr::VRServerDriverHost()->TrackedDeviceAdded(
                m_viveTrackerProxy->GetSerialNumber(),
                vr::TrackedDeviceClass_GenericTracker,
                m_viveTrackerProxy.get()
            )) {
            Warn("Failed to register Vive tracker");
        }
    }
}

Hmd::~Hmd() {
    Debug("Hmd::destructor");

#ifndef _WIN32
    stop_display_timing_pump();
#endif

    if (m_encoder) {
        Debug("Hmd::~Hmd(): Stopping encoder...\n");
        m_encoder->Stop();
        m_encoder.reset();
    }

#ifdef _WIN32
    if (m_D3DRender) {
        m_D3DRender->Shutdown();
        m_D3DRender.reset();
    }
#endif
}

#ifndef _WIN32
void Hmd::start_display_timing_pump(bool vsync_only) {
    m_displayTimingVsyncOnly.store(vsync_only);
    if (m_displayTimingRunning.exchange(true)) {
        // Already running — just switched mode (idle ↔ stream).
        Warn(
            "Hmd: display timing pump mode -> %s",
            vsync_only ? "vsync-only (streaming)" : "pose+vsync (idle)"
        );
        return;
    }
    const int refresh = Settings::Instance().m_refreshRate > 0
        ? Settings::Instance().m_refreshRate
        : 90;
    Warn(
        "Hmd: starting display timing pump at %d Hz mode=%s",
        refresh,
        vsync_only ? "vsync-only" : "pose+vsync"
    );
    m_displayTimingThread = std::thread([this, refresh]() {
        using clock = std::chrono::steady_clock;
        const auto period = std::chrono::duration_cast<clock::duration>(
            std::chrono::duration<double>(1.0 / double(refresh))
        );
        auto next = clock::now();
        uint64_t ticks = 0;
        while (m_displayTimingRunning.load()) {
            const bool vsync_only = m_displayTimingVsyncOnly.load();
            // Idle: re-publish pose + vsync so StartVRCompositor does not 303.
            // Streaming (vsync_only): OnPoseUpdated already pushes ~90 Hz tracking;
            // do NOT re-submit last_pose — that duplicates samples and confuses
            // Linux encode pose matching / client LSW (knock excitation).
            if (!vsync_only && this->object_id != vr::k_unTrackedDeviceIndexInvalid
                && this->last_pose.deviceIsConnected
                && this->last_pose.poseIsValid) {
                this->submit_pose(this->last_pose);
            }
            // SteamVR is told DriverDirectModeSendsVsyncEvents=true on Linux.
            SendVSync();

            ticks++;
            if (ticks == 1 || ticks == (uint64_t)refresh
                || (ticks % (uint64_t(refresh) * 10)) == 0) {
                Warn(
                    "Hmd: display timing ticks=%llu mode=%s",
                    (unsigned long long)ticks,
                    vsync_only ? "stream(vsync-only)" : "idle(pose+vsync)"
                );
            }

            next += period;
            auto now = clock::now();
            if (next < now) {
                next = now + period;
            } else {
                std::this_thread::sleep_until(next);
            }
        }
        Warn("Hmd: display timing pump stopped after %llu ticks", (unsigned long long)ticks);
    });
}

void Hmd::stop_display_timing_pump() {
    if (!m_displayTimingRunning.exchange(false)) {
        return;
    }
    if (m_displayTimingThread.joinable()) {
        m_displayTimingThread.join();
    }
}
#endif

bool Hmd::activate() {
    Debug("Hmd::Activate");
    // Warn so this appears in SteamVR driver log (Info is not driver-logged).
    Warn(
        "Hmd::activate begin object_id=%u device_class=%d serial=%s",
        (unsigned)this->object_id,
        (int)this->device_class,
        this->get_serial_number().c_str()
    );

    auto vr_properties = vr::VRProperties();

    SetOpenvrProps((void*)this, this->device_id);

    vr_properties->SetFloatProperty(
        this->prop_container,
        vr::Prop_DisplayFrequency_Float,
        static_cast<float>(Settings::Instance().m_refreshRate)
    );

    vr::VRDriverInput()->CreateBooleanComponent(this->prop_container, "/proximity", &m_proximity);

#ifdef _WIN32
    float originalIPD
        = vr::VRSettings()->GetFloat(vr::k_pch_SteamVR_Section, vr::k_pch_SteamVR_IPD_Float);
    vr::VRSettings()->SetFloat(vr::k_pch_SteamVR_Section, vr::k_pch_SteamVR_IPD_Float, 0.063);
#endif

    HmdMatrix_SetIdentity(&m_eyeToHeadLeft);
    HmdMatrix_SetIdentity(&m_eyeToHeadRight);

// Disable async reprojection on Linux. Windows interface uses IVRDriverDirectModeComponent
// which never applies reprojection
// Also Disable async reprojection on vulkan
#ifndef _WIN32
    vr::VRSettings()->SetBool(
        vr::k_pch_SteamVR_Section,
        vr::k_pch_SteamVR_EnableLinuxVulkanAsync_Bool,
        Settings::Instance().m_enableLinuxVulkanAsyncCompute
    );
    vr::VRSettings()->SetBool(
        vr::k_pch_SteamVR_Section,
        vr::k_pch_SteamVR_DisableAsyncReprojection_Bool,
        !Settings::Instance().m_enableLinuxAsyncReprojection
    );
#endif

    if (!m_baseComponentsInitialized) {
        m_baseComponentsInitialized = true;

        if (this->device_class == vr::TrackedDeviceClass_HMD) {
#ifdef _WIN32
            m_D3DRender = std::make_shared<CD3DRender>();

            // Use the same adapter as vrcompositor uses. If another adapter is used, vrcompositor
            // says "failed to open shared texture" and then crashes. It seems vrcompositor selects
            // always(?) first adapter. vrcompositor may use Intel iGPU when user sets it as primary
            // adapter. I don't know what happens on laptop which support optimus.
            // Prop_GraphicsAdapterLuid_Uint64 is only for redirect display and is ignored on direct
            // mode driver. So we can't specify an adapter for vrcompositor. m_nAdapterIndex is set
            // 0 on the dashboard.
            if (!m_D3DRender->Initialize(Settings::Instance().m_nAdapterIndex)) {
                Error(
                    "Could not create graphics device for adapter %d.  Requires a minimum of two "
                    "graphics cards.\n",
                    Settings::Instance().m_nAdapterIndex
                );
                return false;
            }

            int32_t nDisplayAdapterIndex;
            if (!m_D3DRender->GetAdapterInfo(&nDisplayAdapterIndex, m_adapterName)) {
                Error("Failed to get primary adapter info!\n");
                return false;
            }

            Info("Using %ls as primary graphics adapter.\n", m_adapterName.c_str());
            Info("OSVer: %ls\n", GetWindowsOSVersion().c_str());

            m_directModeComponent
                = std::make_shared<OvrDirectModeComponent>(m_D3DRender, m_poseHistory);
#endif
        }

        DriverReadyIdle(this->device_class == vr::TrackedDeviceClass_HMD);
        Warn("Hmd::activate: DriverReadyIdle dispatched (default chaperone if HMD)");
    }

    if (this->device_class == vr::TrackedDeviceClass_HMD) {
        // Apply default stereo geometry immediately (client ViewsConfig will refine).
        // Use constructor AVP-like FOV/IPD — not symmetric ±1 rad placeholders.
        SetViewsConfig(this->views_config);
        vr_properties->SetFloatProperty(
            this->prop_container, vr::Prop_UserIpdMeters_Float, this->views_config.ipd_m
        );

        // Submit a connected identity pose immediately so SteamVR has valid HMD tracking
        // before any client connects. Without this, last_pose stays Uninitialized and
        // vrcompositor logs "tracking is not OK" / can fail compositor interface (303).
        // Do NOT use VendorSpecificEvent for VREvent_IpdChanged (105) — outside vendor range.
        vr::DriverPose_t pose = {};
        pose.poseIsValid = true;
        pose.deviceIsConnected = true;
        pose.result = vr::TrackingResult_Running_OK;
        pose.qWorldFromDriverRotation = HmdQuaternion_Init(1, 0, 0, 0);
        pose.qDriverFromHeadRotation = HmdQuaternion_Init(1, 0, 0, 0);
        pose.qRotation = HmdQuaternion_Init(1, 0, 0, 0);
        // Seated height placeholder; client tracking will overwrite on stream.
        pose.vecPosition[1] = 1.6;
        this->submit_pose(pose);
        Warn(
            "Hmd::activate: submitted idle identity pose "
            "(connected, Running_OK, object_id=%u y=1.6 ipd=%.3f m)",
            (unsigned)this->object_id,
            this->views_config.ipd_m
        );
#ifndef _WIN32
        // Keep pose+vsync alive so VRMsg_StartVRCompositor can complete (avoids 10s 303).
        // Switches to vsync-only in StartStreaming (never fully stops).
        start_display_timing_pump(/*vsync_only=*/false);
#endif
    }

    Warn("Hmd::activate complete object_id=%u", (unsigned)this->object_id);
    return true;
}

void* Hmd::get_component(const char* component_name_and_version) {
    Debug("Hmd::GetComponent %s", component_name_and_version);

    // NB: "this" pointer needs to be statically cast to point to the correct vtable

    auto name_and_vers = std::string(component_name_and_version);
    if (name_and_vers == vr::IVRDisplayComponent_Version) {
        return (vr::IVRDisplayComponent*)this;
    }

#ifdef _WIN32
    if (name_and_vers == vr::IVRDriverDirectModeComponent_Version) {
        return m_directModeComponent.get();
    }
#endif

    return nullptr;
}

void Hmd::OnPoseUpdated(
    uint64_t targetTimestampNs, FfiDeviceMotion motion, float poseTimeOffsetS
) {
    Debug("Hmd::OnPoseUpdated");

    if (this->object_id == vr::k_unTrackedDeviceIndexInvalid) {
        return;
    }

    auto pose = vr::DriverPose_t {};
    pose.poseIsValid = true;
    pose.result = vr::TrackingResult_Running_OK;
    pose.deviceIsConnected = true;

    pose.qWorldFromDriverRotation = HmdQuaternion_Init(1, 0, 0, 0);
    pose.qDriverFromHeadRotation = HmdQuaternion_Init(1, 0, 0, 0);

    pose.qRotation = HmdQuaternion_Init(
        motion.orientation.w, motion.orientation.x, motion.orientation.y, motion.orientation.z
    );

    pose.vecPosition[0] = motion.position[0];
    pose.vecPosition[1] = motion.position[1];
    pose.vecPosition[2] = motion.position[2];

    // Match stock Windows / ALVR v20 head semantics:
    // - Client already predicts HEAD pose then zeros linear/angular velocity on the wire
    //   (client_core::send_tracking). Do not re-estimate velocity here.
    // - Do not apply steamvr_pipeline poseTimeOffset on the HMD (controllers still use it).
    //   Offset + finite-difference vel double-predicts and causes micro-overshoot jitter.
    // poseTimeOffsetS is ignored for HMD; kept in the signature for API stability.
    (void)poseTimeOffsetS;
    pose.vecVelocity[0] = 0.0;
    pose.vecVelocity[1] = 0.0;
    pose.vecVelocity[2] = 0.0;
    pose.vecAngularVelocity[0] = 0.0;
    pose.vecAngularVelocity[1] = 0.0;
    pose.vecAngularVelocity[2] = 0.0;
    pose.poseTimeOffset = 0.0;

    this->submit_pose(pose);
    // Confirm tracking → SteamVR TrackedDevicePoseUpdated path (controllers use same API).
    log_steamvr_hmd_pose("OnPoseUpdated", this->object_id, pose, targetTimestampNs);

    m_poseHistory->OnPoseUpdated(targetTimestampNs, motion);

    if (m_viveTrackerProxy)
        m_viveTrackerProxy->update();

#if !defined(_WIN32) && !defined(__APPLE__)
    // This has to be set after initialization is done, because something in vrcompositor is
    // setting it to 90Hz in the meantime
    if (!m_refreshRateSet && m_encoder && m_encoder->IsConnected()) {
        m_refreshRateSet = true;
        vr::VRProperties()->SetFloatProperty(
            this->prop_container,
            vr::Prop_DisplayFrequency_Float,
            static_cast<float>(Settings::Instance().m_refreshRate)
        );
    }
#endif
}

void Hmd::StartStreaming() {
    Debug("Hmd::StartStreaming");
    Warn("Hmd::StartStreaming device_class=%d streamComponentsInitialized=%d",
         (int)this->device_class,
         (int)m_streamComponentsInitialized);

#ifndef _WIN32
    // Stop resubmitting idle poses (client tracking owns poses) but KEEP VsyncEvent
    // at refresh rate — required for SteamVR frame pacing + client late-stage warp.
    start_display_timing_pump(/*vsync_only=*/true);
#endif

    vr::VRDriverInput()->UpdateBooleanComponent(m_proximity, true, 0.0);

    if (m_streamComponentsInitialized) {
        return;
    }

    // Spin up a separate thread to handle the overlapped encoding/transmit step.
    if (this->device_class == vr::TrackedDeviceClass_HMD) {
#ifdef _WIN32
        m_encoder = std::make_shared<CEncoder>();
        try {
            m_encoder->Initialize(m_D3DRender);
        } catch (Exception e) {
            Error(
                "Your GPU does not meet the requirements for video encoding. %s %s\n%s %s\n",
                "If you get this error after changing some settings, you can revert them by",
                "deleting the file \"session.json\" in the installation folder.",
                "Failed to initialize CEncoder:",
                e.what()
            );
        }
        m_encoder->Start();

        m_directModeComponent->SetEncoder(m_encoder);

#elif __APPLE__
        m_encoder = std::make_shared<CEncoder>();
#else
        m_encoder = std::make_shared<CEncoder>(m_poseHistory);
        m_encoder->Start();
        Warn("Hmd::StartStreaming: Linux CEncoder thread started");
#endif
        m_encoder->OnStreamStart();
    }

    m_streamComponentsInitialized = true;
}

void Hmd::StopStreaming() {
    Debug("Hmd::StopStreaming");

    vr::VRDriverInput()->UpdateBooleanComponent(m_proximity, false, 0.0);
#ifndef _WIN32
    // Back to idle readiness mode.
    start_display_timing_pump(/*vsync_only=*/false);
#endif
}

void Hmd::SetViewsConfig(FfiViewsConfig config) {
    Debug("Hmd::SetViewsConfig");

    auto fov_ok = [](const FfiFov& f) {
        return std::isfinite(f.left) && std::isfinite(f.right) && std::isfinite(f.up)
            && std::isfinite(f.down) && std::fabs(f.left) < 2.5f && std::fabs(f.right) < 2.5f
            && std::fabs(f.up) < 2.5f && std::fabs(f.down) < 2.5f;
    };

    // Reject non-finite / absurd FOV entirely. Applying tan(±inf) to SteamVR
    // wrecks projection matrices and late-stage warp on the client.
    // Also require ordered edges (left < right, down < up) like OpenXR FOV.
    auto fov_ordered = [](const FfiFov& f) { return f.left < f.right && f.down < f.up; };
    if (!fov_ok(config.fov[0]) || !fov_ok(config.fov[1]) || !fov_ordered(config.fov[0])
        || !fov_ordered(config.fov[1])) {
        static int reject_count = 0;
        if (reject_count++ < 5 || (reject_count % 50) == 0) {
            Warn(
                "Hmd::SetViewsConfig: REJECT invalid FOV (count=%d) "
                "fovL=[%g,%g,%g,%g] fovR=[%g,%g,%g,%g] ipd=%g — keeping previous views",
                reject_count,
                config.fov[0].left,
                config.fov[0].right,
                config.fov[0].up,
                config.fov[0].down,
                config.fov[1].left,
                config.fov[1].right,
                config.fov[1].up,
                config.fov[1].down,
                config.ipd_m
            );
        }
        return;
    }

    // Guard absurd IPD (e.g. units confusion / ipd=0 from early client packets).
    // Do NOT apply FOV with a substituted IPD alone — require a sane IPD together.
    float ipd = config.ipd_m;
    if (!(ipd > 0.04f && ipd < 0.10f)) {
        static int ipd_reject = 0;
        if (ipd_reject++ < 5 || (ipd_reject % 50) == 0) {
            Warn(
                "Hmd::SetViewsConfig: REJECT invalid ipd_m=%.4f (expected ~0.05–0.08 m) "
                "— keeping previous views (count=%d)",
                ipd,
                ipd_reject
            );
        }
        return;
    }
    this->views_config = config;

    // Allow GetProjectionRaw / GetEyeOutputViewport logs again after each publish
    // so we can verify SteamVR re-queries post-ViewsConfig.
    m_stereoGetLogRemaining = 8;

    Warn(
        "Hmd::SetViewsConfig ipd_m=%.4f fovL=[%.3f,%.3f,%.3f,%.3f] fovR=[%.3f,%.3f,%.3f,%.3f]",
        ipd,
        config.fov[0].left,
        config.fov[0].right,
        config.fov[0].up,
        config.fov[0].down,
        config.fov[1].left,
        config.fov[1].right,
        config.fov[1].up,
        config.fov[1].down
    );

    // The OpenXR spec defines the HMD position as the midpoint
    // between the eyes, so conversion to this is handled by the
    // client. +X is right: left eye at -ipd/2, right at +ipd/2.
    auto left_transform = MATRIX_IDENTITY;
    left_transform.m[0][3] = -ipd / 2.0f;
    auto right_transform = MATRIX_IDENTITY;
    right_transform.m[0][3] = ipd / 2.0f;
    vr::VRServerDriverHost()->SetDisplayEyeToHead(object_id, left_transform, right_transform);

    auto left_proj = fov_to_projection(config.fov[0]);
    auto right_proj = fov_to_projection(config.fov[1]);

    vr::VRServerDriverHost()->SetDisplayProjectionRaw(object_id, left_proj, right_proj);

    // Keep SteamVR global IPD in sync (Windows already sets this on activate).
    // Mismatch with eye-to-head can widen effective stereo separation.
    vr::VRSettings()->SetFloat(vr::k_pch_SteamVR_Section, vr::k_pch_SteamVR_IPD_Float, ipd);

    if (this->prop_container != vr::k_ulInvalidPropertyContainer) {
        vr::VRProperties()->SetFloatProperty(
            this->prop_container, vr::Prop_UserIpdMeters_Float, ipd
        );
    }

    // Stereo geometry dump for RCA (eye swap / too-wide IPD diagnosis).
    Warn(
        "STEREO geom: ipd=%.4fm eyeToHead L.x=%.4f R.x=%.4f "
        "projRaw L tan[l,r,t,b]=[%.3f,%.3f,%.3f,%.3f] "
        "R tan[l,r,t,b]=[%.3f,%.3f,%.3f,%.3f] "
        "viewport L=x0 R=x(width/2) width=%u — "
        "if eyes feel swapped, L/R proj or eyeToHead signs may be inverted",
        ipd,
        left_transform.m[0][3],
        right_transform.m[0][3],
        left_proj.vTopLeft.v[0],
        left_proj.vBottomRight.v[0],
        left_proj.vTopLeft.v[1],
        left_proj.vBottomRight.v[1],
        right_proj.vTopLeft.v[0],
        right_proj.vBottomRight.v[0],
        right_proj.vTopLeft.v[1],
        right_proj.vBottomRight.v[1],
        Settings::Instance().m_renderWidth
    );

#ifdef _WIN32
    if (m_encoder) {
        m_encoder->SetViewsConfig(left_proj, left_transform, right_proj, right_transform);
    }
#endif

    // VREvent_LensDistortionChanged (110) must not go through VendorSpecificEvent
    // (vendor range is 10000-19999). SetDisplayEyeToHead / SetDisplayProjectionRaw above
    // already update the runtime display geometry.
}

void Hmd::GetWindowBounds(int32_t* pnX, int32_t* pnY, uint32_t* pnWidth, uint32_t* pnHeight) {
    Debug(
        "Hmd::GetWindowBounds %dx%d - %dx%d\n",
        0,
        0,
        Settings::Instance().m_renderWidth,
        Settings::Instance().m_renderHeight
    );

    *pnX = 0;
    *pnY = 0;
    *pnWidth = Settings::Instance().m_renderWidth;
    *pnHeight = Settings::Instance().m_renderHeight;
}

bool Hmd::IsDisplayRealDisplay() {
#ifdef _WIN32
    return false;
#else
    return true;
#endif
}

void Hmd::GetRecommendedRenderTargetSize(uint32_t* pnWidth, uint32_t* pnHeight) {
    *pnWidth = Settings::Instance().m_recommendedTargetWidth / 2;
    *pnHeight = Settings::Instance().m_recommendedTargetHeight;
    Debug("Hmd::GetRecommendedRenderTargetSize %dx%d\n", *pnWidth, *pnHeight);
}

void Hmd::GetEyeOutputViewport(
    vr::EVREye eEye, uint32_t* pnX, uint32_t* pnY, uint32_t* pnWidth, uint32_t* pnHeight
) {
    *pnY = 0;
    *pnWidth = Settings::Instance().m_renderWidth / 2;
    *pnHeight = Settings::Instance().m_renderHeight;

    if (eEye == vr::Eye_Left) {
        *pnX = 0;
    } else {
        *pnX = Settings::Instance().m_renderWidth / 2;
    }

    // Log first few queries after each ViewsConfig publish.
    if (m_stereoGetLogRemaining > 0) {
        m_stereoGetLogRemaining--;
        Warn(
            "STEREO GetEyeOutputViewport eye=%s x=%u y=%u w=%u h=%u (fullW=%u)",
            eEye == vr::Eye_Left ? "LEFT" : "RIGHT",
            *pnX,
            *pnY,
            *pnWidth,
            *pnHeight,
            Settings::Instance().m_renderWidth
        );
    }
}

void Hmd::GetProjectionRaw(vr::EVREye eye, float* left, float* right, float* top, float* bottom) {
    auto proj = fov_to_projection(this->views_config.fov[eye]);
    *left = proj.vTopLeft.v[0];
    *right = proj.vBottomRight.v[0];
    *top = proj.vTopLeft.v[1];
    *bottom = proj.vBottomRight.v[1];

    if (m_stereoGetLogRemaining > 0) {
        m_stereoGetLogRemaining--;
        Warn(
            "STEREO GetProjectionRaw eye=%s tan[l,r,t,b]=[%.3f,%.3f,%.3f,%.3f] "
            "fov_rad=[%.3f,%.3f,%.3f,%.3f] ipd=%.4f",
            eye == vr::Eye_Left ? "LEFT" : "RIGHT",
            *left,
            *right,
            *top,
            *bottom,
            this->views_config.fov[eye].left,
            this->views_config.fov[eye].right,
            this->views_config.fov[eye].up,
            this->views_config.fov[eye].down,
            this->views_config.ipd_m
        );
    }
}

vr::DistortionCoordinates_t Hmd::ComputeDistortion(vr::EVREye, float u, float v) {
    return { { u, v }, { u, v }, { u, v } };
}
