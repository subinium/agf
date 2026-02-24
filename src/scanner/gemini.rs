use std::collections::HashMap;
use std::fs;
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::error::AgfError;
use crate::model::{Agent, Session};

use super::truncate;

pub fn scan() -> Result<Vec<Session>, AgfError> {
    let gemini_dir = crate::config::gemini_dir()?;
    let tmp_dir = gemini_dir.join("tmp");

    if !tmp_dir.exists() {
        return Ok(Vec::new());
    }

    let path_map = build_path_map(&gemini_dir);

    let mut sessions = Vec::new();

    let Ok(entries) = fs::read_dir(&tmp_dir) else {
        return Ok(Vec::new());
    };

    for entry in entries.filter_map(|e| e.ok()) {
        let dir_name = entry.file_name().to_string_lossy().to_string();
        let chats_dir = entry.path().join("chats");

        if !chats_dir.is_dir() {
            continue;
        }

        let (project_path, project_name) = resolve_project(&dir_name, &path_map);

        let Ok(chat_entries) = fs::read_dir(&chats_dir) else {
            continue;
        };

        for chat_entry in chat_entries.filter_map(|e| e.ok()) {
            let fname = chat_entry.file_name().to_string_lossy().to_string();
            if !fname.starts_with("session-") || !fname.ends_with(".json") {
                continue;
            }

            if let Some(session) = parse_session(&chat_entry.path(), &project_path, &project_name) {
                sessions.push(session);
            }
        }
    }

    Ok(sessions)
}

/// Build a map: dir_name → full project path.
///
/// Named dirs (e.g. "github") come directly from `projects.json` values.
/// Hash dirs (e.g. "e0dc5a91...") are matched by computing SHA256 of each known path.
fn build_path_map(gemini_dir: &Path) -> HashMap<String, String> {
    let mut map = HashMap::new();

    let projects_file = gemini_dir.join("projects.json");
    let Ok(content) = fs::read_to_string(projects_file) else {
        return map;
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) else {
        return map;
    };
    let Some(projects) = json.get("projects").and_then(|v| v.as_object()) else {
        return map;
    };

    for (path, name) in projects {
        if let Some(name_str) = name.as_str() {
            // Named dir → path
            map.insert(name_str.to_string(), path.clone());
            // Hash dir → path (SHA256 of the full path string)
            map.insert(sha256_hex(path.as_bytes()), path.clone());
        }
    }

    map
}

/// Resolve a project dir name to (project_path, project_name).
fn resolve_project(dir_name: &str, path_map: &HashMap<String, String>) -> (String, String) {
    if let Some(full_path) = path_map.get(dir_name) {
        let name = Path::new(full_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(dir_name)
            .to_string();
        return (full_path.clone(), name);
    }

    // Unknown hash: no path available, use truncated hash as display name
    let short = format!("{}…", &dir_name[..dir_name.len().min(8)]);
    (String::new(), short)
}

/// Parse a Gemini session JSON file into a Session.
fn parse_session(path: &Path, project_path: &str, project_name: &str) -> Option<Session> {
    let content = fs::read_to_string(path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&content).ok()?;

    let session_id = json.get("sessionId")?.as_str()?.to_string();

    // Prefer lastUpdated over startTime
    let timestamp_str = json
        .get("lastUpdated")
        .or_else(|| json.get("startTime"))
        .and_then(|v| v.as_str())?;

    let timestamp = parse_iso8601_ms(timestamp_str)?;

    let summary = extract_summary(&json);

    Some(Session {
        agent: Agent::Gemini,
        session_id,
        project_name: project_name.to_string(),
        project_path: project_path.to_string(),
        summaries: summary.into_iter().collect(),
        timestamp,
        git_branch: None,
        worktree: None,
    })
}

/// Extract the first user message text as a summary.
fn extract_summary(json: &serde_json::Value) -> Option<String> {
    let messages = json.get("messages")?.as_array()?;

    for msg in messages {
        if msg.get("type").and_then(|v| v.as_str()) != Some("user") {
            continue;
        }

        // content is an array of {text} blocks (new format) or a plain string (old format)
        if let Some(arr) = msg.get("content").and_then(|v| v.as_array()) {
            for part in arr {
                if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
                    if !normalized.is_empty() {
                        return Some(truncate(&normalized, 100));
                    }
                }
            }
        } else if let Some(text) = msg.get("content").and_then(|v| v.as_str()) {
            let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
            if !normalized.is_empty() {
                return Some(truncate(&normalized, 100));
            }
        }
    }

    None
}

/// Parse an ISO 8601 / RFC 3339 string to Unix milliseconds.
fn parse_iso8601_ms(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.timestamp_millis())
}

/// SHA256 hex digest.
fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}
