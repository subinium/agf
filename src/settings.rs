use serde::Deserialize;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Deserialize, Default)]
pub struct Settings {
    #[serde(default)]
    pub sort_by: Option<String>, // "time", "name", "agent"
    #[serde(default)]
    pub max_sessions: Option<usize>, // limit number of sessions loaded
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
