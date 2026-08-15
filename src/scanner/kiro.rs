use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use rusqlite::Connection;
use serde_json::Value;

use crate::error::AgfError;
use crate::model::{Agent, Session};

use super::{collapse_whitespace, for_each_bounded_line, project_name_from_path, truncate};

const MAX_V3_METADATA_BYTES: u64 = 2 * 1024 * 1024;
const MAX_V3_EVENT_LINE_BYTES: usize = 1024 * 1024;

pub fn scan() -> Result<Vec<Session>, AgfError> {
    let mut by_id = HashMap::new();
    for session in scan_v2(&crate::config::kiro_data_dir()?.join("data.sqlite3"))? {
        by_id.insert(session.session_id.clone(), session);
    }
    for session in scan_v3(&crate::config::kiro_sessions_dir()?)? {
        match by_id.get(&session.session_id) {
            Some(existing) if existing.timestamp >= session.timestamp => {}
            _ => {
                by_id.insert(session.session_id.clone(), session);
            }
        }
    }
    let mut sessions: Vec<_> = by_id.into_values().collect();
    sessions.sort_by(|a, b| crate::model::compare_sessions(a, b, crate::model::SortMode::Time));
    Ok(sessions)
}

fn scan_v2(db_path: &Path) -> Result<Vec<Session>, AgfError> {
    if !db_path.exists() {
        return Ok(Vec::new());
    }
    let conn = Connection::open_with_flags(
        db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let mut stmt = match conn.prepare(
        // The prompt preview is near the head; never materialize an entire
        // multi-megabyte conversation blob merely to render one summary.
        "SELECT key, conversation_id, substr(value, 1, 262144), updated_at \
         FROM conversations_v2 ORDER BY updated_at DESC",
    ) {
        Ok(statement) => statement,
        Err(error) if error.to_string().contains("no such table") => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, i64>(3)?,
        ))
    })?;
    let mut sessions = Vec::new();
    for row in rows {
        let (directory, conversation_id, value, updated_at) = row?;
        if directory.is_empty() || conversation_id.is_empty() {
            continue;
        }
        sessions.push(Session {
            agent: Agent::Kiro,
            session_id: conversation_id,
            project_name: project_name_from_path(&directory),
            project_path: directory,
            summaries: extract_summary(&value).into_iter().collect(),
            timestamp: updated_at,
            git_branch: None,
            worktree: None,
            recap: None,
            interactive: true,
        });
    }
    Ok(sessions)
}

fn scan_v3(dir: &Path) -> Result<Vec<Session>, AgfError> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut sessions = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if !entry.file_type()?.is_file()
            || path.extension().and_then(|ext| ext.to_str()) != Some("json")
        {
            continue;
        }
        if let Some(session) = parse_v3_metadata(&path)? {
            sessions.push(session);
        }
    }
    Ok(sessions)
}

fn parse_v3_metadata(path: &Path) -> Result<Option<Session>, AgfError> {
    let mut content = String::new();
    File::open(path)?
        .take(MAX_V3_METADATA_BYTES)
        .read_to_string(&mut content)?;
    let Ok(metadata) = serde_json::from_str::<Value>(&content) else {
        return Ok(None);
    };
    let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
        return Ok(None);
    };
    let session_id = v3_session_id_from_metadata(&metadata, stem);
    let Some(project_path) = first_string(
        &metadata,
        &["/cwd", "/workingDirectory", "/session/cwd", "/metadata/cwd"],
    ) else {
        return Ok(None);
    };
    let project_path = project_path.trim().to_string();
    if session_id.is_empty() || project_path.is_empty() {
        return Ok(None);
    }

    let event_path = path.with_extension("jsonl");
    let event = scan_v3_events(&event_path)?;
    let metadata_time = [
        "/updated_at",
        "/created_at",
        "/updatedAt",
        "/lastUpdated",
        "/modifiedAt",
        "/createdAt",
    ]
    .into_iter()
    .filter_map(|pointer| parse_timestamp(metadata.pointer(pointer)))
    .max()
    .unwrap_or(0);
    let file_time = path
        .metadata()
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|time| time.duration_since(std::time::SystemTime::UNIX_EPOCH).ok())
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0);
    let title = first_string(
        &metadata,
        &["/title", "/name", "/session/title", "/metadata/title"],
    )
    .map(str::trim)
    .filter(|title| !title.is_empty())
    .map(|title| truncate(title, 100));
    let mut summaries = Vec::new();
    if let Some(title) = title {
        summaries.push(title);
    }
    if let Some(summary) = event.summary
        && summaries.first() != Some(&summary)
    {
        summaries.push(summary);
    }

    Ok(Some(Session {
        agent: Agent::Kiro,
        session_id,
        project_name: project_name_from_path(&project_path),
        project_path,
        summaries,
        // Preserve the durable raw value until the common scan boundary. If a
        // near-future clock-skewed value is clamped there, the snapshot is kept
        // out of the cache so it can become valid without a source rewrite.
        timestamp: if event.last_activity != 0 {
            event.last_activity
        } else if metadata_time != 0 {
            metadata_time
        } else {
            file_time
        },
        git_branch: first_string(&metadata, &["/gitBranch", "/git/branch"]).map(str::to_string),
        worktree: None,
        recap: None,
        interactive: true,
    }))
}

fn v3_session_id_from_metadata(metadata: &Value, stem: &str) -> String {
    first_string(
        metadata,
        &[
            "/session_id",
            "/sessionId",
            "/id",
            "/session/id",
            "/metadata/sessionId",
        ],
    )
    .unwrap_or(stem)
    .trim()
    .to_string()
}

/// Read the authoritative v3 session id without assuming it matches the
/// metadata filename. Kiro commonly stores generic stems such as
/// `transcript.json`, while the resumable id lives inside the document.
pub(crate) fn v3_session_id(path: &Path) -> Result<Option<String>, AgfError> {
    let mut content = String::new();
    File::open(path)?
        .take(MAX_V3_METADATA_BYTES)
        .read_to_string(&mut content)?;
    let Ok(metadata) = serde_json::from_str::<Value>(&content) else {
        return Ok(None);
    };
    let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
        return Ok(None);
    };
    let id = v3_session_id_from_metadata(&metadata, stem);
    Ok((!id.is_empty()).then_some(id))
}

#[derive(Default)]
struct V3EventInfo {
    summary: Option<String>,
    last_activity: i64,
}

fn scan_v3_events(path: &Path) -> Result<V3EventInfo, AgfError> {
    let mut info = V3EventInfo::default();
    if !path.exists() {
        return Ok(info);
    }
    let read_ok = for_each_bounded_line(path, usize::MAX, MAX_V3_EVENT_LINE_BYTES, |line| {
        let Ok(event) = serde_json::from_slice::<Value>(line) else {
            return;
        };
        let role = match event.get("kind").and_then(Value::as_str) {
            Some("Prompt") => Some("user"),
            Some("AssistantMessage") => Some("assistant"),
            _ => first_string(&event, &["/role", "/message/role", "/data/role"]),
        };
        if !matches!(role, Some("user" | "assistant")) {
            return;
        }
        let event_time = [
            "/data/meta/timestamp",
            "/timestamp",
            "/createdAt",
            "/message/timestamp",
        ]
        .into_iter()
        .filter_map(|pointer| parse_timestamp(event.pointer(pointer)))
        .max()
        .unwrap_or(0);
        info.last_activity = info.last_activity.max(event_time);
        if role == Some("user") && info.summary.is_none() {
            let content = event
                .pointer("/content")
                .or_else(|| event.pointer("/message/content"))
                .or_else(|| event.pointer("/data/content"));
            info.summary = content_text(content)
                .map(|text| collapse_whitespace(&text))
                .filter(|text| !text.is_empty())
                .map(|text| truncate(&text, 100));
        }
    });
    if !read_ok {
        return Err(std::io::Error::other(format!(
            "failed to read Kiro v3 events {}",
            path.display()
        ))
        .into());
    }
    Ok(info)
}

fn first_string<'a>(value: &'a Value, pointers: &[&str]) -> Option<&'a str> {
    pointers
        .iter()
        .find_map(|pointer| value.pointer(pointer).and_then(Value::as_str))
}

fn parse_timestamp(value: Option<&Value>) -> Option<i64> {
    match value? {
        Value::Number(number) => number.as_i64().map(|timestamp| {
            if timestamp < 10_000_000_000 {
                timestamp.saturating_mul(1000)
            } else {
                timestamp
            }
        }),
        Value::String(timestamp) => chrono::DateTime::parse_from_rfc3339(timestamp)
            .ok()
            .map(|timestamp| timestamp.timestamp_millis()),
        _ => None,
    }
}

fn content_text(content: Option<&Value>) -> Option<String> {
    match content? {
        Value::String(text) => Some(text.clone()),
        Value::Array(blocks) => {
            let text = blocks
                .iter()
                .filter_map(|block| {
                    block.get("text").and_then(Value::as_str).or_else(|| {
                        (block.get("kind").and_then(Value::as_str) == Some("text"))
                            .then(|| block.get("data").and_then(Value::as_str))
                            .flatten()
                    })
                })
                .collect::<Vec<_>>()
                .join(" ");
            (!text.is_empty()).then_some(text)
        }
        _ => None,
    }
}

fn extract_summary(value: &str) -> Option<String> {
    let parsed: Value = serde_json::from_str(value).ok()?;
    if let Some(prompt) = parsed
        .get("history")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find_map(|entry| {
            first_string(
                entry,
                &[
                    "/user/content/Prompt/prompt",
                    "/user/content/prompt",
                    "/user/Prompt/prompt",
                ],
            )
        })
    {
        let normalized = collapse_whitespace(prompt);
        if !normalized.is_empty() {
            return Some(truncate(&normalized, 100));
        }
    }
    let messages = parsed.get("messages").and_then(Value::as_array)?;
    for message in messages {
        if message.get("role").and_then(Value::as_str) != Some("user") {
            continue;
        }
        let text = content_text(message.get("content"))?;
        let normalized = collapse_whitespace(&text);
        if !normalized.is_empty() {
            return Some(truncate(&normalized, 100));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn v2_exact_ids_remain_distinct_in_one_cwd() {
        let dir = std::env::temp_dir().join(format!("agf-kiro-v2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("data.sqlite3");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE conversations_v2 (key TEXT, conversation_id TEXT, value TEXT, updated_at INTEGER);",
        )
        .unwrap();
        for (id, timestamp) in [
            ("older", 1_785_542_401_000i64),
            ("newer", 1_785_542_402_000),
        ] {
            conn.execute(
                "INSERT INTO conversations_v2 VALUES (?1, ?2, ?3, ?4)",
                ("/tmp/project", id, r#"{"messages":[]}"#, timestamp),
            )
            .unwrap();
        }
        drop(conn);

        let sessions = scan_v2(&db).unwrap();
        assert_eq!(sessions.len(), 2);
        assert_ne!(sessions[0].session_id, sessions[1].session_id);
    }

    #[test]
    fn extracts_real_v2_history_prompt_dialect() {
        let value =
            r#"{"history":[{"user":{"content":{"Prompt":{"prompt":"  real   v2 prompt  "}}}}]}"#;
        assert_eq!(extract_summary(value).as_deref(), Some("real v2 prompt"));
    }

    #[test]
    fn parses_v3_metadata_and_event_log() {
        let dir = std::env::temp_dir().join(format!("agf-kiro-v3-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let metadata = dir.join("sess_abc.json");
        std::fs::write(
            &metadata,
            r#"{"sessionId":"sess_abc","cwd":"/tmp/project","title":"Refactor","updatedAt":"2026-08-01T00:00:00Z"}"#,
        )
        .unwrap();
        let mut events = File::create(dir.join("sess_abc.jsonl")).unwrap();
        writeln!(
            events,
            r#"{{"role":"user","content":[{{"type":"text","text":"first prompt"}}],"timestamp":"2026-08-01T00:00:01Z"}}"#
        )
        .unwrap();

        let session = parse_v3_metadata(&metadata).unwrap().unwrap();
        assert_eq!(session.session_id, "sess_abc");
        assert_eq!(session.project_path, "/tmp/project");
        assert_eq!(session.summaries, ["Refactor", "first prompt"]);
        assert_eq!(session.timestamp, 1_785_542_401_000);
    }

    #[test]
    fn parses_real_v3_snake_case_replay_dialect() {
        let dir = std::env::temp_dir().join(format!("agf-kiro-v3-real-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let metadata = dir.join("transcript.json");
        std::fs::write(
            &metadata,
            r#"{"session_id":"real-id","created_at":1784126000,"updated_at":1784126037,"cwd":"/tmp/real-project","title":"Real replay"}"#,
        )
        .unwrap();
        let mut events = File::create(dir.join("transcript.jsonl")).unwrap();
        writeln!(
            events,
            r#"{{"version":"v1","kind":"Prompt","data":{{"content":[{{"kind":"text","data":"real first prompt"}}],"meta":{{"timestamp":1784126037}}}}}}"#
        )
        .unwrap();
        writeln!(
            events,
            r#"{{"version":"v1","kind":"AssistantMessage","data":{{"content":[],"meta":{{"timestamp":1784126040}}}}}}"#
        )
        .unwrap();

        let session = parse_v3_metadata(&metadata).unwrap().unwrap();
        assert_eq!(session.session_id, "real-id");
        assert_eq!(session.summaries, ["Real replay", "real first prompt"]);
        assert_eq!(session.timestamp, 1_784_126_040_000);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn durable_v3_updated_at_precedes_newer_file_mtime() {
        use std::time::{Duration, SystemTime};

        let dir = std::env::temp_dir().join(format!("agf-kiro-v3-mtime-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let metadata = dir.join("session.json");
        std::fs::write(
            &metadata,
            r#"{"session_id":"id","updated_at":"2026-08-01T00:00:00Z","cwd":"/tmp/project"}"#,
        )
        .unwrap();
        File::options()
            .write(true)
            .open(&metadata)
            .unwrap()
            .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1_786_320_000))
            .unwrap();

        let session = parse_v3_metadata(&metadata).unwrap().unwrap();
        assert_eq!(session.timestamp, 1_785_542_400_000);
        let _ = std::fs::remove_dir_all(dir);
    }
}
