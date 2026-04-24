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

/// Cached set of executable names found in `$PATH`, built once per process.
/// On Windows entries are lower-cased and `%PATHEXT%` stems are inserted
/// alongside the full filename so bare-name lookups match `.exe`/`.cmd`/etc.
fn path_executables() -> &'static HashSet<String> {
    static CACHE: OnceLock<HashSet<String>> = OnceLock::new();
    CACHE.get_or_init(|| {
        let pathext = windows_pathext();
        let mut set = HashSet::new();
        if let Some(path) = std::env::var_os("PATH") {
            for dir in std::env::split_paths(&path) {
                if let Ok(entries) = std::fs::read_dir(&dir) {
                    for entry in entries.flatten() {
                        if let Some(name) = entry.file_name().to_str() {
                            insert_executable_name(&mut set, name, &pathext);
                        }
                    }
                }
            }
        }
        set
    })
}

/// Return the list of PATHEXT suffixes (lower-cased, each beginning with `.`)
/// on Windows; empty on other platforms.
fn windows_pathext() -> Vec<String> {
    if !cfg!(windows) {
        return Vec::new();
    }
    std::env::var("PATHEXT")
        .unwrap_or_else(|_| String::from(".COM;.EXE;.BAT;.CMD"))
        .split(';')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_lowercase())
        .collect()
}

/// Insert `filename` into `set`. On Windows, also insert its lower-cased
/// stem (without a `PATHEXT` suffix). On other platforms, insert verbatim.
fn insert_executable_name(set: &mut HashSet<String>, filename: &str, pathext: &[String]) {
    if cfg!(windows) {
        let lower = filename.to_lowercase();
        for ext in pathext {
            if let Some(stem) = lower.strip_suffix(ext.as_str()) {
                set.insert(stem.to_string());
                break;
            }
        }
        set.insert(lower);
    } else {
        set.insert(filename.to_string());
    }
}

pub fn is_agent_installed(agent: Agent) -> bool {
    let name = agent.cli_name();
    let execs = path_executables();
    if cfg!(windows) {
        execs.contains(&name.to_lowercase())
    } else {
        execs.contains(name)
    }
}

pub fn installed_agents() -> Vec<Agent> {
    Agent::all()
        .iter()
        .copied()
        .filter(|a| is_agent_installed(*a))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(windows)]
    fn insert_executable_name_adds_stem_on_windows() {
        let pathext: Vec<String> = [".com", ".exe", ".bat", ".cmd"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let mut set = HashSet::new();
        insert_executable_name(&mut set, "Claude.EXE", &pathext);
        // Full lower-cased name and stem both present.
        assert!(set.contains("claude.exe"));
        assert!(set.contains("claude"));
    }

    #[test]
    #[cfg(windows)]
    fn insert_executable_name_no_stem_when_ext_missing() {
        let pathext: Vec<String> = [".exe"].iter().map(|s| s.to_string()).collect();
        let mut set = HashSet::new();
        insert_executable_name(&mut set, "README.md", &pathext);
        assert!(set.contains("readme.md"));
        // No bare "readme" stem should be inserted — .md is not in PATHEXT.
        assert!(!set.contains("readme"));
    }

    #[test]
    #[cfg(not(windows))]
    fn insert_executable_name_verbatim_on_unix() {
        let mut set = HashSet::new();
        insert_executable_name(&mut set, "claude", &[]);
        assert!(set.contains("claude"));
        assert_eq!(set.len(), 1);
    }
}
