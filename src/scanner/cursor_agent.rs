use std::path::{Path, PathBuf};

use rusqlite::Connection;
use walkdir::WalkDir;

use crate::error::AgfError;
use crate::model::{Agent, Session};

pub fn scan() -> Result<Vec<Session>, AgfError> {
    let cursor_dir = crate::config::cursor_dir()?;
    let projects_dir = cursor_dir.join("projects");

    if !projects_dir.exists() {
        return Ok(Vec::new());
    }

    let chats_dir = cursor_dir.join("chats");
    let mut sessions = Vec::new();

    // Walk ~/.cursor/projects/*/agent-transcripts/*.txt
    for entry in WalkDir::new(&projects_dir)
        .min_depth(3)
        .max_depth(3)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();

        if !path.is_file() || path.extension().and_then(|e| e.to_str()) != Some("txt") {
            continue;
        }

        // Parent must be "agent-transcripts"
        let parent = match path.parent() {
            Some(p) if p.file_name().and_then(|n| n.to_str()) == Some("agent-transcripts") => p,
            _ => continue,
        };

        let session_id = match path.file_stem().and_then(|n| n.to_str()) {
            Some(id) => id.to_string(),
            None => continue,
        };

        // Grandparent is the dash-encoded project path
        let encoded_dir = match parent
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
        {
            Some(name) => name.to_string(),
            None => continue,
        };

        // Skip temp directories
        if encoded_dir.starts_with("var-folders") {
            continue;
        }

        let project_path = match decode_dash_path(&encoded_dir) {
            Some(p) => p,
            None => continue,
        };

        let project_name = project_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        let project_path_str = project_path.to_string_lossy().to_string();

        // Try to get metadata from store.db
        let meta = if chats_dir.exists() {
            find_store_db_metadata(&chats_dir, &session_id)
        } else {
            None
        };

        let (summary, timestamp) = match meta {
            Some(m) => (m.name, m.created_at),
            None => {
                // Fall back to transcript file mtime
                let mtime = path
                    .metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0);
                (None, mtime)
            }
        };

        sessions.push(Session {
            agent: Agent::CursorAgent,
            session_id,
            project_name,
            project_path: project_path_str,
            summaries: summary.into_iter().collect(),
            timestamp,
            git_branch: None,
            worktree: None,
        });
    }

    Ok(sessions)
}

struct StoreMeta {
    name: Option<String>,
    created_at: i64,
}

/// Search ~/.cursor/chats/*/<session_id>/store.db and extract metadata.
fn find_store_db_metadata(chats_dir: &Path, session_id: &str) -> Option<StoreMeta> {
    // chats_dir contains workspace-hash directories
    let read_dir = std::fs::read_dir(chats_dir).ok()?;
    for workspace_entry in read_dir.filter_map(|e| e.ok()) {
        let store_path = workspace_entry.path().join(session_id).join("store.db");
        if store_path.exists() {
            return read_store_db(&store_path);
        }
    }
    None
}

/// Read the `meta` table from store.db, hex-decode the value, and parse as JSON.
fn read_store_db(store_path: &Path) -> Option<StoreMeta> {
    let conn = Connection::open_with_flags(
        store_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;

    // The meta table typically has key-value rows with hex-encoded JSON
    let mut stmt = conn
        .prepare("SELECT value FROM cursorDiskKV WHERE key = 'composerData'")
        .ok()?;
    let hex_value: String = stmt.query_row([], |row| row.get(0)).ok()?;

    let json_bytes = hex_decode(&hex_value)?;
    let json_str = std::str::from_utf8(&json_bytes).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(json_str).ok()?;

    let name = parsed
        .get("name")
        .and_then(|v| v.as_str())
        .map(|s| truncate(s, 100));

    // createdAt is in milliseconds
    let created_at = parsed
        .get("createdAt")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    Some(StoreMeta { name, created_at })
}

/// Decode a hex-encoded string to bytes.
fn hex_decode(hex: &str) -> Option<Vec<u8>> {
    if !hex.len().is_multiple_of(2) {
        return None;
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
        .collect()
}

/// Backtracking path decoder: dash-encoded path -> filesystem path.
/// e.g. "Users-subinium-Desktop-my-project" -> /Users/subinium/Desktop/my-project
fn decode_dash_path(encoded: &str) -> Option<PathBuf> {
    let parts: Vec<&str> = encoded.split('-').collect();
    solve(&parts, 0, Path::new("/"))
}

fn solve(parts: &[&str], idx: usize, current: &Path) -> Option<PathBuf> {
    if idx >= parts.len() {
        return if current.is_dir() {
            Some(current.to_path_buf())
        } else {
            None
        };
    }
    // Try longest segment first (greedy — fewer filesystem checks)
    for end in (idx + 1..=parts.len()).rev() {
        let segment = parts[idx..end].join("-");
        let candidate = current.join(&segment);
        if end == parts.len() {
            if candidate.is_dir() {
                return Some(candidate);
            }
        } else if candidate.is_dir() {
            if let Some(result) = solve(parts, end, &candidate) {
                return Some(result);
            }
        }
    }
    None
}

use super::truncate;
