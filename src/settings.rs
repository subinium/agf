use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Settings {
    #[serde(default)]
    pub sort_by: Option<String>, // "time", "name", "agent"
    #[serde(default)]
    pub max_sessions: Option<usize>, // limit number of sessions loaded
    #[serde(default = "default_summary_search_count")]
    pub summary_search_count: usize, // number of summaries included in fuzzy search (default 5)
    #[serde(default = "default_search_scope")]
    pub search_scope: String, // "name_path" (default) | "all"
    #[serde(default)]
    pub editor: Option<String>, // editor command (e.g. "code", "cursor"). Falls back to $EDITOR/$VISUAL
    #[serde(default)]
    pub pinned_sessions: Vec<String>, // session IDs pinned to top of list
    #[serde(default)]
    pub show_recap: bool, // show Claude Code recap (away_summary) instead of last prompt
}

fn default_summary_search_count() -> usize {
    5
}

fn default_search_scope() -> String {
    "name_path".to_string()
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            sort_by: None,
            max_sessions: None,
            summary_search_count: default_summary_search_count(),
            search_scope: default_search_scope(),
            editor: None,
            pinned_sessions: Vec::new(),
            show_recap: false,
        }
    }
}

impl Settings {
    pub fn config_path() -> PathBuf {
        config_path()
    }
}

impl Settings {
    pub fn load() -> Self {
        let path = config_path();
        match fs::read_to_string(&path) {
            Ok(content) => match toml::from_str::<Settings>(&content) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!(
                        "[agf] config parse error at {}: {e} — using defaults",
                        path.display()
                    );
                    Self::default()
                }
            },
            Err(_) => Self::default(),
        }
    }

    /// Persist settings to config.toml.
    pub fn save_editable(&self) {
        let path = config_path();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        // Load existing config and merge editable fields
        let mut existing: toml::Table = fs::read_to_string(&path)
            .ok()
            .and_then(|c| c.parse().ok())
            .unwrap_or_default();

        existing.insert(
            "search_scope".to_string(),
            toml::Value::String(self.search_scope.clone()),
        );
        existing.insert(
            "summary_search_count".to_string(),
            toml::Value::Integer(self.summary_search_count as i64),
        );
        if !self.pinned_sessions.is_empty() {
            existing.insert(
                "pinned_sessions".to_string(),
                toml::Value::Array(
                    self.pinned_sessions
                        .iter()
                        .map(|s| toml::Value::String(s.clone()))
                        .collect(),
                ),
            );
        } else {
            existing.remove("pinned_sessions");
        }
        if self.show_recap {
            existing.insert("show_recap".to_string(), toml::Value::Boolean(true));
        } else {
            existing.remove("show_recap");
        }

        let content = existing.to_string();
        let tmp = path.with_extension("toml.tmp");
        if fs::write(&tmp, &content).is_ok() {
            let _ = fs::rename(&tmp, &path);
        }
    }
}

fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config"))
        .join("agf")
        .join("config.toml")
}
