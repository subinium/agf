use std::fs;
use std::io;
use std::path::Path;

use walkdir::WalkDir;

use crate::config;
use crate::model::{Agent, Session};

/// Delete a session's data files. Returns Ok(()) on success.
/// Only removes session data, NOT the project directory.
pub fn delete_session(session: &Session) -> Result<(), io::Error> {
    // Defense in depth: never accept a session_id that could escape the intended
    // directory via path traversal. Agent scanners should already sanitize IDs.
    debug_assert!(
        !session.session_id.contains('/') && !session.session_id.contains(".."),
        "session_id must not contain path separators or parent traversal",
    );
    match session.agent {
        Agent::ClaudeCode => delete_claude_session(session),
        Agent::Codex => delete_codex_session(session),
        Agent::OpenCode => delete_opencode_session(session),
        Agent::Pi => delete_pi_session(session),
        Agent::Kiro => delete_kiro_session(session),
        Agent::CursorAgent => delete_cursor_agent_session(session),
        Agent::Gemini => delete_gemini_session(session),
        Agent::Hermes => delete_hermes_session(session),
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
    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(line)
        && let Some(v) = parsed.get(key).and_then(|v| v.as_str())
    {
        return v == value;
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
    for entry in WalkDir::new(base).into_iter().flatten() {
        let path = entry.path();
        if path.is_dir() && path.file_name().and_then(|n| n.to_str()) == Some(name) {
            fs::remove_dir_all(path)?;
        }
    }
    Ok(())
}

/// Walk a directory tree and remove any regular file whose name (including
/// extension) matches the target.
fn remove_files_matching_name(base: &Path, name: &str) -> Result<(), io::Error> {
    if !base.is_dir() {
        return Ok(());
    }
    for entry in WalkDir::new(base).into_iter().flatten() {
        let path = entry.path();
        if path.is_file() && path.file_name().and_then(|n| n.to_str()) == Some(name) {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Codex
// ---------------------------------------------------------------------------

/// Codex session data lives in three places (any combination may be present
/// depending on Codex CLI version):
///   * `state_*.sqlite` `threads` row (primary source for current Codex CLI),
///   * `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl` rollout file,
///   * `~/.codex/history.jsonl` per-prompt summary entries.
///
/// We delete from all three so the session does not reappear after the next
/// `agf` scan via the SQLite path.
fn delete_codex_session(session: &Session) -> Result<(), io::Error> {
    let codex_dir = config::codex_dir().map_err(io::Error::other)?;

    // 1. Delete the SQLite row(s). Codex may have multiple `state_*.sqlite`
    //    files (e.g. across CLI upgrades); remove the row from every one
    //    that contains it so the session cannot be revived from a stale db.
    delete_codex_sqlite_rows(&codex_dir, &session.session_id)?;

    // 2. Find and delete the session rollout file
    let sessions_dir = codex_dir.join("sessions");
    if sessions_dir.exists() {
        delete_codex_session_file(&sessions_dir, &session.session_id)?;
    }

    // 3. Rewrite history.jsonl excluding lines with matching session_id
    let history_path = codex_dir.join("history.jsonl");
    if history_path.exists() {
        rewrite_jsonl_excluding(&history_path, "session_id", &session.session_id)?;
    }

    Ok(())
}

/// Remove the `threads` row matching `session_id` from every
/// `state_*.sqlite` in `codex_dir`. Missing tables / open errors on a single
/// file are swallowed so one corrupt or older-schema db cannot block the
/// delete on the others.
fn delete_codex_sqlite_rows(codex_dir: &Path, session_id: &str) -> Result<(), io::Error> {
    let Ok(entries) = fs::read_dir(codex_dir) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let is_state_db = path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("state_") && n.ends_with(".sqlite"));
        if !is_state_db {
            continue;
        }
        let Ok(conn) = rusqlite::Connection::open(&path) else {
            continue;
        };
        // `threads` is the only table the Codex scanner reads from. Older
        // Codex CLI versions may not have this table — ignore the error.
        let _ = conn.execute("DELETE FROM threads WHERE id = ?1", [session_id]);
    }
    Ok(())
}

/// Find and delete the Codex rollout JSONL file matching the given session ID.
fn delete_codex_session_file(sessions_dir: &Path, session_id: &str) -> Result<(), io::Error> {
    for entry in WalkDir::new(sessions_dir).into_iter().flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }

        let Ok(content) = fs::read_to_string(path) else {
            continue;
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
        for entry in WalkDir::new(&session_storage).into_iter().flatten() {
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

    for entry in WalkDir::new(&sessions_dir).into_iter().flatten() {
        let path = entry.path();
        if !path.is_file() || path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }

        let Ok(content) = fs::read_to_string(path) else {
            continue;
        };

        let first_line = match content.lines().next() {
            Some(line) if !line.trim().is_empty() => line.trim(),
            _ => continue,
        };

        if let Ok(value) = serde_json::from_str::<serde_json::Value>(first_line)
            && value.get("type").and_then(|v| v.as_str()) == Some("session")
            && value.get("id").and_then(|v| v.as_str()) == Some(&session.session_id)
        {
            fs::remove_file(path)?;
            return Ok(());
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

/// Cursor Agent sessions are stored across two layouts:
/// - Current (Composer 2+) JSONL: directory at
///   `~/.cursor/projects/*/agent-transcripts/<session_id>/` containing
///   `<session_id>.jsonl`, plus chat metadata under
///   `~/.cursor/chats/<workspace-hash>/<session_id>/store.db`.
/// - Legacy: file at `~/.cursor/projects/*/agent-transcripts/<session_id>.txt`
///   with no `chats/` counterpart.
///
/// Both shapes are still surfaced by the scanner (see `scan_from`), so delete
/// must remove the directory AND the file form — otherwise legacy sessions
/// silently no-op (`remove_dirs_matching_name` filters on `is_dir()`) and the
/// next scan resurrects the orphan.
fn delete_cursor_agent_session(session: &Session) -> Result<(), io::Error> {
    let cursor_dir = config::cursor_dir().map_err(io::Error::other)?;

    // 1. Chat metadata: ~/.cursor/chats/*/<session_id>/
    let chats_dir = cursor_dir.join("chats");
    if chats_dir.exists() {
        remove_dirs_matching_name(&chats_dir, &session.session_id)?;
    }

    // 2. Transcript: directory form (JSONL) and file form (legacy .txt).
    let projects_dir = cursor_dir.join("projects");
    if projects_dir.exists() {
        remove_dirs_matching_name(&projects_dir, &session.session_id)?;
        let legacy_txt = format!("{}.txt", session.session_id);
        remove_files_matching_name(&projects_dir, &legacy_txt)?;
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

    for project_entry in fs::read_dir(&tmp_dir)?.flatten() {
        let chats_dir = project_entry.path().join("chats");
        if !chats_dir.is_dir() {
            continue;
        }

        for chat_entry in fs::read_dir(&chats_dir)?.flatten() {
            let path = chat_entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }

            let Ok(content) = fs::read_to_string(&path) else {
                continue;
            };

            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content)
                && json
                    .get("sessionId")
                    .and_then(|v| v.as_str())
                    .is_some_and(|id| id == session.session_id)
            {
                fs::remove_file(&path)?;
                return Ok(());
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Hermes
// ---------------------------------------------------------------------------

/// Hermes Agent sessions are stored in a SQLite database at
/// `~/.hermes/state.db`. Messages are in a separate `messages` table
/// with a foreign key to `sessions.id`. The four DELETEs are wrapped in
/// a single transaction so a mid-cascade failure can't leave the DB in
/// an inconsistent state (e.g. orphan messages whose sessions row is
/// gone, which would surface as ghost rows on the next scan).
fn delete_hermes_session(session: &Session) -> Result<(), io::Error> {
    let hermes_dir = config::hermes_dir().map_err(io::Error::other)?;
    let db_path = hermes_dir.join("state.db");
    if !db_path.exists() {
        return Ok(());
    }

    let mut conn = rusqlite::Connection::open(&db_path)
        .map_err(|e| io::Error::other(format!("SQLite open error: {e}")))?;

    let tx = conn
        .transaction()
        .map_err(|e| io::Error::other(format!("SQLite begin tx error: {e}")))?;

    // Delete messages first (foreign key constraint).
    tx.execute(
        "DELETE FROM messages WHERE session_id = ?1",
        [&session.session_id],
    )
    .map_err(|e| io::Error::other(format!("SQLite delete messages error: {e}")))?;

    // Delete child sessions' messages and then the child sessions themselves.
    tx.execute(
        "DELETE FROM messages WHERE session_id IN \
         (SELECT id FROM sessions WHERE parent_session_id = ?1)",
        [&session.session_id],
    )
    .map_err(|e| io::Error::other(format!("SQLite delete child messages error: {e}")))?;

    tx.execute(
        "DELETE FROM sessions WHERE parent_session_id = ?1",
        [&session.session_id],
    )
    .map_err(|e| io::Error::other(format!("SQLite delete child sessions error: {e}")))?;

    // Delete the parent session.
    tx.execute("DELETE FROM sessions WHERE id = ?1", [&session.session_id])
        .map_err(|e| io::Error::other(format!("SQLite delete session error: {e}")))?;

    tx.commit()
        .map_err(|e| io::Error::other(format!("SQLite commit error: {e}")))?;

    // Also remove any on-disk session JSON dumps. These are best-effort:
    // a failure here doesn't undo the DB delete (which is the source of
    // truth for the listing), so we swallow the error.
    let sessions_dir = hermes_dir.join("sessions");
    if sessions_dir.exists() {
        let prefix = format!("session_{}", session.session_id);
        if let Ok(entries) = fs::read_dir(&sessions_dir) {
            for entry in entries.flatten() {
                if let Some(name) = entry.file_name().to_str()
                    && name.starts_with(&prefix)
                    && name.ends_with(".json")
                {
                    let _ = fs::remove_file(entry.path());
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_codex_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(name);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Create a minimal `state_*.sqlite` mirroring the Codex schema fields
    /// the scanner reads, then seed two `threads` rows.
    fn seed_state_db(path: &Path, ids: &[&str]) {
        let conn = rusqlite::Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE threads (
                id TEXT PRIMARY KEY,
                cwd TEXT NOT NULL,
                title TEXT NOT NULL DEFAULT '',
                updated_at INTEGER NOT NULL DEFAULT 0,
                archived INTEGER NOT NULL DEFAULT 0,
                git_branch TEXT,
                first_user_message TEXT NOT NULL DEFAULT ''
            );",
        )
        .unwrap();
        for id in ids {
            conn.execute("INSERT INTO threads (id, cwd) VALUES (?1, '/tmp/x')", [id])
                .unwrap();
        }
    }

    fn count_thread(path: &Path, id: &str) -> i64 {
        let conn = rusqlite::Connection::open(path).unwrap();
        conn.query_row("SELECT COUNT(*) FROM threads WHERE id = ?1", [id], |row| {
            row.get(0)
        })
        .unwrap()
    }

    #[test]
    fn delete_codex_sqlite_rows_removes_target_only() {
        let dir = make_codex_dir("agf-test-codex-delete-target");
        let db = dir.join("state_5.sqlite");
        seed_state_db(&db, &["target-id", "keep-id"]);

        delete_codex_sqlite_rows(&dir, "target-id").unwrap();

        assert_eq!(count_thread(&db, "target-id"), 0);
        assert_eq!(count_thread(&db, "keep-id"), 1);
    }

    #[test]
    fn delete_codex_sqlite_rows_walks_every_state_db() {
        let dir = make_codex_dir("agf-test-codex-delete-multi");
        let db4 = dir.join("state_4.sqlite");
        let db5 = dir.join("state_5.sqlite");
        seed_state_db(&db4, &["dup-id"]);
        seed_state_db(&db5, &["dup-id"]);

        delete_codex_sqlite_rows(&dir, "dup-id").unwrap();

        assert_eq!(count_thread(&db4, "dup-id"), 0);
        assert_eq!(count_thread(&db5, "dup-id"), 0);
    }

    #[test]
    fn delete_codex_sqlite_rows_ignores_non_state_files() {
        let dir = make_codex_dir("agf-test-codex-delete-ignore");
        // A non-matching file must not crash the walk.
        fs::write(dir.join("history.jsonl"), b"").unwrap();
        let db = dir.join("state_1.sqlite");
        seed_state_db(&db, &["a"]);

        delete_codex_sqlite_rows(&dir, "a").unwrap();
        assert_eq!(count_thread(&db, "a"), 0);
    }

    #[test]
    fn delete_codex_sqlite_rows_tolerates_missing_threads_table() {
        let dir = make_codex_dir("agf-test-codex-delete-missing-table");
        let db = dir.join("state_0.sqlite");
        // Empty db (no `threads` table) — Codex versions without the table
        // must not abort the delete.
        rusqlite::Connection::open(&db).unwrap();

        // Should swallow the "no such table: threads" error.
        delete_codex_sqlite_rows(&dir, "anything").unwrap();
    }

    #[test]
    fn delete_cursor_agent_removes_transcript_dir_not_sibling() {
        let base = make_codex_dir("agf-test-cursor-delete");
        let transcripts = base.join("projects/encproj/agent-transcripts");

        let target_uuid = "ddddddddddddddddddddddddddddddd1";
        let sibling_uuid = "ddddddddddddddddddddddddddddddd2";

        let target_dir = transcripts.join(target_uuid);
        let sibling_dir = transcripts.join(sibling_uuid);
        fs::create_dir_all(&target_dir).unwrap();
        fs::create_dir_all(&sibling_dir).unwrap();
        fs::write(target_dir.join(format!("{target_uuid}.jsonl")), b"{}").unwrap();
        fs::write(sibling_dir.join(format!("{sibling_uuid}.jsonl")), b"{}").unwrap();

        let projects_dir = base.join("projects");
        remove_dirs_matching_name(&projects_dir, target_uuid).unwrap();

        assert!(!target_dir.exists(), "target session dir should be deleted");
        assert!(sibling_dir.exists(), "sibling session dir must survive");
    }

    /// Regression: legacy Cursor sessions live as plain files at
    /// `projects/<slug>/agent-transcripts/<uuid>.txt`, not as directories.
    /// `remove_dirs_matching_name` filters on `is_dir()`, so the dir-only
    /// pass added in #45 left these files behind and the next scan
    /// resurrected the orphan. Delete must remove both shapes.
    #[test]
    fn delete_cursor_agent_removes_legacy_txt_transcript() {
        let base = make_codex_dir("agf-test-cursor-legacy-txt");
        let transcripts = base.join("projects/encproj/agent-transcripts");
        fs::create_dir_all(&transcripts).unwrap();

        let target_uuid = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeee1";
        let sibling_uuid = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeee2";

        let target_file = transcripts.join(format!("{target_uuid}.txt"));
        let sibling_file = transcripts.join(format!("{sibling_uuid}.txt"));
        fs::write(&target_file, b"legacy transcript").unwrap();
        fs::write(&sibling_file, b"legacy transcript").unwrap();

        let projects_dir = base.join("projects");
        remove_files_matching_name(&projects_dir, &format!("{target_uuid}.txt")).unwrap();

        assert!(
            !target_file.exists(),
            "legacy .txt transcript should be deleted",
        );
        assert!(
            sibling_file.exists(),
            "unrelated sibling legacy .txt must survive",
        );
    }
}
