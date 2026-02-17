use std::path::PathBuf;
use std::fs;
use serde::Deserialize;

#[derive(Debug, Deserialize, Default)]
#[allow(dead_code)]
pub struct Settings {
    #[serde(default)]
    pub default_agent: Option<String>,  // "claude", "codex", "cursor"
    #[serde(default)]
    pub sort_by: Option<String>,        // "time", "name", "agent"
    #[serde(default)]
    pub show_preview: Option<bool>,     // show detail preview by default
    #[serde(default)]
    pub max_sessions: Option<usize>,    // limit number of sessions loaded
}

impl Settings {
    pub fn load() -> Self {
        let path = config_path();
        match fs::read_to_string(&path) {
            Ok(content) => toml::from_str(&content).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }
}

fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config"))
        .join("agf")
        .join("config.toml")
}
