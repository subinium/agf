use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::model::{Agent, Session};
use crate::plugin;

// Since v0.12.0 the cache also stores `agf_version` (the writing binary's
// version); any mismatch on read forces a full rescan (issue #37). That makes
// per-release CACHE_VERSION bumps unnecessary for scanner-behavior changes —
// CACHE_VERSION only needs bumping for schema/interpretation changes that can
// ship *within* one package version (e.g. successive dev builds).
//
// Bumped to 6 in v0.11.3:
// - Cursor scanner now (a) walks both depth-3 .txt and depth-4 .jsonl layouts,
//   (b) reads chat metadata from the `meta` table of store.db, and (c) drops
//   .jsonl transcripts that have no matching store.db (orphans `cursor-agent`
//   itself refuses to resume). Cache entries written by 0.11.x would surface
//   the old orphan-laden list until each transcript's mtime happened to
//   change. Bumping the version forces a one-time rescan on upgrade so the
//   PR description's "35 orphans -> 0" claim actually holds for upgraders.
//
// Bumped to 5 after v0.11.1:
// - Pi scanner now keeps all user-message summaries instead of only the first
//   one, matching the History preview behavior of other agents.
//
// Bumped to 4 after v0.11.1:
// - Pi scanner now extracts first-user-message summaries from JSONL session
//   logs. Older cache entries stored empty Pi summaries with fresh mtimes, so
//   they would otherwise keep rendering only the project name until a source
//   file changed.
//
// Bumped to 3 in v0.11.0:
// - Hermes Agent registered (#34) so the per-agent cache map gains a new key.
// - Hermes session payloads now carry first-user-message previews (only on
//   CLI/TUI session ids) instead of bare source/model fallbacks; older cache
//   entries written by 0.10.x would surface as stale "cli session (...)"
//   summaries until the source DB mtime happens to change.
//   Bumping the version forces a one-time rescan on first 0.11.0 launch.
// Bumped to 7 after v0.12.0:
// - Yolop support adds a new per-agent cache key.
// - Yolop worktree sessions use repo_root for the project name instead of the
//   generated session directory. Local builds share the released 0.12.0
//   package version, so agf_version alone cannot invalidate their old entries.
// - Older nested Yolop worktree sessions recover the original repository name
//   through their `.git` indirection.
// - Deleted nested worktrees recover it from their parent session metadata.
const CACHE_VERSION: u32 = 7;

/// The binary version stamped into every cache write; any mismatch on read
/// invalidates the whole cache (see `parse_cache`).
const AGF_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Serialize, Deserialize)]
struct CacheFile {
    version: u32,
    /// `CARGO_PKG_VERSION` of the binary that wrote the file. Defaults to ""
    /// for caches written before this field existed, which fails the version
    /// match in `parse_cache` and forces the rescan we want.
    #[serde(default)]
    agf_version: String,
    agents: HashMap<Agent, AgentCache>,
}

#[derive(Debug, Serialize, Deserialize)]
struct AgentCache {
    mtime: u64, // Unix seconds of data source last modification
    sessions: Vec<CachedSession>,
}

#[derive(Debug, Serialize, Deserialize)]
struct CachedSession {
    agent: Agent,
    session_id: String,
    project_name: String,
    project_path: String,
    summaries: Vec<String>,
    timestamp: i64,
    git_branch: Option<String>,
    worktree: Option<String>,
    recap: Option<String>,
}

fn cache_path() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".cache"))
        .join("agf")
        .join("sessions.json")
}

/// True when the `AGF_DEBUG` env var is set (any value), probed once per process.
fn debug_enabled() -> bool {
    static CACHE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *CACHE.get_or_init(|| std::env::var("AGF_DEBUG").is_ok())
}

fn to_cached(s: &Session) -> CachedSession {
    CachedSession {
        agent: s.agent,
        session_id: s.session_id.clone(),
        project_name: s.project_name.clone(),
        project_path: s.project_path.clone(),
        summaries: s.summaries.iter().take(10).cloned().collect(),
        timestamp: s.timestamp,
        git_branch: s.git_branch.clone(),
        worktree: s.worktree.clone(),
        recap: s.recap.clone(),
    }
}

fn from_cached(c: &CachedSession) -> Session {
    Session {
        agent: c.agent,
        session_id: c.session_id.clone(),
        project_name: c.project_name.clone(),
        project_path: c.project_path.clone(),
        summaries: c.summaries.clone(),
        timestamp: c.timestamp,
        git_branch: c.git_branch.clone(),
        worktree: c.worktree.clone(),
        recap: c.recap.clone(),
    }
}

fn get_max_mtime(paths: &[PathBuf]) -> u64 {
    use walkdir::WalkDir;
    let mut max = 0u64;
    for p in paths {
        if !p.exists() {
            continue;
        }
        for entry in WalkDir::new(p)
            .max_depth(4)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if let Ok(m) = entry.metadata()
                && let Ok(t) = m.modified()
                && let Ok(d) = t.duration_since(std::time::SystemTime::UNIX_EPOCH)
            {
                max = max.max(d.as_secs());
            }
        }
    }
    max
}

/// Validate raw cache-file content. The cache is usable only when (a) it
/// parses, (b) the schema `version` matches, and (c) it was written by this
/// exact binary version. (c) exists because scanner behavior can change
/// between releases without a schema change — issue #37: mtime checks can't
/// distinguish "the data didn't change" from "the data didn't change but its
/// interpretation just did", so every upgrade pays one full rescan instead of
/// silently serving per-session payloads shaped by an older binary.
fn parse_cache(content: &str, binary_version: &str) -> Result<CacheFile, String> {
    let cache: CacheFile =
        serde_json::from_str(content).map_err(|e| format!("cache parse failed: {e}"))?;
    if cache.version != CACHE_VERSION {
        return Err(format!(
            "cache schema version {} != {}",
            cache.version, CACHE_VERSION
        ));
    }
    if cache.agf_version != binary_version {
        let by = if cache.agf_version.is_empty() {
            "an older agf (pre-agf_version)"
        } else {
            cache.agf_version.as_str()
        };
        return Err(format!("cache built by {by}, current {binary_version}"));
    }
    Ok(cache)
}

/// Load cached sessions. Returns (sessions, stale_agents).
/// stale_agents are agents whose data sources have changed since cache was written.
pub fn load_cache() -> (Vec<Session>, Vec<Agent>) {
    let path = cache_path();
    let content = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return (Vec::new(), Agent::all().to_vec()),
    };

    let cache = match parse_cache(&content, AGF_VERSION) {
        Ok(c) => c,
        Err(why) => {
            if debug_enabled() {
                eprintln!("[agf] {why} → rescanning");
            }
            return (Vec::new(), Agent::all().to_vec());
        }
    };

    let installed: std::collections::HashSet<Agent> =
        crate::config::installed_agents().into_iter().collect();
    let plugins = plugin::all_plugins();
    let mut sessions = Vec::new();
    let mut stale = Vec::new();

    for p in &plugins {
        if !installed.contains(&p.agent()) {
            continue;
        }
        let current_mtime = get_max_mtime(&p.data_sources());

        match cache.agents.get(&p.agent()) {
            Some(ac) if ac.mtime >= current_mtime && current_mtime > 0 => {
                // Cache is fresh
                sessions.extend(ac.sessions.iter().map(from_cached));
            }
            _ => {
                stale.push(p.agent());
            }
        }
    }

    sessions.sort_by_key(|s| std::cmp::Reverse(s.timestamp));
    (sessions, stale)
}

/// Write all sessions to cache, grouped by agent.
///
/// `skip_agents` lists agents whose background scan did not complete this
/// run (e.g. the user exited the TUI before the worker finished); for those
/// we preserve the prior cache entry verbatim so we don't accidentally
/// persist an empty session list with a fresh `mtime`, which would mark the
/// agent "fresh" on the next launch and hide its sessions.
pub fn write_cache(sessions: &[Session], skip_agents: &std::collections::HashSet<Agent>) {
    let path = cache_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    let installed: std::collections::HashSet<Agent> =
        crate::config::installed_agents().into_iter().collect();
    let plugins = plugin::all_plugins();
    let mut agents: HashMap<Agent, AgentCache> = HashMap::new();

    // Carry over prior cache entries for in-flight agents. `parse_cache`
    // gates this on schema AND binary version: entries written by another
    // binary are exactly the stale data issue #37 is about, so on upgrade we
    // drop them and let the next launch rescan those agents instead.
    if !skip_agents.is_empty()
        && let Ok(content) = fs::read_to_string(&path)
        && let Ok(mut prior) = parse_cache(&content, AGF_VERSION)
    {
        for skip in skip_agents {
            // `prior` is owned and dropped right after this block, so move
            // the carried-over entries out instead of cloning them.
            if let Some(entry) = prior.agents.remove(skip) {
                agents.insert(*skip, entry);
            }
        }
    }

    for p in &plugins {
        if !installed.contains(&p.agent()) {
            continue;
        }
        if skip_agents.contains(&p.agent()) {
            continue;
        }
        let agent_sessions: Vec<CachedSession> = sessions
            .iter()
            .filter(|s| s.agent == p.agent())
            .map(to_cached)
            .collect();
        let mtime = get_max_mtime(&p.data_sources());
        agents.insert(
            p.agent(),
            AgentCache {
                mtime,
                sessions: agent_sessions,
            },
        );
    }

    let cache = CacheFile {
        version: CACHE_VERSION,
        agf_version: AGF_VERSION.to_string(),
        agents,
    };

    if let Ok(json) = serde_json::to_string(&cache) {
        let tmp = path.with_extension("json.tmp");
        if fs::write(&tmp, json).is_ok() {
            let _ = fs::rename(&tmp, &path);
        }
    }
}

/// One agent's scan result, streamed back from a worker thread.
pub struct ScanResult {
    pub agent: Agent,
    pub sessions: Vec<Session>,
}

/// Spawn one worker thread per stale agent. Each worker sends its result on
/// `tx` as soon as it finishes — the TUI can ingest results progressively
/// without blocking on the slowest scanner.
///
/// The returned `JoinHandle` resolves once every worker has finished; the
/// caller can use it to know when the channel will close.
pub fn start_stale_scan(stale: &[Agent]) -> std::sync::mpsc::Receiver<ScanResult> {
    use std::sync::mpsc;
    use std::thread;
    use std::time::Instant;

    let debug = debug_enabled();
    let installed: std::collections::HashSet<Agent> =
        crate::config::installed_agents().into_iter().collect();
    let stale: Vec<Agent> = stale
        .iter()
        .copied()
        .filter(|a| installed.contains(a))
        .collect();

    let (tx, rx) = mpsc::channel();
    for agent in stale {
        let tx = tx.clone();
        thread::spawn(move || {
            let start = Instant::now();
            let sessions = match agent {
                Agent::ClaudeCode => crate::scanner::claude::scan().unwrap_or_default(),
                Agent::Codex => crate::scanner::codex::scan().unwrap_or_default(),
                Agent::OpenCode => crate::scanner::opencode::scan().unwrap_or_default(),
                Agent::Pi => crate::scanner::pi::scan().unwrap_or_default(),
                Agent::Kiro => crate::scanner::kiro::scan().unwrap_or_default(),
                Agent::CursorAgent => crate::scanner::cursor_agent::scan().unwrap_or_default(),
                Agent::Gemini => crate::scanner::gemini::scan().unwrap_or_default(),
                Agent::Hermes => crate::scanner::hermes::scan().unwrap_or_default(),
                Agent::Yolop => crate::scanner::yolop::scan().unwrap_or_default(),
            };
            if debug {
                eprintln!(
                    "[agf] {:?} scan: {} sessions in {:?}",
                    agent,
                    sessions.len(),
                    start.elapsed()
                );
            }
            // Receiver dropped (TUI exited): silently ignore.
            let _ = tx.send(ScanResult { agent, sessions });
        });
    }
    // Drop the original sender so the receiver closes once all workers finish.
    drop(tx);
    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache_json(version: u32, agf_version: Option<&str>) -> String {
        let mut v = serde_json::json!({
            "version": version,
            "agents": {}
        });
        if let Some(av) = agf_version {
            v["agf_version"] = serde_json::Value::String(av.to_string());
        }
        v.to_string()
    }

    #[test]
    fn parse_cache_accepts_matching_schema_and_binary_version() {
        let json = cache_json(CACHE_VERSION, Some(AGF_VERSION));
        assert!(parse_cache(&json, AGF_VERSION).is_ok());
    }

    #[test]
    fn parse_cache_rejects_schema_version_mismatch() {
        let json = cache_json(CACHE_VERSION + 1, Some(AGF_VERSION));
        let err = parse_cache(&json, AGF_VERSION).unwrap_err();
        assert!(err.contains("schema version"), "unexpected: {err}");
    }

    #[test]
    fn parse_cache_rejects_binary_version_mismatch() {
        let json = cache_json(CACHE_VERSION, Some("0.0.1"));
        let err = parse_cache(&json, AGF_VERSION).unwrap_err();
        assert!(err.contains("cache built by 0.0.1"), "unexpected: {err}");
    }

    #[test]
    fn parse_cache_rejects_pre_agf_version_caches() {
        // Caches written before the field existed carry no `agf_version` key;
        // the serde default "" must fail the match and force a rescan.
        let json = cache_json(CACHE_VERSION, None);
        let err = parse_cache(&json, AGF_VERSION).unwrap_err();
        assert!(err.contains("pre-agf_version"), "unexpected: {err}");
    }

    #[test]
    fn parse_cache_rejects_garbage() {
        let err = parse_cache("not json", AGF_VERSION).unwrap_err();
        assert!(err.contains("cache parse failed"), "unexpected: {err}");
    }

    #[test]
    fn cache_file_round_trips_and_agent_serializes_as_variant_name() {
        // Protects the on-disk format: `Agent` unit variants must serialize
        // as their exact variant-name strings — the same strings the old
        // hand-rolled agent_to_str/agent_from_str mapping produced.
        assert_eq!(
            serde_json::to_string(&Agent::ClaudeCode).unwrap(),
            "\"ClaudeCode\""
        );

        let mut agents = HashMap::new();
        agents.insert(
            Agent::ClaudeCode,
            AgentCache {
                mtime: 42,
                sessions: vec![CachedSession {
                    agent: Agent::ClaudeCode,
                    session_id: "sid".to_string(),
                    project_name: "proj".to_string(),
                    project_path: "/p".to_string(),
                    summaries: vec!["hello".to_string()],
                    timestamp: 7,
                    git_branch: Some("main".to_string()),
                    worktree: None,
                    recap: None,
                }],
            },
        );
        let cache = CacheFile {
            version: CACHE_VERSION,
            agf_version: AGF_VERSION.to_string(),
            agents,
        };

        let json = serde_json::to_string(&cache).unwrap();
        assert!(json.contains("\"ClaudeCode\""), "unexpected json: {json}");

        let parsed = parse_cache(&json, AGF_VERSION).expect("round-trip parse");
        let entry = &parsed.agents[&Agent::ClaudeCode];
        assert_eq!(entry.mtime, 42);
        assert_eq!(entry.sessions.len(), 1);
        assert_eq!(entry.sessions[0].agent, Agent::ClaudeCode);
        assert_eq!(entry.sessions[0].session_id, "sid");
        assert_eq!(entry.sessions[0].git_branch.as_deref(), Some("main"));
    }
}
