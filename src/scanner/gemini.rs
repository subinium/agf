use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::error::AgfError;
use crate::model::{Agent, Session};

use super::truncate;

/// Maximum bytes to read from a single session file.
/// Gemini session files can balloon to 28 MB+ when tool calls embed full file
/// contents. The JSON header (sessionId, timestamps) fits in the first few
/// hundred bytes; the first user message almost always lands in the first 64 KB.
const MAX_FILE_BYTES: usize = 64 * 1024;

pub fn scan() -> Result<Vec<Session>, AgfError> {
    let gemini_dir = crate::config::gemini_dir()?;
    let tmp_dir = gemini_dir.join("tmp");

    if !tmp_dir.exists() {
        return Ok(Vec::new());
    }

    let path_map = build_path_map(&gemini_dir);

    // Dedup by sessionId: keep the entry with the latest `lastUpdated`.
    // The same session can appear in both a hash dir (old) and a named dir
    // (new) when Gemini CLI migrates a project to projects.json.
    let mut by_id: HashMap<String, Session> = HashMap::new();

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
                let existing = by_id.get(&session.session_id);
                if existing.map_or(true, |e| session.timestamp > e.timestamp) {
                    by_id.insert(session.session_id.clone(), session);
                }
            }
        }
    }

    Ok(by_id.into_values().collect())
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
///
/// For large files (> 64 KB) we read a capped slice and fall back to
/// field extraction if the JSON is truncated.
fn parse_session(path: &Path, project_path: &str, project_name: &str) -> Option<Session> {
    let content = read_capped(path)?;

    // Try full JSON parse first (works for files ≤ 64 KB)
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) {
        let session_id = json.get("sessionId")?.as_str()?.to_string();
        let timestamp_str = json
            .get("lastUpdated")
            .or_else(|| json.get("startTime"))
            .and_then(|v| v.as_str())?;
        let timestamp = parse_iso8601_ms(timestamp_str)?;
        let summary = extract_summary(&json);

        return Some(Session {
            agent: Agent::Gemini,
            session_id,
            project_name: project_name.to_string(),
            project_path: project_path.to_string(),
            summaries: summary.into_iter().collect(),
            timestamp,
            git_branch: None,
            worktree: None,
        });
    }

    // Truncated file — extract key fields with string search.
    // sessionId and timestamps always appear in the first ~300 bytes.
    // The first user message is typically in the first few KB.
    let session_id = extract_str_field(&content, "sessionId")?;
    let timestamp_str = extract_str_field(&content, "lastUpdated")
        .or_else(|| extract_str_field(&content, "startTime"))?;
    let timestamp = parse_iso8601_ms(&timestamp_str)?;
    let summary = extract_summary_partial(&content);

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

/// Read up to MAX_FILE_BYTES from a file.
fn read_capped(path: &Path) -> Option<String> {
    let mut file = fs::File::open(path).ok()?;
    let size = file.metadata().ok()?.len() as usize;

    if size <= MAX_FILE_BYTES {
        return fs::read_to_string(path).ok();
    }

    let mut buf = vec![0u8; MAX_FILE_BYTES];
    let n = file.read(&mut buf).ok()?;
    buf.truncate(n);
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// Extract the first user message text from a fully-parsed JSON value.
fn extract_summary(json: &serde_json::Value) -> Option<String> {
    let messages = json.get("messages")?.as_array()?;

    for msg in messages {
        if msg.get("type").and_then(|v| v.as_str()) != Some("user") {
            continue;
        }

        // content is an array of {text} blocks (new format) or a plain string
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

/// Extract a user message summary from a partial (truncated) JSON string.
/// Looks for the first `"type":"user"` block and extracts adjacent `"text"`.
fn extract_summary_partial(s: &str) -> Option<String> {
    // Find first occurrence of user type marker
    let user_pos = s.find("\"type\":\"user\"")?;
    let after = &s[user_pos..];

    // Look for "text":"..." within the next 1 KB
    let window = &after[..after.len().min(1024)];
    let text = extract_str_field(window, "text")?;
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return None;
    }
    Some(truncate(&normalized, 100))
}

/// Extract a JSON string field value from raw text using simple string search.
/// Works on both full and truncated JSON. Returns None if field not found.
fn extract_str_field(s: &str, field: &str) -> Option<String> {
    let key = format!("\"{}\":\"", field);
    let start = s.find(&key)? + key.len();
    let rest = &s[start..];
    // Find closing quote, respecting escaped quotes
    let mut end = 0;
    let mut chars = rest.char_indices();
    while let Some((i, c)) = chars.next() {
        if c == '\\' {
            chars.next(); // skip escaped char
            continue;
        }
        if c == '"' {
            end = i;
            break;
        }
    }
    if end == 0 {
        return None;
    }
    Some(rest[..end].to_string())
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
