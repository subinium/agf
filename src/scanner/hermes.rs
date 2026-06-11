use rusqlite::Connection;

use crate::error::AgfError;
use crate::model::{Agent, Session};

use super::{char_prefix, collapse_whitespace, push_concat_titles};

/// Trim a chunk of message text down to a single-line preview that fits in
/// a TUI summary row. Collapses whitespace, drops common wrapper tags, and
/// caps to ~160 chars.
fn message_preview(raw: &str) -> Option<String> {
    let stripped = raw
        .replace("<user_query>", " ")
        .replace("</user_query>", " ");
    let collapsed = collapse_whitespace(&stripped);
    if collapsed.is_empty() {
        return None;
    }
    let max_chars = 160;
    if collapsed.chars().count() > max_chars {
        Some(format!("{}…", char_prefix(&collapsed, max_chars)))
    } else {
        Some(collapsed)
    }
}

/// True iff the id looks like a session a user actually started from the
/// Hermes CLI/TUI (id format `YYYYMMDD_HHMMSS_<6hex>`). Hermes also stores
/// long-lived API integration sessions under ids like `dashboard:admin:...`
/// or `api-<16hex>` — those receive messages from external callers (Notion
/// integrations, scheduled cron jobs, dashboard webhooks), so their first
/// `messages.role='user'` row is not the user's own first prompt and would
/// be misleading as a TUI summary preview.
fn is_user_cli_session(id: &str) -> bool {
    // YYYYMMDD = 8 ASCII digits, then `_`, then HHMMSS = 6 ASCII digits,
    // then `_`, then 6 hex chars. The exact suffix length isn't load-bearing
    // here — we just need to distinguish CLI ids from `dashboard:*`,
    // `api-*`, UUID, and named (`research`, `test`) ids.
    let bytes = id.as_bytes();
    if bytes.len() < 16 {
        return false;
    }
    bytes[..8].iter().all(|b| b.is_ascii_digit())
        && bytes[8] == b'_'
        && bytes[9..15].iter().all(|b| b.is_ascii_digit())
        && bytes[15] == b'_'
}

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
///
/// Project path is intentionally empty: Hermes is cwd-independent (the
/// agent runs from `~/.hermes` regardless of where the user invoked it),
/// so resume should not drag the user's shell into `~/.hermes`. The empty
/// project_path is honored by `shell::cd_and`, which skips the `cd` step
/// when the path is empty.
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
    // Aggregate child session titles as extra summaries; pull the first few
    // user messages as a conversation preview so sessions with NULL title
    // (short / un-titled sessions) still surface readable history in the
    // TUI detail pane. We `||| `-join up to 4 user messages chronologically
    // and split them back out in Rust — that's enough to show how the
    // conversation started without flooding the row.
    let mut stmt = conn.prepare(
        "SELECT s.id, \
                s.title, \
                s.source, \
                s.model, \
                s.message_count, \
                CAST(COALESCE(m.last_active, s.started_at) * 1000 AS INTEGER) AS ts_ms, \
                GROUP_CONCAT(child.title, '|||'), \
                ( \
                    SELECT GROUP_CONCAT(content, '|||') FROM ( \
                        SELECT content FROM messages \
                        WHERE session_id = s.id AND role = 'user' AND content IS NOT NULL \
                        ORDER BY timestamp ASC LIMIT 4 \
                    ) \
                ) AS user_msgs \
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
            let user_msgs: Option<String> = row.get(7)?;
            Ok((
                id,
                title,
                source,
                model,
                message_count,
                timestamp,
                child_titles,
                user_msgs,
            ))
        })?
        .filter_map(|r| r.ok())
        .map(
            |(id, title, source, model, message_count, timestamp, child_titles, user_msgs)| {
                // Build summaries: title first, then child session titles,
                // then up to 4 user-message previews so the TUI detail pane
                // shows how the conversation actually went, not just one
                // line.
                let mut summaries: Vec<String> = Vec::new();
                if let Some(ref t) = title
                    && !t.is_empty()
                {
                    summaries.push(t.clone());
                }
                if let Some(ref children) = child_titles {
                    push_concat_titles(&mut summaries, children);
                }
                // Only surface user-message previews for ids that look like
                // CLI/TUI sessions. dashboard:*/api-*/named ids get messages
                // from external integrations (Notion webhooks, scheduled
                // crons, dashboard callers), so their `role='user'` rows
                // are not the user's own prompts and would mislead the
                // TUI summary.
                if is_user_cli_session(&id)
                    && let Some(ref blob) = user_msgs
                {
                    let mut seen: std::collections::HashSet<String> =
                        summaries.iter().cloned().collect();
                    for raw in blob.split("|||") {
                        if let Some(preview) = message_preview(raw)
                            && seen.insert(preview.clone())
                        {
                            summaries.push(preview);
                        }
                    }
                }

                // If we still have nothing, fall back to source + model info
                // so the row is never blank.
                if summaries.is_empty() {
                    let mut fallback = format!("{source} session");
                    if let Some(ref m) = model
                        && !m.is_empty()
                    {
                        // Extract short model name (e.g. "claude-opus-4-6" from "anthropic/claude-opus-4-6")
                        let short = m.rsplit('/').next().unwrap_or(m);
                        fallback = format!("{fallback} ({short})");
                    }
                    if message_count > 0 {
                        fallback = format!("{fallback} — {message_count} msgs");
                    }
                    summaries.push(fallback);
                }

                Session {
                    agent: Agent::Hermes,
                    session_id: id,
                    project_name: "hermes".to_string(),
                    // Empty path → resume runs in the user's current cwd
                    // (Hermes is cwd-independent). See shell::cd_and.
                    project_path: String::new(),
                    summaries,
                    timestamp,
                    git_branch: None,
                    worktree: None,
                    recap: None,
                }
            },
        )
        .collect();

    Ok(sessions)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_preview_collapses_whitespace_and_strips_user_query_tags() {
        let raw = "<user_query>\n  hello\n  there  \n</user_query>";
        assert_eq!(message_preview(raw).as_deref(), Some("hello there"));
    }

    #[test]
    fn message_preview_truncates_with_ellipsis() {
        let raw = "a ".repeat(200);
        let preview = message_preview(&raw).unwrap();
        assert!(preview.chars().count() <= 161);
        assert!(preview.ends_with('…'));
    }

    #[test]
    fn message_preview_returns_none_for_blank_input() {
        assert_eq!(message_preview("   \n\t  "), None);
    }

    #[test]
    fn is_user_cli_session_recognizes_cli_id_format() {
        assert!(is_user_cli_session("20260504_093414_1e69e0"));
        assert!(is_user_cli_session("20260411_001454_e77613"));
    }

    #[test]
    fn is_user_cli_session_rejects_api_and_named_ids() {
        assert!(!is_user_cli_session("dashboard:admin:notion-checker"));
        assert!(!is_user_cli_session("api-1234567890abcdef"));
        assert!(!is_user_cli_session("research"));
        assert!(!is_user_cli_session("test"));
        assert!(!is_user_cli_session("4619234e-10b6-4db2-8745-cc56a2682503"));
    }
}
