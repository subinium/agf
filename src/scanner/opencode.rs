use rusqlite::Connection;

use crate::error::AgfError;
use crate::model::{Agent, Session};

use super::project_name_from_path;

const MAX_TITLE_CHARS: i64 = 200;
const MAX_CHILD_TITLES: i64 = 10;

pub fn scan() -> Result<Vec<Session>, AgfError> {
    let db_path = crate::config::opencode_data_dir()?.join("opencode.db");

    if !db_path.exists() {
        return Ok(Vec::new());
    }

    let conn = Connection::open_with_flags(
        &db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;

    scan_connection(&conn)
}

fn scan_connection(conn: &Connection) -> Result<Vec<Session>, AgfError> {
    // Fetch top-level sessions only (parent_id IS NULL).
    // Bound each title and the child count before aggregation, so SQLite and
    // Rust never materialize an unbounded title blob. JSON preserves literal
    // delimiters; the timestamp/id ordering also makes tied children stable.
    let mut stmt = conn.prepare(
        "SELECT s.id, substr(s.title, 1, ?1), s.directory, s.time_updated, \
                (SELECT json_group_array(title) FROM ( \
                    SELECT substr(sub.title, 1, ?1) AS title FROM session sub \
                    WHERE sub.parent_id = s.id AND sub.title IS NOT NULL \
                    ORDER BY sub.time_updated DESC, sub.id ASC LIMIT ?2 \
                )) \
         FROM session s \
         WHERE s.time_archived IS NULL AND s.parent_id IS NULL \
         ORDER BY s.time_updated DESC, s.id ASC",
    )?;

    let rows = stmt.query_map([MAX_TITLE_CHARS, MAX_CHILD_TITLES], |row| {
        let id: String = row.get(0)?;
        let title: String = row.get(1)?;
        let directory: String = row.get(2)?;
        let time_updated: i64 = row.get(3)?;
        let sub_titles: Option<String> = row.get(4)?;
        Ok((id, title, directory, time_updated, sub_titles))
    })?;
    let mut sessions = Vec::new();
    for row in rows {
        let (id, title, directory, time_updated, sub_titles) = row?;
        let project_name = project_name_from_path(&directory);

        // Parent title first, then deduplicated subagent titles.
        let mut summaries: Vec<String> = Vec::new();
        if !title.is_empty() {
            summaries.push(title);
        }
        if let Some(ref blob) = sub_titles {
            let mut seen = std::collections::HashSet::new();
            for title in serde_json::from_str::<Vec<String>>(blob)? {
                let title = title.trim();
                if !title.is_empty() && seen.insert(title.to_string()) {
                    summaries.push(title.to_string());
                }
            }
        }

        sessions.push(Session {
            agent: Agent::OpenCode,
            session_id: id,
            project_name,
            project_path: directory,
            summaries,
            timestamp: time_updated,
            git_branch: None,
            worktree: None,
            recap: None,
            interactive: true,
        });
    }

    Ok(sessions)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE session (
                id TEXT PRIMARY KEY, title TEXT, directory TEXT,
                time_updated INTEGER, time_archived INTEGER, parent_id TEXT
             );",
        )
        .unwrap();
        conn
    }

    #[test]
    fn titles_are_bounded_before_aggregation_and_children_are_deterministic() {
        let conn = fixture();
        let parent_title = "\u{89aa}".repeat(700_000);
        conn.execute(
            "INSERT INTO session VALUES ('parent', ?1, '/work/project', 100, NULL, NULL)",
            [&parent_title],
        )
        .unwrap();
        for index in (0..64).rev() {
            let title = format!(
                "child {index:02} ||| \"quoted\" {}",
                "\u{754c}".repeat(8192)
            );
            conn.execute(
                "INSERT INTO session VALUES (?1, ?2, '/work/project', 50, NULL, 'parent')",
                rusqlite::params![format!("child-{index:02}"), title],
            )
            .unwrap();
        }
        conn.execute_batch(
            "INSERT INTO session VALUES ('sibling', 'Sibling', '/work/sibling', 90, NULL, NULL);
             INSERT INTO session VALUES ('archived', 'Hidden', '/work/hidden', 110, 1, NULL);",
        )
        .unwrap();

        for reverse in [false, true] {
            conn.pragma_update(None, "reverse_unordered_selects", reverse)
                .unwrap();
            let sessions = scan_connection(&conn).unwrap();
            assert_eq!(sessions.len(), 2);
            assert_eq!(sessions[1].session_id, "sibling");
            let parent = &sessions[0];
            assert_eq!(parent.summaries.len(), 11);
            assert_eq!(parent.summaries[0], "\u{89aa}".repeat(200));
            for (index, title) in parent.summaries[1..].iter().enumerate() {
                assert!(title.starts_with(&format!("child {index:02} ||| \"quoted\" ")));
                assert_eq!(title.chars().count(), 200);
                assert!(title.len() <= 4 * 200);
            }
            assert_eq!(parent.timestamp, 100);
        }
    }

    #[test]
    fn child_titles_keep_existing_trim_and_dedup_semantics() {
        let conn = fixture();
        conn.execute_batch(
            "INSERT INTO session VALUES ('parent', 'Same', '/work/project', 100, NULL, NULL);
             INSERT INTO session VALUES ('a', ' Same ', '', 50, NULL, 'parent');
             INSERT INTO session VALUES ('b', 'Same', '', 50, NULL, 'parent');
             INSERT INTO session VALUES ('c', '   ', '', 50, NULL, 'parent');
             INSERT INTO session VALUES ('d', NULL, '', 50, NULL, 'parent');
             INSERT INTO session VALUES ('e', 'Literal ||| title', '', 50, NULL, 'parent');",
        )
        .unwrap();
        assert_eq!(
            scan_connection(&conn).unwrap()[0].summaries,
            ["Same", "Same", "Literal ||| title"]
        );
    }
}
