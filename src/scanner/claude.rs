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

struct SessionData {
    project: String,
    timestamp: f64,
    summaries: Vec<(f64, String)>, // (timestamp, display) pairs
}

pub fn scan() -> Result<Vec<Session>, AgfError> {
    let path = crate::config::claude_dir()?.join("history.jsonl");
    if !path.exists() {
        return Ok(Vec::new());
    }

    let content = fs::read_to_string(&path)?;
    let mut sessions_map: HashMap<String, SessionData> = HashMap::new();

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

        let data = sessions_map
            .entry(session_id)
            .or_insert_with(|| SessionData {
                project: entry.project.clone().unwrap_or_default(),
                timestamp: ts,
                summaries: Vec::new(),
            });

        // Keep the latest timestamp and project
        if ts >= data.timestamp {
            data.timestamp = ts;
            if let Some(ref proj) = entry.project {
                data.project = proj.clone();
            }
        }

        if let Some(display) = entry.display {
            if !display.is_empty() {
                data.summaries.push((ts, display));
            }
        }
    }

    let mut sessions: Vec<Session> = sessions_map
        .into_iter()
        .filter_map(|(session_id, mut data)| {
            if data.project.is_empty() {
                return None;
            }
            let project_name = std::path::Path::new(&data.project)
                .file_name()?
                .to_str()?
                .to_string();
            let timestamp = data.timestamp as i64;

            // Sort summaries newest-first
            data.summaries
                .sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
            let summaries: Vec<String> = data.summaries.into_iter().map(|(_, s)| s).collect();

            Some(Session {
                agent: Agent::ClaudeCode,
                session_id,
                project_name,
                project_path: data.project,
                summaries,
                timestamp,
                git_branch: None,
                git_dirty: None,
            })
        })
        .collect();

    sessions.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    Ok(sessions)
}
