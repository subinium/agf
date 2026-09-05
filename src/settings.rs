use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

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
    #[serde(default)]
    pub include_non_interactive: bool, // include Codex subagents/exec sessions
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
            include_non_interactive: false,
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
        Self::load_from(&config_path(), |diagnostic| eprintln!("{diagnostic}"))
    }

    fn load_from(path: &Path, report: impl FnOnce(&str)) -> Self {
        match fs::read_to_string(path) {
            Ok(content) => match toml::from_str::<Settings>(&content) {
                Ok(s) => s,
                Err(_) => {
                    // TOML errors can contain source lines and rejected values.
                    report(&format!(
                        "[agf] invalid TOML/settings at {}: using defaults",
                        path.display()
                    ));
                    Self::default()
                }
            },
            Err(_) => Self::default(),
        }
    }

    /// Persist settings to config.toml.
    pub fn save_editable(&self) {
        let path = config_path();
        if let Err(error) = self.save_editable_to(&path) {
            eprintln!("{}", save_error_diagnostic(&path, &error));
        }
    }

    fn save_editable_to(&self, path: &Path) -> io::Result<()> {
        // Only a missing file is a new configuration. Never overwrite an
        // unreadable or malformed existing document with default settings.
        let mut existing: toml::Table = match fs::read_to_string(path) {
            Ok(content) => content.parse().map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "invalid TOML configuration")
            })?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => toml::Table::new(),
            Err(error) => return Err(error),
        };

        existing.insert(
            "search_scope".to_string(),
            toml::Value::String(self.search_scope.clone()),
        );
        existing.insert(
            "summary_search_count".to_string(),
            toml::Value::Integer(
                i64::try_from(self.summary_search_count)
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?,
            ),
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

        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)?;
        }
        crate::fsx::write_atomic(path, existing.to_string().as_bytes())
    }
}

fn save_error_diagnostic(path: &Path, error: &io::Error) -> String {
    // Render only the category, never an error payload that may embed config.
    format!(
        "[agf] could not save config at {} ({:?})",
        path.display(),
        error.kind()
    )
}

fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config"))
        .join("agf")
        .join("config.toml")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static NEXT_ID: AtomicU64 = AtomicU64::new(0);
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "agf-settings-{}-{stamp}-{}",
                std::process::id(),
                NEXT_ID.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn load_diagnostics_do_not_echo_config_secrets() {
        const SECRET: &str = "PRIVATE_CONFIG_SENTINEL_739182";
        let dir = TempDir::new();
        let path = dir.0.join("config.toml");
        for original in [
            format!("api_key = \"{SECRET}\" trailing\n"),
            format!("summary_search_count = \"{SECRET}\"\n"),
        ] {
            fs::write(&path, &original).unwrap();
            let mut diagnostic = String::new();
            let settings = Settings::load_from(&path, |message| diagnostic.push_str(message));
            assert_eq!(
                settings.summary_search_count,
                default_summary_search_count()
            );
            assert_eq!(
                diagnostic,
                format!(
                    "[agf] invalid TOML/settings at {}: using defaults",
                    path.display()
                )
            );
            assert!(!diagnostic.contains(SECRET));
            assert_eq!(fs::read(&path).unwrap(), original.as_bytes());
        }
    }

    #[test]
    fn save_diagnostics_do_not_echo_config_secrets_or_error_payloads() {
        const SECRET: &str = "PRIVATE_CONFIG_SENTINEL_739183";
        let dir = TempDir::new();
        let path = dir.0.join("config.toml");
        let original = format!("api_key = \"{SECRET}\" trailing\n");
        fs::write(&path, &original).unwrap();
        let error = Settings::default().save_editable_to(&path).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(error.to_string(), "invalid TOML configuration");
        let diagnostic = save_error_diagnostic(&path, &error);
        assert!(diagnostic.contains("InvalidData"));
        assert!(!diagnostic.contains(SECRET));
        assert!(!save_error_diagnostic(&path, &io::Error::other(SECRET)).contains(SECRET));
        assert_eq!(fs::read(&path).unwrap(), original.as_bytes());
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1);
    }

    #[test]
    fn editable_save_preserves_malformed_and_non_utf8_config() {
        let dir = TempDir::new();
        let path = dir.0.join("config.toml");
        for original in [b"[sources\nsecret = 'keep'".as_slice(), b"editor = '\xff'"] {
            fs::write(&path, original).unwrap();
            let error = Settings::default().save_editable_to(&path).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidData);
            assert_eq!(fs::read(&path).unwrap(), original);
            assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1);
        }
    }

    #[test]
    fn editable_save_reports_non_not_found_read_errors() {
        let dir = TempDir::new();
        let path = dir.0.join("config.toml");
        fs::create_dir(&path).unwrap();
        let marker = path.join("keep");
        fs::write(&marker, b"original").unwrap();
        assert!(Settings::default().save_editable_to(&path).is_err());
        assert_eq!(fs::read(&marker).unwrap(), b"original");
    }

    #[test]
    fn editable_save_merges_only_editable_fields() {
        let dir = TempDir::new();
        let path = dir.0.join("config.toml");
        fs::write(&path, "editor = 'code'\nshow_recap = true\npinned_sessions = ['old']\n[sources]\ncodex = '/custom'\n").unwrap();
        let settings = Settings {
            search_scope: "all".into(),
            ..Settings::default()
        };
        settings.save_editable_to(&path).unwrap();
        let saved: toml::Table = fs::read_to_string(&path).unwrap().parse().unwrap();
        assert_eq!(saved["editor"].as_str(), Some("code"));
        assert_eq!(saved["sources"]["codex"].as_str(), Some("/custom"));
        assert_eq!(saved["search_scope"].as_str(), Some("all"));
        assert_eq!(saved["summary_search_count"].as_integer(), Some(5));
        assert!(!saved.contains_key("show_recap"));
        assert!(!saved.contains_key("pinned_sessions"));
    }

    #[test]
    fn editable_save_creates_missing_config_and_reports_bad_parent() {
        let dir = TempDir::new();
        let path = dir.0.join("nested/config.toml");
        Settings::default().save_editable_to(&path).unwrap();
        assert!(
            fs::read_to_string(&path)
                .unwrap()
                .parse::<toml::Table>()
                .is_ok()
        );
        assert!(
            Settings::default()
                .save_editable_to(&path.join("config.toml"))
                .is_err()
        );
        assert!(path.is_file());
    }
}
