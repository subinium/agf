use serde::Deserialize;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub sort_by: Option<String>, // "time", "name", "agent"
    #[serde(default)]
    pub max_sessions: Option<usize>, // limit number of sessions loaded
    #[serde(default = "default_summary_search_count")]
    pub summary_search_count: usize, // number of summaries included in fuzzy search (default 5)
    #[serde(default = "default_search_scope")]
    pub search_scope: String, // "name_path" (default) | "all"
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
            Ok(content) => toml::from_str(&content).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    /// Persist editable fields to config.toml, preserving unrelated keys.
    pub fn save_editable(&self) {
        let path = config_path();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        // Re-parse existing file line by line, replacing or appending the two keys
        let existing = fs::read_to_string(&path).unwrap_or_default();
        let mut lines: Vec<String> = existing
            .lines()
            .filter(|l| {
                let t = l.trim_start();
                !t.starts_with("search_scope") && !t.starts_with("summary_search_count")
            })
            .map(|l| l.to_string())
            .collect();

        lines.push(format!("search_scope = {:?}", self.search_scope));
        lines.push(format!(
            "summary_search_count = {}",
            self.summary_search_count
        ));

        let _ = fs::write(&path, lines.join("\n") + "\n");
    }
}

fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config"))
        .join("agf")
        .join("config.toml")
}
