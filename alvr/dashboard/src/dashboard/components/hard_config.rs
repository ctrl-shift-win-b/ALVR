//! SteamVR Streaming config (pre-launch hard config).
//!
//! PC-side setup in the Virtual Desktop spirit: bake display, codec, bitrate, and
//! device identity before SteamVR starts. App Store AVP client 20.14.x.

use alvr_gui_common::theme;
use alvr_packets::ServerRequest;
use alvr_session::{
    BitrateModeDefaultVariant, CodecTypeDefaultVariant, ControllersEmulationModeDefaultVariant,
    FrameSizeDefaultVariant, SessionConfig, SocketProtocolDefaultVariant,
};
use eframe::egui::{self, RichText, ScrollArea, Ui};

/// Practical H.264 per-eye limit on this Linux/NVENC stack (stream fails above ~3500).
pub const H264_MAX_EYE_PX: u32 = 3500;
/// High preset / HEVC target.
pub const HEVC_HIGH_EYE_PX: u32 = 5000;

pub enum HardConfigAction {
    ServerRequest(ServerRequest),
    SolidifyAndLaunch(Box<SessionConfig>),
    ApplySession(Box<SessionConfig>),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StreamingPreset {
    Low,
    Medium,
    High,
    Custom,
}

impl StreamingPreset {
    fn label(self) -> &'static str {
        match self {
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
            Self::Custom => "Custom",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::Low => "90 Hz · 2160×2160 · H.264 · 50 Mbps",
            Self::Medium => "90 Hz · 3400×3400 · H.264 · 70 Mbps",
            Self::High => "90 Hz · 5000×5000 · HEVC · 90 Mbps",
            Self::Custom => "Manual values below",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum CodecChoice {
    H264,
    Hevc,
    Av1,
}

impl CodecChoice {
    fn label(self) -> &'static str {
        match self {
            Self::H264 => "H.264",
            Self::Hevc => "HEVC (H.265)",
            Self::Av1 => "AV1",
        }
    }

    fn avp_note(self) -> &'static str {
        match self {
            Self::H264 => "AVP OK. Practical max ~3500×3500 per eye on this stack.",
            Self::Hevc => "AVP OK. Can use higher res (e.g. 5000×5000).",
            Self::Av1 => "Not supported for App Store AVP — stream typically fails to start.",
        }
    }

    fn to_variant(self) -> CodecTypeDefaultVariant {
        match self {
            Self::H264 => CodecTypeDefaultVariant::H264,
            Self::Hevc => CodecTypeDefaultVariant::Hevc,
            Self::Av1 => CodecTypeDefaultVariant::AV1,
        }
    }

    fn from_variant(v: &CodecTypeDefaultVariant) -> Self {
        match v {
            CodecTypeDefaultVariant::H264 => Self::H264,
            CodecTypeDefaultVariant::Hevc => Self::Hevc,
            CodecTypeDefaultVariant::AV1 => Self::Av1,
        }
    }
}

pub struct HardConfigTab {
    preset: StreamingPreset,
    preferred_fps: f32,
    stream_width: u32,
    stream_height: u32,
    use_absolute_resolution: bool,
    codec: CodecChoice,
    bitrate_mbps: u64,
    controllers_enabled: bool,
    controller_profile: ControllerProfileChoice,
    hand_skeleton: bool,
    hand_skeleton_steamvr_2: bool,
    stream_tcp: bool,
    status_message: Option<String>,
    /// Skip auto Custom flip while applying a preset.
    applying_preset: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ControllerProfileChoice {
    ValveIndex,
    Quest3Plus,
    Quest2Touch,
    QuestPro,
    Pico4,
    RiftSTouch,
}

impl ControllerProfileChoice {
    fn label(self) -> &'static str {
        match self {
            Self::ValveIndex => "Valve Index",
            Self::Quest3Plus => "Quest 3 Touch Plus",
            Self::Quest2Touch => "Quest 2 Touch",
            Self::QuestPro => "Quest Pro",
            Self::Pico4 => "Pico 4",
            Self::RiftSTouch => "Rift S Touch",
        }
    }

    fn to_variant(self) -> ControllersEmulationModeDefaultVariant {
        match self {
            Self::ValveIndex => ControllersEmulationModeDefaultVariant::ValveIndex,
            Self::Quest3Plus => ControllersEmulationModeDefaultVariant::Quest3Plus,
            Self::Quest2Touch => ControllersEmulationModeDefaultVariant::Quest2Touch,
            Self::QuestPro => ControllersEmulationModeDefaultVariant::QuestPro,
            Self::Pico4 => ControllersEmulationModeDefaultVariant::Pico4,
            Self::RiftSTouch => ControllersEmulationModeDefaultVariant::RiftSTouch,
        }
    }

    fn from_variant(v: &ControllersEmulationModeDefaultVariant) -> Self {
        match v {
            ControllersEmulationModeDefaultVariant::ValveIndex => Self::ValveIndex,
            ControllersEmulationModeDefaultVariant::Quest3Plus => Self::Quest3Plus,
            ControllersEmulationModeDefaultVariant::Quest2Touch => Self::Quest2Touch,
            ControllersEmulationModeDefaultVariant::QuestPro => Self::QuestPro,
            ControllersEmulationModeDefaultVariant::Pico4 => Self::Pico4,
            ControllersEmulationModeDefaultVariant::RiftSTouch => Self::RiftSTouch,
            _ => Self::ValveIndex,
        }
    }
}

impl HardConfigTab {
    pub fn new() -> Self {
        Self {
            preset: StreamingPreset::Medium,
            preferred_fps: 90.0,
            stream_width: 3400,
            stream_height: 3400,
            use_absolute_resolution: true,
            codec: CodecChoice::H264,
            bitrate_mbps: 70,
            controllers_enabled: true,
            controller_profile: ControllerProfileChoice::ValveIndex,
            hand_skeleton: false,
            hand_skeleton_steamvr_2: false,
            stream_tcp: true,
            status_message: None,
            applying_preset: false,
        }
    }

    fn mark_custom_if_edited(&mut self) {
        if !self.applying_preset && self.preset != StreamingPreset::Custom {
            self.preset = StreamingPreset::Custom;
        }
    }

    fn apply_preset(&mut self, preset: StreamingPreset) {
        self.applying_preset = true;
        self.preset = preset;
        self.use_absolute_resolution = true;
        match preset {
            StreamingPreset::Low => {
                self.preferred_fps = 90.0;
                self.stream_width = 2160;
                self.stream_height = 2160;
                self.codec = CodecChoice::H264;
                self.bitrate_mbps = 50;
            }
            StreamingPreset::Medium => {
                self.preferred_fps = 90.0;
                self.stream_width = 3400;
                self.stream_height = 3400;
                self.codec = CodecChoice::H264;
                self.bitrate_mbps = 70;
            }
            StreamingPreset::High => {
                self.preferred_fps = 90.0;
                self.stream_width = HEVC_HIGH_EYE_PX;
                self.stream_height = HEVC_HIGH_EYE_PX;
                self.codec = CodecChoice::Hevc;
                self.bitrate_mbps = 90;
            }
            StreamingPreset::Custom => {}
        }
        self.applying_preset = false;
    }

    fn detect_preset(&self) -> StreamingPreset {
        if !self.use_absolute_resolution {
            return StreamingPreset::Custom;
        }
        let sq = |w: u32, h: u32, fps: f32, codec: CodecChoice, br: u64| {
            self.stream_width == w
                && self.stream_height == h
                && (self.preferred_fps - fps).abs() < 0.5
                && self.codec == codec
                && self.bitrate_mbps == br
        };
        if sq(2160, 2160, 90.0, CodecChoice::H264, 50) {
            StreamingPreset::Low
        } else if sq(3400, 3400, 90.0, CodecChoice::H264, 70) {
            StreamingPreset::Medium
        } else if sq(HEVC_HIGH_EYE_PX, HEVC_HIGH_EYE_PX, 90.0, CodecChoice::Hevc, 90) {
            StreamingPreset::High
        } else {
            StreamingPreset::Custom
        }
    }

    pub fn sync_from_session(&mut self, session: &SessionConfig) {
        let ss = &session.session_settings;
        self.preferred_fps = ss.video.preferred_fps;
        self.use_absolute_resolution = matches!(
            ss.video.transcoding_view_resolution.variant,
            FrameSizeDefaultVariant::Absolute
        );
        self.stream_width = ss.video.transcoding_view_resolution.Absolute.width;
        self.stream_height = ss
            .video
            .transcoding_view_resolution
            .Absolute
            .height
            .content
            .max(1);
        if !ss.video.transcoding_view_resolution.Absolute.height.set {
            self.stream_height = self.stream_width;
        }

        self.codec = CodecChoice::from_variant(&ss.video.preferred_codec.variant);
        self.bitrate_mbps = ss.video.bitrate.mode.ConstantMbps.max(1);
        if !matches!(
            ss.video.bitrate.mode.variant,
            BitrateModeDefaultVariant::ConstantMbps
        ) {
            // Adaptive → show Custom; keep ConstantMbps value as last known target
        }

        self.controllers_enabled = ss.headset.controllers.enabled;
        self.controller_profile = ControllerProfileChoice::from_variant(
            &ss.headset.controllers.content.emulation_mode.variant,
        );
        self.hand_skeleton = ss.headset.controllers.content.hand_skeleton.enabled;
        self.hand_skeleton_steamvr_2 = ss
            .headset
            .controllers
            .content
            .hand_skeleton
            .content
            .steamvr_input_2_0;
        self.stream_tcp = matches!(
            ss.connection.stream_protocol.variant,
            SocketProtocolDefaultVariant::Tcp
        );

        self.preset = if matches!(
            ss.video.bitrate.mode.variant,
            BitrateModeDefaultVariant::ConstantMbps
        ) {
            self.detect_preset()
        } else {
            StreamingPreset::Custom
        };
    }

    fn write_into_session(&self, session: &mut SessionConfig) {
        let ss = &mut session.session_settings;

        ss.video.preferred_fps = self.preferred_fps;
        if self.use_absolute_resolution {
            ss.video.transcoding_view_resolution.variant = FrameSizeDefaultVariant::Absolute;
            ss.video.transcoding_view_resolution.Absolute.width = self.stream_width;
            ss.video.transcoding_view_resolution.Absolute.height.set = true;
            ss.video.transcoding_view_resolution.Absolute.height.content = self.stream_height;
            ss.video.emulated_headset_view_resolution =
                ss.video.transcoding_view_resolution.clone();
        }

        ss.video.preferred_codec.variant = self.codec.to_variant();
        ss.video.bitrate.mode.variant = BitrateModeDefaultVariant::ConstantMbps;
        ss.video.bitrate.mode.ConstantMbps = self.bitrate_mbps;

        ss.headset.controllers.enabled = self.controllers_enabled;
        ss.headset.controllers.content.emulation_mode.variant =
            self.controller_profile.to_variant();
        ss.headset.controllers.content.hand_skeleton.enabled = self.hand_skeleton;
        ss.headset
            .controllers
            .content
            .hand_skeleton
            .content
            .steamvr_input_2_0 = self.hand_skeleton && self.hand_skeleton_steamvr_2;

        ss.connection.stream_protocol.variant = if self.stream_tcp {
            SocketProtocolDefaultVariant::Tcp
        } else {
            SocketProtocolDefaultVariant::Udp
        };

        session.hard_config_solidified = false;
    }

    /// Read-only summary for Advanced Settings banner.
    pub fn streaming_summary_line(session: &SessionConfig) -> String {
        let ss = &session.session_settings;
        let w = ss.video.transcoding_view_resolution.Absolute.width;
        let h = if ss.video.transcoding_view_resolution.Absolute.height.set {
            ss.video.transcoding_view_resolution.Absolute.height.content
        } else {
            w
        };
        let codec = match ss.video.preferred_codec.variant {
            CodecTypeDefaultVariant::H264 => "H.264",
            CodecTypeDefaultVariant::Hevc => "HEVC",
            CodecTypeDefaultVariant::AV1 => "AV1",
        };
        let br = ss.video.bitrate.mode.ConstantMbps;
        let mode = match ss.video.bitrate.mode.variant {
            BitrateModeDefaultVariant::ConstantMbps => format!("{br} Mbps constant"),
            BitrateModeDefaultVariant::Adaptive => "adaptive bitrate".into(),
        };
        format!(
            "SteamVR Streaming: {:.0} Hz · {}×{} per eye · {} · {}",
            ss.video.preferred_fps, w, h, codec, mode
        )
    }

    pub fn ui(
        &mut self,
        ui: &mut Ui,
        session: Option<&SessionConfig>,
        steamvr_connected: bool,
    ) -> Vec<HardConfigAction> {
        let mut actions = vec![];

        ScrollArea::vertical().show(ui, |ui| {
            ui.label(
                RichText::new(
                    "Configure streaming on the PC, then solidify before SteamVR starts \
                     (Virtual Desktop–style). Matched to App Store client 20.14.x.",
                )
                .color(theme::FG),
            );
            ui.add_space(8.0);

            if let Some(session) = session {
                let init = &session.openvr_config;
                let status = if session.hard_config_solidified {
                    format!(
                        "Solidified: {}×{} per eye @ {} Hz",
                        init.eye_resolution_width,
                        init.eye_resolution_height,
                        init.refresh_rate
                    )
                } else {
                    "Not solidified — do not launch SteamVR until you solidify.".into()
                };
                ui.colored_label(
                    if session.hard_config_solidified {
                        theme::OK_GREEN
                    } else {
                        theme::log_colors::WARNING_LIGHT
                    },
                    status,
                );
            }

            if steamvr_connected {
                ui.colored_label(
                    theme::log_colors::WARNING_LIGHT,
                    "SteamVR is running. Solidify & Launch will shut it down, bake config, and relaunch.",
                );
            }

            // ----- Presets -----
            ui.add_space(12.0);
            ui.heading("Quality preset");
            ui.label(
                RichText::new("Applies resolution, refresh rate, codec, and bitrate immediately.")
                    .small(),
            );
            ui.horizontal(|ui| {
                for p in [
                    StreamingPreset::Low,
                    StreamingPreset::Medium,
                    StreamingPreset::High,
                    StreamingPreset::Custom,
                ] {
                    let selected = self.preset == p;
                    if ui
                        .selectable_label(selected, RichText::new(p.label()).strong())
                        .on_hover_text(p.description())
                        .clicked()
                    {
                        if p != StreamingPreset::Custom {
                            self.apply_preset(p);
                        } else {
                            self.preset = StreamingPreset::Custom;
                        }
                    }
                }
            });
            ui.label(RichText::new(self.preset.description()).small().color(theme::FG));

            // ----- Display / stream -----
            ui.add_space(16.0);
            ui.heading("Display & stream");
            ui.horizontal(|ui| {
                ui.label("Refresh rate (Hz)");
                let before = self.preferred_fps;
                ui.add(
                    egui::DragValue::new(&mut self.preferred_fps)
                        .range(60.0..=120.0)
                        .speed(1.0),
                );
                if (self.preferred_fps - before).abs() > f32::EPSILON {
                    self.mark_custom_if_edited();
                }
            });

            let abs_before = self.use_absolute_resolution;
            ui.checkbox(
                &mut self.use_absolute_resolution,
                "Absolute per-eye resolution (recommended)",
            );
            if self.use_absolute_resolution != abs_before {
                self.mark_custom_if_edited();
            }

            if self.use_absolute_resolution {
                ui.horizontal(|ui| {
                    ui.label("Per-eye width");
                    let w0 = self.stream_width;
                    ui.add(
                        egui::DragValue::new(&mut self.stream_width)
                            .range(32..=8192)
                            .speed(32),
                    );
                    ui.label("height");
                    let h0 = self.stream_height;
                    ui.add(
                        egui::DragValue::new(&mut self.stream_height)
                            .range(32..=8192)
                            .speed(32),
                    );
                    if self.stream_width != w0 || self.stream_height != h0 {
                        self.mark_custom_if_edited();
                    }
                });
            }

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label("Codec");
                let before = self.codec;
                egui::ComboBox::from_id_salt("streaming_codec")
                    .selected_text(self.codec.label())
                    .show_ui(ui, |ui| {
                        for c in [CodecChoice::H264, CodecChoice::Hevc, CodecChoice::Av1] {
                            ui.selectable_value(&mut self.codec, c, c.label());
                        }
                    });
                if self.codec != before {
                    self.mark_custom_if_edited();
                }
            });
            ui.label(RichText::new(self.codec.avp_note()).small());

            if self.codec == CodecChoice::H264
                && (self.stream_width > H264_MAX_EYE_PX || self.stream_height > H264_MAX_EYE_PX)
            {
                ui.colored_label(
                    theme::log_colors::WARNING_LIGHT,
                    format!(
                        "H.264 above ~{H264_MAX_EYE_PX}×{H264_MAX_EYE_PX} often fails to start on this Linux/NVENC path. \
                         Use HEVC (High preset) or lower resolution."
                    ),
                );
            }
            if self.codec == CodecChoice::Av1 {
                ui.colored_label(
                    theme::KO_RED,
                    "AV1 is not usable with the App Store Vision Pro client in practice.",
                );
            }

            ui.horizontal(|ui| {
                ui.label("Bitrate (Mbps, constant)");
                let b0 = self.bitrate_mbps;
                ui.add(
                    egui::DragValue::new(&mut self.bitrate_mbps)
                        .range(5..=500)
                        .speed(1),
                );
                if self.bitrate_mbps != b0 {
                    self.mark_custom_if_edited();
                }
            });
            ui.label(
                RichText::new("Uses constant bitrate mode (not adaptive).")
                    .small()
                    .color(theme::FG),
            );

            // ----- Controllers -----
            ui.add_space(16.0);
            ui.heading("Controllers (OpenVR)");
            ui.checkbox(&mut self.controllers_enabled, "Controllers enabled");
            ui.horizontal(|ui| {
                ui.label("Emulation");
                egui::ComboBox::from_id_salt("hard_controller_profile")
                    .selected_text(self.controller_profile.label())
                    .show_ui(ui, |ui| {
                        for choice in [
                            ControllerProfileChoice::ValveIndex,
                            ControllerProfileChoice::Quest3Plus,
                            ControllerProfileChoice::Quest2Touch,
                            ControllerProfileChoice::QuestPro,
                            ControllerProfileChoice::Pico4,
                            ControllerProfileChoice::RiftSTouch,
                        ] {
                            ui.selectable_value(
                                &mut self.controller_profile,
                                choice,
                                choice.label(),
                            );
                        }
                    });
            });
            ui.label(
                RichText::new(
                    "Streamer 20.14.1 has no PSVR2 Sense profile. Use Valve Index \
                     (or the profile games expect). PSVR2 still pairs on AVP; buttons remap.",
                )
                .small(),
            );

            ui.add_space(8.0);
            ui.heading("Hand tracking");
            ui.checkbox(
                &mut self.hand_skeleton,
                "Hand skeleton / finger tracking to SteamVR",
            );
            ui.add_enabled_ui(self.hand_skeleton, |ui| {
                ui.checkbox(
                    &mut self.hand_skeleton_steamvr_2,
                    "SteamVR input 2.0 separate hand trackers",
                );
            });

            // ----- Connection -----
            ui.add_space(12.0);
            ui.heading("Connection");
            ui.checkbox(
                &mut self.stream_tcp,
                "Stream over TCP (recommended for Apple Vision Pro)",
            );

            // ----- Actions -----
            ui.add_space(16.0);
            ui.horizontal(|ui| {
                if ui
                    .button(RichText::new("Apply AVP defaults").strong())
                    .clicked()
                {
                    if let Some(session) = session {
                        let mut s = session.clone();
                        s.apply_avp_profile();
                        // Medium streaming defaults for AVP
                        self.apply_preset(StreamingPreset::Medium);
                        self.sync_from_session(&s);
                        // re-apply medium after sync may overwrite from session — write streaming into session
                        self.apply_preset(StreamingPreset::Medium);
                        self.write_into_session(&mut s);
                        self.sync_from_session(&s);
                        self.status_message =
                            Some("Applied AVP + Medium streaming defaults (not solidified).".into());
                        actions.push(HardConfigAction::ApplySession(Box::new(s)));
                    } else {
                        self.apply_preset(StreamingPreset::Medium);
                        self.controller_profile = ControllerProfileChoice::ValveIndex;
                        self.hand_skeleton = false;
                        self.stream_tcp = true;
                        self.status_message = Some("Local defaults set.".into());
                    }
                }

                if ui
                    .button(RichText::new("Solidify & Launch SteamVR").strong())
                    .clicked()
                {
                    if let Some(session) = session {
                        let mut s = session.clone();
                        self.write_into_session(&mut s);
                        s.solidify_hard_config();
                        self.status_message = Some(format!(
                            "Solidified {}×{} @ {} Hz — launching SteamVR.",
                            s.openvr_config.eye_resolution_width,
                            s.openvr_config.eye_resolution_height,
                            s.openvr_config.refresh_rate
                        ));
                        actions.push(HardConfigAction::SolidifyAndLaunch(Box::new(s)));
                    } else {
                        self.status_message =
                            Some("Session not loaded yet — try again in a moment.".into());
                    }
                }

                if ui.button("Save streaming config only").clicked() {
                    if let Some(session) = session {
                        let mut s = session.clone();
                        self.write_into_session(&mut s);
                        s.solidify_hard_config();
                        self.status_message =
                            Some("Streaming config solidified (SteamVR not launched).".into());
                        actions.push(HardConfigAction::ApplySession(Box::new(s)));
                    }
                }
            });

            if let Some(msg) = &self.status_message {
                ui.add_space(8.0);
                ui.label(msg);
            }
        });

        actions
    }
}
