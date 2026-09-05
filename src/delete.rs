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
    let deleted = delete_agent_sessions(session.agent, &ids)?;
    if deleted.contains(&session.session_id) {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("session data was not found: {}", session.session_id),
        ))
    }
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
        if let Ok(actually_deleted) = delete_agent_sessions(agent, &borrowed) {
            if !actually_deleted.is_empty() {
                deleted.insert(agent, actually_deleted);
            }
            continue;
        }
        // The batch aborted somewhere, and a batch cannot say how far it got.
        // Retry the ids one at a time to recover per-session truth: a row may
        // only disappear from the list once its own data is gone. This costs a
        // pass per id, but only on the error path.
        let individually: HashSet<String> = ids
            .iter()
            .filter_map(|id| {
                delete_agent_sessions(agent, &HashSet::from([id.as_str()]))
                    .ok()
                    .filter(|removed| removed.contains(id))
                    .map(|_| id.clone())
            })
            .collect();
        if !individually.is_empty() {
            deleted.insert(agent, individually);
        }
    }
    deleted
}

fn delete_agent_sessions(agent: Agent, ids: &HashSet<&str>) -> Result<HashSet<String>, io::Error> {
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
        Agent::Grok => Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Grok deletion is disabled: use Grok Build's session picker so its search index and active-session state stay consistent",
        )),
        Agent::Kimi => Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Kimi deletion is disabled: use Kimi Code's session picker so session_index.jsonl stays consistent",
        )),
        Agent::Qwen => Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Qwen deletion is disabled: use Qwen Code's /delete command so active and archived state stay consistent",
        )),
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
        Agent::Gemini => Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Gemini deletion is disabled: use Gemini CLI's session deletion so subagents, plans, tracker, and tool-output artifacts stay consistent",
        )),
        Agent::Hermes => delete_hermes_sessions(ids),
        Agent::Yolop => delete_yolop_sessions(ids),
        Agent::PrimeAgent => Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Prime Agent deletion is disabled: use Prime Agent's /resume picker so active daemon sessions are protected",
        )),
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
) -> Result<HashSet<String>, io::Error> {
    let mut deleted = HashSet::new();
    if !base.is_dir() {
        return Ok(deleted);
    }
    for entry in WalkDir::new(base) {
        let entry = entry.map_err(io::Error::other)?;
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
                deleted.insert(name.to_string());
            }
        } else if file_type.is_file() && file_names.contains(name) {
            fs::remove_file(path)?;
            if let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) {
                deleted.insert(stem.to_string());
            }
        }
    }
    Ok(deleted)
}

// ---------------------------------------------------------------------------
// Claude Code
// ---------------------------------------------------------------------------

/// Claude history is append-only and may have a live writer, so deletion never
/// rewrites it. Removing the exact transcript is sufficient: the scanner's
/// live-set filter ignores retained history summaries without a transcript.
fn delete_claude_sessions(ids: &HashSet<&str>) -> Result<HashSet<String>, io::Error> {
    let claude_dir = config::claude_dir().map_err(io::Error::other)?;
    let projects_dir = claude_dir.join("projects");
    let transcript_names: Vec<String> = ids.iter().map(|id| format!("{id}.jsonl")).collect();
    let transcript_names: HashSet<&str> = transcript_names.iter().map(String::as_str).collect();
    remove_matching_entries(&projects_dir, ids, &transcript_names)
}

// ---------------------------------------------------------------------------
// Codex
// ---------------------------------------------------------------------------

/// Codex session data lives in three places (any combination may be present
/// depending on Codex CLI version):
///   * `state_*.sqlite` `threads` row (primary source for current Codex CLI),
///   * `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl` rollout file,
///   * `~/.codex/history.jsonl` per-prompt summary entries (left append-only).
///
/// We delete from all three so the session does not reappear after the next
/// `agf` scan via the SQLite path.
fn delete_codex_sessions(ids: &HashSet<&str>) -> Result<HashSet<String>, io::Error> {
    let codex_dir = config::codex_dir().map_err(io::Error::other)?;
    let sqlite_dir = config::codex_sqlite_dir().map_err(io::Error::other)?;

    let mut deleted = delete_codex_sqlite_rows(&sqlite_dir, ids)?;

    let sessions_dir = codex_dir.join("sessions");
    if sessions_dir.exists() {
        deleted.extend(delete_codex_session_files(&sessions_dir, ids)?);
    }

    Ok(deleted)
}

/// Remove the `threads` rows matching `ids` from every `state_*.sqlite` in
/// `codex_dir`. Older schemas without `threads` are ignored, but lock/open/
/// write failures are surfaced so the UI never reports a deletion that did
/// not actually reach the current Codex database.
fn delete_codex_sqlite_rows(
    codex_dir: &Path,
    ids: &HashSet<&str>,
) -> Result<HashSet<String>, io::Error> {
    let mut deleted = HashSet::new();
    let entries = match fs::read_dir(codex_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(deleted),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let is_state_db = path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("state_") && n.ends_with(".sqlite"));
        if !is_state_db {
            continue;
        }
        let mut conn = rusqlite::Connection::open(&path)
            .map_err(|error| io::Error::other(format!("{}: {error}", path.display())))?;
        let tx = conn.transaction().map_err(io::Error::other)?;
        for id in ids {
            match tx.execute("DELETE FROM threads WHERE id = ?1", [*id]) {
                Ok(count) => {
                    if count > 0 {
                        deleted.insert((*id).to_string());
                    }
                }
                Err(error) if error.to_string().contains("no such table") => break,
                Err(error) => return Err(io::Error::other(error)),
            }
        }
        tx.commit().map_err(io::Error::other)?;
    }
    Ok(deleted)
}

/// Find and delete the Codex rollout JSONL files matching `ids`.
fn delete_codex_session_files(
    sessions_dir: &Path,
    ids: &HashSet<&str>,
) -> Result<HashSet<String>, io::Error> {
    let mut deleted = HashSet::new();
    for entry in WalkDir::new(sessions_dir) {
        let entry = entry.map_err(io::Error::other)?;
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
            deleted.insert(payload_id.to_string());
        }
    }
    Ok(deleted)
}

// ---------------------------------------------------------------------------
// OpenCode
// ---------------------------------------------------------------------------

/// OpenCode sessions are stored in a SQLite database at
/// `~/.local/share/opencode/opencode.db`.
fn delete_opencode_sessions(ids: &HashSet<&str>) -> Result<HashSet<String>, io::Error> {
    let opencode_dir = config::opencode_data_dir().map_err(io::Error::other)?;
    let db_path = opencode_dir.join("opencode.db");
    if !db_path.exists() {
        return Ok(HashSet::new());
    }

    let mut conn = rusqlite::Connection::open(&db_path)
        .map_err(|e| io::Error::other(format!("SQLite open error: {e}")))?;
    let tx = conn
        .transaction()
        .map_err(|e| io::Error::other(format!("SQLite begin tx error: {e}")))?;
    let mut deleted = HashSet::new();
    for id in ids {
        let count = tx
            .execute("DELETE FROM session WHERE id = ?1", [*id])
            .map_err(|e| io::Error::other(format!("SQLite delete error: {e}")))?;
        if count > 0 {
            deleted.insert((*id).to_string());
        }
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

    Ok(deleted)
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
fn delete_pi_style_sessions(
    sessions_dir: &Path,
    ids: &HashSet<&str>,
) -> Result<HashSet<String>, io::Error> {
    let mut deleted = HashSet::new();
    if !sessions_dir.exists() {
        return Ok(deleted);
    }

    for entry in WalkDir::new(sessions_dir) {
        let entry = entry.map_err(io::Error::other)?;
        let path = entry.path();
        if !entry.file_type().is_file()
            || path.extension().and_then(|e| e.to_str()) != Some("jsonl")
        {
            continue;
        }

        let matching_id = read_head_lines(path, PI_HEADER_SCAN_LINES, PI_HEADER_SCAN_BYTES)
            .iter()
            .find_map(|line| pi_header_id(line, ids));
        if let Some(id) = matching_id {
            fs::remove_file(path)?;
            deleted.insert(id);
        }
    }

    Ok(deleted)
}

fn pi_header_id(line: &str, ids: &HashSet<&str>) -> Option<String> {
    if line.is_empty() {
        return None;
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
        return None;
    };
    (value.get("type").and_then(|v| v.as_str()) == Some("session"))
        .then(|| {
            value
                .get("id")
                .and_then(|v| v.as_str())
                .filter(|id| ids.contains(*id))
                .map(str::to_string)
        })
        .flatten()
}

// ---------------------------------------------------------------------------
// Kiro
// ---------------------------------------------------------------------------

/// Kiro sessions are stored in a SQLite database at
/// `~/Library/Application Support/kiro-cli/data.sqlite3` (macOS) or
/// `~/.local/share/kiro-cli/data.sqlite3` (Linux).
fn delete_kiro_sessions(ids: &HashSet<&str>) -> Result<HashSet<String>, io::Error> {
    let mut deleted = HashSet::new();
    let data_dir = config::kiro_data_dir().map_err(io::Error::other)?;
    let db_path = data_dir.join("data.sqlite3");
    if db_path.exists() {
        let mut conn = rusqlite::Connection::open(&db_path)
            .map_err(|e| io::Error::other(format!("SQLite open error: {e}")))?;
        let tx = conn
            .transaction()
            .map_err(|e| io::Error::other(format!("SQLite begin tx error: {e}")))?;
        for id in ids {
            match tx.execute(
                "DELETE FROM conversations_v2 WHERE conversation_id = ?1",
                [*id],
            ) {
                Ok(count) => {
                    if count > 0 {
                        deleted.insert((*id).to_string());
                    }
                }
                Err(error) if error.to_string().contains("no such table") => break,
                Err(error) => {
                    return Err(io::Error::other(format!("SQLite delete error: {error}")));
                }
            }
        }
        tx.commit()
            .map_err(|e| io::Error::other(format!("SQLite commit error: {e}")))?;
    }

    let sessions_dir = config::kiro_sessions_dir().map_err(io::Error::other)?;
    if sessions_dir.is_dir() {
        for entry in fs::read_dir(&sessions_dir)? {
            let entry = entry?;
            let metadata_path = entry.path();
            if !entry.file_type()?.is_file()
                || metadata_path.extension().and_then(|ext| ext.to_str()) != Some("json")
            {
                continue;
            }
            let Some(id) = crate::scanner::kiro::v3_session_id(&metadata_path)
                .map_err(io::Error::other)?
                .filter(|id| ids.contains(id.as_str()))
            else {
                continue;
            };
            fs::remove_file(&metadata_path)?;
            let event_path = metadata_path.with_extension("jsonl");
            if event_path.is_file() {
                fs::remove_file(event_path)?;
            }
            deleted.insert(id);
        }
    }
    Ok(deleted)
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
fn delete_cursor_agent_sessions(ids: &HashSet<&str>) -> Result<HashSet<String>, io::Error> {
    let cursor_dir = config::cursor_dir().map_err(io::Error::other)?;

    // 1. Chat metadata: ~/.cursor/chats/*/<session_id>/
    let mut deleted = remove_matching_entries(&cursor_dir.join("chats"), ids, &HashSet::new())?;

    // 2. Transcript: directory form (JSONL) and file form (legacy .txt), in a
    //    single traversal of the projects tree.
    let legacy_names: Vec<String> = ids.iter().map(|id| format!("{id}.txt")).collect();
    let legacy_names: HashSet<&str> = legacy_names.iter().map(String::as_str).collect();
    deleted.extend(remove_matching_entries(
        &cursor_dir.join("projects"),
        ids,
        &legacy_names,
    )?);
    Ok(deleted)
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
fn delete_hermes_sessions(ids: &HashSet<&str>) -> Result<HashSet<String>, io::Error> {
    let mut deleted = HashSet::new();
    let hermes_dir = config::hermes_dir().map_err(io::Error::other)?;
    let db_path = hermes_dir.join("state.db");
    if !db_path.exists() {
        return Ok(deleted);
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
        let count = tx
            .execute("DELETE FROM sessions WHERE id = ?1", [*id])
            .map_err(|e| io::Error::other(format!("SQLite delete session error: {e}")))?;
        if count > 0 {
            deleted.insert((*id).to_string());
        }
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
                && ids.iter().any(|id| name == format!("session_{id}.json"))
            {
                let _ = fs::remove_file(entry.path());
            }
        }
    }

    Ok(deleted)
}

// ---------------------------------------------------------------------------
// Yolop
// ---------------------------------------------------------------------------

fn delete_yolop_sessions(ids: &HashSet<&str>) -> Result<HashSet<String>, io::Error> {
    let mut deleted = HashSet::new();
    let sessions_dir = config::yolop_sessions_dir().map_err(io::Error::other)?;
    for id in ids {
        if delete_yolop_session_from(&sessions_dir, id)? {
            deleted.insert((*id).to_string());
        }
    }
    Ok(deleted)
}

fn delete_yolop_session_from(sessions_dir: &Path, session_id: &str) -> Result<bool, io::Error> {
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
    let mut deleted = false;
    let session_dir = sessions_dir.join(session_id);
    if session_dir.is_dir() {
        fs::remove_dir_all(session_dir)?;
        deleted = true;
    }
    let legacy_log = sessions_dir.join(format!("{session_id}.jsonl"));
    if legacy_log.is_file() {
        fs::remove_file(legacy_log)?;
        deleted = true;
    }
    Ok(deleted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn ids(list: &[&'static str]) -> HashSet<&'static str> {
        list.iter().copied().collect()
    }

    fn make_dir(_name: &str) -> std::path::PathBuf {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "agf-delete-test-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
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

    #[test]
    fn gemini_deletion_is_native_managed_for_single_and_bulk_paths() {
        assert!(!Agent::Gemini.supports_delete());
        let error = delete_agent_sessions(Agent::Gemini, &ids(&["selected-id"])).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
        let selection =
            HashMap::from([(Agent::Gemini, HashSet::from(["selected-id".to_string()]))]);
        assert!(delete_selection(&selection).is_empty());
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
    fn delete_selection_reports_only_sessions_with_backing_data() {
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

        assert_eq!(deleted.get(&Agent::Yolop), None);
        assert_eq!(deleted.get(&Agent::ClaudeCode), None);
    }

    /// A batch that aborts must not turn a merely valid-looking ID into a
    /// successful deletion when no authoritative backing data was present.
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

        assert!(deleted.is_empty());
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
