use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::{
    hardware::Position,
    target::{AgentPhase, TargetId},
};

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum T3StateSource {
    /// Prefer T3's authenticated API when paired, otherwise use local SQLite.
    #[default]
    Auto,
    Api,
    Sqlite,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    pub default_target: TargetId,
    pub target_order: Vec<TargetId>,
    pub targets: TargetsConfig,

    pub t3_state_source: T3StateSource,
    pub t3_runtime: PathBuf,
    pub t3_database: PathBuf,
    pub t3_http_url: Option<String>,
    pub t3_bearer_token_env: String,
    pub t3_app_name_contains: String,

    pub codex_bin: String,
    pub codex_app_name_contains: String,
    pub codex_source_kinds: Vec<String>,

    pub actuation_threshold: f32,
    pub release_threshold: f32,
    pub brightness: f32,
    pub poll_interval_ms: u64,
    pub combo_hold_ms: u64,
    pub key_hold_ms: u64,
    pub double_tap_ms: u64,
    pub depth_reversal_hysteresis: f32,
    pub hall_keys: [Position; 3],
    pub layer_buttons: [Position; 3],

    /// Compatibility with v0.3 configuration files. When present, these layers
    /// replace `targets.t3.layers`.
    #[serde(rename = "layers", skip_serializing_if = "Vec::is_empty")]
    legacy_t3_layers: Vec<LayerConfig>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct TargetsConfig {
    pub t3: TargetConfig,
    pub codex: TargetConfig,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TargetConfig {
    pub accent: String,
    pub status: StatusColors,
    pub layers: Vec<LayerConfig>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StatusColors {
    pub idle: String,
    pub starting: String,
    pub running: String,
    pub waiting_approval: String,
    pub waiting_input: String,
    pub completed: String,
    pub failed: String,
    pub unknown: String,
}

impl StatusColors {
    pub fn color_for(&self, phase: AgentPhase) -> &str {
        match phase {
            AgentPhase::Idle => &self.idle,
            AgentPhase::Starting => &self.starting,
            AgentPhase::Running => &self.running,
            AgentPhase::WaitingApproval => &self.waiting_approval,
            AgentPhase::WaitingInput => &self.waiting_input,
            AgentPhase::Completed => &self.completed,
            AgentPhase::Failed => &self.failed,
        }
    }

    fn values(&self) -> [&str; 8] {
        [
            &self.idle,
            &self.starting,
            &self.running,
            &self.waiting_approval,
            &self.waiting_input,
            &self.completed,
            &self.failed,
            &self.unknown,
        ]
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LayerConfig {
    pub name: String,
    pub color: String,
    pub actions: [String; 3],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub double_tap_action: Option<String>,
    #[serde(
        default = "default_key_gestures",
        skip_serializing_if = "key_gestures_are_unset"
    )]
    pub key_gestures: [KeyGestureConfig; 3],
    pub hold: HoldLayerConfig,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct KeyGestureConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hold_action: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub double_tap_action: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depth_actions: Vec<DepthActionConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depth_fallback_action: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actuation_threshold: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_threshold: Option<f32>,
}

impl KeyGestureConfig {
    pub fn depth_enabled(&self) -> bool {
        !self.depth_actions.is_empty()
    }

    pub fn effective_actuation_threshold(&self, default: f32) -> f32 {
        self.actuation_threshold
            .or_else(|| self.depth_actions.first().map(|action| action.at))
            .unwrap_or(default)
    }

    pub fn effective_release_threshold(&self, default: f32) -> f32 {
        self.release_threshold.unwrap_or(default)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct DepthActionConfig {
    pub at: f32,
    pub action: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct HoldLayerConfig {
    pub name: String,
    pub color: String,
    pub actions: [String; 3],
    #[serde(
        default = "default_key_gestures",
        skip_serializing_if = "key_gestures_are_unset"
    )]
    pub key_gestures: [KeyGestureConfig; 3],
}

impl Default for TargetsConfig {
    fn default() -> Self {
        Self {
            t3: t3_target(),
            codex: codex_target(),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        Self {
            default_target: TargetId::T3,
            target_order: vec![TargetId::T3, TargetId::Codex],
            targets: TargetsConfig::default(),
            t3_state_source: T3StateSource::Auto,
            t3_runtime: home.join(".t3/userdata/server-runtime.json"),
            t3_database: home.join(".t3/userdata/state.sqlite"),
            t3_http_url: None,
            t3_bearer_token_env: "UWU_VIBE_T3_BEARER_TOKEN".into(),
            t3_app_name_contains: "T3 Code".into(),
            codex_bin: "codex".into(),
            codex_app_name_contains: "ChatGPT".into(),
            codex_source_kinds: vec!["cli".into(), "vscode".into(), "appServer".into()],
            actuation_threshold: 0.42,
            release_threshold: 0.18,
            brightness: 0.65,
            poll_interval_ms: 750,
            combo_hold_ms: 350,
            key_hold_ms: 350,
            double_tap_ms: 250,
            depth_reversal_hysteresis: 0.04,
            hall_keys: [
                Position::new(2, 1),
                Position::new(2, 3),
                Position::new(2, 5),
            ],
            layer_buttons: [
                Position::new(3, 2),
                Position::new(3, 3),
                Position::new(3, 4),
            ],
            legacy_t3_layers: Vec::new(),
        }
    }
}

impl Config {
    pub fn load(path: Option<&Path>) -> Result<Self> {
        let Some(path) = path else {
            return Ok(Self::default());
        };
        let source = fs::read_to_string(path)
            .with_context(|| format!("failed to read config {}", path.display()))?;
        let mut config: Self = toml::from_str(&source)
            .with_context(|| format!("failed to parse config {}", path.display()))?;
        if !config.legacy_t3_layers.is_empty() {
            config.targets.t3.layers = std::mem::take(&mut config.legacy_t3_layers);
        }
        config.validate()?;
        Ok(config)
    }

    pub fn target(&self, target: TargetId) -> &TargetConfig {
        match target {
            TargetId::T3 => &self.targets.t3,
            TargetId::Codex => &self.targets.codex,
        }
    }

    pub fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            !self.target_order.is_empty(),
            "target_order cannot be empty"
        );
        anyhow::ensure!(
            self.target_order.contains(&self.default_target),
            "default_target must appear in target_order"
        );
        let unique = self.target_order.iter().copied().collect::<HashSet<_>>();
        anyhow::ensure!(
            unique.len() == self.target_order.len(),
            "target_order cannot contain duplicates"
        );
        anyhow::ensure!(
            (0.0..=1.0).contains(&self.brightness),
            "brightness must be 0..1"
        );
        anyhow::ensure!(
            self.release_threshold < self.actuation_threshold,
            "release_threshold must be below actuation_threshold"
        );
        anyhow::ensure!(
            self.poll_interval_ms >= 100,
            "poll_interval_ms must be at least 100"
        );
        anyhow::ensure!(
            (100..=5000).contains(&self.combo_hold_ms),
            "combo_hold_ms must be between 100 and 5000"
        );
        anyhow::ensure!(
            (100..=5000).contains(&self.key_hold_ms),
            "key_hold_ms must be between 100 and 5000"
        );
        anyhow::ensure!(
            (100..=1000).contains(&self.double_tap_ms),
            "double_tap_ms must be between 100 and 1000"
        );
        anyhow::ensure!(
            (0.01..=0.25).contains(&self.depth_reversal_hysteresis),
            "depth_reversal_hysteresis must be between 0.01 and 0.25"
        );
        anyhow::ensure!(
            !self.codex_bin.trim().is_empty(),
            "codex_bin cannot be empty"
        );
        if let Some(url) = &self.t3_http_url {
            anyhow::ensure!(
                url.starts_with("http://") || url.starts_with("https://"),
                "t3_http_url must start with http:// or https://"
            );
        }
        for target in TargetId::ALL {
            self.validate_target(target, self.target(target))?;
        }
        Ok(())
    }

    fn validate_target(&self, id: TargetId, target: &TargetConfig) -> Result<()> {
        anyhow::ensure!(
            target.layers.len() == 3,
            "target {id} requires exactly three layers"
        );
        crate::rgb::Rgb::from_hex(&target.accent)
            .with_context(|| format!("invalid accent color for target {id}"))?;
        for color in target.status.values() {
            crate::rgb::Rgb::from_hex(color)
                .with_context(|| format!("invalid status color for target {id}"))?;
        }
        for layer in &target.layers {
            crate::rgb::Rgb::from_hex(&layer.color)
                .with_context(|| format!("invalid color for target {id} layer {}", layer.name))?;
            crate::rgb::Rgb::from_hex(&layer.hold.color).with_context(|| {
                format!("invalid hold color for target {id} layer {}", layer.name)
            })?;
            for (index, key) in layer.key_gestures.iter().enumerate() {
                self.validate_key_config(id, &layer.name, index, key)?;
            }
            for (index, key) in layer.hold.key_gestures.iter().enumerate() {
                self.validate_key_config(id, &layer.hold.name, index, key)?;
            }
        }
        Ok(())
    }

    fn validate_key_config(
        &self,
        id: TargetId,
        map_name: &str,
        index: usize,
        key: &KeyGestureConfig,
    ) -> Result<()> {
        let mut previous_depth = None;
        for depth_action in &key.depth_actions {
            anyhow::ensure!(
                (0.0..=1.0).contains(&depth_action.at),
                "invalid depth action point for target {id} map {map_name} key {}",
                index + 1
            );
            anyhow::ensure!(
                previous_depth.is_none_or(|previous| depth_action.at > previous),
                "depth action points must be strictly increasing for target {id} map {map_name} key {}",
                index + 1
            );
            anyhow::ensure!(
                !depth_action.action.trim().is_empty() && depth_action.action.trim() != "none",
                "depth actions cannot be empty or none for target {id} map {map_name} key {}",
                index + 1
            );
            previous_depth = Some(depth_action.at);
        }
        if key.depth_enabled() {
            anyhow::ensure!(
                !action_is_set(key.hold_action.as_deref())
                    && !action_is_set(key.double_tap_action.as_deref()),
                "depth actions cannot be combined with hold or double-tap actions for target {id} map {map_name} key {}",
                index + 1
            );
        } else {
            anyhow::ensure!(
                !action_is_set(key.depth_fallback_action.as_deref()),
                "depth_fallback_action requires depth_actions for target {id} map {map_name} key {}",
                index + 1
            );
        }
        let actuation = key.effective_actuation_threshold(self.actuation_threshold);
        let release = key.effective_release_threshold(self.release_threshold);
        anyhow::ensure!(
            (0.0..=1.0).contains(&actuation),
            "invalid actuation threshold for target {id} map {map_name} key {}",
            index + 1
        );
        anyhow::ensure!(
            (0.0..=1.0).contains(&release),
            "invalid release threshold for target {id} map {map_name} key {}",
            index + 1
        );
        anyhow::ensure!(
            release < actuation,
            "release threshold must be below actuation threshold for target {id} map {map_name} key {}",
            index + 1
        );
        Ok(())
    }
}

fn t3_target() -> TargetConfig {
    TargetConfig {
        accent: "#7c6cff".into(),
        status: StatusColors {
            idle: "#19191e".into(),
            starting: "#5078ff".into(),
            running: "#2878ff".into(),
            waiting_approval: "#ff5a32".into(),
            waiting_input: "#ffbe2d".into(),
            completed: "#37dc78".into(),
            failed: "#ff2341".into(),
            unknown: "#19191e".into(),
        },
        layers: standard_layers(
            ["chat.new", "commandPalette.toggle", "diff.toggle"],
            ["thread.previous", "thread.next", "chat.newLocal"],
            ["terminal.toggle", "preview.toggle", "modelPicker.toggle"],
        ),
    }
}

fn codex_target() -> TargetConfig {
    let mut layers = standard_layers(
        ["chat.new", "commandPalette.toggle", "diff.toggle"],
        ["thread.previous", "thread.next", "chat.newLocal"],
        ["terminal.toggle", "preview.toggle", "modelPicker.toggle"],
    );
    for (layer, (color, hold_color)) in layers.iter_mut().zip([
        ("#10a37f", "#48c6a9"),
        ("#4388ff", "#35b8ed"),
        ("#ffad32", "#ff6b6b"),
    ]) {
        layer.color = color.into();
        layer.hold.color = hold_color.into();
    }
    TargetConfig {
        accent: "#10a37f".into(),
        status: StatusColors {
            idle: "#d8d8d8".into(),
            starting: "#4388ff".into(),
            running: "#4388ff".into(),
            waiting_approval: "#ffad32".into(),
            waiting_input: "#ffd04a".into(),
            completed: "#42d77d".into(),
            failed: "#ff3b4f".into(),
            unknown: "#19191e".into(),
        },
        layers,
    }
}

fn standard_layers(
    chat_actions: [&str; 3],
    navigation_actions: [&str; 3],
    tool_actions: [&str; 3],
) -> Vec<LayerConfig> {
    vec![
        LayerConfig {
            name: "Agents".into(),
            color: "#7c6cff".into(),
            actions: strings(["thread.jump.1", "thread.jump.2", "thread.jump.3"]),
            double_tap_action: None,
            key_gestures: default_key_gestures(),
            hold: HoldLayerConfig {
                name: "More agents".into(),
                color: "#d06cff".into(),
                actions: strings(["thread.jump.4", "thread.jump.5", "thread.jump.6"]),
                key_gestures: default_key_gestures(),
            },
        },
        LayerConfig {
            name: "Chat".into(),
            color: "#24c8db".into(),
            actions: strings(chat_actions),
            double_tap_action: Some("target.next".into()),
            key_gestures: default_key_gestures(),
            hold: HoldLayerConfig {
                name: "Navigate".into(),
                color: "#24db8f".into(),
                actions: strings(navigation_actions),
                key_gestures: default_key_gestures(),
            },
        },
        LayerConfig {
            name: "Tools".into(),
            color: "#ff9f43".into(),
            actions: strings(tool_actions),
            double_tap_action: None,
            key_gestures: default_key_gestures(),
            hold: HoldLayerConfig {
                name: "Workspace".into(),
                color: "#ff5f57".into(),
                actions: strings(["sidebar.toggle", "rightPanel.toggle", "target.next"]),
                key_gestures: default_key_gestures(),
            },
        },
    ]
}

fn strings(values: [&str; 3]) -> [String; 3] {
    values.map(str::to_owned)
}

fn default_key_gestures() -> [KeyGestureConfig; 3] {
    std::array::from_fn(|_| KeyGestureConfig::default())
}

fn key_gestures_are_unset(gestures: &[KeyGestureConfig; 3]) -> bool {
    gestures
        .iter()
        .all(|gesture| gesture == &KeyGestureConfig::default())
}

fn action_is_set(action: Option<&str>) -> bool {
    action.is_some_and(|action| {
        let action = action.trim();
        !action.is_empty() && action != "none"
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_round_trips_through_toml() {
        let original = Config::default();
        let encoded = toml::to_string(&original).unwrap();
        let decoded: Config = toml::from_str(&encoded).unwrap();
        decoded.validate().unwrap();
        assert_eq!(decoded.targets.t3.layers[0].actions[0], "thread.jump.1");
        assert_eq!(
            decoded.targets.codex.layers[2].hold.actions[2],
            "target.next"
        );
        assert_eq!(
            decoded.targets.codex.layers[1].double_tap_action,
            Some("target.next".into())
        );
        assert_eq!(decoded.layer_buttons[2], Position::new(3, 4));
    }

    #[test]
    fn legacy_layers_replace_the_t3_keymap() {
        let mut legacy = Config::default();
        let mut layers = legacy.targets.t3.layers.clone();
        layers[0].name = "Legacy".into();
        legacy.legacy_t3_layers = layers;
        legacy.targets = TargetsConfig::default();
        let encoded = toml::to_string(&legacy).unwrap();
        let path = std::env::temp_dir().join(format!(
            "uwu-vibe-config-test-{}-{}.toml",
            std::process::id(),
            "legacy"
        ));
        fs::write(&path, encoded).unwrap();
        let decoded = Config::load(Some(&path)).unwrap();
        fs::remove_file(path).unwrap();
        assert_eq!(decoded.targets.t3.layers[0].name, "Legacy");
    }

    #[test]
    fn config_without_double_tap_fields_remains_valid() {
        let encoded = toml::to_string(&Config::default())
            .unwrap()
            .lines()
            .filter(|line| {
                !line.starts_with("double_tap_ms") && !line.starts_with("double_tap_action")
            })
            .collect::<Vec<_>>()
            .join("\n");
        let decoded: Config = toml::from_str(&encoded).unwrap();
        decoded.validate().unwrap();
        assert_eq!(decoded.double_tap_ms, 250);
        assert!(
            decoded
                .targets
                .t3
                .layers
                .iter()
                .all(|layer| layer.double_tap_action.is_none())
        );
    }

    #[test]
    fn per_key_gestures_and_thresholds_round_trip() {
        let mut config = Config::default();
        config.targets.codex.layers[0].key_gestures[0] = KeyGestureConfig {
            hold_action: Some("chat.new".into()),
            double_tap_action: Some("thread.jump.4".into()),
            depth_actions: Vec::new(),
            depth_fallback_action: None,
            actuation_threshold: Some(0.7),
            release_threshold: Some(0.2),
        };
        let encoded = toml::to_string(&config).unwrap();
        let decoded: Config = toml::from_str(&encoded).unwrap();
        decoded.validate().unwrap();
        let gesture = &decoded.targets.codex.layers[0].key_gestures[0];
        assert_eq!(gesture.hold_action.as_deref(), Some("chat.new"));
        assert_eq!(gesture.actuation_threshold, Some(0.7));
        assert_eq!(gesture.release_threshold, Some(0.2));
    }

    #[test]
    fn per_key_release_must_be_below_actuation() {
        let mut config = Config::default();
        config.targets.t3.layers[0].key_gestures[0].actuation_threshold = Some(0.5);
        config.targets.t3.layers[0].key_gestures[0].release_threshold = Some(0.6);
        assert!(config.validate().is_err());
    }

    #[test]
    fn depth_actions_round_trip_and_use_the_first_point_for_actuation() {
        let mut config = Config::default();
        let gesture = &mut config.targets.codex.layers[0].key_gestures[0];
        gesture.depth_actions = vec![
            DepthActionConfig {
                at: 0.33,
                action: "thread.jump.1".into(),
            },
            DepthActionConfig {
                at: 0.66,
                action: "thread.jump.2".into(),
            },
            DepthActionConfig {
                at: 0.95,
                action: "thread.jump.3".into(),
            },
        ];
        let encoded = toml::to_string(&config).unwrap();
        let decoded: Config = toml::from_str(&encoded).unwrap();
        decoded.validate().unwrap();
        let gesture = &decoded.targets.codex.layers[0].key_gestures[0];
        assert_eq!(gesture.depth_actions.len(), 3);
        assert_eq!(
            gesture.effective_actuation_threshold(config.actuation_threshold),
            0.33
        );
    }

    #[test]
    fn depth_actions_cannot_mix_with_timing_gestures_or_be_unsorted() {
        let mut config = Config::default();
        let gesture = &mut config.targets.t3.layers[0].key_gestures[0];
        gesture.depth_actions = vec![
            DepthActionConfig {
                at: 0.66,
                action: "thread.jump.2".into(),
            },
            DepthActionConfig {
                at: 0.33,
                action: "thread.jump.1".into(),
            },
        ];
        assert!(config.validate().is_err());

        let gesture = &mut config.targets.t3.layers[0].key_gestures[0];
        gesture.depth_actions.swap(0, 1);
        gesture.hold_action = Some("chat.new".into());
        assert!(config.validate().is_err());
    }

    #[test]
    fn documented_inline_depth_mapping_parses() {
        #[derive(Deserialize)]
        struct Example {
            #[serde(default = "default_key_gestures")]
            key_gestures: [KeyGestureConfig; 3],
        }

        let example: Example = toml::from_str(
            r#"
key_gestures = [
  { depth_actions = [{ at = 0.33, action = "light" }, { at = 0.66, action = "deep" }] },
  {},
  {}
]
"#,
        )
        .unwrap();
        assert_eq!(example.key_gestures[0].depth_actions.len(), 2);
        assert!(example.key_gestures[1].depth_actions.is_empty());
    }

    #[test]
    fn target_order_must_include_the_default() {
        let config = Config {
            target_order: vec![TargetId::Codex],
            ..Config::default()
        };
        assert!(config.validate().is_err());
    }
}
