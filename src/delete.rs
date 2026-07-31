use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::Path;

use walkdir::WalkDir;

use crate::config;
use crate::model::{Agent, Session};
use crate::scanner::{read_first_line, read_head_lines};

/// Delete one session's data files. Returns Ok(()) on success.
/// Only removes session data, NOT the project directory.
pub fn delete_session(session: &Session) -> Result<(), io::Error> {
    let ids = HashSet::from([session.session_id.as_str()]);
    delete_agent_sessions(session.agent, &ids)
}

/// Delete every session named in `selection`, doing **one** filesystem or
/// database pass per agent rather than one per session.
///
/// Bulk delete used to call `delete_session` in a loop, so N selected sessions
/// meant N full walks of `~/.claude/projects` (or of the pi sessions tree, each
/// walk reading every transcript in full) plus N read-modify-write cycles over
/// `history.jsonl`. Grouping by agent makes that one pass regardless of N.
///
/// Returns the subset that was actually removed, so the caller can keep the
/// rows whose data is still on disk visible instead of showing a delete that
/// did not happen.
pub fn delete_selection(
    selection: &HashMap<Agent, HashSet<String>>,
) -> HashMap<Agent, HashSet<String>> {
    let mut deleted = HashMap::new();
    for (&agent, ids) in selection {
        if ids.is_empty() {
            continue;
        }
        let borrowed: HashSet<&str> = ids.iter().map(String::as_str).collect();
        if delete_agent_sessions(agent, &borrowed).is_ok() {
            deleted.insert(agent, ids.clone());
            continue;
        }
        // The batch aborted somewhere, and a batch cannot say how far it got.
        // Retry the ids one at a time to recover per-session truth: a row may
        // only disappear from the list once its own data is gone. This costs a
        // pass per id, but only on the error path.
        let individually: HashSet<String> = ids
            .iter()
            .filter(|id| delete_agent_sessions(agent, &HashSet::from([id.as_str()])).is_ok())
            .cloned()
            .collect();
        if !individually.is_empty() {
            deleted.insert(agent, individually);
        }
    }
    deleted
}

fn delete_agent_sessions(agent: Agent, ids: &HashSet<&str>) -> Result<(), io::Error> {
    // Refuse the whole batch if any id is unsafe: acting on part of a delete
    // whose input we distrust is worse than refusing all of it.
    if let Some(bad) = ids.iter().find(|id| !is_safe_session_id(id)) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("refusing to delete with unsafe session id: {bad:?}"),
        ));
    }
    match agent {
        Agent::ClaudeCode => delete_claude_sessions(ids),
        Agent::Codex => delete_codex_sessions(ids),
        Agent::OpenCode => delete_opencode_sessions(ids),
        Agent::Pi => {
            delete_pi_style_sessions(&config::pi_sessions_dir().map_err(io::Error::other)?, ids)
        }
        Agent::OhMyPi => delete_pi_style_sessions(
            &config::oh_my_pi_sessions_dir().map_err(io::Error::other)?,
            ids,
        ),
        Agent::Kiro => delete_kiro_sessions(ids),
        Agent::CursorAgent => delete_cursor_agent_sessions(ids),
        Agent::Gemini => delete_gemini_sessions(ids),
        Agent::Hermes => delete_hermes_sessions(ids),
        Agent::Yolop => delete_yolop_sessions(ids),
    }
}

/// Reject ids that could escape the directory they get joined onto, or that
/// name nothing at all.
///
/// Scanners validate what they emit, but a `Session` is also rebuilt from
/// `~/.cache/agf/sessions.json` — an ordinary, user-writable JSON file that
/// re-runs none of those checks. This was previously a `debug_assert!`, which
/// the release profile compiles out, leaving the release binary with no guard
/// at all on the one path (Yolop) that joins an id straight onto a directory
/// and calls `remove_dir_all`.
fn is_safe_session_id(id: &str) -> bool {
    !id.is_empty()
        && id != "."
        && !id.contains('/')
        && !id.contains('\\')
        && !id.contains("..")
        && !id.contains('\0')
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Rewrite a JSONL file, excluding every line whose `json_key` is in `values`.
///
/// Skips the write entirely when nothing matched, so a bulk delete that touches
/// one agent does not rewrite another's multi-MB history file for nothing.
fn rewrite_jsonl_excluding(
    path: &Path,
    json_key: &str,
    values: &HashSet<&str>,
) -> Result<(), io::Error> {
    let content = fs::read_to_string(path)?;
    let mut kept = String::with_capacity(content.len());
    let mut dropped_any = false;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if line_field_matches_any(trimmed, json_key, values) {
            dropped_any = true;
            continue;
        }
        kept.push_str(line);
        kept.push('\n');
    }

    if !dropped_any {
        return Ok(());
    }
    crate::fsx::write_atomic(path, kept.as_bytes())
}

/// Check whether a JSON line's `key` holds any of `values`.
fn line_field_matches_any(line: &str, key: &str, values: &HashSet<&str>) -> bool {
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(line) else {
        return false;
    };
    parsed
        .get(key)
        .and_then(|v| v.as_str())
        .is_some_and(|v| values.contains(v))
}

/// Single walk of `base` removing every directory named in `dir_names` and
/// every regular file named in `file_names`.
///
/// Both shapes are handled in one traversal because Cursor stores current
/// sessions as `<id>/` directories and legacy ones as `<id>.txt` files under
/// the same tree.
fn remove_matching_entries(
    base: &Path,
    dir_names: &HashSet<&str>,
    file_names: &HashSet<&str>,
) -> Result<(), io::Error> {
    if !base.is_dir() {
        return Ok(());
    }
    for entry in WalkDir::new(base).into_iter().flatten() {
        // `entry.file_type()` reuses the readdir result; `path.is_dir()` would
        // pay a fresh stat for every file in the tree.
        let file_type = entry.file_type();
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if file_type.is_dir() {
            if dir_names.contains(name) {
                fs::remove_dir_all(path)?;
            }
        } else if file_type.is_file() && file_names.contains(name) {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Claude Code
// ---------------------------------------------------------------------------

/// Claude sessions are listed as lines in `~/.claude/history.jsonl`.
/// We rewrite the file excluding all lines whose `sessionId` matches, and
/// remove any project-specific session directory under `~/.claude/projects/`.
fn delete_claude_sessions(ids: &HashSet<&str>) -> Result<(), io::Error> {
    let claude_dir = config::claude_dir().map_err(io::Error::other)?;

    let history_path = claude_dir.join("history.jsonl");
    if history_path.exists() {
        rewrite_jsonl_excluding(&history_path, "sessionId", ids)?;
    }

    let projects_dir = claude_dir.join("projects");
    remove_matching_entries(&projects_dir, ids, &HashSet::new())
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
fn delete_codex_sessions(ids: &HashSet<&str>) -> Result<(), io::Error> {
    let codex_dir = config::codex_dir().map_err(io::Error::other)?;

    delete_codex_sqlite_rows(&codex_dir, ids)?;

    let sessions_dir = codex_dir.join("sessions");
    if sessions_dir.exists() {
        delete_codex_session_files(&sessions_dir, ids)?;
    }

    let history_path = codex_dir.join("history.jsonl");
    if history_path.exists() {
        rewrite_jsonl_excluding(&history_path, "session_id", ids)?;
    }

    Ok(())
}

/// Remove the `threads` rows matching `ids` from every `state_*.sqlite` in
/// `codex_dir`. Missing tables / open errors on a single file are swallowed so
/// one corrupt or older-schema db cannot block the delete on the others.
fn delete_codex_sqlite_rows(codex_dir: &Path, ids: &HashSet<&str>) -> Result<(), io::Error> {
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
        for id in ids {
            let _ = conn.execute("DELETE FROM threads WHERE id = ?1", [*id]);
        }
    }
    Ok(())
}

/// Find and delete the Codex rollout JSONL files matching `ids`.
fn delete_codex_session_files(sessions_dir: &Path, ids: &HashSet<&str>) -> Result<(), io::Error> {
    let mut remaining = ids.len();
    for entry in WalkDir::new(sessions_dir).into_iter().flatten() {
        if remaining == 0 {
            break;
        }
        let path = entry.path();
        if !entry.file_type().is_file()
            || path.extension().and_then(|e| e.to_str()) != Some("jsonl")
        {
            continue;
        }

        // The rollout id lives in the very first record; reading the whole
        // file (as this used to) means slurping every rollout on disk.
        let Some(first_line) = read_first_line(path) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(first_line.trim()) else {
            continue;
        };
        let payload_id = value
            .get("payload")
            .and_then(|p| p.get("id"))
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        if ids.contains(payload_id) {
            fs::remove_file(path)?;
            remaining -= 1;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// OpenCode
// ---------------------------------------------------------------------------

/// OpenCode sessions are stored in a SQLite database at
/// `~/.local/share/opencode/opencode.db`.
fn delete_opencode_sessions(ids: &HashSet<&str>) -> Result<(), io::Error> {
    let opencode_dir = config::opencode_data_dir().map_err(io::Error::other)?;
    let db_path = opencode_dir.join("opencode.db");
    if !db_path.exists() {
        return Ok(());
    }

    let mut conn = rusqlite::Connection::open(&db_path)
        .map_err(|e| io::Error::other(format!("SQLite open error: {e}")))?;
    let tx = conn
        .transaction()
        .map_err(|e| io::Error::other(format!("SQLite begin tx error: {e}")))?;
    for id in ids {
        tx.execute("DELETE FROM session WHERE id = ?1", [*id])
            .map_err(|e| io::Error::other(format!("SQLite delete error: {e}")))?;
    }
    tx.commit()
        .map_err(|e| io::Error::other(format!("SQLite commit error: {e}")))?;

    // Also remove JSON storage mirrors if they exist. Best-effort: the DB is
    // the source of truth for the listing.
    let session_storage = opencode_dir.join("storage/session");
    if session_storage.is_dir() {
        for entry in WalkDir::new(&session_storage).into_iter().flatten() {
            let path = entry.path();
            if entry.file_type().is_file()
                && path
                    .file_stem()
                    .and_then(|n| n.to_str())
                    .is_some_and(|stem| ids.contains(stem))
            {
                let _ = fs::remove_file(path);
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Pi / Oh My Pi
// ---------------------------------------------------------------------------

/// The `session` header record is line 1 for pi and line 2 for Oh My Pi (which
/// writes a `title` record first). These bounds cover both with a wide margin
/// while keeping a bulk delete from reading every byte of every transcript.
const PI_HEADER_SCAN_LINES: usize = 32;
const PI_HEADER_SCAN_BYTES: u64 = 64 * 1024;

/// pi and Oh My Pi both store sessions as JSONL files under
/// `<root>/<encoded-cwd>/<timestamp>_<sessionId>.jsonl`.
fn delete_pi_style_sessions(sessions_dir: &Path, ids: &HashSet<&str>) -> Result<(), io::Error> {
    if !sessions_dir.exists() {
        return Ok(());
    }

    let mut remaining = ids.len();
    for entry in WalkDir::new(sessions_dir).into_iter().flatten() {
        if remaining == 0 {
            break;
        }
        let path = entry.path();
        if !entry.file_type().is_file()
            || path.extension().and_then(|e| e.to_str()) != Some("jsonl")
        {
            continue;
        }

        let matches = read_head_lines(path, PI_HEADER_SCAN_LINES, PI_HEADER_SCAN_BYTES)
            .iter()
            .any(|line| pi_header_matches(line, ids));
        if matches {
            fs::remove_file(path)?;
            remaining -= 1;
        }
    }

    Ok(())
}

fn pi_header_matches(line: &str, ids: &HashSet<&str>) -> bool {
    if line.is_empty() {
        return false;
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
        return false;
    };
    value.get("type").and_then(|v| v.as_str()) == Some("session")
        && value
            .get("id")
            .and_then(|v| v.as_str())
            .is_some_and(|id| ids.contains(id))
}

// ---------------------------------------------------------------------------
// Kiro
// ---------------------------------------------------------------------------

/// Kiro sessions are stored in a SQLite database at
/// `~/Library/Application Support/kiro-cli/data.sqlite3` (macOS) or
/// `~/.local/share/kiro-cli/data.sqlite3` (Linux).
fn delete_kiro_sessions(ids: &HashSet<&str>) -> Result<(), io::Error> {
    let data_dir = config::kiro_data_dir().map_err(io::Error::other)?;
    let db_path = data_dir.join("data.sqlite3");
    if !db_path.exists() {
        return Ok(());
    }

    let mut conn = rusqlite::Connection::open(&db_path)
        .map_err(|e| io::Error::other(format!("SQLite open error: {e}")))?;
    let tx = conn
        .transaction()
        .map_err(|e| io::Error::other(format!("SQLite begin tx error: {e}")))?;
    for id in ids {
        tx.execute(
            "DELETE FROM conversations_v2 WHERE conversation_id = ?1",
            [*id],
        )
        .map_err(|e| io::Error::other(format!("SQLite delete error: {e}")))?;
    }
    tx.commit()
        .map_err(|e| io::Error::other(format!("SQLite commit error: {e}")))
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
/// silently no-op and the next scan resurrects the orphan.
fn delete_cursor_agent_sessions(ids: &HashSet<&str>) -> Result<(), io::Error> {
    let cursor_dir = config::cursor_dir().map_err(io::Error::other)?;

    // 1. Chat metadata: ~/.cursor/chats/*/<session_id>/
    remove_matching_entries(&cursor_dir.join("chats"), ids, &HashSet::new())?;

    // 2. Transcript: directory form (JSONL) and file form (legacy .txt), in a
    //    single traversal of the projects tree.
    let legacy_names: Vec<String> = ids.iter().map(|id| format!("{id}.txt")).collect();
    let legacy_names: HashSet<&str> = legacy_names.iter().map(String::as_str).collect();
    remove_matching_entries(&cursor_dir.join("projects"), ids, &legacy_names)
}

// ---------------------------------------------------------------------------
// Gemini
// ---------------------------------------------------------------------------

/// Gemini sessions are stored as JSON files under
/// `~/.gemini/tmp/<project-name-or-hash>/chats/session-<date>-<short-id>.json`.
fn delete_gemini_sessions(ids: &HashSet<&str>) -> Result<(), io::Error> {
    let gemini_dir = config::gemini_dir().map_err(io::Error::other)?;
    let tmp_dir = gemini_dir.join("tmp");
    if !tmp_dir.exists() {
        return Ok(());
    }

    let mut remaining = ids.len();
    for project_entry in fs::read_dir(&tmp_dir)?.flatten() {
        if remaining == 0 {
            break;
        }
        let chats_dir = project_entry.path().join("chats");
        if !chats_dir.is_dir() {
            continue;
        }

        for chat_entry in fs::read_dir(&chats_dir)?.flatten() {
            if remaining == 0 {
                break;
            }
            let path = chat_entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }

            let Ok(content) = fs::read_to_string(&path) else {
                continue;
            };
            let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) else {
                continue;
            };
            if json
                .get("sessionId")
                .and_then(|v| v.as_str())
                .is_some_and(|id| ids.contains(id))
            {
                fs::remove_file(&path)?;
                remaining -= 1;
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
/// with a foreign key to `sessions.id`. Every DELETE is wrapped in a
/// single transaction so a mid-cascade failure can't leave the DB in
/// an inconsistent state (e.g. orphan messages whose sessions row is
/// gone, which would surface as ghost rows on the next scan).
fn delete_hermes_sessions(ids: &HashSet<&str>) -> Result<(), io::Error> {
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

    for id in ids {
        // Delete messages first (foreign key constraint).
        tx.execute("DELETE FROM messages WHERE session_id = ?1", [*id])
            .map_err(|e| io::Error::other(format!("SQLite delete messages error: {e}")))?;

        // Delete child sessions' messages and then the child sessions themselves.
        tx.execute(
            "DELETE FROM messages WHERE session_id IN \
             (SELECT id FROM sessions WHERE parent_session_id = ?1)",
            [*id],
        )
        .map_err(|e| io::Error::other(format!("SQLite delete child messages error: {e}")))?;

        tx.execute("DELETE FROM sessions WHERE parent_session_id = ?1", [*id])
            .map_err(|e| io::Error::other(format!("SQLite delete child sessions error: {e}")))?;

        // Delete the parent session.
        tx.execute("DELETE FROM sessions WHERE id = ?1", [*id])
            .map_err(|e| io::Error::other(format!("SQLite delete session error: {e}")))?;
    }

    tx.commit()
        .map_err(|e| io::Error::other(format!("SQLite commit error: {e}")))?;

    // Also remove any on-disk session JSON dumps. These are best-effort:
    // a failure here doesn't undo the DB delete (which is the source of
    // truth for the listing), so we swallow the error.
    let sessions_dir = hermes_dir.join("sessions");
    if let Ok(entries) = fs::read_dir(&sessions_dir) {
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str()
                && name.ends_with(".json")
                && ids
                    .iter()
                    .any(|id| name.starts_with(&format!("session_{id}")))
            {
                let _ = fs::remove_file(entry.path());
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Yolop
// ---------------------------------------------------------------------------

fn delete_yolop_sessions(ids: &HashSet<&str>) -> Result<(), io::Error> {
    let sessions_dir = config::yolop_sessions_dir().map_err(io::Error::other)?;
    for id in ids {
        delete_yolop_session_from(&sessions_dir, id)?;
    }
    Ok(())
}

fn delete_yolop_session_from(sessions_dir: &Path, session_id: &str) -> Result<(), io::Error> {
    // Yolop is the only agent whose delete joins the id straight onto a
    // directory and calls `remove_dir_all`, so it re-checks the id against the
    // exact `session_<32 hex>` shape its scanner emits — a stricter gate than
    // the generic `is_safe_session_id` traversal check.
    if !crate::scanner::yolop::valid_session_id(session_id) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("not a Yolop session id: {session_id:?}"),
        ));
    }
    let session_dir = sessions_dir.join(session_id);
    if session_dir.is_dir() {
        fs::remove_dir_all(session_dir)?;
    }
    let legacy_log = sessions_dir.join(format!("{session_id}.jsonl"));
    if legacy_log.is_file() {
        fs::remove_file(legacy_log)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn ids(list: &[&'static str]) -> HashSet<&'static str> {
        list.iter().copied().collect()
    }

    fn make_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(name);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Create a minimal `state_*.sqlite` mirroring the Codex schema fields
    /// the scanner reads, then seed `threads` rows.
    fn seed_state_db(path: &Path, rows: &[&str]) {
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
        for id in rows {
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

    // -- session id safety ---------------------------------------------------

    #[test]
    fn is_safe_session_id_rejects_traversal_and_separators() {
        assert!(is_safe_session_id("019e14f4-c9a5-76dc-b7b6-0613e602a620"));
        // Hermes ids carry colons; those are harmless as a path component.
        assert!(is_safe_session_id("dashboard:admin:main"));

        assert!(!is_safe_session_id(""));
        assert!(!is_safe_session_id("."));
        assert!(!is_safe_session_id(".."));
        assert!(!is_safe_session_id("../../etc"));
        assert!(!is_safe_session_id("a/b"));
        assert!(!is_safe_session_id(r"a\b"));
    }

    /// Regression: the traversal guard used to be a `debug_assert!`, so the
    /// release binary had none. A tampered `~/.cache/agf/sessions.json` could
    /// therefore drive `remove_dir_all` outside the sessions directory.
    #[test]
    fn delete_agent_sessions_refuses_traversal_id_in_release_semantics() {
        let err = delete_agent_sessions(Agent::Yolop, &ids(&["../../evil"])).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn delete_yolop_session_from_rejects_non_yolop_id() {
        let base = make_dir("agf-test-yolop-reject");
        let victim = base.join("not-a-session-id");
        fs::create_dir(&victim).unwrap();

        let err = delete_yolop_session_from(&base, "not-a-session-id").unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert!(victim.exists(), "a non-Yolop id must not delete anything");
        let _ = fs::remove_dir_all(base);
    }

    // -- codex ---------------------------------------------------------------

    #[test]
    fn delete_codex_sqlite_rows_removes_target_only() {
        let dir = make_dir("agf-test-codex-delete-target");
        let db = dir.join("state_5.sqlite");
        seed_state_db(&db, &["target-id", "keep-id"]);

        delete_codex_sqlite_rows(&dir, &ids(&["target-id"])).unwrap();

        assert_eq!(count_thread(&db, "target-id"), 0);
        assert_eq!(count_thread(&db, "keep-id"), 1);
    }

    #[test]
    fn delete_codex_sqlite_rows_walks_every_state_db() {
        let dir = make_dir("agf-test-codex-delete-multi");
        let db4 = dir.join("state_4.sqlite");
        let db5 = dir.join("state_5.sqlite");
        seed_state_db(&db4, &["dup-id"]);
        seed_state_db(&db5, &["dup-id"]);

        delete_codex_sqlite_rows(&dir, &ids(&["dup-id"])).unwrap();

        assert_eq!(count_thread(&db4, "dup-id"), 0);
        assert_eq!(count_thread(&db5, "dup-id"), 0);
    }

    #[test]
    fn delete_codex_sqlite_rows_removes_a_whole_batch_in_one_pass() {
        let dir = make_dir("agf-test-codex-delete-batch");
        let db = dir.join("state_9.sqlite");
        seed_state_db(&db, &["a", "b", "keep"]);

        delete_codex_sqlite_rows(&dir, &ids(&["a", "b"])).unwrap();

        assert_eq!(count_thread(&db, "a"), 0);
        assert_eq!(count_thread(&db, "b"), 0);
        assert_eq!(count_thread(&db, "keep"), 1);
    }

    #[test]
    fn delete_codex_sqlite_rows_ignores_non_state_files() {
        let dir = make_dir("agf-test-codex-delete-ignore");
        // A non-matching file must not crash the walk.
        fs::write(dir.join("history.jsonl"), b"").unwrap();
        let db = dir.join("state_1.sqlite");
        seed_state_db(&db, &["a"]);

        delete_codex_sqlite_rows(&dir, &ids(&["a"])).unwrap();
        assert_eq!(count_thread(&db, "a"), 0);
    }

    #[test]
    fn delete_codex_sqlite_rows_tolerates_missing_threads_table() {
        let dir = make_dir("agf-test-codex-delete-missing-table");
        let db = dir.join("state_0.sqlite");
        // Empty db (no `threads` table) — Codex versions without the table
        // must not abort the delete.
        rusqlite::Connection::open(&db).unwrap();

        delete_codex_sqlite_rows(&dir, &ids(&["anything"])).unwrap();
    }

    #[test]
    fn delete_codex_session_files_matches_rollout_id_without_reading_whole_file() {
        let dir = make_dir("agf-test-codex-rollout");
        let target = dir.join("rollout-target.jsonl");
        let sibling = dir.join("rollout-sibling.jsonl");
        let mut f = fs::File::create(&target).unwrap();
        writeln!(f, r#"{{"payload":{{"id":"target-id"}}}}"#).unwrap();
        // Body far larger than the first-line budget: it must never be read.
        writeln!(f, "{}", "x".repeat(2 * 1024 * 1024)).unwrap();
        fs::write(&sibling, br#"{"payload":{"id":"sibling-id"}}"#).unwrap();

        delete_codex_session_files(&dir, &ids(&["target-id"])).unwrap();

        assert!(!target.exists());
        assert!(sibling.exists());
        let _ = fs::remove_dir_all(dir);
    }

    // -- shared jsonl rewrite ------------------------------------------------

    #[test]
    fn rewrite_jsonl_excluding_drops_every_id_in_the_batch() {
        let dir = make_dir("agf-test-history-rewrite");
        let path = dir.join("history.jsonl");
        fs::write(
            &path,
            concat!(
                "{\"sessionId\":\"a\",\"display\":\"one\"}\n",
                "{\"sessionId\":\"keep\",\"display\":\"two\"}\n",
                "{\"sessionId\":\"b\",\"display\":\"three\"}\n",
            ),
        )
        .unwrap();

        rewrite_jsonl_excluding(&path, "sessionId", &ids(&["a", "b"])).unwrap();

        let out = fs::read_to_string(&path).unwrap();
        assert_eq!(out, "{\"sessionId\":\"keep\",\"display\":\"two\"}\n");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn rewrite_jsonl_excluding_leaves_file_untouched_when_nothing_matches() {
        let dir = make_dir("agf-test-history-noop");
        let path = dir.join("history.jsonl");
        let original = "{\"sessionId\":\"keep\"}\n\n{\"sessionId\":\"other\"}\n";
        fs::write(&path, original).unwrap();

        rewrite_jsonl_excluding(&path, "sessionId", &ids(&["absent"])).unwrap();

        // Byte-identical: no rewrite happened at all, blank line included.
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
        let _ = fs::remove_dir_all(dir);
    }

    // -- cursor --------------------------------------------------------------

    #[test]
    fn delete_cursor_agent_removes_transcript_dir_not_sibling() {
        let base = make_dir("agf-test-cursor-delete");
        let transcripts = base.join("projects/encproj/agent-transcripts");

        let target_uuid = "ddddddddddddddddddddddddddddddd1";
        let sibling_uuid = "ddddddddddddddddddddddddddddddd2";

        let target_dir = transcripts.join(target_uuid);
        let sibling_dir = transcripts.join(sibling_uuid);
        fs::create_dir_all(&target_dir).unwrap();
        fs::create_dir_all(&sibling_dir).unwrap();
        fs::write(target_dir.join(format!("{target_uuid}.jsonl")), b"{}").unwrap();
        fs::write(sibling_dir.join(format!("{sibling_uuid}.jsonl")), b"{}").unwrap();

        remove_matching_entries(
            &base.join("projects"),
            &ids(&[target_uuid]),
            &HashSet::new(),
        )
        .unwrap();

        assert!(!target_dir.exists(), "target session dir should be deleted");
        assert!(sibling_dir.exists(), "sibling session dir must survive");
    }

    /// Regression: legacy Cursor sessions live as plain files at
    /// `projects/<slug>/agent-transcripts/<uuid>.txt`, not as directories, so a
    /// dir-only pass left them behind and the next scan resurrected the orphan.
    /// Delete must remove both shapes — in one traversal.
    #[test]
    fn delete_cursor_agent_removes_dir_and_legacy_txt_in_one_pass() {
        let base = make_dir("agf-test-cursor-legacy-txt");
        let transcripts = base.join("projects/encproj/agent-transcripts");
        fs::create_dir_all(&transcripts).unwrap();

        let target_uuid = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeee1";
        let sibling_uuid = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeee2";

        let target_dir = transcripts.join(target_uuid);
        fs::create_dir_all(&target_dir).unwrap();
        let target_file = transcripts.join(format!("{target_uuid}.txt"));
        let sibling_file = transcripts.join(format!("{sibling_uuid}.txt"));
        fs::write(&target_file, b"legacy transcript").unwrap();
        fs::write(&sibling_file, b"legacy transcript").unwrap();

        let legacy = format!("{target_uuid}.txt");
        remove_matching_entries(
            &base.join("projects"),
            &ids(&[target_uuid]),
            &HashSet::from([legacy.as_str()]),
        )
        .unwrap();

        assert!(!target_dir.exists(), "current-format dir should be deleted");
        assert!(
            !target_file.exists(),
            "legacy .txt transcript should be deleted",
        );
        assert!(
            sibling_file.exists(),
            "unrelated sibling legacy .txt must survive",
        );
    }

    // -- yolop ---------------------------------------------------------------

    #[test]
    fn delete_yolop_session_removes_only_target_folder_and_legacy_log() {
        let base = make_dir("agf-test-yolop-delete");
        let target = "session_019e3db018a17450aba5407af5777237";
        let sibling = "session_019f4fec10e370b2be16cca7debb6ab1";
        fs::create_dir(base.join(target)).unwrap();
        fs::create_dir(base.join(sibling)).unwrap();
        fs::write(base.join(format!("{target}.jsonl")), b"{}").unwrap();

        delete_yolop_session_from(&base, target).unwrap();

        assert!(!base.join(target).exists());
        assert!(!base.join(format!("{target}.jsonl")).exists());
        assert!(base.join(sibling).exists());
    }

    // -- batch plumbing ------------------------------------------------------

    /// The batch entry point must report per agent, and an agent whose pass
    /// was refused must not appear as deleted — the TUI keeps those rows.
    #[test]
    fn delete_selection_reports_only_agents_whose_pass_succeeded() {
        let good = "session_019e3db018a17450aba5407af5777237";
        let selection = HashMap::from([
            (Agent::Yolop, HashSet::from([good.to_string()])),
            (
                Agent::ClaudeCode,
                // Traversal id: the whole ClaudeCode batch is refused.
                HashSet::from(["../../evil".to_string()]),
            ),
        ]);

        let deleted = delete_selection(&selection);

        assert_eq!(
            deleted.get(&Agent::Yolop),
            Some(&HashSet::from([good.to_string()]))
        );
        assert_eq!(deleted.get(&Agent::ClaudeCode), None);
    }

    /// A batch that aborts must not take healthy siblings down with it: the
    /// per-id retry recovers exactly the ones whose data really is gone.
    #[test]
    fn delete_selection_falls_back_to_per_session_truth_when_the_batch_fails() {
        let good = "session_019e3db018a17450aba5407af5777237";
        let selection = HashMap::from([(
            Agent::Yolop,
            // One valid id alongside one the guard refuses: the batch is
            // rejected as a unit, then the retry sorts them out.
            HashSet::from([good.to_string(), "../../evil".to_string()]),
        )]);

        let deleted = delete_selection(&selection);

        assert_eq!(
            deleted.get(&Agent::Yolop),
            Some(&HashSet::from([good.to_string()])),
            "the valid session must still be reported as deleted"
        );
    }

    #[test]
    fn delete_selection_skips_empty_buckets() {
        let selection = HashMap::from([(Agent::Yolop, HashSet::new())]);
        assert!(delete_selection(&selection).is_empty());
    }

    // -- pi / oh my pi -------------------------------------------------------

    #[test]
    fn delete_pi_style_session_accepts_oh_my_pi_title_slot() {
        let root = make_dir("agf-test-omp-delete");
        let target = root.join("target.jsonl");
        let sibling = root.join("sibling.jsonl");
        fs::write(
            &target,
            concat!(
                "{\"type\":\"title\",\"title\":\"Target\"}\n",
                "{\"type\":\"session\",\"id\":\"target-id\",\"cwd\":\"/tmp/x\"}\n"
            ),
        )
        .unwrap();
        fs::write(
            &sibling,
            "{\"type\":\"session\",\"id\":\"sibling-id\",\"cwd\":\"/tmp/x\"}\n",
        )
        .unwrap();

        delete_pi_style_sessions(&root, &ids(&["target-id"])).unwrap();

        assert!(!target.exists());
        assert!(sibling.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn delete_pi_style_session_removes_a_batch_and_spares_the_rest() {
        let root = make_dir("agf-test-pi-batch");
        for id in ["a", "b", "keep"] {
            fs::write(
                root.join(format!("{id}.jsonl")),
                format!("{{\"type\":\"session\",\"id\":\"{id}\",\"cwd\":\"/tmp/x\"}}\n"),
            )
            .unwrap();
        }

        delete_pi_style_sessions(&root, &ids(&["a", "b"])).unwrap();

        assert!(!root.join("a.jsonl").exists());
        assert!(!root.join("b.jsonl").exists());
        assert!(root.join("keep.jsonl").exists());
        let _ = fs::remove_dir_all(root);
    }

    /// The header is bounded to the first lines on purpose: a `session` record
    /// buried megabytes deep is not a real session file, and honouring it would
    /// mean reading every transcript in full on every delete.
    #[test]
    fn delete_pi_style_session_ignores_a_header_past_the_head_budget() {
        let root = make_dir("agf-test-pi-bounded");
        let path = root.join("huge.jsonl");
        let mut f = fs::File::create(&path).unwrap();
        for _ in 0..PI_HEADER_SCAN_LINES + 8 {
            writeln!(f, r#"{{"type":"message","role":"user"}}"#).unwrap();
        }
        writeln!(f, r#"{{"type":"session","id":"deep-id","cwd":"/tmp/x"}}"#).unwrap();
        drop(f);

        delete_pi_style_sessions(&root, &ids(&["deep-id"])).unwrap();

        assert!(path.exists());
        let _ = fs::remove_dir_all(root);
    }
}
