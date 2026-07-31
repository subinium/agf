use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader};

use rusqlite::Connection;
use walkdir::WalkDir;

use crate::error::AgfError;
use crate::model::{Agent, Session};
use crate::scanner::{first_line_truncated, project_name_from_path, read_first_line};

pub fn scan() -> Result<Vec<Session>, AgfError> {
    let codex_dir = crate::config::codex_dir()?;

    // Build the set of session IDs whose rollout JSONL still exists on disk.
    // Used to filter out — and prune from SQLite — `threads` rows that were
    // left behind when the JSONL was deleted manually (e.g. the user wiped
    // `~/.codex/sessions/`). Without pruning, those stale rows keep showing
    // up in `agf list`/`agf stats` forever and cannot be revived.
    //
    // `None` means we could not read the sessions tree reliably (permission
    // denied, transient I/O). In that case we fall back to the legacy
    // behavior — surface every SQLite row, prune nothing — to avoid
    // wiping live rows on a flaky filesystem read.
    let live_session_ids = collect_live_session_ids(&codex_dir);

    // Collect summaries from history.jsonl, pre-filtered against the live set
    // when known. `history.jsonl` is append-only and grows unbounded — power
    // users hit tens of MB after months of daily use — so keeping every
    // historical entry in memory wastes both RAM and post-loop sort time when
    // only the currently-listed sessions need summaries. (Surfaced by the
    // v0.11.3 post-ship audit; the `CACHE_VERSION=6` bump forces a cold
    // rescan on every upgrader, which would otherwise pay this cost on first
    // launch.)
    let summaries = read_history_summaries(&codex_dir, live_session_ids.as_ref());

    // Primary: read from SQLite (state_*.sqlite)
    let mut sessions = scan_sqlite(&codex_dir, &summaries, live_session_ids.as_ref());

    // Fallback: if SQLite found nothing, try JSONL walkdir
    if sessions.is_empty() {
        sessions = scan_jsonl(&codex_dir, &summaries);
    }

    sessions.sort_by_key(|s| std::cmp::Reverse(s.timestamp));
    Ok(sessions)
}

/// Walk `~/.codex/sessions/` and return the set of `payload.id` values found
/// in the first line of each `*.jsonl` rollout file. The Codex rollout
/// filename is `rollout-*.jsonl` (no session_id in the path), so we have to
/// open each file's first line to get the id.
///
/// Returns `None` only when the sessions directory exists but cannot be
/// traversed — a transient I/O error there must NOT be interpreted as
/// "all rows are orphaned", since prune would then nuke the entire
/// `threads` table. A missing directory is a legitimate empty-set result.
fn collect_live_session_ids(codex_dir: &std::path::Path) -> Option<HashSet<String>> {
    let sessions_dir = codex_dir.join("sessions");
    let mut ids = HashSet::new();
    if !sessions_dir.exists() {
        return Some(ids);
    }
    let walker = WalkDir::new(&sessions_dir).into_iter();
    let mut had_walk_error = false;
    for entry in walker {
        let Ok(entry) = entry else {
            had_walk_error = true;
            continue;
        };
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let Some(first_line) = read_first_line(path) else {
            continue;
        };
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(first_line.trim())
            && let Some(id) = value
                .get("payload")
                .and_then(|p| p.get("id"))
                .and_then(|v| v.as_str())
            && !id.is_empty()
        {
            ids.insert(id.to_string());
        }
    }
    // If the walk itself failed (permission denied, transient I/O), treat
    // the live-set as unknown: caller must skip pruning rather than risk
    // wiping live rows.
    if had_walk_error && ids.is_empty() {
        return None;
    }
    Some(ids)
}

/// Delete `threads` rows whose `id` is in `orphan_ids` from every
/// `state_*.sqlite` under `codex_dir`. Errors on a single db are swallowed
/// so a corrupt or older-schema file cannot block pruning the others.
fn prune_orphan_threads(codex_dir: &std::path::Path, orphan_ids: &[String]) {
    if orphan_ids.is_empty() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(codex_dir) else {
        return;
    };
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        let is_state_db = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.starts_with("state_") && n.ends_with(".sqlite"))
            .unwrap_or(false);
        if !is_state_db {
            continue;
        }
        let Ok(conn) = Connection::open(&path) else {
            continue;
        };
        let Ok(tx) = conn.unchecked_transaction() else {
            continue;
        };
        for id in orphan_ids {
            let _ = tx.execute("DELETE FROM threads WHERE id = ?1", [id]);
        }
        let _ = tx.commit();
    }
}

/// Read sessions from Codex SQLite database (state_*.sqlite).
/// This is the primary source — covers CLI, desktop app (vscode), and exec sessions.
///
/// `live_session_ids` is the set of session IDs whose rollout JSONL exists on
/// disk. Rows whose id is *not* in this set are treated as orphans (the user
/// — or some other tool — deleted the JSONL but the SQLite row was left
/// behind), excluded from the returned list, and pruned from every
/// `state_*.sqlite` so they do not reappear on the next scan. Orphan rows
/// are dead weight regardless: Codex cannot resume them once the rollout
/// JSONL is gone, so removing them from SQLite is not data loss.
fn scan_sqlite(
    codex_dir: &std::path::Path,
    summaries: &HashMap<String, Vec<String>>,
    live_session_ids: Option<&HashSet<String>>,
) -> Vec<Session> {
    // Find the latest state_*.sqlite file
    let Some(db_path) = find_state_db(codex_dir) else {
        return Vec::new();
    };

    let Ok(conn) =
        Connection::open_with_flags(&db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
    else {
        return Vec::new();
    };

    let Ok(mut stmt) = conn.prepare(
        "SELECT id, cwd, title, updated_at, git_branch, first_user_message
         FROM threads
         WHERE archived = 0 AND cwd != ''
         ORDER BY updated_at DESC",
    ) else {
        return Vec::new();
    };

    let Ok(rows) = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2).unwrap_or_default(),
            row.get::<_, i64>(3).unwrap_or(0),
            row.get::<_, Option<String>>(4)?,
            row.get::<_, String>(5).unwrap_or_default(),
        ))
    }) else {
        return Vec::new();
    };

    let mut sessions = Vec::new();
    let mut orphan_ids: Vec<String> = Vec::new();
    for row in rows.flatten() {
        let (session_id, cwd, title, updated_at, git_branch, first_msg) = row;

        if session_id.is_empty() || cwd.is_empty() {
            continue;
        }

        // Only filter/prune when we have a trustworthy live set. If the
        // sessions tree could not be enumerated (`None`), surface the row
        // as-is — better stale than nuked.
        if let Some(live) = live_session_ids
            && !live.contains(&session_id)
        {
            orphan_ids.push(session_id);
            continue;
        }

        let project_name = project_name_from_path(&cwd);

        // updated_at is Unix seconds — convert to millis. Saturate so a
        // corrupt/tampered value can't overflow i64 and wrap to a garbage
        // (often negative) timestamp that jumps the session to a list extreme.
        let timestamp = updated_at.saturating_mul(1000);

        // Build summaries: prefer history.jsonl, fall back to title/first_msg
        let session_summaries = if let Some(s) = summaries.get(&session_id) {
            s.clone()
        } else {
            // Use first line of title (can be very long), or first_user_message
            let summary =
                first_line_truncated(&title, 200).or_else(|| first_line_truncated(&first_msg, 200));
            match summary {
                Some(s) => vec![s],
                None => Vec::new(),
            }
        };

        sessions.push(Session {
            agent: Agent::Codex,
            session_id,
            project_name,
            project_path: cwd,
            summaries: session_summaries,
            timestamp,
            git_branch,
            worktree: None,
            recap: None,
        });
    }

    // Drop the read-only connection before re-opening read-write for prune.
    drop(stmt);
    drop(conn);

    if !orphan_ids.is_empty() {
        prune_orphan_threads(codex_dir, &orphan_ids);
    }

    sessions
}

/// Find the latest state_*.sqlite file in the codex directory.
fn find_state_db(codex_dir: &std::path::Path) -> Option<std::path::PathBuf> {
    let entries = std::fs::read_dir(codex_dir).ok()?;
    entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("state_") && n.ends_with(".sqlite"))
                .unwrap_or(false)
        })
        // Lexicographic max picks the latest db (state_5 > state_4 etc.).
        .max()
}

/// Fallback: scan JSONL session files via walkdir (legacy format).
fn scan_jsonl(
    codex_dir: &std::path::Path,
    summaries: &HashMap<String, Vec<String>>,
) -> Vec<Session> {
    use serde::Deserialize;

    #[derive(Deserialize)]
    struct SessionMeta {
        #[serde(rename = "type")]
        entry_type: Option<String>,
        payload: Option<SessionPayload>,
    }

    #[derive(Deserialize)]
    struct SessionPayload {
        id: Option<String>,
        cwd: Option<String>,
        timestamp: Option<String>,
        git: Option<GitInfo>,
    }

    #[derive(Deserialize)]
    struct GitInfo {
        branch: Option<String>,
    }

    let sessions_dir = codex_dir.join("sessions");
    let mut sessions = Vec::new();

    if !sessions_dir.exists() {
        return sessions;
    }

    for entry in WalkDir::new(&sessions_dir)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }

        let Some(first_line) = read_first_line(path) else {
            continue;
        };

        let Ok(meta) = serde_json::from_str::<SessionMeta>(first_line.trim()) else {
            continue;
        };

        if meta.entry_type.as_deref() != Some("session_meta") {
            continue;
        }

        let Some(payload) = meta.payload else {
            continue;
        };

        let session_id = match payload.id {
            Some(id) if !id.is_empty() => id,
            _ => continue,
        };

        let cwd = match payload.cwd {
            Some(cwd) if !cwd.is_empty() => cwd,
            _ => continue,
        };

        let project_name = project_name_from_path(&cwd);

        let timestamp = payload
            .timestamp
            .as_deref()
            .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
            .map(|dt| dt.timestamp_millis())
            .unwrap_or(0);

        let git_branch = payload.git.and_then(|g| g.branch);
        let session_summaries = summaries.get(&session_id).cloned().unwrap_or_default();

        sessions.push(Session {
            agent: Agent::Codex,
            session_id,
            project_name,
            project_path: cwd,
            summaries: session_summaries,
            timestamp,
            git_branch,
            worktree: None,
            recap: None,
        });
    }

    sessions
}

#[derive(serde::Deserialize)]
struct HistoryEntry {
    session_id: Option<String>,
    ts: Option<f64>,
    text: Option<String>,
}

/// Read `~/.codex/history.jsonl` and group user-prompt summaries by
/// `session_id`, newest-first.
///
/// `live_session_ids` is the set of session IDs whose rollout JSONL still
/// exists on disk; when `Some`, lines whose `session_id` is not in the set
/// are skipped early so historical entries for long-deleted sessions never
/// reach the HashMap. `None` means the caller couldn't enumerate the live set
/// reliably (permission denied / transient I/O); the legacy behavior — keep
/// every entry — is preserved to match `scan_sqlite`'s same-condition
/// fallback at `live_session_ids.is_none()`.
fn read_history_summaries(
    codex_dir: &std::path::Path,
    live_session_ids: Option<&HashSet<String>>,
) -> HashMap<String, Vec<String>> {
    let path = codex_dir.join("history.jsonl");
    let mut summaries: HashMap<String, Vec<(f64, String)>> = HashMap::new();

    let Ok(file) = File::open(&path) else {
        return HashMap::new();
    };

    for line in BufReader::new(file).lines() {
        let Ok(line) = line else {
            continue;
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<HistoryEntry>(line) else {
            continue;
        };
        let session_id = match entry.session_id {
            Some(id) if !id.is_empty() => id,
            _ => continue,
        };
        if let Some(live) = live_session_ids
            && !live.contains(&session_id)
        {
            continue;
        }
        let ts = entry.ts.unwrap_or(0.0);
        let text = match entry.text {
            Some(t) if !t.is_empty() => t,
            _ => continue,
        };
        summaries.entry(session_id).or_default().push((ts, text));
    }

    summaries
        .into_iter()
        .map(|(k, mut v)| {
            v.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
            (k, v.into_iter().map(|(_, s)| s).collect())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;

    /// Allocate a fresh empty temp directory for a single test, mirroring
    /// the helper used in `delete::tests`. We do not pull in the
    /// `tempfile` crate because the rest of the codebase does not depend
    /// on it.
    fn make_codex_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("agf-codex-test-{name}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Create a `state_*.sqlite` with the columns the scanner reads and
    /// seed it with `(id, cwd)` rows.
    fn seed_state_db(path: &std::path::Path, rows: &[(&str, &str)]) {
        let conn = Connection::open(path).expect("open state db");
        conn.execute_batch(
            "CREATE TABLE threads (
                id TEXT PRIMARY KEY,
                cwd TEXT NOT NULL DEFAULT '',
                title TEXT NOT NULL DEFAULT '',
                updated_at INTEGER NOT NULL DEFAULT 0,
                git_branch TEXT,
                first_user_message TEXT NOT NULL DEFAULT '',
                archived INTEGER NOT NULL DEFAULT 0
            );",
        )
        .expect("create threads");
        for (id, cwd) in rows {
            conn.execute("INSERT INTO threads (id, cwd) VALUES (?1, ?2)", [*id, *cwd])
                .expect("insert thread");
        }
    }

    fn count_threads(path: &std::path::Path) -> i64 {
        let conn = Connection::open(path).expect("open db");
        conn.query_row("SELECT COUNT(*) FROM threads", [], |r| r.get(0))
            .expect("count")
    }

    #[test]
    fn prune_orphan_threads_removes_only_target_ids() {
        let dir = make_codex_dir("prune-target");
        let db = dir.join("state_5.sqlite");
        seed_state_db(
            &db,
            &[
                ("orphan-1", "/some/cwd"),
                ("live-1", "/some/cwd"),
                ("orphan-2", "/some/cwd"),
            ],
        );

        prune_orphan_threads(&dir, &["orphan-1".to_string(), "orphan-2".to_string()]);

        let conn = Connection::open(&db).unwrap();
        let remaining: Vec<String> = conn
            .prepare("SELECT id FROM threads ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(remaining, vec!["live-1".to_string()]);
    }

    #[test]
    fn prune_orphan_threads_walks_every_state_db() {
        let dir = make_codex_dir("prune-walk");
        let db4 = dir.join("state_4.sqlite");
        let db5 = dir.join("state_5.sqlite");
        seed_state_db(&db4, &[("orphan", "/x"), ("live", "/x")]);
        seed_state_db(&db5, &[("orphan", "/x"), ("live", "/x")]);

        prune_orphan_threads(&dir, &["orphan".to_string()]);

        assert_eq!(count_threads(&db4), 1);
        assert_eq!(count_threads(&db5), 1);
    }

    #[test]
    fn prune_orphan_threads_ignores_non_state_files() {
        let dir = make_codex_dir("prune-ignore");
        let db = dir.join("state_5.sqlite");
        seed_state_db(&db, &[("orphan", "/x")]);
        // Sibling files that must not interfere with the walk
        fs::write(dir.join("history.jsonl"), b"{}").unwrap();
        fs::write(dir.join("auth.json"), b"{}").unwrap();

        prune_orphan_threads(&dir, &["orphan".to_string()]);
        assert_eq!(count_threads(&db), 0);
    }

    #[test]
    fn prune_orphan_threads_tolerates_missing_threads_table() {
        let dir = make_codex_dir("prune-noschema");
        let bad = dir.join("state_3.sqlite");
        // Db with no `threads` table — older Codex CLI schema.
        let conn = Connection::open(&bad).unwrap();
        conn.execute_batch("CREATE TABLE other (id TEXT);").unwrap();
        drop(conn);

        let good = dir.join("state_5.sqlite");
        seed_state_db(&good, &[("orphan", "/x")]);

        prune_orphan_threads(&dir, &["orphan".to_string()]);
        // `bad` was silently skipped; `good` got pruned.
        assert_eq!(count_threads(&good), 0);
    }

    #[test]
    fn collect_live_session_ids_returns_empty_when_sessions_dir_missing() {
        let dir = make_codex_dir("live-missing");
        // No `sessions/` subdir under codex_dir.
        let live = collect_live_session_ids(&dir);
        assert_eq!(live, Some(HashSet::new()));
    }

    #[test]
    fn collect_live_session_ids_extracts_payload_id_from_rollout_jsonl() {
        let dir = make_codex_dir("live-extract");
        let day_dir = dir.join("sessions/2026/04/29");
        fs::create_dir_all(&day_dir).unwrap();
        let mut f = fs::File::create(day_dir.join("rollout-2026-04-29-abc.jsonl")).unwrap();
        // First line is `session_meta` with payload.id; the scanner only
        // needs to extract that id.
        writeln!(
            f,
            r#"{{"type":"session_meta","payload":{{"id":"session-abc","cwd":"/x","timestamp":"2026-04-29T00:00:00Z"}}}}"#
        )
        .unwrap();

        let live = collect_live_session_ids(&dir).expect("walk ok");
        assert!(live.contains("session-abc"));
        assert_eq!(live.len(), 1);
    }

    fn seed_history_jsonl(dir: &std::path::Path, entries: &[(&str, f64, &str)]) {
        let mut f = fs::File::create(dir.join("history.jsonl")).unwrap();
        for (sid, ts, text) in entries {
            writeln!(f, r#"{{"session_id":"{sid}","ts":{ts},"text":"{text}"}}"#).unwrap();
        }
    }

    /// Pre-filter: when a live session id set is provided, history entries for
    /// any other session id are skipped early so they never reach the
    /// returned `HashMap`. Bounds memory growth on `history.jsonl`, which is
    /// append-only and reaches tens of MB for power users.
    #[test]
    fn read_history_summaries_pre_filters_against_live_session_ids() {
        let dir = make_codex_dir("hist-prefilter");
        seed_history_jsonl(
            &dir,
            &[
                ("live-a", 100.0, "kept-A1"),
                ("dead-b", 110.0, "dropped"),
                ("live-a", 120.0, "kept-A2"),
                ("dead-c", 130.0, "dropped"),
            ],
        );
        let mut live = HashSet::new();
        live.insert("live-a".to_string());

        let summaries = read_history_summaries(&dir, Some(&live));

        assert_eq!(summaries.len(), 1);
        // Newest-first order: 120.0 before 100.0.
        assert_eq!(
            summaries.get("live-a").map(|v| v.as_slice()),
            Some(&["kept-A2".to_string(), "kept-A1".to_string()][..])
        );
        assert!(!summaries.contains_key("dead-b"));
        assert!(!summaries.contains_key("dead-c"));
    }

    /// `None` (caller could not enumerate the live set) preserves the legacy
    /// "keep every entry" behavior — mirrors `scan_sqlite`'s same-condition
    /// fallback so a transient I/O error on the sessions tree can't wipe
    /// summaries from the listing.
    #[test]
    fn read_history_summaries_keeps_all_when_live_set_is_none() {
        let dir = make_codex_dir("hist-no-filter");
        seed_history_jsonl(&dir, &[("a", 1.0, "kept-a"), ("b", 2.0, "kept-b")]);

        let summaries = read_history_summaries(&dir, None);

        assert_eq!(summaries.len(), 2);
        assert!(summaries.contains_key("a"));
        assert!(summaries.contains_key("b"));
    }
}
