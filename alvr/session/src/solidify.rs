//! Hard SteamVR configuration solidification for ALVR 20.14.x.
//!
//! Bakes OpenVR + encoder init fields into `openvr_config` before SteamVR launches so
//! the connect path never needs to restart, and NvEnc gets valid presets (not 0 / "p0").

use crate::{
    BodyTrackingBDConfig, BodyTrackingSinkConfig, ControllersEmulationMode, FrameSize, OpenvrConfig,
    SessionConfig, SocketProtocolDefaultVariant,
};
use alvr_common::settings_schema::Switch;

/// Per-eye reference when resolution is Scale and no client has been seen.
pub const SOLIDIFY_REFERENCE_VIEW_WIDTH: u32 = 2144;
pub const SOLIDIFY_REFERENCE_VIEW_HEIGHT: u32 = 2144;

fn align32(value: f32) -> u32 {
    ((value / 32.).floor() * 32.) as u32
}

pub fn resolve_view_resolution(config: &FrameSize, default_res: (u32, u32)) -> (u32, u32) {
    let (default_w, default_h) = default_res;
    let res = match config {
        FrameSize::Scale(scale) => (default_w as f32 * scale, default_h as f32 * scale),
        FrameSize::Absolute { width, height } => {
            let width = *width as f32;
            let height = height.map_or_else(
                || {
                    if default_w > 0 {
                        width * default_h as f32 / default_w as f32
                    } else {
                        width
                    }
                },
                |h| h as f32,
            );
            (width, height)
        }
    };
    (align32(res.0).max(32), align32(res.1).max(32))
}

/// Full openvr_config from session settings + locked per-eye resolution / FPS.
/// Mirrors server_core::contruct_openvr_config so NvEnc presets (P1–P7, tune 1–4) are valid.
pub fn solidify_hard_config(session: &mut SessionConfig) {
    let settings = session.to_settings();

    let reference = (
        SOLIDIFY_REFERENCE_VIEW_WIDTH,
        SOLIDIFY_REFERENCE_VIEW_HEIGHT,
    );
    let (eye_w, eye_h) =
        resolve_view_resolution(&settings.video.transcoding_view_resolution, reference);
    let (target_w, target_h) =
        resolve_view_resolution(&settings.video.emulated_headset_view_resolution, reference);

    let mut controller_is_tracker = false;
    let mut controller_profile = 0i32;
    let mut use_separate_hand_trackers = false;
    let controllers_enabled = if let Switch::Enabled(config) = &settings.headset.controllers {
        controller_is_tracker =
            matches!(config.emulation_mode, ControllersEmulationMode::ViveTracker);
        controller_profile = match config.emulation_mode {
            ControllersEmulationMode::RiftSTouch => 0,
            ControllersEmulationMode::Quest2Touch => 1,
            ControllersEmulationMode::Quest3Plus => 2,
            ControllersEmulationMode::QuestPro => 3,
            ControllersEmulationMode::Pico4 => 10,
            ControllersEmulationMode::ValveIndex => 20,
            ControllersEmulationMode::ViveWand => 40,
            ControllersEmulationMode::ViveTracker => 41,
            ControllersEmulationMode::Custom { .. } => 500,
        };
        use_separate_hand_trackers = config
            .hand_skeleton
            .as_option()
            .is_some_and(|c| c.steamvr_input_2_0);
        true
    } else {
        false
    };

    let body_tracking_vive_enabled =
        if let Switch::Enabled(config) = &settings.headset.body_tracking {
            matches!(config.sink, BodyTrackingSinkConfig::FakeViveTracker)
        } else {
            false
        };

    let body_tracking_has_legs = settings
        .headset
        .body_tracking
        .as_option()
        .and_then(|c| c.sources.body_tracking_fb.as_option().cloned())
        .map(|c| c.full_body)
        .or_else(|| {
            settings.headset.body_tracking.as_option().map(|c| {
                matches!(
                    c.sources.body_tracking_bd.as_option(),
                    Some(BodyTrackingBDConfig::BodyTracking { .. })
                )
            })
        })
        .unwrap_or(false);

    let mut foveation_center_size_x = 0.0;
    let mut foveation_center_size_y = 0.0;
    let mut foveation_center_shift_x = 0.0;
    let mut foveation_center_shift_y = 0.0;
    let mut foveation_edge_ratio_x = 0.0;
    let mut foveation_edge_ratio_y = 0.0;
    let enable_foveated_encoding =
        if let Switch::Enabled(config) = &settings.video.foveated_encoding {
            foveation_center_size_x = config.center_size_x;
            foveation_center_size_y = config.center_size_y;
            foveation_center_shift_x = config.center_shift_x;
            foveation_center_shift_y = config.center_shift_y;
            foveation_edge_ratio_x = config.edge_ratio_x;
            foveation_edge_ratio_y = config.edge_ratio_y;
            true
        } else {
            false
        };

    let mut brightness = 0.0;
    let mut contrast = 0.0;
    let mut saturation = 0.0;
    let mut gamma = 0.0;
    let mut sharpening = 0.0;
    let enable_color_correction =
        if let Switch::Enabled(config) = &settings.video.color_correction {
            brightness = config.brightness;
            contrast = config.contrast;
            saturation = config.saturation;
            gamma = config.gamma;
            sharpening = config.sharpening;
            true
        } else {
            false
        };

    let nvenc = settings.video.encoder_config.nvenc.clone();
    let amf = settings.video.encoder_config.amf.clone();
    let hdr = settings.video.encoder_config.hdr.clone();

    // NvEnc: enum values are already 1..=7 (P1–P7) and 1..=4 (tune). Never leave 0.
    let nvenc_quality = (nvenc.quality_preset as u32).clamp(1, 7);
    let nvenc_tuning = (nvenc.tuning_preset as u32).clamp(1, 4);

    session.openvr_config = OpenvrConfig {
        eye_resolution_width: eye_w,
        eye_resolution_height: eye_h,
        target_eye_resolution_width: target_w,
        target_eye_resolution_height: target_h,
        refresh_rate: settings.video.preferred_fps.round().max(1.0) as u32,
        tracking_ref_only: settings.headset.tracking_ref_only,
        enable_vive_tracker_proxy: settings.headset.enable_vive_tracker_proxy,
        minimum_idr_interval_ms: settings.connection.minimum_idr_interval_ms,
        adapter_index: settings.video.adapter_index,
        codec: settings.video.preferred_codec as _,
        h264_profile: settings.video.encoder_config.h264_profile as u32,
        rate_control_mode: settings.video.encoder_config.rate_control_mode as u32,
        filler_data: settings.video.encoder_config.filler_data,
        entropy_coding: settings.video.encoder_config.entropy_coding as u32,
        force_hdr_srgb_correction: hdr.force_hdr_srgb_correction,
        clamp_hdr_extended_range: hdr.clamp_hdr_extended_range,
        enable_amf_pre_analysis: amf.enable_pre_analysis,
        enable_vbaq: settings.video.encoder_config.enable_vbaq,
        enable_amf_hmqb: amf.enable_hmqb,
        use_amf_preproc: amf.use_preproc,
        amf_preproc_sigma: amf.preproc_sigma,
        amf_preproc_tor: amf.preproc_tor,
        nvenc_quality_preset: nvenc_quality,
        encoder_quality_preset: settings.video.encoder_config.quality_preset as u32,
        force_sw_encoding: settings
            .video
            .encoder_config
            .software
            .force_software_encoding,
        sw_thread_count: settings.video.encoder_config.software.thread_count,
        controllers_enabled,
        controller_is_tracker,
        body_tracking_vive_enabled,
        body_tracking_has_legs,
        enable_foveated_encoding,
        foveation_center_size_x,
        foveation_center_size_y,
        foveation_center_shift_x,
        foveation_center_shift_y,
        foveation_edge_ratio_x,
        foveation_edge_ratio_y,
        enable_color_correction,
        brightness,
        contrast,
        saturation,
        gamma,
        sharpening,
        linux_async_compute: settings.extra.patches.linux_async_compute,
        linux_async_reprojection: settings.extra.patches.linux_async_reprojection,
        nvenc_tuning_preset: nvenc_tuning,
        nvenc_multi_pass: nvenc.multi_pass as u32,
        nvenc_adaptive_quantization_mode: nvenc.adaptive_quantization_mode as u32,
        nvenc_low_delay_key_frame_scale: nvenc.low_delay_key_frame_scale,
        nvenc_refresh_rate: nvenc.refresh_rate,
        enable_intra_refresh: nvenc.enable_intra_refresh,
        intra_refresh_period: nvenc.intra_refresh_period,
        intra_refresh_count: nvenc.intra_refresh_count,
        max_num_ref_frames: nvenc.max_num_ref_frames,
        gop_length: nvenc.gop_length,
        p_frame_strategy: nvenc.p_frame_strategy,
        nvenc_rate_control_mode: nvenc.rate_control_mode,
        rc_buffer_size: nvenc.rc_buffer_size,
        rc_initial_delay: nvenc.rc_initial_delay,
        rc_max_bitrate: nvenc.rc_max_bitrate,
        rc_average_bitrate: nvenc.rc_average_bitrate,
        nvenc_enable_weighted_prediction: nvenc.enable_weighted_prediction,
        capture_frame_dir: settings.extra.capture.capture_frame_dir.clone(),
        amd_bitrate_corruption_fix: settings.video.bitrate.image_corruption_fix,
        use_separate_hand_trackers,
        _controller_profile: controller_profile,
        _server_impl_debug: settings.extra.logging.debug_groups.server_impl,
        _client_impl_debug: settings.extra.logging.debug_groups.client_impl,
        _server_core_debug: settings.extra.logging.debug_groups.server_core,
        _client_core_debug: settings.extra.logging.debug_groups.client_core,
        _connection_debug: settings.extra.logging.debug_groups.connection,
        _sockets_debug: settings.extra.logging.debug_groups.sockets,
        _server_gfx_debug: settings.extra.logging.debug_groups.server_gfx,
        _client_gfx_debug: settings.extra.logging.debug_groups.client_gfx,
        _encoder_debug: settings.extra.logging.debug_groups.encoder,
        _decoder_debug: settings.extra.logging.debug_groups.decoder,
        use_10bit_encoder: settings.video.encoder_config.use_10bit,
        use_full_range_encoding: settings.video.encoder_config.use_full_range,
        encoding_gamma: settings.video.encoder_config.encoding_gamma,
        enable_hdr: hdr.enable_hdr,
    };

    session.hard_config_solidified = true;
}

/// Defaults safe for App Store AVP client 20.14.1.
pub fn apply_avp_profile(session: &mut SessionConfig) {
    let ss = &mut session.session_settings;
    ss.headset.controllers.enabled = true;
    ss.headset.controllers.content.emulation_mode.variant =
        crate::ControllersEmulationModeDefaultVariant::ValveIndex;
    ss.headset.controllers.content.hand_skeleton.enabled = false;
    ss.headset
        .controllers
        .content
        .hand_skeleton
        .content
        .steamvr_input_2_0 = false;
    ss.connection.stream_protocol.variant = SocketProtocolDefaultVariant::Tcp;
    // Sensible NVENC defaults for a 5090-class card
    ss.video.encoder_config.nvenc.quality_preset.variant =
        crate::EncoderQualityPresetNvidiaDefaultVariant::P1;
    ss.video.encoder_config.nvenc.tuning_preset.variant =
        crate::NvencTuningPresetDefaultVariant::UltraLowLatency;
    session.hard_config_solidified = false;
}

impl SessionConfig {
    pub fn solidify_hard_config(&mut self) {
        solidify_hard_config(self);
    }

    pub fn apply_avp_profile(&mut self) {
        apply_avp_profile(self);
    }

    pub fn locked_openvr_layout(&self) -> &OpenvrConfig {
        &self.openvr_config
    }
}
