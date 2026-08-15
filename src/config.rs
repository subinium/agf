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

pub fn oh_my_pi_sessions_dir() -> Result<PathBuf, AgfError> {
    Ok(home_dir()?.join(".omp/agent/sessions"))
}

pub fn gemini_dir() -> Result<PathBuf, AgfError> {
    Ok(home_dir()?.join(".gemini"))
}

pub fn cursor_dir() -> Result<PathBuf, AgfError> {
    Ok(home_dir()?.join(".cursor"))
}

pub fn hermes_dir() -> Result<PathBuf, AgfError> {
    Ok(home_dir()?.join(".hermes"))
}

pub fn yolop_sessions_dir() -> Result<PathBuf, AgfError> {
    dirs::data_dir()
        .map(|d| d.join("yolop").join("sessions"))
        .ok_or(AgfError::NoDataDir)
}

pub fn prime_agent_dir() -> Result<PathBuf, AgfError> {
    if let Some(path) = std::env::var_os("PRIME_AGENT_CODING_AGENT_DIR")
        && !path.is_empty()
    {
        return expand_tilde(PathBuf::from(path));
    }
    Ok(home_dir()?.join(".prime/agent"))
}

pub fn prime_sessions_dir() -> Result<PathBuf, AgfError> {
    for key in [
        "PRIME_AGENT_SESSION_DIR",
        "PRIME_AGENT_CODING_AGENT_SESSION_DIR",
    ] {
        if let Some(path) = std::env::var_os(key)
            && !path.is_empty()
        {
            return resolve_configured_path(PathBuf::from(path));
        }
    }

    let agent_dir = prime_agent_dir()?;
    let cwd = std::env::current_dir()?;
    prime_sessions_dir_from_settings(&agent_dir, &cwd)
}

fn prime_sessions_dir_from_settings(
    agent_dir: &std::path::Path,
    cwd: &std::path::Path,
) -> Result<PathBuf, AgfError> {
    let global = agent_dir.join("settings.json");
    let project = cwd.join(".prime/agent/settings.json");
    // Prime Agent merges project settings over global settings. Keep the last
    // configured value so a project-local sessionDir gets the same precedence.
    let configured = [global, project]
        .into_iter()
        .rev()
        .find_map(|settings_path| {
            let content = std::fs::read_to_string(&settings_path).ok()?;
            let settings = serde_json::from_str::<serde_json::Value>(&content).ok()?;
            settings
                .get("sessionDir")
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|path| !path.is_empty())
                .map(PathBuf::from)
        });
    match configured {
        // Prime expands `~` in SettingsManager but otherwise passes relative
        // paths unchanged to Node's filesystem APIs, so both global and
        // project settings resolve from the agent process cwd.
        Some(path) => resolve_configured_path_from(path, cwd),
        None => Ok(agent_dir.join("sessions")),
    }
}

fn resolve_configured_path(path: PathBuf) -> Result<PathBuf, AgfError> {
    let cwd = std::env::current_dir()?;
    resolve_configured_path_from(path, &cwd)
}

fn resolve_configured_path_from(path: PathBuf, cwd: &std::path::Path) -> Result<PathBuf, AgfError> {
    let expanded = expand_tilde(path)?;
    if expanded.is_absolute() {
        Ok(expanded)
    } else {
        Ok(cwd.join(expanded))
    }
}

fn expand_tilde(path: PathBuf) -> Result<PathBuf, AgfError> {
    let Some(text) = path.to_str() else {
        return Ok(path);
    };
    if text == "~" {
        return home_dir();
    }
    if let Some(rest) = text.strip_prefix("~/") {
        return Ok(home_dir()?.join(rest));
    }
    Ok(path)
}

pub fn kiro_data_dir() -> Result<PathBuf, AgfError> {
    // Kiro CLI stores data via dirs::data_local_dir()
    // macOS: ~/Library/Application Support/kiro-cli/
    // Linux: ~/.local/share/kiro-cli/
    dirs::data_local_dir()
        .map(|d| d.join("kiro-cli"))
        .ok_or(AgfError::NoDataDir)
}

pub fn kiro_sessions_dir() -> Result<PathBuf, AgfError> {
    let home = std::env::var_os("KIRO_HOME")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .map(expand_tilde)
        .transpose()?
        .unwrap_or(home_dir()?.join(".kiro"));
    Ok(home.join("sessions/cli"))
}

fn sqlite_sources(path: PathBuf) -> Vec<PathBuf> {
    let wal = path.with_file_name(format!(
        "{}-wal",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
    ));
    vec![path, wal]
}

/// Paths whose newest mtime decides whether `agent`'s cache entry is still
/// fresh (see `cache::load_cache`).
///
/// These must cover **every** file the agent's scanner reads, not just its
/// primary index: a source the scanner consults but this list omits can change
/// without invalidating the cache, so the TUI serves a stale payload until some
/// unrelated file happens to move. Keep each entry as narrow as the scanner
/// allows — every path here is walked (stat-only) on each launch.
pub fn data_sources(agent: Agent) -> Vec<PathBuf> {
    match agent {
        // `projects/` is load-bearing, not decorative: the scanner reads
        // `projects/*/<id>.jsonl` for the worktree label, the `aiTitle`, the
        // `away_summary` recap and the orphan filter. Recaps in particular are
        // appended to the transcript *after* the last prompt, so history.jsonl
        // alone would keep serving a one-cycle-stale recap forever.
        Agent::ClaudeCode => claude_dir()
            .map(|d| vec![d.join("history.jsonl"), d.join("projects")])
            .unwrap_or_default(),
        Agent::Codex => codex_dir()
            .map(|dir| {
                let mut sources = vec![dir.join("sessions"), dir.join("history.jsonl")];
                if let Ok(entries) = std::fs::read_dir(&dir) {
                    for path in entries.flatten().map(|entry| entry.path()).filter(|path| {
                        path.file_name()
                            .and_then(|name| name.to_str())
                            .is_some_and(|name| {
                                name.starts_with("state_") && name.ends_with(".sqlite")
                            })
                    }) {
                        sources.extend(sqlite_sources(path));
                    }
                }
                sources
            })
            .unwrap_or_default(),
        Agent::OpenCode => opencode_data_dir()
            .map(|d| sqlite_sources(d.join("opencode.db")))
            .unwrap_or_default(),
        Agent::Pi => pi_sessions_dir().map(|d| vec![d]).unwrap_or_default(),
        Agent::OhMyPi => oh_my_pi_sessions_dir().map(|d| vec![d]).unwrap_or_default(),
        Agent::Kiro => {
            let mut sources = kiro_data_dir()
                .map(|d| sqlite_sources(d.join("data.sqlite3")))
                .unwrap_or_default();
            if let Ok(dir) = kiro_sessions_dir() {
                sources.push(dir);
            }
            sources
        }
        Agent::CursorAgent => cursor_dir()
            .map(|d| vec![d.join("chats"), d.join("projects")])
            .unwrap_or_default(),
        Agent::Gemini => gemini_dir()
            .map(|d| vec![d.join("tmp"), d.join("projects.json")])
            .unwrap_or_default(),
        Agent::Hermes => hermes_dir()
            .map(|d| sqlite_sources(d.join("state.db")))
            .unwrap_or_default(),
        Agent::Yolop => yolop_sessions_dir().map(|d| vec![d]).unwrap_or_default(),
        Agent::PrimeAgent => {
            let mut sources = Vec::new();
            if let Ok(dir) = prime_agent_dir() {
                sources.push(dir.join("settings.json"));
            }
            if let Ok(cwd) = std::env::current_dir() {
                sources.push(cwd.join(".prime/agent/settings.json"));
            }
            if let Ok(dir) = prime_sessions_dir() {
                sources.push(dir);
            }
            sources
        }
    }
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
                        if is_executable_path(&entry.path())
                            && let Some(name) = entry.file_name().to_str()
                        {
                            insert_executable_name(&mut set, name, &pathext);
                        }
                    }
                }
            }
        }
        set
    })
}

fn is_executable_path(path: &std::path::Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
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
                set.insert(lower);
                break;
            }
        }
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
    fn prime_project_settings_override_global_and_resolve_from_cwd() {
        let root = std::env::temp_dir().join(format!("agf-prime-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let agent_dir = root.join("global-agent");
        let cwd = root.join("project");
        std::fs::create_dir_all(cwd.join(".prime/agent")).unwrap();
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::write(
            agent_dir.join("settings.json"),
            r#"{"sessionDir":"global-sessions"}"#,
        )
        .unwrap();
        std::fs::write(
            cwd.join(".prime/agent/settings.json"),
            r#"{"sessionDir":"project-sessions"}"#,
        )
        .unwrap();

        let resolved = prime_sessions_dir_from_settings(&agent_dir, &cwd).unwrap();
        assert_eq!(resolved, cwd.join("project-sessions"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn prime_global_relative_session_dir_resolves_from_process_cwd() {
        let root =
            std::env::temp_dir().join(format!("agf-prime-global-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let agent_dir = root.join("global-agent");
        let cwd = root.join("project");
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::write(
            agent_dir.join("settings.json"),
            r#"{"sessionDir":"global-sessions"}"#,
        )
        .unwrap();

        let resolved = prime_sessions_dir_from_settings(&agent_dir, &cwd).unwrap();
        assert_eq!(resolved, cwd.join("global-sessions"));
        let _ = std::fs::remove_dir_all(root);
    }

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
    #[cfg(unix)]
    fn executable_detection_rejects_plain_files_and_accepts_exec_bits() {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("agf-executable-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("agent");
        std::fs::File::create(&path)
            .unwrap()
            .write_all(b"#!/bin/sh\n")
            .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(!is_executable_path(&path));
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(is_executable_path(&path));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    #[cfg(windows)]
    fn insert_executable_name_ignores_non_pathext_files() {
        let pathext: Vec<String> = [".exe"].iter().map(|s| s.to_string()).collect();
        let mut set = HashSet::new();
        insert_executable_name(&mut set, "README.md", &pathext);
        assert!(!set.contains("readme.md"));
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
