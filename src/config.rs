use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::OnceLock;

use crate::error::AgfError;
use crate::model::Agent;

pub fn home_dir() -> Result<PathBuf, AgfError> {
    dirs::home_dir().ok_or(AgfError::NoHomeDir)
}

pub fn claude_dir() -> Result<PathBuf, AgfError> {
    match std::env::var_os("CLAUDE_CONFIG_DIR").filter(|path| !path.is_empty()) {
        Some(path) => Ok(absolute_env_path(
            PathBuf::from(path),
            &std::env::current_dir()?,
        )),
        None => Ok(home_dir()?.join(".claude")),
    }
}

pub fn codex_dir() -> Result<PathBuf, AgfError> {
    codex_dir_from(
        std::env::var_os("CODEX_HOME").map(PathBuf::from),
        &home_dir()?,
    )
}

fn codex_dir_from(
    configured: Option<PathBuf>,
    home: &std::path::Path,
) -> Result<PathBuf, AgfError> {
    match configured.filter(|path| !path.as_os_str().is_empty()) {
        Some(path) => {
            // Match Codex: an explicit CODEX_HOME must already be a directory.
            // Do not expand a literal '~' here; the provider does not either.
            if !std::fs::metadata(&path)?.is_dir() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "CODEX_HOME is not a directory",
                )
                .into());
            }
            Ok(path.canonicalize()?)
        }
        None => Ok(home.join(".codex")),
    }
}

/// SQLite state is independent from rollout/history storage. User config
/// takes precedence over the environment, whose relative paths use cwd.
pub fn codex_sqlite_dir() -> Result<PathBuf, AgfError> {
    codex_sqlite_dir_from(
        &codex_dir()?,
        std::env::var("CODEX_SQLITE_HOME")
            .ok()
            .map(|value| PathBuf::from(value.trim())),
        &std::env::current_dir()?,
    )
}

fn codex_sqlite_dir_from(
    codex_home: &std::path::Path,
    env: Option<PathBuf>,
    cwd: &std::path::Path,
) -> Result<PathBuf, AgfError> {
    use std::io::Read;
    #[derive(serde::Deserialize)]
    struct StorageConfig {
        sqlite_home: Option<PathBuf>,
    }
    let path = codex_home.join("config.toml");
    let configured = match std::fs::File::open(&path) {
        Ok(file) => {
            let mut content = String::new();
            file.take(1024 * 1024 + 1).read_to_string(&mut content)?;
            if content.len() > 1024 * 1024 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "Codex config.toml exceeds 1 MiB",
                )
                .into());
            }
            // Fail closed on malformed config instead of deleting from an
            // unrelated fallback store that Codex itself would not select.
            toml::from_str::<StorageConfig>(&content)
                .map_err(|_| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("{}: invalid TOML storage configuration", path.display()),
                    )
                })?
                .sqlite_home
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    if let Some(path) = configured {
        return resolve_codex_sqlite_path(path, codex_home);
    }
    match env.filter(|path| !path.as_os_str().is_empty()) {
        Some(path) => resolve_codex_sqlite_path(path, cwd),
        None => Ok(codex_home.to_path_buf()),
    }
}

fn resolve_codex_sqlite_path(path: PathBuf, base: &std::path::Path) -> Result<PathBuf, AgfError> {
    let path = resolve_configured_path_from(path, base)?;
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    Ok(normalized)
}

fn absolute_env_path(path: PathBuf, cwd: &std::path::Path) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    }
}

pub fn grok_dir() -> Result<PathBuf, AgfError> {
    if let Some(path) = std::env::var_os("GROK_HOME").filter(|path| !path.is_empty()) {
        return resolve_configured_path(PathBuf::from(path));
    }
    Ok(home_dir()?.join(".grok"))
}

pub fn kimi_code_dir() -> Result<PathBuf, AgfError> {
    if let Some(path) = std::env::var_os("KIMI_CODE_HOME").filter(|path| !path.is_empty()) {
        return resolve_configured_path(PathBuf::from(path));
    }
    Ok(home_dir()?.join(".kimi-code"))
}

pub fn qwen_global_dir() -> Result<PathBuf, AgfError> {
    if let Some(path) = std::env::var_os("QWEN_HOME").filter(|path| !path.is_empty()) {
        return resolve_configured_path(PathBuf::from(path));
    }
    Ok(home_dir()?.join(".qwen"))
}

pub fn qwen_runtime_dir() -> Result<PathBuf, AgfError> {
    if let Some(path) = std::env::var_os("QWEN_RUNTIME_DIR").filter(|path| !path.is_empty()) {
        return resolve_configured_path(PathBuf::from(path));
    }
    let global = qwen_global_dir()?;
    let cwd = std::env::current_dir()?;
    qwen_runtime_dir_from_settings(&global, &cwd)
}

fn qwen_runtime_dir_from_settings(
    global_dir: &std::path::Path,
    cwd: &std::path::Path,
) -> Result<PathBuf, AgfError> {
    let mut configured = None;
    for settings_path in [
        global_dir.join("settings.json"),
        cwd.join(".qwen/settings.json"),
    ] {
        let Some(path) = std::fs::read_to_string(settings_path)
            .ok()
            .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok())
            .and_then(|settings| {
                settings
                    .pointer("/advanced/runtimeOutputDir")
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|path| !path.is_empty())
                    .map(PathBuf::from)
            })
        else {
            continue;
        };
        configured = Some(path);
    }
    match configured {
        // Qwen resolves both user and workspace relative values from the
        // active project root; workspace settings override user settings.
        Some(path) => resolve_configured_path_from(path, cwd),
        None => Ok(global_dir.to_path_buf()),
    }
}

pub fn opencode_data_dir() -> Result<PathBuf, AgfError> {
    if let Some(path) = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
    {
        return Ok(path.join("opencode"));
    }
    Ok(home_dir()?.join(".local/share/opencode"))
}

pub fn pi_sessions_dir() -> Result<PathBuf, AgfError> {
    if let Some(path) =
        std::env::var_os("PI_CODING_AGENT_SESSION_DIR").filter(|path| !path.is_empty())
    {
        return resolve_configured_path(PathBuf::from(path));
    }
    if let Some(path) = std::env::var_os("PI_CODING_AGENT_DIR").filter(|path| !path.is_empty()) {
        return Ok(resolve_configured_path(PathBuf::from(path))?.join("sessions"));
    }
    Ok(home_dir()?.join(".pi/agent/sessions"))
}

pub fn oh_my_pi_sessions_dir() -> Result<PathBuf, AgfError> {
    Ok(home_dir()?.join(".omp/agent/sessions"))
}

pub fn gemini_dir() -> Result<PathBuf, AgfError> {
    if let Some(path) = std::env::var_os("GEMINI_CLI_HOME").filter(|path| !path.is_empty()) {
        return Ok(
            absolute_env_path(PathBuf::from(path), &std::env::current_dir()?).join(".gemini"),
        );
    }
    Ok(home_dir()?.join(".gemini"))
}

pub fn cursor_dir() -> Result<PathBuf, AgfError> {
    Ok(home_dir()?.join(".cursor"))
}

pub fn hermes_dir() -> Result<PathBuf, AgfError> {
    if let Some(path) = std::env::var("HERMES_HOME")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|path| !path.is_empty())
    {
        return Ok(absolute_env_path(
            PathBuf::from(path),
            &std::env::current_dir()?,
        ));
    }
    if cfg!(windows) {
        return Ok(std::env::var("LOCALAPPDATA")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
            .unwrap_or(home_dir()?.join("AppData/Local"))
            .join("hermes"));
    }
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
        return Ok(home_dir()?.join(rest.trim_start_matches('/')));
    }
    #[cfg(windows)]
    if let Some(rest) = text.strip_prefix("~\\") {
        return Ok(home_dir()?.join(rest.trim_start_matches('\\')));
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

fn grok_summary_sources() -> Vec<PathBuf> {
    let Ok(root) = grok_dir().map(|dir| dir.join("sessions")) else {
        return Vec::new();
    };
    // A non-existent marker keeps the configured root identity in the
    // fingerprint without recursively walking every rewind/checkpoint file.
    let mut sources = vec![root.join(".agf-session-root")];
    let Ok(cwds) = std::fs::read_dir(&root) else {
        return sources;
    };
    for cwd in cwds
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
    {
        let Ok(sessions) = std::fs::read_dir(cwd.path()) else {
            continue;
        };
        for session in sessions
            .flatten()
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        {
            let summary = session.path().join("summary.json");
            if summary.is_file() {
                sources.push(summary);
            }
        }
    }
    sources
}

fn kimi_state_sources() -> Vec<PathBuf> {
    let Ok(home) = kimi_code_dir() else {
        return Vec::new();
    };
    let root = home.join("sessions");
    let mut sources = vec![
        home.join("session_index.jsonl"),
        root.join(".agf-session-root"),
    ];
    let Ok(workspaces) = std::fs::read_dir(&root) else {
        return sources;
    };
    for workspace in workspaces
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
    {
        let Ok(sessions) = std::fs::read_dir(workspace.path()) else {
            continue;
        };
        for session in sessions
            .flatten()
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        {
            let direct = session.path().join("state.json");
            let legacy = session.path().join("session-meta/state.json");
            if direct.is_file() {
                sources.push(direct);
            } else if legacy.is_file() {
                sources.push(legacy);
            }
        }
    }
    sources
}

fn qwen_chat_sources(root: &std::path::Path) -> Vec<PathBuf> {
    let mut sources = vec![root.join(".agf-session-root")];
    let Ok(projects) = std::fs::read_dir(root) else {
        return sources;
    };
    for project in projects
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
    {
        let chats = project.path().join("chats");
        let Ok(entries) = std::fs::read_dir(chats) else {
            continue;
        };
        sources.extend(
            entries
                .flatten()
                .filter_map(|entry| {
                    entry
                        .file_type()
                        .ok()
                        .filter(|kind| kind.is_file())
                        .map(|_| entry.path())
                })
                .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("jsonl")),
        );
    }
    sources
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
                let mut sources = vec![
                    dir.join("sessions"),
                    dir.join("history.jsonl"),
                    dir.join("config.toml"),
                ];
                let Ok(sqlite_dir) = codex_sqlite_dir() else {
                    return sources;
                };
                // Keep root identity even when the configured store is empty.
                sources.push(sqlite_dir.join(".agf-state-root"));
                if let Ok(entries) = std::fs::read_dir(&sqlite_dir) {
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
        Agent::Grok => grok_summary_sources(),
        Agent::Kimi => kimi_state_sources(),
        Agent::Qwen => {
            let mut sources = Vec::new();
            if let Ok(global) = qwen_global_dir() {
                sources.push(global.join("settings.json"));
            }
            if let Ok(cwd) = std::env::current_dir() {
                sources.push(cwd.join(".qwen/settings.json"));
            }
            if let Ok(dir) = qwen_runtime_dir() {
                sources.extend(qwen_chat_sources(&dir.join("projects")));
                sources.extend(qwen_chat_sources(&dir.join("tmp")));
            }
            sources
        }
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

/// Cache the first executable PATH match as an absolute path. Relative PATH
/// entries must not be reinterpreted after the generated command changes cwd.
fn path_executables() -> &'static HashMap<String, PathBuf> {
    static CACHE: OnceLock<HashMap<String, PathBuf>> = OnceLock::new();
    CACHE.get_or_init(|| {
        let Ok(cwd) = std::env::current_dir() else {
            return HashMap::new();
        };
        collect_path_executables(
            std::env::var_os("PATH").as_deref(),
            &cwd,
            &windows_pathext(),
        )
    })
}

fn collect_path_executables(
    path: Option<&std::ffi::OsStr>,
    cwd: &std::path::Path,
    pathext: &[String],
) -> HashMap<String, PathBuf> {
    let mut executables = HashMap::new();
    let Some(path) = path else {
        return executables;
    };
    for dir in std::env::split_paths(path) {
        let dir = absolute_env_path(dir, cwd);
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        let mut paths: Vec<_> = entries.flatten().map(|entry| entry.path()).collect();
        if cfg!(windows) {
            paths.sort_by_key(|path| {
                let name = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_lowercase();
                pathext
                    .iter()
                    .position(|ext| name.ends_with(ext))
                    .unwrap_or(usize::MAX)
            });
        }
        for path in paths {
            if is_executable_path(&path)
                && let Some(name) = path.file_name().and_then(|name| name.to_str())
            {
                let mut names = HashSet::new();
                insert_executable_name(&mut names, name, pathext);
                for name in names {
                    executables.entry(name).or_insert_with(|| path.clone());
                }
            }
        }
    }
    executables
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

/// Explicit Cursor executable selection is identity-safe: never guess that an
/// unrelated `agent` on PATH belongs to Cursor, and never run it to probe.
pub fn agent_program(agent: Agent) -> String {
    let override_value = (agent == Agent::CursorAgent)
        .then(|| std::env::var("AGF_CURSOR_CLI").ok())
        .flatten();
    let name = agent_program_from(
        agent,
        override_value.as_deref(),
        std::env::current_dir().ok().as_deref(),
    );
    resolved_program(&name)
        .and_then(|path| path.to_str().map(str::to_string))
        .unwrap_or(name)
}

fn agent_program_from(
    agent: Agent,
    cursor_override: Option<&str>,
    cwd: Option<&std::path::Path>,
) -> String {
    let Some(value) = cursor_override
        .filter(|value| !value.is_empty())
        .filter(|_| agent == Agent::CursorAgent)
    else {
        return agent.cli_name().to_string();
    };
    let path = std::path::Path::new(value);
    if path.components().count() > 1
        && !path.is_absolute()
        && let Some(cwd) = cwd
    {
        return cwd.join(path).to_string_lossy().into_owned();
    }
    value.to_string()
}

pub fn is_agent_installed(agent: Agent) -> bool {
    is_program_installed(&agent_program(agent))
}

/// Freeze only the selected provider's verified storage roots before a resume
/// changes cwd. Never include credentials or copy the general environment.
pub fn resume_environment(agent: Agent) -> Result<BTreeMap<String, String>, String> {
    let mut environment = BTreeMap::new();
    let configured = |name: &str| std::env::var_os(name).is_some_and(|value| !value.is_empty());
    let mut insert = |name: &str, path: Result<PathBuf, AgfError>| -> Result<(), String> {
        let path = path.map_err(|error| error.to_string())?;
        let path = std::path::absolute(path).map_err(|error| error.to_string())?;
        let text = path
            .to_str()
            .ok_or_else(|| format!("{name} is not valid UTF-8"))?;
        environment.insert(name.to_string(), text.to_string());
        Ok(())
    };
    match agent {
        Agent::Codex => {
            if configured("CODEX_HOME") {
                insert("CODEX_HOME", codex_dir())?;
            }
            // Config-relative sqlite_home remains authoritative after CODEX_HOME
            // is frozen; this also freezes a cwd-relative environment fallback.
            insert("CODEX_SQLITE_HOME", codex_sqlite_dir())?;
        }
        Agent::ClaudeCode if configured("CLAUDE_CONFIG_DIR") => {
            insert("CLAUDE_CONFIG_DIR", claude_dir())?
        }
        Agent::Grok if configured("GROK_HOME") => insert("GROK_HOME", grok_dir())?,
        Agent::Kimi if configured("KIMI_CODE_HOME") => insert("KIMI_CODE_HOME", kimi_code_dir())?,
        Agent::Qwen => {
            if configured("QWEN_HOME") {
                insert("QWEN_HOME", qwen_global_dir())?;
            }
            insert("QWEN_RUNTIME_DIR", qwen_runtime_dir())?;
        }
        Agent::OpenCode if configured("XDG_DATA_HOME") => insert(
            "XDG_DATA_HOME",
            opencode_data_dir().map(|path| path.parent().expect("opencode suffix").to_path_buf()),
        )?,
        Agent::Pi => {
            if configured("PI_CODING_AGENT_DIR") {
                insert(
                    "PI_CODING_AGENT_DIR",
                    std::env::var_os("PI_CODING_AGENT_DIR")
                        .map(PathBuf::from)
                        .map(resolve_configured_path)
                        .expect("configured environment"),
                )?;
            }
            if configured("PI_CODING_AGENT_SESSION_DIR") {
                insert("PI_CODING_AGENT_SESSION_DIR", pi_sessions_dir())?;
            }
        }
        Agent::Kiro if configured("KIRO_HOME") => {
            insert(
                "KIRO_HOME",
                kiro_sessions_dir().map(|path| {
                    path.parent()
                        .and_then(std::path::Path::parent)
                        .expect("sessions/cli suffix")
                        .to_path_buf()
                }),
            )?;
        }
        Agent::Gemini if configured("GEMINI_CLI_HOME") => insert(
            "GEMINI_CLI_HOME",
            gemini_dir().map(|path| path.parent().expect(".gemini suffix").to_path_buf()),
        )?,
        Agent::Hermes if configured("HERMES_HOME") => insert("HERMES_HOME", hermes_dir())?,
        Agent::PrimeAgent => {
            if configured("PRIME_AGENT_CODING_AGENT_DIR") {
                insert("PRIME_AGENT_CODING_AGENT_DIR", prime_agent_dir())?;
            }
            insert("PRIME_AGENT_SESSION_DIR", prime_sessions_dir())?;
        }
        _ => {}
    }
    Ok(environment)
}

pub fn is_program_installed(name: &str) -> bool {
    resolved_program(name).is_some()
}

fn resolved_program(name: &str) -> Option<PathBuf> {
    let path = std::path::Path::new(name);
    if path.is_absolute() || path.components().count() > 1 {
        let path = std::path::absolute(path).ok()?;
        if cfg!(windows) {
            let extensions = windows_pathext();
            if extensions
                .iter()
                .any(|ext| name.to_lowercase().ends_with(ext))
                && is_executable_path(&path)
            {
                return Some(path);
            }
            return extensions
                .iter()
                .map(|ext| PathBuf::from(format!("{}{ext}", path.display())))
                .find(|path| is_executable_path(path));
        }
        return is_executable_path(&path).then_some(path);
    }
    let execs = path_executables();
    if cfg!(windows) {
        execs
            .get(&name.to_lowercase())
            .filter(|path| path.to_str().is_some())
            .cloned()
    } else {
        execs
            .get(name)
            .filter(|path| path.to_str().is_some())
            .cloned()
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

    struct CompatFixture(PathBuf);

    impl CompatFixture {
        fn new() -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "agf-compat-{}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            for dir in [
                "home",
                "cwd",
                "cwd/bin",
                "codex",
                "sqlite",
                "ignored-sqlite",
                "claude",
                "local",
                "roaming",
            ] {
                std::fs::create_dir_all(root.join(dir)).unwrap();
            }
            Self(root.canonicalize().unwrap())
        }

        fn child(&self, case: &str) -> std::process::Command {
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    "config::tests::compat_roots_child",
                    "--nocapture",
                ])
                .env_clear()
                .env("AGF_COMPAT_ROOT", &self.0)
                .env("AGF_COMPAT_CASE", case)
                .env("AGF_SHELL", "posix")
                .env("HOME", self.0.join("home"))
                .env("USERPROFILE", self.0.join("home"))
                .env("APPDATA", self.0.join("roaming"))
                .env("LOCALAPPDATA", self.0.join("local"))
                .env("PATH", self.0.join("cwd/bin"))
                .current_dir(self.0.join("cwd"));
            for name in ["SystemRoot", "SystemDrive"] {
                if let Some(value) = std::env::var_os(name) {
                    command.env(name, value);
                }
            }
            command
        }
    }

    impl Drop for CompatFixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn run_child(command: &mut std::process::Command) {
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// Windows current_dir() and canonicalize() can spell the same absolute
    /// path differently. Compare existing fixture identities, not prefixes.
    fn assert_same_existing_path(
        actual: impl AsRef<std::path::Path>,
        expected: impl AsRef<std::path::Path>,
    ) {
        let actual = actual.as_ref();
        let expected = expected.as_ref();
        assert!(actual.is_absolute(), "not absolute: {}", actual.display());
        assert!(
            expected.is_absolute(),
            "not absolute: {}",
            expected.display()
        );
        assert_eq!(
            actual.canonicalize().unwrap(),
            expected.canonicalize().unwrap()
        );
    }

    #[test]
    fn compat_fixture_paths_compare_native_and_canonical_spellings() {
        let fixture = CompatFixture::new();
        let native = std::env::temp_dir().join(fixture.0.file_name().unwrap());
        assert_same_existing_path(native.join("cwd"), fixture.0.join("cwd"));
        synthetic_executable(&native.join("cwd/fixture-file"));
        assert_same_existing_path(
            native.join("cwd/fixture-file"),
            fixture.0.join("cwd/fixture-file"),
        );
    }

    #[test]
    fn compat_codex_config_precedence_and_relative_bases() {
        let fixture = CompatFixture::new();
        let home = fixture.0.join("codex");
        let cwd = fixture.0.join("cwd");
        let env = Some(PathBuf::from("env state"));
        assert_eq!(codex_sqlite_dir_from(&home, None, &cwd).unwrap(), home);
        assert_eq!(
            codex_sqlite_dir_from(&home, env.clone(), &cwd).unwrap(),
            cwd.join("env state")
        );
        assert_eq!(
            codex_sqlite_dir_from(&home, Some(PathBuf::from("nonexistent/../env state")), &cwd)
                .unwrap(),
            cwd.join("env state")
        );
        std::fs::write(
            home.join("config.toml"),
            "sqlite_home = 'state \u{c800}\u{c7a5}'\n",
        )
        .unwrap();
        assert_eq!(
            codex_sqlite_dir_from(&home, env, &cwd).unwrap(),
            home.join("state \u{c800}\u{c7a5}")
        );
        std::fs::write(home.join("config.toml"), "sqlite_home = 123\n").unwrap();
        assert!(codex_sqlite_dir_from(&home, Some(fixture.0.join("sqlite")), &cwd).is_err());
        std::fs::write(home.join("config.toml"), "sqlite_home = [\n").unwrap();
        assert!(codex_sqlite_dir_from(&home, None, &cwd).is_err());
        std::fs::write(home.join("config.toml"), "#".repeat(1024 * 1024 + 1)).unwrap();
        assert!(codex_sqlite_dir_from(&home, None, &cwd).is_err());
    }

    #[test]
    fn compat_codex_explicit_home_must_exist_and_be_directory() {
        let fixture = CompatFixture::new();
        let home = fixture.0.join("home");
        assert_eq!(codex_dir_from(None, &home).unwrap(), home.join(".codex"));
        assert_eq!(
            codex_dir_from(Some(PathBuf::new()), &home).unwrap(),
            home.join(".codex")
        );
        assert_eq!(
            codex_dir_from(Some(fixture.0.join("codex")), &home).unwrap(),
            fixture.0.join("codex")
        );
        let missing = fixture.0.join("missing");
        assert!(codex_dir_from(Some(missing.clone()), &home).is_err());
        assert!(!missing.exists());
        std::fs::write(&missing, b"file").unwrap();
        assert!(codex_dir_from(Some(missing), &home).is_err());
    }

    #[test]
    fn compat_codex_parse_errors_never_echo_secret_fields_or_values() {
        let fixture = CompatFixture::new();
        let home = fixture.0.join("codex");
        for content in [
            "private_token = 'fixture-secret-never-disclose'\nsqlite_home = [\n",
            "sqlite_home = { private_token = 'fixture-secret-never-disclose' }\n",
        ] {
            std::fs::write(home.join("config.toml"), content).unwrap();
            let error = codex_sqlite_dir_from(&home, None, &fixture.0.join("cwd")).unwrap_err();
            for rendered in [error.to_string(), format!("{error:?}")] {
                assert!(rendered.contains("invalid TOML storage configuration"));
                assert!(!rendered.contains("fixture-secret-never-disclose"));
                assert!(!rendered.contains("private_token"));
            }
        }
    }

    fn synthetic_executable(path: &std::path::Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"synthetic executable; never run").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
    }

    #[test]
    fn compat_executables_keep_first_relative_path_match_across_resume_cwds() {
        let fixture = CompatFixture::new();
        let root = &fixture.0;
        for name in ["claude", "kiro-cli", "agent"] {
            let name = if cfg!(windows) {
                format!("{name}.exe")
            } else {
                name.to_string()
            };
            for dir in ["cwd/first's bin", "later bin", "resume project/first's bin"] {
                synthetic_executable(&root.join(dir).join(&name));
            }
            // A directory with an executable-looking name is not executable.
            std::fs::create_dir_all(root.join("cwd/non-executable").join(&name)).unwrap();
        }
        let path = std::env::join_paths([
            PathBuf::from("non-executable"),
            PathBuf::from("first's bin"),
            root.join("later bin"),
        ])
        .unwrap();
        run_child(
            fixture
                .child("relative-executable")
                .env("PATH", &path)
                .env("CLAUDE_CONFIG_DIR", "../claude")
                .env("AGF_CURSOR_CLI", "agent"),
        );
        run_child(
            fixture
                .child("relative-executable")
                .env("PATH", path)
                .env("CLAUDE_CONFIG_DIR", "../claude")
                .env(
                    "AGF_CURSOR_CLI",
                    if cfg!(windows) {
                        "first's bin/agent.exe"
                    } else {
                        "first's bin/agent"
                    },
                ),
        );
    }

    #[test]
    fn compat_path_lookup_preserves_empty_components_and_executable_checks() {
        let fixture = CompatFixture::new();
        let cwd = fixture.0.join("cwd");
        let filename = if cfg!(windows) {
            "cwd-only.exe"
        } else {
            "cwd-only"
        };
        synthetic_executable(&cwd.join(filename));
        let map =
            collect_path_executables(Some(std::ffi::OsStr::new("")), &cwd, &windows_pathext());
        assert_same_existing_path(map.get("cwd-only").unwrap(), cwd.join(filename));
        assert!(collect_path_executables(None, &cwd, &windows_pathext()).is_empty());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(cwd.join(filename), std::fs::Permissions::from_mode(0o600))
                .unwrap();
            assert!(collect_path_executables(Some(std::ffi::OsStr::new("")), &cwd, &[]).is_empty());
        }
    }

    #[cfg(windows)]
    #[test]
    fn compat_path_lookup_respects_pathext_precedence() {
        let fixture = CompatFixture::new();
        let cwd = fixture.0.join("cwd");
        synthetic_executable(&cwd.join("agent.EXE"));
        synthetic_executable(&cwd.join("agent.CMD"));
        let map = collect_path_executables(
            Some(std::ffi::OsStr::new("")),
            &cwd,
            &[".cmd".into(), ".exe".into()],
        );
        assert_same_existing_path(map.get("agent").unwrap(), cwd.join("agent.CMD"));
        assert_same_existing_path(map.get("agent.exe").unwrap(), cwd.join("agent.EXE"));
    }

    fn assert_compat_relative_executable(root: &std::path::Path) {
        use crate::model::{Action, Session};
        use crate::shell::CommandShell;
        for (agent, name) in [
            (Agent::ClaudeCode, "claude"),
            (Agent::CursorAgent, "agent"),
            (Agent::Kiro, "kiro-cli"),
        ] {
            let filename = if cfg!(windows) {
                format!("{name}.exe")
            } else {
                name.to_string()
            };
            let expected = root.join("cwd/first's bin").join(&filename);
            assert!(
                root.join("resume project/first's bin")
                    .join(&filename)
                    .is_file()
            );
            let session = Session {
                agent,
                session_id: "id'quoted".into(),
                project_name: "p".into(),
                project_path: root.join("resume project").to_string_lossy().into_owned(),
                summaries: Vec::new(),
                timestamp: 0,
                git_branch: None,
                worktree: None,
                recap: None,
                interactive: true,
            };
            let plan = crate::action::resume_plan(&session, None).unwrap();
            assert!(plan.executable_found);
            assert!(std::path::Path::new(&plan.program).is_file());
            assert_same_existing_path(&plan.program, &expected);
            assert_eq!(plan.cwd.as_deref(), Some(session.project_path.as_str()));
            let quoted = CommandShell::Posix.quote(&plan.program);
            for command in [
                agent.resume_cmd(&session.session_id, &CommandShell::Posix),
                agent.new_session_command(&CommandShell::Posix),
                crate::action::generate_command(&session, Action::Resume, None).unwrap(),
                crate::action::generate_command(&session, Action::NewSession, None).unwrap(),
                crate::action::resume_with_flags(&session, ""),
                crate::action::new_session_with_flags(&session, agent, ""),
            ] {
                assert!(
                    command.contains(&quoted),
                    "executable not frozen: {command}"
                );
            }
            let ps_program = format!("& {}", CommandShell::PowerShell.quote(&plan.program));
            assert!(
                agent
                    .resume_cmd(&session.session_id, &CommandShell::PowerShell)
                    .starts_with(&ps_program)
            );
            assert!(
                agent
                    .new_session_command(&CommandShell::PowerShell)
                    .starts_with(&ps_program)
            );
            if agent == Agent::Kiro {
                assert!(
                    agent
                        .new_session_command(&CommandShell::Posix)
                        .ends_with(" chat")
                );
            }
        }
        assert_eq!(agent_program(Agent::Yolop), "yolop");
        assert!(!is_agent_installed(Agent::Yolop));
    }

    #[test]
    fn compat_relative_env_roots_and_fingerprints_are_consistent() {
        let fixture = CompatFixture::new();
        for directory in [
            "cwd/pi sessions",
            "cwd/hermes custom",
            "cwd/gemini home/.gemini",
        ] {
            std::fs::create_dir_all(fixture.0.join(directory)).unwrap();
        }
        std::fs::write(fixture.0.join("sqlite/state_9.sqlite"), b"fixture").unwrap();
        std::fs::write(fixture.0.join("sqlite/state_9.sqlite-wal"), b"fixture wal").unwrap();
        run_child(
            fixture
                .child("roots")
                .env("CODEX_HOME", "../codex")
                .env("CODEX_SQLITE_HOME", " ../sqlite ")
                .env("CLAUDE_CONFIG_DIR", "../claude")
                .env("PI_CODING_AGENT_DIR", "pi custom")
                .env("PI_CODING_AGENT_SESSION_DIR", "pi sessions")
                .env("HERMES_HOME", " hermes custom ")
                .env("GEMINI_CLI_HOME", "gemini home"),
        );
    }

    #[test]
    fn compat_cursor_never_selects_an_unrelated_agent_implicitly() {
        let fixture = CompatFixture::new();
        let executable =
            fixture
                .0
                .join("cwd/bin")
                .join(if cfg!(windows) { "agent.exe" } else { "agent" });
        std::fs::write(&executable, b"unrelated program - must not be run").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        run_child(&mut fixture.child("cursor-default"));
        run_child(
            fixture
                .child("cursor-explicit")
                .env("AGF_CURSOR_CLI", executable),
        );
        run_child(
            fixture
                .child("cursor-missing")
                .env("AGF_CURSOR_CLI", "missing-cursor"),
        );
    }

    #[test]
    fn compat_cursor_program_override_is_one_literal_executable() {
        let cwd = std::path::Path::new("/fixture");
        assert_eq!(
            agent_program_from(Agent::CursorAgent, None, Some(cwd)),
            "cursor-agent"
        );
        assert_eq!(
            agent_program_from(Agent::CursorAgent, Some(""), Some(cwd)),
            "cursor-agent"
        );
        assert_eq!(
            agent_program_from(Agent::Codex, Some("agent"), Some(cwd)),
            "codex"
        );
        assert_eq!(
            agent_program_from(Agent::CursorAgent, Some("agent --print nope"), Some(cwd)),
            "agent --print nope"
        );
        assert_eq!(
            agent_program_from(Agent::CursorAgent, Some("bin/Cursor's agent"), Some(cwd)),
            cwd.join("bin/Cursor's agent").to_string_lossy()
        );
    }

    fn seed_compat_codex_db(
        path: &std::path::Path,
        rollout_root: &std::path::Path,
        wal: bool,
    ) -> rusqlite::Connection {
        let connection = rusqlite::Connection::open(path).unwrap();
        if wal {
            connection
                .execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;")
                .unwrap();
        }
        connection.execute_batch("CREATE TABLE threads (id TEXT PRIMARY KEY, cwd TEXT, title TEXT, updated_at INTEGER, git_branch TEXT, first_user_message TEXT, archived INTEGER, rollout_path TEXT);").unwrap();
        for id in ["selected-id", "keep-id"] {
            let rollout = rollout_root.join(format!("rollout-{id}.jsonl"));
            connection.execute("INSERT INTO threads VALUES (?1, ?2, 'fixture', 1785542400, NULL, 'fixture', 0, ?3)", rusqlite::params![id, rollout_root.to_string_lossy(), rollout.to_string_lossy()]).unwrap();
        }
        connection
    }

    #[test]
    fn compat_scans_read_only_and_delete_only_configured_roots_with_wal() {
        let fixture = CompatFixture::new();
        let root = &fixture.0;
        let codex = root.join("codex \u{c800}\u{c7a5}");
        let claude = root.join("claude \u{c800}\u{c7a5}");
        std::fs::create_dir_all(codex.join("sessions")).unwrap();
        std::fs::create_dir_all(claude.join("projects/fixture")).unwrap();
        std::fs::create_dir_all(root.join("home/.codex")).unwrap();
        std::fs::create_dir_all(root.join("home/.claude/projects/fixture")).unwrap();
        std::fs::write(codex.join("config.toml"), "sqlite_home = '../sqlite'\n").unwrap();
        let mut claude_history = String::new();
        for id in ["selected-id", "keep-id"] {
            std::fs::write(
                codex.join("sessions").join(format!("rollout-{id}.jsonl")),
                format!("{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{id}\"}}}}\n"),
            )
            .unwrap();
            let transcript =
                serde_json::json!({"type":"user", "cwd":root.join("cwd"), "sessionId":id});
            std::fs::write(
                claude.join("projects/fixture").join(format!("{id}.jsonl")),
                format!("{transcript}\n"),
            )
            .unwrap();
            std::fs::write(
                root.join("home/.claude/projects/fixture")
                    .join(format!("{id}.jsonl")),
                b"default home untouched",
            )
            .unwrap();
            let history = serde_json::json!({"sessionId":id, "project":root.join("cwd"), "timestamp":1785542400000_i64, "display":"fixture prompt"});
            claude_history.push_str(&format!("{history}\n"));
        }
        std::fs::write(claude.join("history.jsonl"), claude_history).unwrap();
        std::fs::write(
            codex.join("history.jsonl"),
            "{\"session_id\":\"selected-id\",\"ts\":1785542400,\"text\":\"selected prompt\"}\n",
        )
        .unwrap();
        let writer = seed_compat_codex_db(
            &root.join("sqlite/state_9.sqlite"),
            &codex.join("sessions"),
            true,
        );
        for decoy in [
            root.join("home/.codex/state_9.sqlite"),
            codex.join("state_9.sqlite"),
            root.join("ignored-sqlite/state_9.sqlite"),
        ] {
            drop(seed_compat_codex_db(&decoy, &codex.join("sessions"), false));
        }
        run_child(
            fixture
                .child("scan-delete")
                .env("CODEX_HOME", &codex)
                .env("CODEX_SQLITE_HOME", root.join("ignored-sqlite"))
                .env("CLAUDE_CONFIG_DIR", &claude),
        );
        let count: i64 = writer
            .query_row("SELECT count(*) FROM threads", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
        drop(writer);
    }

    #[test]
    fn compat_resume_freezes_relative_roots_before_changing_cwd() {
        let fixture = CompatFixture::new();
        let root = &fixture.0;
        for path in [
            "codex root \u{c800}\u{c7a5}",
            "claude root's \u{c800}\u{c7a5}",
            "qwen root",
            "cwd/.qwen",
            "cwd/other project \u{c791}\u{c5c5}",
            "cwd/runtime's store",
        ] {
            std::fs::create_dir_all(root.join(path)).unwrap();
        }
        std::fs::write(
            root.join("codex root \u{c800}\u{c7a5}/config.toml"),
            "sqlite_home = '../sqlite'\n",
        )
        .unwrap();
        std::fs::write(
            root.join("cwd/.qwen/settings.json"),
            r#"{"advanced":{"runtimeOutputDir":"runtime's store"}}"#,
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for name in ["codex", "claude", "qwen"] {
                let executable = root.join("cwd/bin").join(name);
                std::fs::write(&executable, "#!/bin/sh\nprintf '%s\\n' \"$PWD\" \"$CODEX_HOME\" \"$CODEX_SQLITE_HOME\" \"$CLAUDE_CONFIG_DIR\" \"$QWEN_HOME\" \"$QWEN_RUNTIME_DIR\" \"$@\"\n").unwrap();
                std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))
                    .unwrap();
            }
        }
        let mut command = fixture.child("rebase");
        command
            .env("CODEX_HOME", "../codex root \u{c800}\u{c7a5}")
            .env("CODEX_SQLITE_HOME", "../ignored-sqlite")
            .env("CLAUDE_CONFIG_DIR", "../claude root's \u{c800}\u{c7a5}")
            .env("QWEN_HOME", "../qwen root")
            .env("ANTHROPIC_API_KEY", "fixture-secret-never-in-plan")
            .env("OPENAI_API_KEY", "fixture-secret-never-in-plan");
        #[cfg(unix)]
        command.env(
            "PATH",
            std::env::join_paths([
                root.join("cwd/bin"),
                PathBuf::from("/usr/bin"),
                PathBuf::from("/bin"),
            ])
            .unwrap(),
        );
        run_child(&mut command);
    }

    fn assert_compat_rebase(root: &std::path::Path) {
        use crate::model::{Action, Session};
        use crate::shell::CommandShell;
        let cases: &[(Agent, &[&str])] = &[
            (Agent::Codex, &["CODEX_HOME", "CODEX_SQLITE_HOME"]),
            (Agent::ClaudeCode, &["CLAUDE_CONFIG_DIR"]),
            (Agent::Qwen, &["QWEN_HOME", "QWEN_RUNTIME_DIR"]),
        ];
        let cwd = root.join("cwd/other project \u{c791}\u{c5c5}");
        for &(agent, keys) in cases {
            let session = Session {
                agent,
                session_id: "id'$(not-executed)".into(),
                project_name: "p".into(),
                project_path: cwd.to_string_lossy().into_owned(),
                summaries: Vec::new(),
                timestamp: 0,
                git_branch: None,
                worktree: None,
                recap: None,
                interactive: true,
            };
            let original: Vec<_> = keys.iter().map(std::env::var_os).collect();
            let plan = crate::action::resume_plan(&session, None).unwrap();
            assert_eq!(
                plan.env.keys().map(String::as_str).collect::<Vec<_>>(),
                keys
            );
            assert!(
                plan.env
                    .values()
                    .all(|value| std::path::Path::new(value).is_absolute())
            );
            assert!(
                !serde_json::to_string(&plan)
                    .unwrap()
                    .contains("fixture-secret")
            );
            assert_eq!(plan.cwd.as_deref(), Some(session.project_path.as_str()));
            match agent {
                Agent::Codex => {
                    assert_same_existing_path(
                        &plan.env["CODEX_HOME"],
                        root.join("codex root \u{c800}\u{c7a5}"),
                    );
                    assert_same_existing_path(&plan.env["CODEX_SQLITE_HOME"], root.join("sqlite"));
                }
                Agent::ClaudeCode => assert_same_existing_path(
                    &plan.env["CLAUDE_CONFIG_DIR"],
                    root.join("claude root's \u{c800}\u{c7a5}"),
                ),
                Agent::Qwen => {
                    assert_same_existing_path(&plan.env["QWEN_HOME"], root.join("qwen root"));
                    assert_same_existing_path(
                        &plan.env["QWEN_RUNTIME_DIR"],
                        root.join("cwd/runtime's store"),
                    );
                }
                _ => unreachable!(),
            }
            let command = crate::action::generate_command(&session, Action::Resume, None).unwrap();
            assert!(command.contains(" command "));
            let with_flags = crate::action::resume_with_flags(&session, "");
            assert_eq!(command, with_flags);
            let new_command =
                crate::action::generate_command(&session, Action::NewSession, None).unwrap();
            assert_eq!(
                new_command,
                crate::action::new_session_with_flags(&session, agent, "")
            );
            for (key, value) in &plan.env {
                assert!(command.contains(&format!("{key}={}", CommandShell::Posix.quote(value))));
                assert!(
                    new_command.contains(&format!("{key}={}", CommandShell::Posix.quote(value)))
                );
            }
            #[cfg(unix)]
            {
                let direct = std::process::Command::new(&plan.program)
                    .args(&plan.args)
                    .envs(&plan.env)
                    .current_dir(&cwd)
                    .output()
                    .unwrap();
                assert!(direct.status.success());
                for shell_command in [command, with_flags] {
                    let output = std::process::Command::new("/bin/sh")
                        .args(["-c", &shell_command])
                        .output()
                        .unwrap();
                    assert!(
                        output.status.success(),
                        "{}",
                        String::from_utf8_lossy(&output.stderr)
                    );
                    assert_eq!(output.stdout, direct.stdout);
                }
                let output = String::from_utf8(direct.stdout).unwrap();
                assert_eq!(output.lines().next(), Some(session.project_path.as_str()));
                assert_eq!(output.lines().last(), Some(session.session_id.as_str()));
                let key = keys[0];
                let scoped = CommandShell::Posix.with_environment("true", &plan.env);
                let check = format!("{scoped}; printf '%s' \"${key}\"");
                let output = std::process::Command::new("/bin/sh")
                    .args(["-c", &check])
                    .output()
                    .unwrap();
                assert!(output.status.success());
                assert_eq!(output.stdout, std::env::var(key).unwrap().as_bytes());
            }
            assert_eq!(
                keys.iter().map(std::env::var_os).collect::<Vec<_>>(),
                original
            );
        }
    }

    fn assert_compat_scan_delete(root: &std::path::Path) {
        let codex = codex_dir().unwrap();
        let claude = claude_dir().unwrap();
        let db = root.join("sqlite/state_9.sqlite");
        let wal = root.join("sqlite/state_9.sqlite-wal");
        let read_paths = [
            db.clone(),
            wal,
            codex.join("history.jsonl"),
            claude.join("history.jsonl"),
        ];
        let before: Vec<_> = read_paths
            .iter()
            .map(|path| {
                (
                    std::fs::read(path).unwrap(),
                    std::fs::metadata(path).unwrap().modified().unwrap(),
                )
            })
            .collect();
        let codex_sessions = crate::scanner::codex::scan().unwrap();
        let claude_sessions = crate::scanner::claude::scan().unwrap();
        assert_eq!(codex_sessions.len(), 2);
        assert_eq!(claude_sessions.len(), 2);
        for (path, snapshot) in read_paths.iter().zip(&before) {
            assert_eq!(
                &(
                    std::fs::read(path).unwrap(),
                    std::fs::metadata(path).unwrap().modified().unwrap()
                ),
                snapshot,
                "scan changed {}",
                path.display()
            );
        }
        let sources = data_sources(Agent::Codex);
        assert!(
            sources
                .iter()
                .any(|path| path.canonicalize().ok() == Some(db.clone()))
        );
        assert!(!sources.contains(&codex.join("state_9.sqlite")));
        for sessions in [codex_sessions, claude_sessions] {
            let selected = sessions
                .iter()
                .find(|session| session.session_id == "selected-id")
                .unwrap();
            assert_same_existing_path(
                &selected.project_path,
                if selected.agent == Agent::Codex {
                    codex.join("sessions")
                } else {
                    root.join("cwd")
                },
            );
            crate::delete::delete_session(selected).unwrap();
        }
        assert!(!codex.join("sessions/rollout-selected-id.jsonl").exists());
        assert!(codex.join("sessions/rollout-keep-id.jsonl").exists());
        assert!(!claude.join("projects/fixture/selected-id.jsonl").exists());
        assert!(claude.join("projects/fixture/keep-id.jsonl").exists());
        assert!(
            root.join("home/.claude/projects/fixture/selected-id.jsonl")
                .exists()
        );
        for decoy in [
            root.join("home/.codex/state_9.sqlite"),
            codex.join("state_9.sqlite"),
            root.join("ignored-sqlite/state_9.sqlite"),
        ] {
            let connection = rusqlite::Connection::open_with_flags(
                decoy,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .unwrap();
            let count: i64 = connection
                .query_row("SELECT count(*) FROM threads", [], |row| row.get(0))
                .unwrap();
            assert_eq!(count, 2);
        }
        assert_eq!(
            crate::scanner::codex::scan().unwrap()[0].session_id,
            "keep-id"
        );
        assert_eq!(
            crate::scanner::claude::scan().unwrap()[0].session_id,
            "keep-id"
        );
        assert_eq!(
            std::fs::read(codex.join("history.jsonl")).unwrap(),
            before[2].0
        );
        assert_eq!(
            std::fs::read(claude.join("history.jsonl")).unwrap(),
            before[3].0
        );
        let remaining = crate::scanner::codex::scan().unwrap().remove(0);
        std::fs::write(codex.join("config.toml"), "sqlite_home = [\n").unwrap();
        assert!(crate::scanner::codex::scan().is_err());
        assert!(crate::delete::delete_session(&remaining).is_err());
        assert!(codex.join("sessions/rollout-keep-id.jsonl").exists());
    }

    #[test]
    fn compat_roots_child() {
        let Ok(case) = std::env::var("AGF_COMPAT_CASE") else {
            return;
        };
        let root = PathBuf::from(std::env::var_os("AGF_COMPAT_ROOT").unwrap());
        match case.as_str() {
            "relative-executable" => assert_compat_relative_executable(&root),
            "scan-delete" => assert_compat_scan_delete(&root),
            "rebase" => assert_compat_rebase(&root),
            "roots" => {
                assert_same_existing_path(codex_dir().unwrap(), root.join("codex"));
                assert_same_existing_path(codex_sqlite_dir().unwrap(), root.join("sqlite"));
                assert_same_existing_path(claude_dir().unwrap(), root.join("claude"));
                let sources = data_sources(Agent::Codex);
                assert!(sources.contains(&root.join("codex/config.toml")));
                for filename in ["state_9.sqlite", "state_9.sqlite-wal"] {
                    assert!(
                        sources.iter().any(|path| path.canonicalize().ok()
                            == Some(root.join("sqlite").join(filename)))
                    );
                }
                assert!(sources.contains(&root.join("codex/sessions")));
                assert_same_existing_path(pi_sessions_dir().unwrap(), root.join("cwd/pi sessions"));
                assert_same_existing_path(hermes_dir().unwrap(), root.join("cwd/hermes custom"));
                assert_same_existing_path(
                    gemini_dir().unwrap(),
                    root.join("cwd/gemini home/.gemini"),
                );
                let pi_sources = data_sources(Agent::Pi);
                assert_eq!(pi_sources.len(), 1);
                assert_same_existing_path(&pi_sources[0], root.join("cwd/pi sessions"));
            }
            "cursor-default" => {
                assert!(is_program_installed("agent"));
                assert!(!is_agent_installed(Agent::CursorAgent));
                assert_eq!(agent_program(Agent::CursorAgent), "cursor-agent");
            }
            "cursor-explicit" | "cursor-missing" => {
                use crate::model::Session;
                let session = Session {
                    agent: Agent::CursorAgent,
                    session_id: "id'$(nope)".into(),
                    project_name: "p".into(),
                    project_path: String::new(),
                    summaries: Vec::new(),
                    timestamp: 0,
                    git_branch: None,
                    worktree: None,
                    recap: None,
                    interactive: true,
                };
                let plan = crate::action::resume_plan(&session, None).unwrap();
                assert_eq!(plan.executable_found, case == "cursor-explicit");
                assert_eq!(
                    plan.executable_found,
                    is_agent_installed(Agent::CursorAgent)
                );
                assert_eq!(plan.args, ["--resume", "id'$(nope)"]);
                if plan.executable_found {
                    assert_same_existing_path(
                        &plan.program,
                        std::env::var("AGF_CURSOR_CLI").unwrap(),
                    );
                } else {
                    assert_eq!(plan.program, std::env::var("AGF_CURSOR_CLI").unwrap());
                }
                assert!(plan.cwd.is_none());
                assert_eq!(
                    session
                        .agent
                        .resume_cmd(&session.session_id, &crate::shell::CommandShell::Posix),
                    format!(
                        "{} --resume {}",
                        crate::shell::CommandShell::Posix.quote(&plan.program),
                        crate::shell::CommandShell::Posix.quote(&session.session_id)
                    )
                );
            }
            _ => panic!("unexpected fixture case"),
        }
    }

    #[test]
    fn qwen_workspace_runtime_dir_overrides_user_and_resolves_from_cwd() {
        let root = std::env::temp_dir().join(format!("agf-qwen-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let global = root.join("global");
        let cwd = root.join("project");
        std::fs::create_dir_all(cwd.join(".qwen")).unwrap();
        std::fs::create_dir_all(&global).unwrap();
        std::fs::write(
            global.join("settings.json"),
            r#"{"advanced":{"runtimeOutputDir":"user-runtime"}}"#,
        )
        .unwrap();
        std::fs::write(
            cwd.join(".qwen/settings.json"),
            r#"{"advanced":{"runtimeOutputDir":"workspace-runtime"}}"#,
        )
        .unwrap();

        let resolved = qwen_runtime_dir_from_settings(&global, &cwd).unwrap();
        assert_eq!(resolved, cwd.join("workspace-runtime"));
        let _ = std::fs::remove_dir_all(root);
    }

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
