use rusqlite::Connection;

use crate::error::AgfError;
use crate::model::{Agent, Session};

/// Scan Hermes Agent sessions from `~/.hermes/state.db`.
///
/// Hermes stores sessions in a SQLite database with a `sessions` table
/// (metadata, title, token counts) and a `messages` table (conversation
/// content). Timestamps are Unix-epoch floats (seconds), which we convert
/// to milliseconds to match the agf `Session.timestamp` convention.
///
/// Sessions with a `parent_session_id` are delegation/compression children
/// and are excluded from the top-level listing (their titles are aggregated
/// as additional summaries on the parent, matching the OpenCode pattern).
pub fn scan() -> Result<Vec<Session>, AgfError> {
    let db_path = crate::config::hermes_dir()?.join("state.db");

    if !db_path.exists() {
        return Ok(Vec::new());
    }

    let conn = Connection::open_with_flags(
        &db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;

    // Fetch top-level sessions (no parent), ordered by most recent activity.
    // Use MAX(messages.timestamp) as last_active, falling back to started_at.
    // Aggregate child session titles as extra summaries.
    let mut stmt = conn.prepare(
        "SELECT s.id, \
                s.title, \
                s.source, \
                s.model, \
                s.message_count, \
                CAST(COALESCE(m.last_active, s.started_at) * 1000 AS INTEGER) AS ts_ms, \
                GROUP_CONCAT(child.title, '|||') \
         FROM sessions s \
         LEFT JOIN ( \
             SELECT session_id, MAX(timestamp) AS last_active \
             FROM messages GROUP BY session_id \
         ) m ON m.session_id = s.id \
         LEFT JOIN sessions child ON child.parent_session_id = s.id \
         WHERE s.parent_session_id IS NULL \
         GROUP BY s.id \
         ORDER BY ts_ms DESC",
    )?;

    let sessions = stmt
        .query_map([], |row| {
            let id: String = row.get(0)?;
            let title: Option<String> = row.get(1)?;
            let source: String = row.get(2)?;
            let model: Option<String> = row.get(3)?;
            let message_count: i64 = row.get(4)?;
            let timestamp: i64 = row.get(5)?;
            let child_titles: Option<String> = row.get(6)?;
            Ok((id, title, source, model, message_count, timestamp, child_titles))
        })?
        .filter_map(|r| r.ok())
        .map(|(id, title, source, model, message_count, timestamp, child_titles)| {
            // Build summaries: title first, then child session titles.
            let mut summaries: Vec<String> = Vec::new();
            if let Some(ref t) = title {
                if !t.is_empty() {
                    summaries.push(t.clone());
                }
            }
            if let Some(ref children) = child_titles {
                let mut seen = std::collections::HashSet::new();
                for t in children.split("|||") {
                    let t = t.trim();
                    if !t.is_empty() && seen.insert(t.to_string()) {
                        summaries.push(t.to_string());
                    }
                }
            }

            // If no title, build a fallback from source + model info.
            if summaries.is_empty() {
                let mut fallback = format!("{source} session");
                if let Some(ref m) = model {
                    if !m.is_empty() {
                        // Extract short model name (e.g. "claude-opus-4-6" from "anthropic/claude-opus-4-6")
                        let short = m.rsplit('/').next().unwrap_or(m);
                        fallback = format!("{fallback} ({short})");
                    }
                }
                if message_count > 0 {
                    fallback = format!("{fallback} — {message_count} msgs");
                }
                summaries.push(fallback);
            }

            // Hermes doesn't store working directory per session.
            // Default to the hermes home directory.
            let hermes_home = crate::config::hermes_dir()
                .map(|d| d.to_string_lossy().into_owned())
                .unwrap_or_else(|_| "~/.hermes".to_string());

            Session {
                agent: Agent::Hermes,
                session_id: id,
                project_name: "hermes".to_string(),
                project_path: hermes_home,
                summaries,
                timestamp,
                git_branch: None,
                worktree: None,
                recap: None,
            }
        })
        .collect();

    Ok(sessions)
}
