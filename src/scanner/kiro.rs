use rusqlite::Connection;

use crate::error::AgfError;
use crate::model::{Agent, Session};

pub fn scan() -> Result<Vec<Session>, AgfError> {
    let db_path = crate::config::kiro_data_dir()?.join("data.sqlite3");

    if !db_path.exists() {
        return Ok(Vec::new());
    }

    let conn = Connection::open_with_flags(
        &db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;

    // Kiro stores conversations in conversations_v2 table
    // key = project directory path, conversation_id = session UUID
    // updated_at = Unix timestamp in milliseconds
    let mut stmt = match conn.prepare(
        "SELECT key, conversation_id, value, updated_at \
         FROM conversations_v2 \
         ORDER BY updated_at DESC",
    ) {
        Ok(s) => s,
        Err(_) => return Ok(Vec::new()),
    };

    let sessions = stmt
        .query_map([], |row| {
            let directory: String = row.get(0)?;
            let conversation_id: String = row.get(1)?;
            let value: String = row.get(2)?;
            let updated_at: i64 = row.get(3)?;
            Ok((directory, conversation_id, value, updated_at))
        })?
        .filter_map(|r| r.ok())
        .map(|(directory, conversation_id, value, updated_at)| {
            let project_name = std::path::Path::new(&directory)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();

            let summary = extract_summary(&value);

            Session {
                agent: Agent::Kiro,
                session_id: conversation_id,
                project_name,
                project_path: directory,
                summary,
                timestamp: updated_at,
                git_branch: None,
                git_dirty: None,
            }
        })
        .collect();

    Ok(sessions)
}

/// Extract the first user message from the conversation JSON as a summary.
fn extract_summary(value: &str) -> Option<String> {
    let parsed: serde_json::Value = serde_json::from_str(value).ok()?;
    let messages = parsed.get("messages").and_then(|v| v.as_array())?;
    for msg in messages {
        let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("");
        if role == "user" {
            // Content can be a string or an array of content blocks
            if let Some(content) = msg.get("content").and_then(|v| v.as_str()) {
                let trimmed = content.trim();
                if !trimmed.is_empty() {
                    return Some(truncate(trimmed, 100));
                }
            }
            if let Some(parts) = msg.get("content").and_then(|v| v.as_array()) {
                for part in parts {
                    if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                        let trimmed = text.trim();
                        if !trimmed.is_empty() {
                            return Some(truncate(trimmed, 100));
                        }
                    }
                }
            }
        }
    }
    None
}

use super::truncate;
