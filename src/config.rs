use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::hardware::Position;

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
    pub t3_state_source: T3StateSource,
    pub t3_runtime: PathBuf,
    pub t3_database: PathBuf,
    pub t3_http_url: Option<String>,
    pub t3_bearer_token_env: String,
    pub t3_app_name_contains: String,
    pub actuation_threshold: f32,
    pub release_threshold: f32,
    pub brightness: f32,
    pub poll_interval_ms: u64,
    pub combo_hold_ms: u64,
    pub hall_keys: [Position; 3],
    pub layer_buttons: [Position; 3],
    pub layers: Vec<LayerConfig>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LayerConfig {
    pub name: String,
    pub color: String,
    pub actions: [String; 3],
    pub hold: HoldLayerConfig,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct HoldLayerConfig {
    pub name: String,
    pub color: String,
    pub actions: [String; 3],
}

impl Default for Config {
    fn default() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        Self {
            t3_state_source: T3StateSource::Auto,
            t3_runtime: home.join(".t3/userdata/server-runtime.json"),
            t3_database: home.join(".t3/userdata/state.sqlite"),
            t3_http_url: None,
            t3_bearer_token_env: "T3_UWU_BEARER_TOKEN".into(),
            t3_app_name_contains: "T3 Code".into(),
            actuation_threshold: 0.42,
            release_threshold: 0.18,
            brightness: 0.65,
            poll_interval_ms: 750,
            combo_hold_ms: 350,
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
            layers: vec![
                LayerConfig {
                    name: "Agents".into(),
                    color: "#7c6cff".into(),
                    actions: [
                        "thread.jump.1".into(),
                        "thread.jump.2".into(),
                        "thread.jump.3".into(),
                    ],
                    hold: HoldLayerConfig {
                        name: "More agents".into(),
                        color: "#d06cff".into(),
                        actions: [
                            "thread.jump.4".into(),
                            "thread.jump.5".into(),
                            "thread.jump.6".into(),
                        ],
                    },
                },
                LayerConfig {
                    name: "Chat".into(),
                    color: "#24c8db".into(),
                    actions: [
                        "chat.new".into(),
                        "commandPalette.toggle".into(),
                        "diff.toggle".into(),
                    ],
                    hold: HoldLayerConfig {
                        name: "Navigate".into(),
                        color: "#24db8f".into(),
                        actions: [
                            "thread.previous".into(),
                            "thread.next".into(),
                            "chat.newLocal".into(),
                        ],
                    },
                },
                LayerConfig {
                    name: "Tools".into(),
                    color: "#ff9f43".into(),
                    actions: [
                        "terminal.toggle".into(),
                        "preview.toggle".into(),
                        "modelPicker.toggle".into(),
                    ],
                    hold: HoldLayerConfig {
                        name: "Workspace".into(),
                        color: "#ff5f57".into(),
                        actions: [
                            "sidebar.toggle".into(),
                            "rightPanel.toggle".into(),
                            "preview.refresh".into(),
                        ],
                    },
                },
            ],
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
        let config: Self = toml::from_str(&source)
            .with_context(|| format!("failed to parse config {}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        anyhow::ensure!(self.layers.len() == 3, "exactly three layers are required");
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
        if let Some(url) = &self.t3_http_url {
            anyhow::ensure!(
                url.starts_with("http://") || url.starts_with("https://"),
                "t3_http_url must start with http:// or https://"
            );
        }
        for layer in self.layers.iter().take(3) {
            crate::rgb::Rgb::from_hex(&layer.color)
                .with_context(|| format!("invalid color for layer {}", layer.name))?;
            crate::rgb::Rgb::from_hex(&layer.hold.color)
                .with_context(|| format!("invalid hold color for layer {}", layer.name))?;
        }
        Ok(())
    }
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
        assert_eq!(decoded.layers[0].actions[0], "thread.jump.1");
        assert_eq!(decoded.layers[0].hold.actions[0], "thread.jump.4");
        assert_eq!(decoded.layer_buttons[2], Position::new(3, 4));
    }

    #[test]
    fn hold_layer_is_required() {
        let encoded = toml::to_string(&Config::default()).unwrap();
        let without_hold = encoded
            .lines()
            .take_while(|line| *line != "[layers.hold]")
            .collect::<Vec<_>>()
            .join("\n");
        assert!(toml::from_str::<Config>(&without_hold).is_err());
    }
}
