use rusqlite::Connection;
use std::collections::{HashMap, HashSet};
use walkdir::WalkDir;

use crate::error::AgfError;
use crate::model::{Agent, Session};
use crate::scanner::{
    first_line_truncated, for_each_bounded_line, project_name_from_path, read_first_line,
    read_first_line_result,
};

const HISTORY_LINE_BYTES: usize = 2 * 1024 * 1024;
const MAX_SUMMARIES: usize = 10;

pub fn scan() -> Result<Vec<Session>, AgfError> {
    let codex_dir = crate::config::codex_dir()?;

    // SQLite is the current Codex source of truth and already stores title /
    // first prompt metadata. Query it first; opening every rollout merely to
    // reconstruct the same live-id set made a 4k-session store pay thousands
    // of file opens on every cold CLI scan.
    let has_rollout_path = state_db_has_rollout_path(&codex_dir)?;
    let live_session_ids = if has_rollout_path {
        None
    } else {
        collect_live_session_ids(&codex_dir)
    };
    if let Some(mut sessions) = scan_sqlite(&codex_dir, &HashMap::new(), live_session_ids.as_ref())?
    {
        let live_ids: HashSet<String> = sessions
            .iter()
            .map(|session| session.session_id.clone())
            .collect();
        let summaries = read_history_summaries(&codex_dir, Some(&live_ids))?;
        for session in &mut sessions {
            if let Some(history) = summaries.get(&session.session_id) {
                session.summaries.clone_from(history);
            }
        }
        sessions.sort_by(|a, b| crate::model::compare_sessions(a, b, crate::model::SortMode::Time));
        return Ok(sessions);
    }

    // Build the set of session IDs whose rollout JSONL still exists on disk.
    // This is only a read-side visibility filter: scans must never mutate
    // Codex's own state database. Destructive cleanup belongs behind AGF's
    // explicit delete flow, not list/resume/stats/watch.
    //
    // `None` means we could not read the sessions tree reliably (permission
    // denied, transient I/O). In that case we fall back to the legacy
    // behavior — surface every SQLite row — to avoid hiding live rows on a
    // flaky or partially-readable filesystem.
    // Collect summaries from history.jsonl, pre-filtered against the live set
    // when known. `history.jsonl` is append-only and grows unbounded — power
    // users hit tens of MB after months of daily use — so keeping every
    // historical entry in memory wastes both RAM and post-loop sort time when
    // only the currently-listed sessions need summaries. (Surfaced by the
    // v0.11.3 post-ship audit; the `CACHE_VERSION=6` bump forces a cold
    // rescan on every upgrader, which would otherwise pay this cost on first
    // launch.)
    let summaries = read_history_summaries(&codex_dir, live_session_ids.as_ref())?;

    // Legacy fallback for installs without a usable state database.
    let mut sessions = scan_jsonl(&codex_dir, &summaries)?;

    sessions.sort_by_key(|s| std::cmp::Reverse(s.timestamp));
    Ok(sessions)
}

/// Walk `~/.codex/sessions/` and return the set of `payload.id` values found
/// in the first line of each `*.jsonl` rollout file. The Codex rollout
/// filename is `rollout-*.jsonl` (no session_id in the path), so we have to
/// open each file's first line to get the id.
///
/// Returns `None` whenever the tree cannot be enumerated *completely*.
/// A partial set is not trustworthy enough to hide SQLite rows: one unreadable
/// or malformed rollout must fail closed to the unfiltered SQLite view.
/// A missing directory is a legitimate empty-set result.
fn collect_live_session_ids(codex_dir: &std::path::Path) -> Option<HashSet<String>> {
    let sessions_dir = codex_dir.join("sessions");
    let mut ids = HashSet::new();
    if !sessions_dir.exists() {
        return Some(ids);
    }
    let walker = WalkDir::new(&sessions_dir).into_iter();
    let mut complete = true;
    for entry in walker {
        let Ok(entry) = entry else {
            complete = false;
            continue;
        };
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let Some(first_line) = read_first_line(path) else {
            complete = false;
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(first_line.trim()) else {
            complete = false;
            continue;
        };
        let Some(id) = value
            .get("payload")
            .and_then(|p| p.get("id"))
            .and_then(|v| v.as_str())
            .filter(|id| !id.is_empty())
        else {
            complete = false;
            continue;
        };
        ids.insert(id.to_string());
    }
    complete.then_some(ids)
}

/// Read sessions from Codex SQLite database (state_*.sqlite).
/// This is the primary source — covers CLI, desktop app (vscode), and exec sessions.
///
/// `live_session_ids` is the set of session IDs whose rollout JSONL exists on
/// disk. Rows whose id is *not* in a complete set are hidden from the result,
/// but never deleted from Codex's database.
fn scan_sqlite(
    codex_dir: &std::path::Path,
    summaries: &HashMap<String, Vec<String>>,
    live_session_ids: Option<&HashSet<String>>,
) -> Result<Option<Vec<Session>>, AgfError> {
    // Find the latest state_*.sqlite file
    let Some(db_path) = find_state_db(codex_dir)? else {
        return Ok(None);
    };

    let conn = Connection::open_with_flags(&db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;

    let columns = table_columns(&conn, "threads")?;
    if columns.is_empty() {
        return Ok(None);
    }
    let source_column = if columns.contains("source") {
        "source"
    } else {
        "''"
    };
    let thread_source_column = if columns.contains("thread_source") {
        "thread_source"
    } else {
        "NULL"
    };
    let agent_role_column = if columns.contains("agent_role") {
        "agent_role"
    } else {
        "NULL"
    };
    let rollout_path_column = if columns.contains("rollout_path") {
        "rollout_path"
    } else {
        "NULL"
    };
    let requires_rollout_path = columns.contains("rollout_path");
    let timestamp_column = if columns.contains("updated_at_ms") {
        "COALESCE(NULLIF(updated_at_ms, 0), updated_at * 1000)"
    } else {
        "updated_at * 1000"
    };
    let query = format!(
        "SELECT id, cwd, title, {timestamp_column}, git_branch, first_user_message, \
         {source_column}, {thread_source_column}, {agent_role_column}, {rollout_path_column} \
         FROM threads WHERE archived = 0 AND cwd != '' ORDER BY {timestamp_column} DESC"
    );
    let mut stmt = conn.prepare(&query)?;

    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2).unwrap_or_default(),
            row.get::<_, i64>(3).unwrap_or(0),
            row.get::<_, Option<String>>(4)?,
            row.get::<_, String>(5).unwrap_or_default(),
            row.get::<_, String>(6).unwrap_or_default(),
            row.get::<_, Option<String>>(7).unwrap_or_default(),
            row.get::<_, Option<String>>(8).unwrap_or_default(),
            row.get::<_, Option<String>>(9).unwrap_or_default(),
        ))
    })?;

    let mut sessions = Vec::new();
    for row in rows {
        let (
            session_id,
            cwd,
            title,
            timestamp,
            git_branch,
            first_msg,
            source,
            thread_source,
            agent_role,
            rollout_path,
        ) = row?;

        if session_id.is_empty() || cwd.is_empty() {
            continue;
        }

        if requires_rollout_path
            && !rollout_path
                .as_deref()
                .is_some_and(|path| !path.is_empty() && std::path::Path::new(path).is_file())
        {
            continue;
        }

        // Only filter when we have a trustworthy live set. If the
        // sessions tree could not be enumerated (`None`), surface the row
        // as-is — better stale than hidden.
        if let Some(live) = live_session_ids
            && !live.contains(&session_id)
        {
            continue;
        }

        let project_name = project_name_from_path(&cwd);

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
            interactive: is_interactive_source(
                &source,
                thread_source.as_deref(),
                agent_role.as_deref(),
            ),
        });
    }

    Ok(Some(sessions))
}

fn table_columns(conn: &Connection, table: &str) -> Result<HashSet<String>, rusqlite::Error> {
    let mut statement = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    statement
        .query_map([], |row| row.get::<_, String>(1))
        .and_then(Iterator::collect)
}

fn state_db_has_rollout_path(codex_dir: &std::path::Path) -> Result<bool, AgfError> {
    let Some(path) = find_state_db(codex_dir)? else {
        return Ok(false);
    };
    let conn = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    Ok(table_columns(&conn, "threads")?.contains("rollout_path"))
}

fn is_interactive_source(
    source: &str,
    thread_source: Option<&str>,
    agent_role: Option<&str>,
) -> bool {
    if matches!(thread_source, Some("subagent" | "exec")) || agent_role.is_some() {
        return false;
    }
    !matches!(source, "exec" | "subagent") && !source.contains("\"subagent\"")
}

/// Find the latest state_*.sqlite file in the codex directory.
fn find_state_db(codex_dir: &std::path::Path) -> std::io::Result<Option<std::path::PathBuf>> {
    let entries = match std::fs::read_dir(codex_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut paths = Vec::new();
    for entry in entries {
        paths.push(entry?.path());
    }
    Ok(paths
        .into_iter()
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("state_") && n.ends_with(".sqlite"))
        })
        // Rank by the numeric suffix, not the filename: lexicographic order
        // puts `state_10.sqlite` *below* `state_9.sqlite`, which would silently
        // start reading a superseded database the moment Codex reaches 10.
        // An unparsable suffix sorts below every numbered db (`None < Some`)
        // but is still eligible, so an unexpected name never empties the list.
        .max_by(|a, b| {
            state_db_index(a)
                .cmp(&state_db_index(b))
                .then_with(|| a.cmp(b))
        }))
}

/// Numeric suffix of a `state_<n>.sqlite` filename, or `None` if the name
/// doesn't have that shape.
fn state_db_index(path: &std::path::Path) -> Option<u64> {
    path.file_name()
        .and_then(|n| n.to_str())?
        .strip_prefix("state_")?
        .strip_suffix(".sqlite")?
        .parse()
        .ok()
}

/// Fallback: scan JSONL session files via walkdir (legacy format).
fn scan_jsonl(
    codex_dir: &std::path::Path,
    summaries: &HashMap<String, Vec<String>>,
) -> Result<Vec<Session>, AgfError> {
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
        return Ok(sessions);
    }

    for entry in WalkDir::new(&sessions_dir) {
        let entry = entry.map_err(std::io::Error::other)?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }

        let Some(first_line) = read_first_line_result(path)? else {
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

        let header_timestamp = payload
            .timestamp
            .as_deref()
            .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
            .map(|dt| dt.timestamp_millis())
            .unwrap_or(0);
        // Legacy rollout headers contain creation time. The rollout mtime is
        // the durable last-activity fallback and keeps legacy sessions aligned
        // with the SQLite scanner's updated_at semantics.
        let mtime = path
            .metadata()
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .and_then(|duration| i64::try_from(duration.as_millis()).ok())
            .unwrap_or(0);
        // Keep the raw durable candidate until the common scan boundary. A
        // near-future mtime must make the snapshot non-cacheable; clamping it
        // here to the creation header would permanently hide the later-valid
        // activity time behind an unchanged source fingerprint.
        let timestamp = header_timestamp.max(mtime);

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
            interactive: true,
        });
    }

    Ok(sessions)
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
) -> Result<HashMap<String, Vec<String>>, AgfError> {
    let path = codex_dir.join("history.jsonl");
    let mut summaries: HashMap<String, Vec<(f64, String)>> = HashMap::new();

    if !path.exists() {
        return Ok(HashMap::new());
    }

    let completed = for_each_bounded_line(&path, usize::MAX, HISTORY_LINE_BYTES, |line| {
        if line.iter().all(u8::is_ascii_whitespace) {
            return;
        }
        let Ok(entry) = serde_json::from_slice::<HistoryEntry>(line) else {
            return;
        };
        let session_id = match entry.session_id {
            Some(id) if !id.is_empty() => id,
            _ => return,
        };
        if let Some(live) = live_session_ids
            && !live.contains(&session_id)
        {
            return;
        }
        let ts = entry.ts.unwrap_or(0.0);
        let text = match entry.text {
            Some(t) if !t.is_empty() => t,
            _ => return,
        };
        let entries = summaries.entry(session_id).or_default();
        if entries.len() < MAX_SUMMARIES {
            entries.push((ts, text));
        } else if let Some((oldest_index, _)) = entries
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
            && ts > entries[oldest_index].0
        {
            entries[oldest_index] = (ts, text);
        }
    });
    if !completed {
        return Err(std::io::Error::other(format!("could not read {}", path.display())).into());
    }

    Ok(summaries
        .into_iter()
        .map(|(k, mut v)| {
            v.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
            (k, v.into_iter().map(|(_, s)| s).collect())
        })
        .collect())
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
    fn scan_sqlite_never_mutates_codex_state() {
        let dir = make_codex_dir("read-only");
        let db = dir.join("state_5.sqlite");
        seed_state_db(&db, &[("orphan", "/x"), ("live", "/x")]);
        let live = HashSet::from(["live".to_string()]);

        let sessions = scan_sqlite(&dir, &HashMap::new(), Some(&live))
            .unwrap()
            .unwrap();

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, "live");
        assert_eq!(count_threads(&db), 2, "a scan must not delete Codex rows");
    }

    #[test]
    fn scan_sqlite_requires_a_real_rollout_when_schema_exposes_its_path() {
        let dir = make_codex_dir("rollout-path");
        let db = dir.join("state_5.sqlite");
        let live_rollout = dir.join("live.jsonl");
        fs::write(&live_rollout, b"{}\n").unwrap();
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE threads (
                id TEXT PRIMARY KEY, cwd TEXT NOT NULL, title TEXT NOT NULL DEFAULT '',
                updated_at INTEGER NOT NULL DEFAULT 0, archived INTEGER NOT NULL DEFAULT 0,
                git_branch TEXT, first_user_message TEXT NOT NULL DEFAULT '', rollout_path TEXT
            );",
        )
        .unwrap();
        for (id, path) in [
            ("live", live_rollout.to_string_lossy().into_owned()),
            ("empty", String::new()),
            (
                "missing",
                dir.join("missing.jsonl").to_string_lossy().into_owned(),
            ),
        ] {
            conn.execute(
                "INSERT INTO threads (id, cwd, rollout_path) VALUES (?1, '/tmp/p', ?2)",
                rusqlite::params![id, path],
            )
            .unwrap();
        }
        drop(conn);

        let sessions = scan_sqlite(&dir, &HashMap::new(), None).unwrap().unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, "live");
    }

    #[test]
    fn existing_broken_state_db_is_a_scan_error_not_an_empty_success() {
        let dir = make_codex_dir("broken-state");
        fs::write(dir.join("state_1.sqlite"), b"not a sqlite database").unwrap();

        assert!(scan_sqlite(&dir, &HashMap::new(), None).is_err());
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

    #[test]
    fn collect_live_session_ids_rejects_a_partial_or_malformed_tree() {
        let dir = make_codex_dir("live-partial");
        let day_dir = dir.join("sessions/2026/04/29");
        fs::create_dir_all(&day_dir).unwrap();
        fs::write(
            day_dir.join("rollout-valid.jsonl"),
            br#"{"type":"session_meta","payload":{"id":"session-abc"}}
"#,
        )
        .unwrap();
        fs::write(day_dir.join("rollout-broken.jsonl"), b"not-json\n").unwrap();

        assert_eq!(collect_live_session_ids(&dir), None);
    }

    #[test]
    fn codex_source_classification_matches_resume_semantics() {
        assert!(is_interactive_source("cli", Some("user"), None));
        assert!(is_interactive_source("vscode", None, None));
        assert!(!is_interactive_source("exec", Some("user"), None));
        assert!(!is_interactive_source(
            r#"{"subagent":{"thread_spawn":{"depth":1}}}"#,
            Some("subagent"),
            Some("worker")
        ));
        // Older schemas have none of the classification columns. Preserve
        // their historical top-level behavior instead of hiding everything.
        assert!(is_interactive_source("", None, None));
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

        let summaries = read_history_summaries(&dir, Some(&live)).unwrap();

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

        let summaries = read_history_summaries(&dir, None).unwrap();

        assert_eq!(summaries.len(), 2);
        assert!(summaries.contains_key("a"));
        assert!(summaries.contains_key("b"));
    }
}

#[cfg(test)]
mod state_db_tests {
    use super::*;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("agf-codex-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Regression: ranking by filename put `state_10.sqlite` *below*
    /// `state_9.sqlite`, so the scanner would silently start reading a
    /// superseded database once Codex's schema counter reached double digits.
    #[test]
    fn find_state_db_ranks_by_number_not_lexicographically() {
        let dir = temp_dir("state-order");
        for name in ["state_4.sqlite", "state_9.sqlite", "state_10.sqlite"] {
            std::fs::write(dir.join(name), b"").unwrap();
        }

        let found = find_state_db(&dir).unwrap().unwrap();

        assert_eq!(found.file_name().unwrap(), "state_10.sqlite");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn find_state_db_still_returns_an_unnumbered_db_when_it_is_all_there_is() {
        let dir = temp_dir("state-unnumbered");
        std::fs::write(dir.join("state_backup.sqlite"), b"").unwrap();
        std::fs::write(dir.join("history.jsonl"), b"").unwrap();

        let found = find_state_db(&dir).unwrap().unwrap();

        assert_eq!(found.file_name().unwrap(), "state_backup.sqlite");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn find_state_db_prefers_a_numbered_db_over_an_unnumbered_one() {
        let dir = temp_dir("state-mixed");
        std::fs::write(dir.join("state_backup.sqlite"), b"").unwrap();
        std::fs::write(dir.join("state_2.sqlite"), b"").unwrap();

        let found = find_state_db(&dir).unwrap().unwrap();

        assert_eq!(found.file_name().unwrap(), "state_2.sqlite");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn find_state_db_returns_none_without_any_state_file() {
        let dir = temp_dir("state-none");
        std::fs::write(dir.join("history.jsonl"), b"").unwrap();
        assert!(find_state_db(&dir).unwrap().is_none());
        let _ = std::fs::remove_dir_all(dir);
    }
}
