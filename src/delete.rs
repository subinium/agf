use std::fs;
use std::io;
use std::path::Path;

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
        Agent::Pi => delete_pi_session(session),
        Agent::Kiro => delete_kiro_session(session),
        Agent::CursorAgent => delete_cursor_agent_session(session),
        Agent::Gemini => delete_gemini_session(session),
    }
}

// ---------------------------------------------------------------------------
// Shared JSONL helpers
// ---------------------------------------------------------------------------

/// Rewrite a JSONL file, excluding all lines where `json_key` matches `value`.
fn rewrite_jsonl_excluding(path: &Path, json_key: &str, value: &str) -> Result<(), io::Error> {
    let content = fs::read_to_string(path)?;
    let mut kept_lines: Vec<&str> = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if line_has_field_value(trimmed, json_key, value) {
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

/// Check if a JSON line contains `"key": "value"`.
fn line_has_field_value(line: &str, key: &str, value: &str) -> bool {
    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(line) {
        if let Some(v) = parsed.get(key).and_then(|v| v.as_str()) {
            return v == value;
        }
    }
    false
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

    let history_path = claude_dir.join("history.jsonl");
    if history_path.exists() {
        rewrite_jsonl_excluding(&history_path, "sessionId", &session.session_id)?;
    }

    let projects_dir = claude_dir.join("projects");
    if projects_dir.exists() {
        remove_dirs_matching_name(&projects_dir, &session.session_id)?;
    }

    Ok(())
}

/// Walk a directory tree and remove any subdirectory whose name matches the target.
fn remove_dirs_matching_name(base: &Path, name: &str) -> Result<(), io::Error> {
    if !base.is_dir() {
        return Ok(());
    }
    for entry in WalkDir::new(base).into_iter().filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.is_dir() && path.file_name().and_then(|n| n.to_str()) == Some(name) {
            fs::remove_dir_all(path)?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Codex
// ---------------------------------------------------------------------------

/// Codex session files live under `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`.
/// We find the file whose first line's `payload.id` matches and delete it.
/// We also rewrite `~/.codex/history.jsonl` excluding matching `session_id` entries.
fn delete_codex_session(session: &Session) -> Result<(), io::Error> {
    let codex_dir = config::codex_dir().map_err(io::Error::other)?;

    // 1. Find and delete the session rollout file
    let sessions_dir = codex_dir.join("sessions");
    if sessions_dir.exists() {
        delete_codex_session_file(&sessions_dir, &session.session_id)?;
    }

    // 2. Rewrite history.jsonl excluding lines with matching session_id
    let history_path = codex_dir.join("history.jsonl");
    if history_path.exists() {
        rewrite_jsonl_excluding(&history_path, "session_id", &session.session_id)?;
    }

    Ok(())
}

/// Find and delete the Codex rollout JSONL file matching the given session ID.
fn delete_codex_session_file(sessions_dir: &Path, session_id: &str) -> Result<(), io::Error> {
    for entry in WalkDir::new(sessions_dir)
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

        if let Ok(value) = serde_json::from_str::<serde_json::Value>(first_line) {
            let payload_id = value
                .get("payload")
                .and_then(|p| p.get("id"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if payload_id == session_id {
                fs::remove_file(path)?;
                return Ok(());
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// OpenCode
// ---------------------------------------------------------------------------

/// OpenCode sessions are stored in a SQLite database at
/// `~/.local/share/opencode/opencode.db`.
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

// ---------------------------------------------------------------------------
// Pi
// ---------------------------------------------------------------------------

/// Pi sessions are stored as JSONL files under
/// `~/.pi/agent/sessions/<encoded-cwd>/<timestamp>_<sessionId>.jsonl`.
fn delete_pi_session(session: &Session) -> Result<(), io::Error> {
    let sessions_dir = config::pi_sessions_dir().map_err(io::Error::other)?;
    if !sessions_dir.exists() {
        return Ok(());
    }

    for entry in WalkDir::new(&sessions_dir)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();
        if !path.is_file() || path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
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

        if let Ok(value) = serde_json::from_str::<serde_json::Value>(first_line) {
            if value.get("type").and_then(|v| v.as_str()) == Some("session")
                && value.get("id").and_then(|v| v.as_str()) == Some(&session.session_id)
            {
                fs::remove_file(path)?;
                return Ok(());
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Kiro
// ---------------------------------------------------------------------------

/// Kiro sessions are stored in a SQLite database at
/// `~/Library/Application Support/kiro-cli/data.sqlite3` (macOS) or
/// `~/.local/share/kiro-cli/data.sqlite3` (Linux).
fn delete_kiro_session(session: &Session) -> Result<(), io::Error> {
    let data_dir = config::kiro_data_dir().map_err(io::Error::other)?;
    let db_path = data_dir.join("data.sqlite3");
    if !db_path.exists() {
        return Ok(());
    }

    let conn = rusqlite::Connection::open(&db_path)
        .map_err(|e| io::Error::other(format!("SQLite open error: {e}")))?;

    conn.execute(
        "DELETE FROM conversations_v2 WHERE conversation_id = ?1",
        [&session.session_id],
    )
    .map_err(|e| io::Error::other(format!("SQLite delete error: {e}")))?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Cursor Agent
// ---------------------------------------------------------------------------

/// Cursor Agent sessions are stored in two locations:
/// 1. `~/.cursor/chats/<workspace-hash>/<session_id>/store.db` (SQLite)
/// 2. `~/.cursor/projects/*/agent-transcripts/<session_id>.txt` (transcript files)
fn delete_cursor_agent_session(session: &Session) -> Result<(), io::Error> {
    let cursor_dir = config::cursor_dir().map_err(io::Error::other)?;

    // 1. Remove chat directory: ~/.cursor/chats/*/<session_id>/
    let chats_dir = cursor_dir.join("chats");
    if chats_dir.exists() {
        remove_dirs_matching_name(&chats_dir, &session.session_id)?;
    }

    // 2. Remove transcript files: ~/.cursor/projects/*/agent-transcripts/<session_id>.txt
    let projects_dir = cursor_dir.join("projects");
    if projects_dir.exists() {
        let transcript_name = format!("{}.txt", session.session_id);
        for entry in WalkDir::new(&projects_dir)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            if path.is_file() && path.file_name().and_then(|n| n.to_str()) == Some(&transcript_name)
            {
                let _ = fs::remove_file(path);
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Gemini
// ---------------------------------------------------------------------------

/// Gemini sessions are stored as JSON files under
/// `~/.gemini/tmp/<project-name-or-hash>/chats/session-<date>-<short-id>.json`.
fn delete_gemini_session(session: &Session) -> Result<(), io::Error> {
    let gemini_dir = config::gemini_dir().map_err(io::Error::other)?;
    let tmp_dir = gemini_dir.join("tmp");
    if !tmp_dir.exists() {
        return Ok(());
    }

    for project_entry in fs::read_dir(&tmp_dir)?.filter_map(|e| e.ok()) {
        let chats_dir = project_entry.path().join("chats");
        if !chats_dir.is_dir() {
            continue;
        }

        for chat_entry in fs::read_dir(&chats_dir)?.filter_map(|e| e.ok()) {
            let path = chat_entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }

            let Ok(content) = fs::read_to_string(&path) else {
                continue;
            };

            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) {
                if json
                    .get("sessionId")
                    .and_then(|v| v.as_str())
                    .map(|id| id == session.session_id)
                    .unwrap_or(false)
                {
                    fs::remove_file(&path)?;
                    return Ok(());
                }
            }
        }
    }

    Ok(())
}
