use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::model::{Agent, Session};
use crate::plugin;

const CACHE_VERSION: u32 = 2;

#[derive(Serialize, Deserialize)]
struct CacheFile {
    version: u32,
    agents: HashMap<String, AgentCache>,
}

#[derive(Serialize, Deserialize)]
struct AgentCache {
    mtime: u64, // Unix seconds of data source last modification
    sessions: Vec<CachedSession>,
}

#[derive(Serialize, Deserialize)]
struct CachedSession {
    agent: String,
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

fn agent_from_str(s: &str) -> Option<Agent> {
    match s {
        "ClaudeCode" => Some(Agent::ClaudeCode),
        "Codex" => Some(Agent::Codex),
        "OpenCode" => Some(Agent::OpenCode),
        "Pi" => Some(Agent::Pi),
        "Kiro" => Some(Agent::Kiro),
        "CursorAgent" => Some(Agent::CursorAgent),
        "Gemini" => Some(Agent::Gemini),
        _ => None,
    }
}

fn agent_to_str(a: Agent) -> &'static str {
    match a {
        Agent::ClaudeCode => "ClaudeCode",
        Agent::Codex => "Codex",
        Agent::OpenCode => "OpenCode",
        Agent::Pi => "Pi",
        Agent::Kiro => "Kiro",
        Agent::CursorAgent => "CursorAgent",
        Agent::Gemini => "Gemini",
    }
}

fn to_cached(s: &Session) -> CachedSession {
    CachedSession {
        agent: agent_to_str(s.agent).to_string(),
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

fn from_cached(c: &CachedSession) -> Option<Session> {
    let agent = agent_from_str(&c.agent)?;
    Some(Session {
        agent,
        session_id: c.session_id.clone(),
        project_name: c.project_name.clone(),
        project_path: c.project_path.clone(),
        summaries: c.summaries.clone(),
        timestamp: c.timestamp,
        git_branch: c.git_branch.clone(),
        worktree: c.worktree.clone(),
        recap: c.recap.clone(),
    })
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
            if let Ok(m) = entry.metadata() {
                if let Ok(t) = m.modified() {
                    if let Ok(d) = t.duration_since(std::time::SystemTime::UNIX_EPOCH) {
                        max = max.max(d.as_secs());
                    }
                }
            }
        }
    }
    max
}

/// Load cached sessions. Returns (sessions, stale_agents).
/// stale_agents are agents whose data sources have changed since cache was written.
pub fn load_cache() -> (Vec<Session>, Vec<Agent>) {
    let path = cache_path();
    let content = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return (Vec::new(), Agent::all().to_vec()),
    };

    let cache: CacheFile = match serde_json::from_str::<CacheFile>(&content) {
        Ok(c) if c.version == CACHE_VERSION => c,
        Ok(c) => {
            if std::env::var("AGF_DEBUG").is_ok() {
                eprintln!(
                    "[agf] cache version {} != {} → rescanning",
                    c.version, CACHE_VERSION
                );
            }
            return (Vec::new(), Agent::all().to_vec());
        }
        Err(e) => {
            if std::env::var("AGF_DEBUG").is_ok() {
                eprintln!("[agf] cache parse failed: {e} → rescanning");
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
        let key = agent_to_str(p.agent());
        let current_mtime = get_max_mtime(&p.data_sources());

        match cache.agents.get(key) {
            Some(ac) if ac.mtime >= current_mtime && current_mtime > 0 => {
                // Cache is fresh
                for cs in &ac.sessions {
                    if let Some(s) = from_cached(cs) {
                        sessions.push(s);
                    }
                }
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
pub fn write_cache(sessions: &[Session]) {
    let path = cache_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    let installed: std::collections::HashSet<Agent> =
        crate::config::installed_agents().into_iter().collect();
    let plugins = plugin::all_plugins();
    let mut agents: HashMap<String, AgentCache> = HashMap::new();

    for p in &plugins {
        if !installed.contains(&p.agent()) {
            continue;
        }
        let key = agent_to_str(p.agent()).to_string();
        let agent_sessions: Vec<CachedSession> = sessions
            .iter()
            .filter(|s| s.agent == p.agent())
            .map(to_cached)
            .collect();
        let mtime = get_max_mtime(&p.data_sources());
        agents.insert(
            key,
            AgentCache {
                mtime,
                sessions: agent_sessions,
            },
        );
    }

    let cache = CacheFile {
        version: CACHE_VERSION,
        agents,
    };

    if let Ok(json) = serde_json::to_string(&cache) {
        let tmp = path.with_extension("json.tmp");
        if fs::write(&tmp, json).is_ok() {
            let _ = fs::rename(&tmp, &path);
        }
    }
}

/// Scan only the specified agents and merge with existing sessions.
pub fn scan_stale_agents(stale: &[Agent], existing: &mut Vec<Session>) {
    use std::thread;

    let installed: std::collections::HashSet<Agent> =
        crate::config::installed_agents().into_iter().collect();
    let stale: Vec<Agent> = stale
        .iter()
        .copied()
        .filter(|a| installed.contains(a))
        .collect();

    let handles: Vec<_> = stale
        .iter()
        .map(|agent| {
            let agent = *agent;
            thread::spawn(move || match agent {
                Agent::ClaudeCode => crate::scanner::claude::scan().unwrap_or_default(),
                Agent::Codex => crate::scanner::codex::scan().unwrap_or_default(),
                Agent::OpenCode => crate::scanner::opencode::scan().unwrap_or_default(),
                Agent::Pi => crate::scanner::pi::scan().unwrap_or_default(),
                Agent::Kiro => crate::scanner::kiro::scan().unwrap_or_default(),
                Agent::CursorAgent => crate::scanner::cursor_agent::scan().unwrap_or_default(),
                Agent::Gemini => crate::scanner::gemini::scan().unwrap_or_default(),
            })
        })
        .collect();

    // Remove old sessions for stale agents
    existing.retain(|s| !stale.contains(&s.agent));

    // Add fresh sessions
    for h in handles {
        if let Ok(new_sessions) = h.join() {
            existing.extend(new_sessions);
        }
    }

    existing.sort_by_key(|s| std::cmp::Reverse(s.timestamp));
}
