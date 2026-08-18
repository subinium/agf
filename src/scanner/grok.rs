use std::io::Read;
use std::path::Path;

use serde_json::Value;

use crate::error::AgfError;
use crate::model::{Agent, Session};

use super::{collapse_whitespace, project_name_from_path, truncate};

const MAX_SUMMARY_BYTES: u64 = 2 * 1024 * 1024;

pub fn scan() -> Result<Vec<Session>, AgfError> {
    scan_from(&crate::config::grok_dir()?.join("sessions"))
}

fn scan_from(root: &Path) -> Result<Vec<Session>, AgfError> {
    if !root.exists() {
        return Ok(Vec::new());
    }

    let mut sessions = Vec::new();
    for cwd_entry in std::fs::read_dir(root)? {
        let cwd_entry = cwd_entry?;
        if !cwd_entry.file_type()?.is_dir() {
            continue;
        }
        for session_entry in std::fs::read_dir(cwd_entry.path())? {
            let session_entry = session_entry?;
            if !session_entry.file_type()?.is_dir() {
                continue;
            }
            let summary_path = session_entry.path().join("summary.json");
            if !summary_path.is_file() {
                continue;
            }
            if let Some(session) = parse_summary(&summary_path)? {
                sessions.push(session);
            }
        }
    }
    sessions.sort_by(|a, b| crate::model::compare_sessions(a, b, crate::model::SortMode::Time));
    Ok(sessions)
}

fn parse_summary(path: &Path) -> Result<Option<Session>, AgfError> {
    let metadata = path.metadata()?;
    if metadata.len() > MAX_SUMMARY_BYTES {
        return Ok(None);
    }
    let mut content = String::with_capacity(metadata.len() as usize);
    std::fs::File::open(path)?
        .take(MAX_SUMMARY_BYTES + 1)
        .read_to_string(&mut content)?;
    let Ok(summary) = serde_json::from_str::<Value>(&content) else {
        return Ok(None);
    };

    let Some(directory_id) = path
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .map(str::trim)
        .filter(|id| !id.is_empty())
    else {
        return Ok(None);
    };
    let Some(stored_id) = summary
        .pointer("/info/id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
    else {
        return Ok(None);
    };
    // Grok resumes by the directory/session id. A mismatched summary is not a
    // trustworthy resume target and must not surface under either identity.
    if stored_id != directory_id {
        return Ok(None);
    }
    let Some(project_path) = summary
        .pointer("/info/cwd")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|cwd| !cwd.is_empty())
        .map(str::to_string)
    else {
        return Ok(None);
    };

    let mut summaries = Vec::new();
    for pointer in ["/generated_title", "/session_summary", "/last_turn_summary"] {
        let Some(text) = summary
            .pointer(pointer)
            .and_then(Value::as_str)
            .map(collapse_whitespace)
            .filter(|text| !text.is_empty())
            .map(|text| truncate(&text, 200))
        else {
            continue;
        };
        if !summaries.contains(&text) {
            summaries.push(text);
        }
    }

    let file_time = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0);
    let timestamp = ["/last_active_at", "/updated_at", "/created_at"]
        .into_iter()
        .find_map(|pointer| parse_timestamp(summary.pointer(pointer)))
        .unwrap_or(file_time);
    let session_kind = summary
        .get("session_kind")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let hidden = summary.get("hidden").and_then(Value::as_bool) == Some(true);

    Ok(Some(Session {
        agent: Agent::Grok,
        session_id: directory_id.to_string(),
        project_name: project_name_from_path(&project_path),
        project_path,
        summaries,
        timestamp,
        git_branch: summary
            .get("head_branch")
            .and_then(Value::as_str)
            .map(str::to_string),
        worktree: summary
            .get("worktree_label")
            .and_then(Value::as_str)
            .map(str::to_string),
        recap: summary
            .get("last_recap")
            .and_then(Value::as_str)
            .map(collapse_whitespace)
            .filter(|recap| !recap.is_empty())
            .map(|recap| truncate(&recap, 400)),
        interactive: !hidden && !matches!(session_kind, "subagent" | "subagent_fork"),
    }))
}

fn parse_timestamp(value: Option<&Value>) -> Option<i64> {
    match value? {
        Value::String(timestamp) => chrono::DateTime::parse_from_rfc3339(timestamp)
            .ok()
            .map(|timestamp| timestamp.timestamp_millis()),
        Value::Number(timestamp) => timestamp.as_i64().map(epoch_millis),
        _ => None,
    }
    .filter(|timestamp| *timestamp > 0)
}

fn epoch_millis(timestamp: i64) -> i64 {
    if timestamp.unsigned_abs() < 10_000_000_000 {
        timestamp.saturating_mul(1000)
    } else {
        timestamp
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_official_summary_fields_and_activity_precedence() {
        let root = std::env::temp_dir().join(format!("agf-grok-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let session_dir = root.join("encoded-cwd").join("019c-grok-session");
        std::fs::create_dir_all(&session_dir).unwrap();
        std::fs::write(
            session_dir.join("summary.json"),
            r#"{
              "info":{"id":"019c-grok-session","cwd":"/work/grok-project"},
              "session_summary":"Initial title",
              "generated_title":"Fix cache races",
              "created_at":"2026-08-01T00:00:00Z",
              "updated_at":"2026-08-02T00:00:00Z",
              "last_active_at":"2026-08-03T04:05:06Z",
              "head_branch":"fix/cache",
              "worktree_label":"cache-wt",
              "last_turn_summary":"Added a regression test",
              "last_recap":"The scanner is ready for review"
            }"#,
        )
        .unwrap();

        let sessions = scan_from(&root).unwrap();
        assert_eq!(sessions.len(), 1);
        let session = &sessions[0];
        assert_eq!(session.agent, Agent::Grok);
        assert_eq!(session.project_name, "grok-project");
        assert_eq!(session.summaries[0], "Fix cache races");
        assert_eq!(session.git_branch.as_deref(), Some("fix/cache"));
        assert_eq!(session.worktree.as_deref(), Some("cache-wt"));
        assert_eq!(
            session.timestamp,
            chrono::DateTime::parse_from_rfc3339("2026-08-03T04:05:06Z")
                .unwrap()
                .timestamp_millis()
        );
        assert_eq!(
            session.recap.as_deref(),
            Some("The scanner is ready for review")
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
