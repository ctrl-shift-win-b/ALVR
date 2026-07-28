//! Hard SteamVR configuration solidification for ALVR 20.14.x.
//!
//! OpenVR device identity is fixed at driver init. Changing it used to force full SteamVR
//! restarts on client connect. Solidify bakes layout into `openvr_config` *before* launch so
//! the connect path never needs to restart.

use crate::{
    ControllersEmulationMode, FrameSize, OpenvrConfig, SessionConfig, SocketProtocolDefaultVariant,
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

/// Bake hard layout fields into `openvr_config` and mark the session solidified.
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

    let cfg = &mut session.openvr_config;
    cfg.eye_resolution_width = eye_w;
    cfg.eye_resolution_height = eye_h;
    cfg.target_eye_resolution_width = target_w;
    cfg.target_eye_resolution_height = target_h;
    cfg.refresh_rate = settings.video.preferred_fps.round().max(1.0) as u32;
    cfg.controllers_enabled = controllers_enabled;
    cfg.controller_is_tracker = controller_is_tracker;
    cfg.use_separate_hand_trackers = use_separate_hand_trackers;
    cfg._controller_profile = controller_profile;
    cfg.adapter_index = settings.video.adapter_index;
    cfg.tracking_ref_only = settings.headset.tracking_ref_only;
    cfg.enable_vive_tracker_proxy = settings.headset.enable_vive_tracker_proxy;
    cfg.linux_async_compute = settings.extra.patches.linux_async_compute;
    cfg.linux_async_reprojection = settings.extra.patches.linux_async_reprojection;

    session.hard_config_solidified = true;
}

/// Defaults safe for App Store AVP client 20.14.1 (no PSVR2 Sense profile in this streamer).
pub fn apply_avp_profile(session: &mut SessionConfig) {
    let ss = &mut session.session_settings;
    ss.headset.controllers.enabled = true;
    // Valve Index is the best generic SteamVR profile on 20.14.1 for non-Quest controllers.
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
