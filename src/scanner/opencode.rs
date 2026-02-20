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

    let mut stmt = conn.prepare(
        "SELECT id, title, directory, time_updated \
         FROM session \
         WHERE time_archived IS NULL \
         ORDER BY time_updated DESC",
    )?;

    let sessions = stmt
        .query_map([], |row| {
            let id: String = row.get(0)?;
            let title: String = row.get(1)?;
            let directory: String = row.get(2)?;
            let time_updated: i64 = row.get(3)?;
            Ok((id, title, directory, time_updated))
        })?
        .filter_map(|r| r.ok())
        .map(|(id, title, directory, time_updated)| {
            let project_name = std::path::Path::new(&directory)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();

            Session {
                agent: Agent::OpenCode,
                session_id: id,
                project_name,
                project_path: directory,
                summaries: if title.is_empty() {
                    Vec::new()
                } else {
                    vec![title]
                },
                timestamp: time_updated,
                git_branch: None,
                git_dirty: None,
            }
        })
        .collect();

    Ok(sessions)
}
