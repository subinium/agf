use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::Connection;
use serde::Deserialize;

use crate::error::AgfError;
use crate::model::{Agent, Session};

#[derive(Deserialize)]
struct CursorChatMeta {
    name: Option<String>,
    #[serde(rename = "createdAt")]
    created_at: Option<f64>,
    mode: Option<String>,
}

pub fn scan() -> Result<Vec<Session>, AgfError> {
    let cursor_dir = crate::config::cursor_dir()?;
    let projects_dir = cursor_dir.join("projects");

    if !projects_dir.exists() {
        return Ok(Vec::new());
    }

    let mut sessions = Vec::new();

    let entries = match fs::read_dir(&projects_dir) {
        Ok(e) => e,
        Err(_) => return Ok(Vec::new()),
    };

    for entry in entries.filter_map(|e| e.ok()) {
        let dir_name = match entry.file_name().into_string() {
            Ok(name) => name,
            Err(_) => continue,
        };

        if !entry.path().is_dir() {
            continue;
        }

        let project_path = reconstruct_path(&dir_name);
        let project_name = Path::new(&project_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&dir_name)
            .to_string();

        let path_md5 = format!("{:x}", md5::compute(&project_path));

        // Look for agent transcripts
        let transcripts_dir = entry.path().join("agent-transcripts");
        if !transcripts_dir.exists() {
            continue;
        }

        let transcript_entries = match fs::read_dir(&transcripts_dir) {
            Ok(e) => e,
            Err(_) => continue,
        };

        for transcript in transcript_entries.filter_map(|e| e.ok()) {
            let file_name = match transcript.file_name().into_string() {
                Ok(name) => name,
                Err(_) => continue,
            };

            let session_id = match file_name.strip_suffix(".txt") {
                Some(id) => id.to_string(),
                None => continue,
            };

            // Try reading chat metadata from SQLite
            let chat_meta = read_chat_meta(&cursor_dir, &path_md5, &session_id);

            let (summary, timestamp, _mode) = match chat_meta {
                Some(meta) => (
                    meta.name,
                    meta.created_at.map(|t| t as i64).unwrap_or(0),
                    meta.mode,
                ),
                None => (None, 0, None),
            };

            sessions.push(Session {
                agent: Agent::Cursor,
                session_id,
                project_name: project_name.clone(),
                project_path: project_path.clone(),
                summary,
                timestamp,
                git_branch: None,
                git_dirty: None,
            });
        }
    }

    sessions.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    Ok(sessions)
}

/// Reconstruct an absolute path from Cursor's dash-encoded directory name.
///
/// Cursor encodes `/Users/foo/project` as `-Users-foo-project`.
/// We try replacing dashes with `/` progressively, checking if the resulting
/// path exists on disk. If no valid reconstruction is found, we return
/// a best-effort replacement of the leading dash with `/`.
fn reconstruct_path(encoded: &str) -> String {
    // Replace leading dash with /
    let with_leading_slash = if let Some(rest) = encoded.strip_prefix('-') {
        format!("/{rest}")
    } else {
        return encoded.to_string();
    };

    // Try progressive replacement: find valid paths by replacing dashes with /
    let best = try_reconstruct(&with_leading_slash);
    if Path::new(&best).exists() {
        return best;
    }

    // Fallback: simple replace all dashes
    with_leading_slash
}

fn try_reconstruct(path_with_dashes: &str) -> String {
    // Split into segments by dash and try to find the longest valid prefix
    let chars: Vec<char> = path_with_dashes.chars().collect();
    let dash_positions: Vec<usize> = chars
        .iter()
        .enumerate()
        .filter(|(i, c)| **c == '-' && *i > 0)
        .map(|(i, _)| i)
        .collect();

    if dash_positions.is_empty() {
        return path_with_dashes.to_string();
    }

    // Try replacing each dash with / and check if it produces a valid directory
    let mut best_path = path_with_dashes.to_string();
    let mut result = chars.clone();

    for &pos in &dash_positions {
        result[pos] = '/';
        let candidate: String = result.iter().collect();
        // Check if any prefix of the candidate is a valid directory
        let candidate_path = PathBuf::from(&candidate);
        if candidate_path.exists() {
            best_path = candidate;
        } else {
            // Try if parent exists
            if let Some(parent) = candidate_path.parent() {
                if parent.exists() {
                    best_path = candidate;
                } else {
                    // Revert this replacement
                    result[pos] = '-';
                }
            } else {
                result[pos] = '-';
            }
        }
    }

    best_path
}

fn read_chat_meta(
    cursor_dir: &Path,
    path_md5: &str,
    session_id: &str,
) -> Option<CursorChatMeta> {
    let db_path = cursor_dir
        .join("chats")
        .join(path_md5)
        .join(session_id)
        .join("store.db");

    if !db_path.exists() {
        return None;
    }

    let conn = Connection::open(&db_path).ok()?;
    let hex_value: String = conn
        .query_row(
            "SELECT value FROM meta WHERE key = '0'",
            [],
            |row| row.get(0),
        )
        .ok()?;

    let decoded = hex::decode(&hex_value).ok()?;
    let json_str = String::from_utf8(decoded).ok()?;
    serde_json::from_str(&json_str).ok()
}
