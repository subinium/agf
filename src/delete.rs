use std::fs;
use std::io;
use std::path::Path;

use serde::Deserialize;
use walkdir::WalkDir;

use crate::config;
use crate::model::{Agent, Session};

/// Delete a session's data files. Returns Ok(()) on success.
/// Only removes session data, NOT the project directory.
pub fn delete_session(session: &Session) -> Result<(), io::Error> {
    match session.agent {
        Agent::ClaudeCode => delete_claude_session(session),
        Agent::Codex => delete_codex_session(session),
        Agent::OpenCode => delete_opencode_session(session),
    }
}

// ---------------------------------------------------------------------------
// Claude Code
// ---------------------------------------------------------------------------

/// Claude sessions are stored as lines in `~/.claude/history.jsonl`.
/// We rewrite the file excluding all lines whose `sessionId` matches.
/// We also remove any project-specific session data under
/// `~/.claude/projects/<project>/sessions/<sessionId>/`.
fn delete_claude_session(session: &Session) -> Result<(), io::Error> {
    let claude_dir = config::claude_dir().map_err(io::Error::other)?;

    // 1. Rewrite history.jsonl without this session's lines
    let history_path = claude_dir.join("history.jsonl");
    if history_path.exists() {
        rewrite_jsonl_excluding_session_id(&history_path, &session.session_id)?;
    }

    // 2. Remove project session data under ~/.claude/projects/
    //    Claude stores project data keyed by the project path.
    //    The directory structure is:
    //      ~/.claude/projects/<encoded_path>/<sessionId>/
    //    Walk the projects dir and look for a subdirectory matching session_id.
    let projects_dir = claude_dir.join("projects");
    if projects_dir.exists() {
        remove_session_dirs_recursive(&projects_dir, &session.session_id)?;
    }

    Ok(())
}

/// Rewrite a JSONL file, excluding all lines where `sessionId` matches.
fn rewrite_jsonl_excluding_session_id(path: &Path, session_id: &str) -> Result<(), io::Error> {
    let content = fs::read_to_string(path)?;
    let mut kept_lines: Vec<&str> = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if line_has_session_id(trimmed, session_id) {
            continue; // skip this line
        }
        kept_lines.push(line);
    }

    let new_content = if kept_lines.is_empty() {
        String::new()
    } else {
        let mut out = kept_lines.join("\n");
        out.push('\n');
        out
    };

    fs::write(path, new_content)
}

/// Check if a JSON line contains `"sessionId": "<id>"`.
/// We parse minimally with serde_json::Value to be resilient to schema changes.
fn line_has_session_id(line: &str, session_id: &str) -> bool {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(line) {
        if let Some(id) = value.get("sessionId").and_then(|v| v.as_str()) {
            return id == session_id;
        }
    }
    false
}

/// Walk a directory tree and remove any subdirectory whose name matches session_id.
fn remove_session_dirs_recursive(base: &Path, session_id: &str) -> Result<(), io::Error> {
    if !base.is_dir() {
        return Ok(());
    }
    for entry in WalkDir::new(base).into_iter().filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.is_dir() && path.file_name().and_then(|n| n.to_str()) == Some(session_id) {
            fs::remove_dir_all(path)?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Codex
// ---------------------------------------------------------------------------

/// Minimal struct to extract `payload.id` from the first line of a Codex session file.
#[derive(Deserialize)]
struct CodexSessionMeta {
    payload: Option<CodexPayload>,
}

#[derive(Deserialize)]
struct CodexPayload {
    id: Option<String>,
}

/// Codex session files live under `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`.
/// We find the file whose first line's `payload.id` matches and delete it.
/// We also rewrite `~/.codex/history.jsonl` excluding matching `session_id` entries.
fn delete_codex_session(session: &Session) -> Result<(), io::Error> {
    let codex_dir = config::codex_dir().map_err(io::Error::other)?;

    // 1. Find and delete the session rollout file
    let sessions_dir = codex_dir.join("sessions");
    if sessions_dir.exists() {
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

            let meta: CodexSessionMeta = match serde_json::from_str(first_line) {
                Ok(m) => m,
                Err(_) => continue,
            };

            let payload_id = meta.payload.and_then(|p| p.id).unwrap_or_default();
            if payload_id == session.session_id {
                fs::remove_file(path)?;
                break;
            }
        }
    }

    // 2. Rewrite history.jsonl excluding lines with matching session_id
    let history_path = codex_dir.join("history.jsonl");
    if history_path.exists() {
        rewrite_jsonl_excluding_codex_session_id(&history_path, &session.session_id)?;
    }

    Ok(())
}

/// Rewrite a Codex history JSONL file, excluding lines where `session_id` matches.
fn rewrite_jsonl_excluding_codex_session_id(
    path: &Path,
    session_id: &str,
) -> Result<(), io::Error> {
    let content = fs::read_to_string(path)?;
    let mut kept_lines: Vec<&str> = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if line_has_codex_session_id(trimmed, session_id) {
            continue;
        }
        kept_lines.push(line);
    }

    let new_content = if kept_lines.is_empty() {
        String::new()
    } else {
        let mut out = kept_lines.join("\n");
        out.push('\n');
        out
    };

    fs::write(path, new_content)
}

/// Check if a Codex history JSON line contains `"session_id": "<id>"`.
fn line_has_codex_session_id(line: &str, session_id: &str) -> bool {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(line) {
        if let Some(id) = value.get("session_id").and_then(|v| v.as_str()) {
            return id == session_id;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// OpenCode
// ---------------------------------------------------------------------------

/// OpenCode sessions are stored in a SQLite database at
/// `~/.local/share/opencode/opencode.db`.
/// We delete the row from the `session` table; foreign-key cascades
/// remove related `message` and `part` rows automatically.
fn delete_opencode_session(session: &Session) -> Result<(), io::Error> {
    let opencode_dir = config::opencode_data_dir().map_err(io::Error::other)?;
    let db_path = opencode_dir.join("opencode.db");
    if !db_path.exists() {
        return Ok(());
    }

    let conn = rusqlite::Connection::open(&db_path)
        .map_err(|e| io::Error::other(format!("SQLite open error: {e}")))?;

    conn.execute("DELETE FROM session WHERE id = ?1", [&session.session_id])
        .map_err(|e| io::Error::other(format!("SQLite delete error: {e}")))?;

    // Also remove JSON storage mirror if it exists
    let session_storage = opencode_dir.join("storage/session");
    if session_storage.exists() {
        // Walk looking for <projectId>/<sessionId>.json
        for entry in WalkDir::new(&session_storage)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            if path.is_file()
                && path.file_stem().and_then(|n| n.to_str()) == Some(&session.session_id)
            {
                let _ = fs::remove_file(path);
            }
        }
    }

    Ok(())
}
