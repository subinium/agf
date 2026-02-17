use std::collections::HashMap;
use std::fs;

use serde::Deserialize;

use crate::error::AgfError;
use crate::model::{Agent, Session};

#[derive(Deserialize)]
struct ClaudeEntry {
    display: Option<String>,
    timestamp: Option<f64>,
    project: Option<String>,
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
}

pub fn scan() -> Result<Vec<Session>, AgfError> {
    let path = crate::config::claude_dir()?.join("history.jsonl");
    if !path.exists() {
        return Ok(Vec::new());
    }

    let content = fs::read_to_string(&path)?;
    let mut latest: HashMap<String, ClaudeEntry> = HashMap::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let entry: ClaudeEntry = match serde_json::from_str(line) {
            Ok(e) => e,
            Err(_) => continue,
        };
        let session_id = match &entry.session_id {
            Some(id) if !id.is_empty() => id.clone(),
            _ => continue,
        };
        let ts = entry.timestamp.unwrap_or(0.0);
        let existing_ts = latest
            .get(&session_id)
            .and_then(|e| e.timestamp)
            .unwrap_or(0.0);
        if ts >= existing_ts {
            latest.insert(session_id, entry);
        }
    }

    let mut sessions: Vec<Session> = latest
        .into_iter()
        .filter_map(|(session_id, entry)| {
            let project = entry.project?;
            let project_name = std::path::Path::new(&project)
                .file_name()?
                .to_str()?
                .to_string();
            let timestamp = entry.timestamp.unwrap_or(0.0) as i64;

            Some(Session {
                agent: Agent::ClaudeCode,
                session_id,
                project_name,
                project_path: project,
                summary: entry.display,
                timestamp,
                git_branch: None,
                git_dirty: None,
            })
        })
        .collect();

    sessions.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    Ok(sessions)
}
