//! Pre-SteamVR hard configuration UI (ALVR 20.14.x / App Store AVP client).
//!
//! Settings here define OpenVR device identity and must be solidified before SteamVR starts.

use alvr_gui_common::theme;
use alvr_packets::ServerRequest;
use alvr_session::{
    ControllersEmulationModeDefaultVariant, FrameSizeDefaultVariant, SessionConfig,
    SocketProtocolDefaultVariant,
};
use eframe::egui::{self, RichText, ScrollArea, Ui};

pub enum HardConfigAction {
    ServerRequest(ServerRequest),
    SolidifyAndLaunch(Box<SessionConfig>),
    ApplySession(Box<SessionConfig>),
}

pub struct HardConfigTab {
    preferred_fps: f32,
    stream_width: u32,
    stream_height: u32,
    use_absolute_resolution: bool,
    controllers_enabled: bool,
    controller_profile: ControllerProfileChoice,
    hand_skeleton: bool,
    hand_skeleton_steamvr_2: bool,
    stream_tcp: bool,
    status_message: Option<String>,
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
            preferred_fps: 90.0,
            stream_width: 2144,
            stream_height: 2144,
            use_absolute_resolution: true,
            controllers_enabled: true,
            controller_profile: ControllerProfileChoice::ValveIndex,
            hand_skeleton: false,
            hand_skeleton_steamvr_2: false,
            stream_tcp: true,
            status_message: None,
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
                    "Hard Config is locked into SteamVR at launch (like Virtual Desktop). \
                     Matched to App Store client 20.14.1. Change these, then Solidify & Launch.",
                )
                .color(theme::FG),
            );
            ui.add_space(8.0);

            if let Some(session) = session {
                let init = &session.openvr_config;
                let status = if session.hard_config_solidified {
                    format!(
                        "Solidified: {}x{} per eye @ {}Hz",
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

            ui.add_space(12.0);
            ui.heading("Display (OpenVR HMD)");
            ui.horizontal(|ui| {
                ui.label("Preferred FPS");
                ui.add(
                    egui::DragValue::new(&mut self.preferred_fps)
                        .range(60.0..=120.0)
                        .speed(1.0),
                );
            });
            ui.checkbox(
                &mut self.use_absolute_resolution,
                "Use absolute per-eye resolution (recommended)",
            );
            if self.use_absolute_resolution {
                ui.horizontal(|ui| {
                    ui.label("Per-eye width");
                    ui.add(
                        egui::DragValue::new(&mut self.stream_width)
                            .range(32..=8192)
                            .speed(32),
                    );
                    ui.label("height");
                    ui.add(
                        egui::DragValue::new(&mut self.stream_height)
                            .range(32..=8192)
                            .speed(32),
                    );
                });
            }

            ui.add_space(12.0);
            ui.heading("Controllers (OpenVR devices)");
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
                    "Note: streamer 20.14.1 has no PSVR2 Sense profile. Use Valve Index \
                     (or the profile games expect). PSVR2 hardware still pairs on AVP; \
                     ALVR remaps buttons to the chosen SteamVR profile.",
                )
                .small(),
            );

            ui.add_space(8.0);
            ui.heading("Hand tracking (hard lock)");
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

            ui.add_space(12.0);
            ui.heading("Connection");
            ui.checkbox(
                &mut self.stream_tcp,
                "Stream over TCP (recommended for Apple Vision Pro)",
            );

            ui.add_space(16.0);
            ui.horizontal(|ui| {
                if ui
                    .button(RichText::new("Apply AVP defaults").strong())
                    .clicked()
                {
                    if let Some(session) = session {
                        let mut s = session.clone();
                        s.apply_avp_profile();
                        self.sync_from_session(&s);
                        self.status_message =
                            Some("Applied AVP defaults (not solidified yet).".into());
                        actions.push(HardConfigAction::ApplySession(Box::new(s)));
                    } else {
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
                            "Solidified {}x{} @ {}Hz — launching SteamVR.",
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

                if ui.button("Save hard config only").clicked() {
                    if let Some(session) = session {
                        let mut s = session.clone();
                        self.write_into_session(&mut s);
                        s.solidify_hard_config();
                        self.status_message =
                            Some("Hard config solidified (SteamVR not launched).".into());
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
