mod settings;
mod solidify;

pub use settings::*;
pub use solidify::*;
pub use settings_schema;

use alvr_common::{
    anyhow::{bail, Result},
    semver::Version,
    ConnectionState, ToAny, ALVR_VERSION,
};
use serde::{Deserialize, Serialize};
use serde_json as json;
use settings_schema::{NumberType, SchemaNode};
use std::{
    collections::{HashMap, HashSet},
    net::IpAddr,
};

// SessionSettings is similar to Settings but it contains every branch, even unused ones. This is
// the settings representation that the UI uses.
pub type SessionSettings = settings::SettingsDefault;

/// Headset chosen in Hard Config before SteamVR starts. SteamVR snapshots
/// `GetProjectionRaw` at driver activate, so this must match the client that
/// will connect.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "snake_case")]
pub enum HeadsetHardProfile {
    #[default]
    Avp,
    Quest3,
}

impl HeadsetHardProfile {
    pub fn label(self) -> &'static str {
        match self {
            Self::Avp => "Apple Vision Pro",
            Self::Quest3 => "Quest 3",
        }
    }
}

/// Stereo frustum published to SteamVR at HMD activate (radians, OpenXR convention).
/// Default is the App Store AVP ViewsConfig this branch was calibrated against.
#[derive(Serialize, Deserialize, PartialEq, Clone, Copy, Debug)]
#[serde(default)]
pub struct BakedStereoGeometry {
    pub ipd_m: f32,
    pub fov_l_left: f32,
    pub fov_l_right: f32,
    pub fov_l_up: f32,
    pub fov_l_down: f32,
    pub fov_r_left: f32,
    pub fov_r_right: f32,
    pub fov_r_up: f32,
    pub fov_r_down: f32,
}

impl BakedStereoGeometry {
    /// App Store AVP client 20.14.1 (valid ViewsConfig on this host).
    pub const AVP: Self = Self {
        ipd_m: 0.063,
        fov_l_left: -1.054,
        fov_l_right: 0.791,
        fov_l_up: 0.878,
        fov_l_down: -0.791,
        fov_r_left: -0.793,
        fov_r_right: 1.057,
        fov_r_up: 0.881,
        fov_r_down: -0.793,
    };

    /// Quest 3 ALVR client ViewsConfig captured on this host (matches HMD geometry DB).
    pub const QUEST3: Self = Self {
        ipd_m: 0.0632,
        fov_l_left: -0.942,
        fov_l_right: 0.698,
        fov_l_up: 0.768,
        fov_l_down: -0.960,
        fov_r_left: -0.698,
        fov_r_right: 0.942,
        fov_r_up: 0.768,
        fov_r_down: -0.960,
    };

    pub fn for_profile(profile: HeadsetHardProfile) -> Self {
        match profile {
            HeadsetHardProfile::Avp => Self::AVP,
            HeadsetHardProfile::Quest3 => Self::QUEST3,
        }
    }

    pub fn from_client_fov(ipd_m: f32, fov: [alvr_common::Fov; 2]) -> Self {
        Self {
            ipd_m,
            fov_l_left: fov[0].left,
            fov_l_right: fov[0].right,
            fov_l_up: fov[0].up,
            fov_l_down: fov[0].down,
            fov_r_left: fov[1].left,
            fov_r_right: fov[1].right,
            fov_r_up: fov[1].up,
            fov_r_down: fov[1].down,
        }
    }

    pub fn is_valid(self) -> bool {
        self.ipd_m > 0.04
            && self.ipd_m < 0.10
            && self.fov_l_left < self.fov_l_right
            && self.fov_l_down < self.fov_l_up
            && self.fov_r_left < self.fov_r_right
            && self.fov_r_down < self.fov_r_up
            && self.fov_l_left.is_finite()
            && self.fov_l_right.is_finite()
            && self.fov_l_up.is_finite()
            && self.fov_l_down.is_finite()
            && self.fov_r_left.is_finite()
            && self.fov_r_right.is_finite()
            && self.fov_r_up.is_finite()
            && self.fov_r_down.is_finite()
            && self.fov_l_left.abs() < 2.5
            && self.fov_l_right.abs() < 2.5
            && self.fov_l_up.abs() < 2.5
            && self.fov_l_down.abs() < 2.5
            && self.fov_r_left.abs() < 2.5
            && self.fov_r_right.abs() < 2.5
            && self.fov_r_up.abs() < 2.5
            && self.fov_r_down.abs() < 2.5
    }
}

impl Default for BakedStereoGeometry {
    fn default() -> Self {
        Self::AVP
    }
}

/// Last valid ViewsConfig from a connected client, for refining baked stereo.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct LastClientStereo {
    pub display_name: String,
    pub stereo: BakedStereoGeometry,
}

// This structure is used to store the minimum configuration data that ALVR driver needs to
// initialize OpenVR before having the chance to communicate with a client. When a client is
// connected, a new OpenvrConfig instance is generated, then the connection is accepted only if that
// instance is equivalent to the one stored in the session, otherwise SteamVR is restarted.
// Other components (like the encoder, audio recorder) don't need this treatment and are initialized
// dynamically.
// todo: properties that can be set after the OpenVR initialization should be removed and set with
// UpdateForStream.
#[expect(clippy::pub_underscore_fields)]
#[derive(Serialize, Deserialize, PartialEq, Default, Clone, Debug)]
pub struct OpenvrConfig {
    pub eye_resolution_width: u32,
    pub eye_resolution_height: u32,
    pub target_eye_resolution_width: u32,
    pub target_eye_resolution_height: u32,
    pub tracking_ref_only: bool,
    pub enable_vive_tracker_proxy: bool,
    pub minimum_idr_interval_ms: u64,
    pub adapter_index: u32,
    pub codec: u8,
    pub h264_profile: u32,
    pub refresh_rate: u32,
    pub use_10bit_encoder: bool,
    pub use_full_range_encoding: bool,
    pub encoding_gamma: f32,
    pub enable_hdr: bool,
    pub force_hdr_srgb_correction: bool,
    pub clamp_hdr_extended_range: bool,
    pub enable_amf_pre_analysis: bool,
    pub enable_vbaq: bool,
    pub enable_amf_hmqb: bool,
    pub use_amf_preproc: bool,
    pub amf_preproc_sigma: u32,
    pub amf_preproc_tor: u32,
    pub encoder_quality_preset: u32,
    pub rate_control_mode: u32,
    pub filler_data: bool,
    pub entropy_coding: u32,
    pub force_sw_encoding: bool,
    pub sw_thread_count: u32,
    pub controller_is_tracker: bool,
    pub controllers_enabled: bool,
    pub body_tracking_vive_enabled: bool,
    pub body_tracking_has_legs: bool,
    pub enable_foveated_encoding: bool,
    pub foveation_center_size_x: f32,
    pub foveation_center_size_y: f32,
    pub foveation_center_shift_x: f32,
    pub foveation_center_shift_y: f32,
    pub foveation_edge_ratio_x: f32,
    pub foveation_edge_ratio_y: f32,
    pub enable_color_correction: bool,
    pub brightness: f32,
    pub contrast: f32,
    pub saturation: f32,
    pub gamma: f32,
    pub sharpening: f32,
    pub linux_async_compute: bool,
    pub linux_async_reprojection: bool,
    pub nvenc_quality_preset: u32,
    pub nvenc_tuning_preset: u32,
    pub nvenc_multi_pass: u32,
    pub nvenc_adaptive_quantization_mode: u32,
    pub nvenc_low_delay_key_frame_scale: i64,
    pub nvenc_refresh_rate: i64,
    pub enable_intra_refresh: bool,
    pub intra_refresh_period: i64,
    pub intra_refresh_count: i64,
    pub max_num_ref_frames: i64,
    pub gop_length: i64,
    pub p_frame_strategy: i64,
    pub nvenc_rate_control_mode: i64,
    pub rc_buffer_size: i64,
    pub rc_initial_delay: i64,
    pub rc_max_bitrate: i64,
    pub rc_average_bitrate: i64,
    pub nvenc_enable_weighted_prediction: bool,
    pub capture_frame_dir: String,
    pub amd_bitrate_corruption_fix: bool,
    pub use_separate_hand_trackers: bool,

    /// Stereo frustum for SteamVR at driver activate. Missing in old session.json
    /// deserializes to AVP (this branch's previous hardcoded default).
    #[serde(default)]
    pub default_stereo: BakedStereoGeometry,

    // these settings are not used on the C++ side, but we need them to correctly trigger a SteamVR
    // restart
    pub _controller_profile: i32,
    pub _server_impl_debug: bool,
    pub _client_impl_debug: bool,
    pub _server_core_debug: bool,
    pub _client_core_debug: bool,
    pub _connection_debug: bool,
    pub _sockets_debug: bool,
    pub _server_gfx_debug: bool,
    pub _client_gfx_debug: bool,
    pub _encoder_debug: bool,
    pub _decoder_debug: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ClientConnectionConfig {
    pub display_name: String,
    pub current_ip: Option<IpAddr>,
    pub manual_ips: HashSet<IpAddr>,
    pub trusted: bool,
    pub connection_state: ConnectionState,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SessionConfig {
    pub server_version: Version,
    pub openvr_config: OpenvrConfig,
    // The hashmap key is the hostname
    pub client_connections: HashMap<String, ClientConnectionConfig>,
    pub session_settings: SessionSettings,
    /// When true, hard OpenVR layout was baked before SteamVR start. Connect path must not
    /// request a SteamVR restart; use locked openvr_config geometry instead.
    #[serde(default)]
    pub hard_config_solidified: bool,
    /// Headset selected in Hard Config. Controls baked stereo + controller/protocol defaults.
    #[serde(default)]
    pub headset_hard_profile: HeadsetHardProfile,
    /// FOV/IPD that will be written into `openvr_config` on solidify.
    #[serde(default)]
    pub baked_stereo: BakedStereoGeometry,
    /// Last valid client ViewsConfig (updated while streaming). Used to refine baked stereo.
    #[serde(default)]
    pub last_client_stereo: Option<LastClientStereo>,
}

impl Default for SessionConfig {
    fn default() -> Self {
        let mut session = Self {
            server_version: ALVR_VERSION.clone(),
            openvr_config: OpenvrConfig {
                // avoid realistic resolutions, as on first start, on Linux, it
                // could trigger direct mode on an existing monitor
                eye_resolution_width: 800,
                eye_resolution_height: 900,
                target_eye_resolution_width: 800,
                target_eye_resolution_height: 900,
                adapter_index: 0,
                refresh_rate: 60,
                controllers_enabled: false,
                body_tracking_vive_enabled: false,
                enable_foveated_encoding: false,
                enable_color_correction: false,
                linux_async_reprojection: false,
                capture_frame_dir: "/tmp".into(),
                ..<_>::default()
            },
            client_connections: HashMap::new(),
            session_settings: settings::session_settings_default(),
            hard_config_solidified: false,
            headset_hard_profile: HeadsetHardProfile::Avp,
            baked_stereo: BakedStereoGeometry::AVP,
            last_client_stereo: None,
        };
        solidify::solidify_hard_config(&mut session);
        session
    }
}

impl SessionConfig {
    // If json_value is not a valid representation of SessionConfig (because of version upgrade),
    // use some fuzzy logic to extrapolate as much information as possible.
    // Since SessionConfig cannot have a schema (because SessionSettings would need to also have a
    // schema, but it is generated out of our control), we only do basic name checking on fields and
    // deserialization will fail if the type of values does not match. Because of this,
    // `session_settings` must be handled separately to do a better job of retrieving data using the
    // settings schema.
    pub fn merge_from_json(&mut self, json_value: &json::Value) -> Result<()> {
        const SESSION_SETTINGS_STR: &str = "session_settings";

        if let Ok(session_desc) = json::from_value(json_value.clone()) {
            *self = session_desc;
            return Ok(());
        }

        // Note: unwrap is safe because current session is expected to serialize correctly
        let old_session_json = json::to_value(self.clone()).unwrap();
        let old_session_fields = old_session_json.as_object().unwrap();

        let maybe_session_settings_json =
            json_value
                .get(SESSION_SETTINGS_STR)
                .map(|new_session_settings_json| {
                    extrapolate_session_settings_from_session_settings(
                        &old_session_fields[SESSION_SETTINGS_STR],
                        new_session_settings_json,
                        &Settings::schema(settings::session_settings_default()),
                    )
                });

        let new_fields = old_session_fields
            .iter()
            .map(|(name, json_field_value)| {
                let new_json_field_value = if name == SESSION_SETTINGS_STR {
                    json::to_value(settings::session_settings_default()).unwrap()
                } else {
                    json_value.get(name).unwrap_or(json_field_value).clone()
                };
                (name.clone(), new_json_field_value)
            })
            .collect();
        // Failure to extrapolate other session_desc fields is not notified.
        let mut session_desc_mut =
            json::from_value::<SessionConfig>(json::Value::Object(new_fields)).unwrap_or_default();

        match maybe_session_settings_json
            .to_any()
            .and_then(|s| serde_json::from_value::<SessionSettings>(s).map_err(|e| e.into()))
        {
            Ok(session_settings) => {
                session_desc_mut.session_settings = session_settings;
                *self = session_desc_mut;
                Ok(())
            }
            Err(e) => {
                *self = session_desc_mut;

                bail!("Error while deserializing extrapolated session settings: {e}")
            }
        }
    }

    pub fn to_settings(&self) -> Settings {
        let session_settings_json = json::to_value(&self.session_settings).unwrap();
        let schema = Settings::schema(settings::session_settings_default());

        json::from_value::<Settings>(json_session_settings_to_settings(
            &session_settings_json,
            &schema,
        ))
        .map_err(|e| dbg!(e))
        .unwrap()
    }
}

// Current data extrapolation strategy: match both field name and value type exactly.
// Integer bounds are not validated, if they do not match the schema, deserialization will fail and
// all data is lost.
// Future strategies: check if value respects schema constraints, fuzzy field name matching, accept
// integer to float and float to integer, tree traversal.
fn extrapolate_session_settings_from_session_settings(
    old_session_settings: &json::Value,
    new_session_settings: &json::Value,
    schema: &SchemaNode,
) -> json::Value {
    match schema {
        SchemaNode::Section {
            entries,
            gui_collapsible,
        } => json::Value::Object({
            let mut entries: json::Map<String, json::Value> = entries
                .iter()
                .map(|named_entry| {
                    let value_json = extrapolate_session_settings_from_session_settings(
                        &old_session_settings[&named_entry.name],
                        &new_session_settings[&named_entry.name],
                        &named_entry.content,
                    );
                    (named_entry.name.clone(), value_json)
                })
                .collect();

            if *gui_collapsible {
                let collapsed_json = if new_session_settings["gui_collapsed"].is_boolean() {
                    new_session_settings["gui_collapsed"].clone()
                } else {
                    old_session_settings["gui_collapsed"].clone()
                };
                entries.insert("gui_collapsed".into(), collapsed_json);
            }

            entries
        }),

        SchemaNode::Choice { variants, .. } => {
            let variant_json = json::from_value(new_session_settings["variant"].clone())
                .ok()
                .filter(|variant_str| {
                    variants
                        .iter()
                        .any(|named_entry| *variant_str == named_entry.name)
                })
                .map_or_else(
                    || old_session_settings["variant"].clone(),
                    json::Value::String,
                );

            let mut fields: json::Map<_, _> = variants
                .iter()
                .filter_map(|named_entry| {
                    named_entry.content.as_ref().map(|data_schema| {
                        let value_json = extrapolate_session_settings_from_session_settings(
                            &old_session_settings[&named_entry.name],
                            &new_session_settings[&named_entry.name],
                            data_schema,
                        );
                        (named_entry.name.clone(), value_json)
                    })
                })
                .collect();
            fields.insert("variant".into(), variant_json);

            json::Value::Object(fields)
        }

        SchemaNode::Optional { content, .. } => {
            let set_value = new_session_settings["set"]
                .as_bool()
                .unwrap_or_else(|| old_session_settings["set"].as_bool().unwrap());

            let content_json = extrapolate_session_settings_from_session_settings(
                &old_session_settings["content"],
                &new_session_settings["content"],
                content,
            );

            json::json!({
                "set": set_value,
                "content": content_json
            })
        }

        SchemaNode::Switch { content, .. } => {
            let enabled = new_session_settings["enabled"]
                .as_bool()
                .unwrap_or_else(|| old_session_settings["enabled"].as_bool().unwrap());

            let content_json = extrapolate_session_settings_from_session_settings(
                &old_session_settings["content"],
                &new_session_settings["content"],
                content,
            );

            json::json!({
                "enabled": enabled,
                "content": content_json
            })
        }

        SchemaNode::Boolean { .. } => {
            if new_session_settings.is_boolean() {
                new_session_settings.clone()
            } else {
                old_session_settings.clone()
            }
        }

        SchemaNode::Number { ty, .. } => {
            if let Some(value) = new_session_settings.as_f64() {
                match ty {
                    NumberType::UnsignedInteger => json::Value::from(value.abs() as u64),
                    NumberType::SignedInteger => json::Value::from(value as i64),
                    NumberType::Float => new_session_settings.clone(),
                }
            } else {
                old_session_settings.clone()
            }
        }

        SchemaNode::Text { .. } => {
            if new_session_settings.is_string() {
                new_session_settings.clone()
            } else {
                old_session_settings.clone()
            }
        }

        SchemaNode::Array(array_schema) => {
            let gui_collapsed = new_session_settings["gui_collapsed"]
                .as_bool()
                .unwrap_or_else(|| old_session_settings["gui_collapsed"].as_bool().unwrap());

            let array_vec = array_schema
                .iter()
                .enumerate()
                .map(|(idx, schema)| {
                    extrapolate_session_settings_from_session_settings(
                        &old_session_settings["content"][idx],
                        &new_session_settings["content"][idx],
                        schema,
                    )
                })
                .collect::<Vec<_>>();

            json::json!({
                "gui_collapsed": gui_collapsed,
                "content": array_vec
            })
        }

        SchemaNode::Vector {
            default_element, ..
        } => {
            let gui_collapsed = new_session_settings["gui_collapsed"]
                .as_bool()
                .unwrap_or_else(|| old_session_settings["gui_collapsed"].as_bool().unwrap());

            let element_json = extrapolate_session_settings_from_session_settings(
                &old_session_settings["element"],
                &new_session_settings["element"],
                default_element,
            );

            let content_json =
                json::from_value::<Vec<json::Value>>(new_session_settings["content"].clone())
                    .ok()
                    .map(|vec| {
                        vec.iter()
                            .enumerate()
                            .map(|(idx, new_element)| {
                                extrapolate_session_settings_from_session_settings(
                                    &old_session_settings["content"]
                                        .get(idx)
                                        .cloned()
                                        .unwrap_or_else(|| element_json.clone()),
                                    new_element,
                                    default_element,
                                )
                            })
                            .collect()
                    })
                    .map_or_else(
                        || old_session_settings["content"].clone(),
                        json::Value::Array,
                    );

            json::json!({
                "gui_collapsed": gui_collapsed,
                "element": element_json,
                "content": content_json
            })
        }

        SchemaNode::Dictionary { default_value, .. } => {
            let gui_collapsed = new_session_settings["gui_collapsed"]
                .as_bool()
                .unwrap_or_else(|| old_session_settings["gui_collapsed"].as_bool().unwrap());

            let key_str = new_session_settings["key"]
                .as_str()
                .unwrap_or_else(|| old_session_settings["key"].as_str().unwrap());

            let value_json = extrapolate_session_settings_from_session_settings(
                &old_session_settings["value"],
                &new_session_settings["value"],
                default_value,
            );

            let content_json = json::from_value::<HashMap<String, json::Value>>(
                new_session_settings["content"].clone(),
            )
            .ok()
            .map(|map| {
                map.iter()
                    .map(|(key, new_value)| {
                        let value = extrapolate_session_settings_from_session_settings(
                            &old_session_settings["content"]
                                .get(key)
                                .cloned()
                                .unwrap_or_else(|| value_json.clone()),
                            new_value,
                            default_value,
                        );
                        (key, value)
                    })
                    .map(|(key, value)| {
                        json::Value::Array(vec![json::Value::String(key.clone()), value])
                    })
                    .collect()
            })
            .map_or_else(
                || old_session_settings["content"].clone(),
                json::Value::Array,
            );

            json::json!({
                "gui_collapsed": gui_collapsed,
                "key": key_str,
                "value": value_json,
                "content": content_json
            })
        }
        _ => unreachable!(),
    }
}

// session_settings does not get validated here, it must be already valid
fn json_session_settings_to_settings(
    session_settings: &json::Value,
    schema: &SchemaNode,
) -> json::Value {
    match schema {
        SchemaNode::Section { entries, .. } => json::Value::Object(
            entries
                .iter()
                .map(|named_entry| {
                    (
                        named_entry.name.clone(),
                        json_session_settings_to_settings(
                            &session_settings[&named_entry.name],
                            &named_entry.content,
                        ),
                    )
                })
                .collect(),
        ),

        SchemaNode::Choice { variants, .. } => {
            let variant = session_settings["variant"].as_str().unwrap();
            let maybe_content = variants
                .iter()
                .find(|named_entry| named_entry.name == variant)
                .and_then(|named_entry| named_entry.content.as_ref())
                .map(|data_schema| {
                    json_session_settings_to_settings(&session_settings[variant], data_schema)
                });
            if let Some(content) = maybe_content {
                json::json!({ variant: content })
            } else {
                json::Value::String(variant.to_owned())
            }
        }

        SchemaNode::Optional { content, .. } => {
            if session_settings["set"].as_bool().unwrap() {
                json_session_settings_to_settings(&session_settings["content"], content)
            } else {
                json::Value::Null
            }
        }

        SchemaNode::Switch { content, .. } => {
            if session_settings["enabled"].as_bool().unwrap() {
                let content =
                    json_session_settings_to_settings(&session_settings["content"], content);

                json::json!({ "Enabled": content })
            } else {
                json::Value::String("Disabled".into())
            }
        }

        SchemaNode::Boolean { .. } | SchemaNode::Number { .. } | SchemaNode::Text { .. } => {
            session_settings.clone()
        }

        SchemaNode::Array(array_schema) => json::Value::Array(
            array_schema
                .iter()
                .enumerate()
                .map(|(idx, element_schema)| {
                    json_session_settings_to_settings(
                        &session_settings["content"][idx],
                        element_schema,
                    )
                })
                .collect(),
        ),

        SchemaNode::Vector {
            default_element, ..
        } => json::to_value(
            session_settings["content"]
                .as_array()
                .unwrap()
                .iter()
                .map(|element_json| {
                    json_session_settings_to_settings(element_json, default_element)
                })
                .collect::<Vec<_>>(),
        )
        .unwrap(),

        SchemaNode::Dictionary { default_value, .. } => json::to_value(
            json::from_value::<Vec<(String, json::Value)>>(session_settings["content"].clone())
                .unwrap()
                .into_iter()
                .map(|(key, value_json)| {
                    (
                        key,
                        json_session_settings_to_settings(&value_json, default_value),
                    )
                })
                .collect::<Vec<_>>(),
        )
        .unwrap(),
        _ => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_manual_session_to_settings() {
        let default = session_settings_default();
        let settings_schema = json::to_value(&default).unwrap();
        let schema = Settings::schema(default);

        let _settings = json::from_value::<Settings>(json_session_settings_to_settings(
            &settings_schema,
            &schema,
        ))
        .err();
    }

    #[test]
    fn test_session_to_settings() {
        let _settings = SessionConfig::default().to_settings();
    }

    #[test]
    fn test_session_extrapolation_trivial() {
        SessionConfig::default()
            .merge_from_json(&json::to_value(SessionConfig::default()).unwrap())
            .unwrap();
    }

    #[test]
    fn test_session_extrapolation_diff() {
        let input_json_string = r#"{
            "session_settings": {
              "fjdshfks": false,
              "video": {
                "preferred_fps": 60.0
              },
              "headset": {
                "gui_collapsed": false,
                "controllers": {
                  "enabled": false
                }
              }
            }
          }"#;

        let mut session = SessionConfig::default();
        session
            .merge_from_json(&json::from_str(input_json_string).unwrap())
            .unwrap();

        let settings = session.to_settings();

        assert_eq!(settings.video.preferred_fps, 60.0);
        assert!(settings.headset.controllers.as_option().is_none());
    }
}
