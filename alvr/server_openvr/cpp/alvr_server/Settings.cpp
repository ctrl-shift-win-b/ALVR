#include "Settings.h"
#include "Logger.h"
#include "bindings.h"
#include <cstdlib>
#include <filesystem>
#include <fstream>
#include <streambuf>
#include <string>

#define PICOJSON_USE_INT64
#include "include/picojson.h"

using namespace std;

extern uint64_t g_DriverTestMode;

Settings Settings::m_Instance;

Settings::Settings()
    : m_loaded(false) { }

Settings::~Settings() { }

void Settings::Load() {
    try {
        auto sessionFile = std::ifstream(g_sessionPath);

        auto json = std::string(
            std::istreambuf_iterator<char>(sessionFile), std::istreambuf_iterator<char>()
        );

        picojson::value v;
        std::string err = picojson::parse(v, json);
        if (!err.empty()) {
            Error("Error on parsing session config (%s): %hs\n", g_sessionPath, err.c_str());
            return;
        }

        auto config = v.get("openvr_config");

        m_refreshRate = (int)config.get("refresh_rate").get<int64_t>();
        m_renderWidth = config.get("eye_resolution_width").get<int64_t>() * 2;
        m_renderHeight = config.get("eye_resolution_height").get<int64_t>();
        m_recommendedTargetWidth = config.get("target_eye_resolution_width").get<int64_t>() * 2;
        m_recommendedTargetHeight = config.get("target_eye_resolution_height").get<int64_t>();
        m_nAdapterIndex = (int32_t)config.get("adapter_index").get<int64_t>();
        m_captureFrameDir = config.get("capture_frame_dir").get<std::string>();

        m_enableFoveatedEncoding = config.get("enable_foveated_encoding").get<bool>();
        m_foveationCenterSizeX = (float)config.get("foveation_center_size_x").get<double>();
        m_foveationCenterSizeY = (float)config.get("foveation_center_size_y").get<double>();
        m_foveationCenterShiftX = (float)config.get("foveation_center_shift_x").get<double>();
        m_foveationCenterShiftY = (float)config.get("foveation_center_shift_y").get<double>();
        m_foveationEdgeRatioX = (float)config.get("foveation_edge_ratio_x").get<double>();
        m_foveationEdgeRatioY = (float)config.get("foveation_edge_ratio_y").get<double>();

        m_enableColorCorrection = config.get("enable_color_correction").get<bool>();
        m_brightness = (float)config.get("brightness").get<double>();
        m_contrast = (float)config.get("contrast").get<double>();
        m_saturation = (float)config.get("saturation").get<double>();
        m_gamma = (float)config.get("gamma").get<double>();
        m_sharpening = (float)config.get("sharpening").get<double>();

        m_codec = (int32_t)config.get("codec").get<int64_t>();
        m_h264Profile = (int32_t)config.get("h264_profile").get<int64_t>();
        m_rateControlMode = (uint32_t)config.get("rate_control_mode").get<int64_t>();
        m_fillerData = config.get("filler_data").get<bool>();
        m_entropyCoding = (uint32_t)config.get("entropy_coding").get<int64_t>();
        m_use10bitEncoder = config.get("use_10bit_encoder").get<bool>();
        m_useFullRangeEncoding = config.get("use_full_range_encoding").get<bool>();
        m_encodingGamma = config.get("encoding_gamma").get<double>();
        m_enableHdr = config.get("enable_hdr").get<bool>();
        m_forceHdrSrgbCorrection = config.get("force_hdr_srgb_correction").get<bool>();
        m_clampHdrExtendedRange = config.get("clamp_hdr_extended_range").get<bool>();
        m_enableAmfPreAnalysis = config.get("enable_amf_pre_analysis").get<bool>();
        m_enableVbaq = config.get("enable_vbaq").get<bool>();
        m_enableAmfHmqb = config.get("enable_amf_hmqb").get<bool>();
        m_useAmfPreproc = config.get("use_amf_preproc").get<bool>();
        m_amfPreProcSigma = (uint32_t)config.get("amf_preproc_sigma").get<int64_t>();
        m_amfPreProcTor = (uint32_t)config.get("amf_preproc_tor").get<int64_t>();
        m_encoderQualityPreset = (uint32_t)config.get("encoder_quality_preset").get<int64_t>();
        m_amdBitrateCorruptionFix = (bool)config.get("amd_bitrate_corruption_fix").get<bool>();
        m_nvencQualityPreset = (uint32_t)config.get("nvenc_quality_preset").get<int64_t>();
        m_force_sw_encoding = config.get("force_sw_encoding").get<bool>();
        m_swThreadCount = (int32_t)config.get("sw_thread_count").get<int64_t>();

        m_nvencTuningPreset = (uint32_t)config.get("nvenc_tuning_preset").get<int64_t>();
        m_nvencMultiPass = (uint32_t)config.get("nvenc_multi_pass").get<int64_t>();
        m_nvencAdaptiveQuantizationMode
            = (uint32_t)config.get("nvenc_adaptive_quantization_mode").get<int64_t>();
        m_nvencLowDelayKeyFrameScale = config.get("nvenc_low_delay_key_frame_scale").get<int64_t>();
        m_nvencRefreshRate = config.get("nvenc_refresh_rate").get<int64_t>();
        m_nvencEnableIntraRefresh = config.get("enable_intra_refresh").get<bool>();
        m_nvencIntraRefreshPeriod = config.get("intra_refresh_period").get<int64_t>();
        m_nvencIntraRefreshCount = config.get("intra_refresh_count").get<int64_t>();
        m_nvencMaxNumRefFrames = config.get("max_num_ref_frames").get<int64_t>();
        m_nvencGopLength = config.get("gop_length").get<int64_t>();
        m_nvencPFrameStrategy = config.get("p_frame_strategy").get<int64_t>();
        m_nvencRateControlMode = config.get("nvenc_rate_control_mode").get<int64_t>();
        m_nvencRcBufferSize = config.get("rc_buffer_size").get<int64_t>();
        m_nvencRcInitialDelay = config.get("rc_initial_delay").get<int64_t>();
        m_nvencRcMaxBitrate = config.get("rc_max_bitrate").get<int64_t>();
        m_nvencRcAverageBitrate = config.get("rc_average_bitrate").get<int64_t>();
        m_nvencEnableWeightedPrediction
            = config.get("nvenc_enable_weighted_prediction").get<bool>();

        m_minimumIdrIntervalMs = config.get("minimum_idr_interval_ms").get<int64_t>();

        m_enableViveTrackerProxy = config.get("enable_vive_tracker_proxy").get<bool>();
        m_TrackingRefOnly = config.get("tracking_ref_only").get<bool>();
        m_enableLinuxVulkanAsyncCompute = config.get("linux_async_compute").get<bool>();
        m_enableLinuxAsyncReprojection = config.get("linux_async_reprojection").get<bool>();

        m_enableControllers = config.get("controllers_enabled").get<bool>();
        m_controllerIsTracker = config.get("controller_is_tracker").get<bool>();

        m_enableBodyTrackingFakeVive = config.get("body_tracking_vive_enabled").get<bool>();
        m_bodyTrackingHasLegs = config.get("body_tracking_has_legs").get<bool>();

        m_useSeparateHandTrackers = config.get("use_separate_hand_trackers").get<bool>();

        auto json_f32 = [](const picojson::value& parent, const char* key, float fallback) {
            auto v = parent.get(key);
            if (v.is<double>())
                return (float)v.get<double>();
            if (v.is<int64_t>())
                return (float)v.get<int64_t>();
            return fallback;
        };
        auto stereo = config.get("default_stereo");
        m_defaultIpdM = json_f32(stereo, "ipd_m", 0.063f);
        m_defaultFovLLeft = json_f32(stereo, "fov_l_left", -1.054f);
        m_defaultFovLRight = json_f32(stereo, "fov_l_right", 0.791f);
        m_defaultFovLUp = json_f32(stereo, "fov_l_up", 0.878f);
        m_defaultFovLDown = json_f32(stereo, "fov_l_down", -0.791f);
        m_defaultFovRLeft = json_f32(stereo, "fov_r_left", -0.793f);
        m_defaultFovRRight = json_f32(stereo, "fov_r_right", 1.057f);
        m_defaultFovRUp = json_f32(stereo, "fov_r_up", 0.881f);
        m_defaultFovRDown = json_f32(stereo, "fov_r_down", -0.793f);

        Info("Render Target: %d %d\n", m_renderWidth, m_renderHeight);
        Info("Refresh Rate: %d\n", m_refreshRate);
        Info(
            "Baked stereo ipd=%.4f fovL=[%.3f,%.3f,%.3f,%.3f] fovR=[%.3f,%.3f,%.3f,%.3f]",
            m_defaultIpdM,
            m_defaultFovLLeft,
            m_defaultFovLRight,
            m_defaultFovLUp,
            m_defaultFovLDown,
            m_defaultFovRLeft,
            m_defaultFovRRight,
            m_defaultFovRUp,
            m_defaultFovRDown
        );
        m_loaded = true;
    } catch (std::exception& e) {
        Error("Exception on parsing session config (%s): %hs\n", g_sessionPath, e.what());
    }
}
