use std::collections::HashMap;
use std::fs;

use serde::Deserialize;
use walkdir::WalkDir;

use crate::error::AgfError;
use crate::model::{Agent, Session};

#[derive(Deserialize)]
struct SessionMeta {
    #[serde(rename = "type")]
    entry_type: Option<String>,
    payload: Option<SessionPayload>,
}

#[derive(Deserialize)]
struct SessionPayload {
    id: Option<String>,
    cwd: Option<String>,
    timestamp: Option<String>,
    git: Option<GitInfo>,
}

#[derive(Deserialize)]
struct GitInfo {
    branch: Option<String>,
}

#[derive(Deserialize)]
struct HistoryEntry {
    session_id: Option<String>,
    ts: Option<f64>,
    text: Option<String>,
}

pub fn scan() -> Result<Vec<Session>, AgfError> {
    let codex_dir = crate::config::codex_dir()?;
    let sessions_dir = codex_dir.join("sessions");

    // Collect summaries from history.jsonl (keyed by session_id, newest-first)
    let summaries = read_history_summaries(&codex_dir);

    let mut sessions = Vec::new();

    if !sessions_dir.exists() {
        return Ok(sessions);
    }

    for entry in WalkDir::new(&sessions_dir)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }

        let content = match fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let first_line = match content.lines().next() {
            Some(line) if !line.trim().is_empty() => line.trim(),
            _ => continue,
        };

        let meta: SessionMeta = match serde_json::from_str(first_line) {
            Ok(m) => m,
            Err(_) => continue,
        };

        if meta.entry_type.as_deref() != Some("session_meta") {
            continue;
        }

        let payload = match meta.payload {
            Some(p) => p,
            None => continue,
        };

        let session_id = match payload.id {
            Some(id) if !id.is_empty() => id,
            _ => continue,
        };

        let cwd = match payload.cwd {
            Some(cwd) if !cwd.is_empty() => cwd,
            _ => continue,
        };

        let project_name = std::path::Path::new(&cwd)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        let timestamp = payload
            .timestamp
            .as_deref()
            .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
            .map(|dt| dt.timestamp_millis())
            .unwrap_or(0);

        let git_branch = payload.git.and_then(|g| g.branch);

        let session_summaries = summaries.get(&session_id).cloned().unwrap_or_default();

        sessions.push(Session {
            agent: Agent::Codex,
            session_id,
            project_name,
            project_path: cwd,
            summaries: session_summaries,
            timestamp,
            git_branch,
            git_dirty: None,
        });
    }

    sessions.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    Ok(sessions)
}

fn read_history_summaries(codex_dir: &std::path::Path) -> HashMap<String, Vec<String>> {
    let path = codex_dir.join("history.jsonl");
    let mut summaries: HashMap<String, Vec<(f64, String)>> = HashMap::new();

    let content = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return HashMap::new(),
    };

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let entry: HistoryEntry = match serde_json::from_str(line) {
            Ok(e) => e,
            Err(_) => continue,
        };
        let session_id = match entry.session_id {
            Some(id) if !id.is_empty() => id,
            _ => continue,
        };
        let ts = entry.ts.unwrap_or(0.0);
        let text = match entry.text {
            Some(t) if !t.is_empty() => t,
            _ => continue,
        };
        summaries.entry(session_id).or_default().push((ts, text));
    }

    summaries
        .into_iter()
        .map(|(k, mut v)| {
            v.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
            (k, v.into_iter().map(|(_, s)| s).collect())
        })
        .collect()
}
