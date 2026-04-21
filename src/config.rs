use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::OnceLock;

use crate::error::AgfError;
use crate::model::Agent;

pub fn home_dir() -> Result<PathBuf, AgfError> {
    dirs::home_dir().ok_or(AgfError::NoHomeDir)
}

pub fn claude_dir() -> Result<PathBuf, AgfError> {
    Ok(home_dir()?.join(".claude"))
}

pub fn codex_dir() -> Result<PathBuf, AgfError> {
    Ok(home_dir()?.join(".codex"))
}

pub fn opencode_data_dir() -> Result<PathBuf, AgfError> {
    Ok(home_dir()?.join(".local/share/opencode"))
}

pub fn pi_sessions_dir() -> Result<PathBuf, AgfError> {
    Ok(home_dir()?.join(".pi/agent/sessions"))
}

pub fn gemini_dir() -> Result<PathBuf, AgfError> {
    Ok(home_dir()?.join(".gemini"))
}

pub fn cursor_dir() -> Result<PathBuf, AgfError> {
    Ok(home_dir()?.join(".cursor"))
}

pub fn kiro_data_dir() -> Result<PathBuf, AgfError> {
    // Kiro CLI stores data via dirs::data_local_dir()
    // macOS: ~/Library/Application Support/kiro-cli/
    // Linux: ~/.local/share/kiro-cli/
    dirs::data_local_dir()
        .map(|d| d.join("kiro-cli"))
        .ok_or(AgfError::NoHomeDir)
}

/// Cached set of executable names found in `$PATH`. Built once per process so
/// `is_agent_installed` does not fork a `which` subprocess per agent.
fn path_executables() -> &'static HashSet<String> {
    static CACHE: OnceLock<HashSet<String>> = OnceLock::new();
    CACHE.get_or_init(|| {
        let mut set = HashSet::new();
        if let Some(path) = std::env::var_os("PATH") {
            for dir in std::env::split_paths(&path) {
                if let Ok(entries) = std::fs::read_dir(&dir) {
                    for entry in entries.flatten() {
                        if let Some(name) = entry.file_name().to_str() {
                            set.insert(name.to_string());
                        }
                    }
                }
            }
        }
        set
    })
}

pub fn is_agent_installed(agent: Agent) -> bool {
    path_executables().contains(agent.cli_name())
}

pub fn installed_agents() -> Vec<Agent> {
    Agent::all()
        .iter()
        .copied()
        .filter(|a| is_agent_installed(*a))
        .collect()
}
