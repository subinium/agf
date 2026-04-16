use rusqlite::Connection;

use crate::error::AgfError;
use crate::model::{Agent, Session};

pub fn scan() -> Result<Vec<Session>, AgfError> {
    let db_path = crate::config::opencode_data_dir()?.join("opencode.db");

    if !db_path.exists() {
        return Ok(Vec::new());
    }

    let conn = Connection::open_with_flags(
        &db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;

    // Fetch top-level sessions only (parent_id IS NULL).
    // Aggregate subagent titles as additional summaries so the preview shows
    // what each subagent was working on.
    let mut stmt = conn.prepare(
        "SELECT s.id, s.title, s.directory, s.time_updated, \
                GROUP_CONCAT(sub.title, '|||') \
         FROM session s \
         LEFT JOIN session sub ON sub.parent_id = s.id \
         WHERE s.time_archived IS NULL AND s.parent_id IS NULL \
         GROUP BY s.id \
         ORDER BY s.time_updated DESC",
    )?;

    let sessions = stmt
        .query_map([], |row| {
            let id: String = row.get(0)?;
            let title: String = row.get(1)?;
            let directory: String = row.get(2)?;
            let time_updated: i64 = row.get(3)?;
            let sub_titles: Option<String> = row.get(4)?;
            Ok((id, title, directory, time_updated, sub_titles))
        })?
        .filter_map(|r| r.ok())
        .map(|(id, title, directory, time_updated, sub_titles)| {
            let project_name = std::path::Path::new(&directory)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();

            // Parent title first, then deduplicated subagent titles.
            let mut summaries: Vec<String> = Vec::new();
            if !title.is_empty() {
                summaries.push(title);
            }
            if let Some(sub) = sub_titles {
                let mut seen = std::collections::HashSet::new();
                for t in sub.split("|||") {
                    let t = t.trim();
                    if !t.is_empty() && seen.insert(t.to_string()) {
                        summaries.push(t.to_string());
                    }
                }
            }

            Session {
                agent: Agent::OpenCode,
                session_id: id,
                project_name,
                project_path: directory,
                summaries,
                timestamp: time_updated,
                git_branch: None,
                worktree: None,
                recap: None,
            }
        })
        .collect();

    Ok(sessions)
}
