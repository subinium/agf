use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};

use rusqlite::Connection;

use crate::error::AgfError;
use crate::model::{Agent, Session};
use crate::scanner::{first_line_truncated, read_first_line};

pub fn scan() -> Result<Vec<Session>, AgfError> {
    let codex_dir = crate::config::codex_dir()?;

    // Collect summaries from history.jsonl (keyed by session_id, newest-first)
    let summaries = read_history_summaries(&codex_dir);

    // Primary: read from SQLite (state_*.sqlite)
    let mut sessions = scan_sqlite(&codex_dir, &summaries);

    // Fallback: if SQLite found nothing, try JSONL walkdir
    if sessions.is_empty() {
        sessions = scan_jsonl(&codex_dir, &summaries);
    }

    sessions.sort_by_key(|s| std::cmp::Reverse(s.timestamp));
    Ok(sessions)
}

/// Read sessions from Codex SQLite database (state_*.sqlite).
/// This is the primary source — covers CLI, desktop app (vscode), and exec sessions.
fn scan_sqlite(
    codex_dir: &std::path::Path,
    summaries: &HashMap<String, Vec<String>>,
) -> Vec<Session> {
    // Find the latest state_*.sqlite file
    let db_path = match find_state_db(codex_dir) {
        Some(p) => p,
        None => return Vec::new(),
    };

    let conn =
        match Connection::open_with_flags(&db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY) {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };

    let mut stmt = match conn.prepare(
        "SELECT id, cwd, title, updated_at, git_branch, first_user_message
         FROM threads
         WHERE archived = 0 AND cwd != ''
         ORDER BY updated_at DESC",
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };

    let rows = match stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2).unwrap_or_default(),
            row.get::<_, i64>(3).unwrap_or(0),
            row.get::<_, Option<String>>(4)?,
            row.get::<_, String>(5).unwrap_or_default(),
        ))
    }) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };

    let mut sessions = Vec::new();
    for row in rows.flatten() {
        let (session_id, cwd, title, updated_at, git_branch, first_msg) = row;

        if session_id.is_empty() || cwd.is_empty() {
            continue;
        }

        let project_name = std::path::Path::new(&cwd)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        // updated_at is Unix seconds — convert to millis
        let timestamp = updated_at * 1000;

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

    sessions
}

/// Find the latest state_*.sqlite file in the codex directory.
fn find_state_db(codex_dir: &std::path::Path) -> Option<std::path::PathBuf> {
    let entries = std::fs::read_dir(codex_dir).ok()?;
    let mut candidates: Vec<std::path::PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("state_") && n.ends_with(".sqlite"))
                .unwrap_or(false)
        })
        .collect();

    // Sort descending by name so state_5 > state_4 etc.
    candidates.sort_by(|a, b| b.cmp(a));
    candidates.into_iter().next()
}

/// Fallback: scan JSONL session files via walkdir (legacy format).
fn scan_jsonl(
    codex_dir: &std::path::Path,
    summaries: &HashMap<String, Vec<String>>,
) -> Vec<Session> {
    use serde::Deserialize;
    use walkdir::WalkDir;

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

        let first_line = match read_first_line(path) {
            Some(line) => line,
            None => continue,
        };

        let meta: SessionMeta = match serde_json::from_str(first_line.trim()) {
            Ok(m) => m,
            Err(_) => continue,
        };

        if meta.entry_type.as_deref() != Some("session_meta") {
            continue;
        }

        let payload = match meta.payload {
            Some(p) => p,
            None => continue,
        };

        let session_id = match payload.id {
            Some(id) if !id.is_empty() => id,
            _ => continue,
        };

        let cwd = match payload.cwd {
            Some(cwd) if !cwd.is_empty() => cwd,
            _ => continue,
        };

        let project_name = std::path::Path::new(&cwd)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

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

fn read_history_summaries(codex_dir: &std::path::Path) -> HashMap<String, Vec<String>> {
    let path = codex_dir.join("history.jsonl");
    let mut summaries: HashMap<String, Vec<(f64, String)>> = HashMap::new();

    let file = match File::open(&path) {
        Ok(f) => f,
        Err(_) => return HashMap::new(),
    };

    for line in BufReader::new(file).lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };
        let line = line.trim().to_owned();
        if line.is_empty() {
            continue;
        }
        let entry: HistoryEntry = match serde_json::from_str(&line) {
            Ok(e) => e,
            Err(_) => continue,
        };
        let session_id = match entry.session_id {
            Some(id) if !id.is_empty() => id,
            _ => continue,
        };
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
