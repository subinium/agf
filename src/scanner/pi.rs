use std::collections::HashSet;

use serde::Deserialize;
use walkdir::WalkDir;

use crate::error::AgfError;
use crate::model::{Agent, Session};
use crate::scanner::read_first_line;

#[derive(Deserialize)]
struct PiSessionHeader {
    #[serde(rename = "type")]
    entry_type: Option<String>,
    id: Option<String>,
    timestamp: Option<String>,
    cwd: Option<String>,
}

pub fn scan() -> Result<Vec<Session>, AgfError> {
    let sessions_dir = crate::config::pi_sessions_dir()?;
    if !sessions_dir.exists() {
        return Ok(Vec::new());
    }

    let mut sessions = Vec::new();

    for entry in WalkDir::new(&sessions_dir)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();
        if !path.is_file() || path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }

        // Read just the first line rather than loading the whole JSONL.
        let first_line = match read_first_line(path) {
            Some(line) => line,
            None => continue,
        };

        let header: PiSessionHeader = match serde_json::from_str(first_line.trim()) {
            Ok(h) => h,
            Err(_) => continue,
        };

        if header.entry_type.as_deref() != Some("session") {
            continue;
        }

        let session_id = match header.id {
            Some(id) => id,
            None => continue,
        };

        let cwd = match header.cwd {
            Some(cwd) => cwd,
            None => continue,
        };

        let timestamp = header
            .timestamp
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(&t).ok())
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|| {
                path.metadata()
                    .and_then(|m| m.modified())
                    .map(|t| {
                        t.duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis() as i64
                    })
                    .unwrap_or(0)
            });

        let project_name = std::path::Path::new(&cwd)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        sessions.push(Session {
            agent: Agent::Pi,
            session_id,
            project_name,
            project_path: cwd,
            summaries: Vec::new(),
            timestamp,
            git_branch: None,
            worktree: None,
            recap: None,
        });
    }

    // Sort by timestamp desc, keep only the most recent session per project
    // (pi --resume only resumes the latest session for a directory)
    sessions.sort_by_key(|s| std::cmp::Reverse(s.timestamp));
    let mut seen = HashSet::new();
    sessions.retain(|s| seen.insert(s.project_path.clone()));

    Ok(sessions)
}
