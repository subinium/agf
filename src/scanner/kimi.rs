use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

use serde_json::Value;

use crate::error::AgfError;
use crate::model::{Agent, Session};

use super::{collapse_whitespace, for_each_bounded_line, project_name_from_path, truncate};

const MAX_STATE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_INDEX_LINE_BYTES: usize = 64 * 1024;

pub fn scan() -> Result<Vec<Session>, AgfError> {
    let home = crate::config::kimi_code_dir()?;
    scan_from(&home.join("sessions"), &home.join("session_index.jsonl"))
}

fn scan_from(sessions_root: &Path, index_path: &Path) -> Result<Vec<Session>, AgfError> {
    if !sessions_root.exists() {
        return Ok(Vec::new());
    }
    let indexed_workdirs = read_index_workdirs(index_path)?;
    let mut sessions = Vec::new();
    for workspace_entry in std::fs::read_dir(sessions_root)? {
        let workspace_entry = workspace_entry?;
        if !workspace_entry.file_type()?.is_dir() {
            continue;
        }
        for session_entry in std::fs::read_dir(workspace_entry.path())? {
            let session_entry = session_entry?;
            if !session_entry.file_type()?.is_dir() {
                continue;
            }
            let session_dir = session_entry.path();
            let state_path = if session_dir.join("state.json").is_file() {
                session_dir.join("state.json")
            } else {
                session_dir.join("session-meta/state.json")
            };
            if !state_path.is_file() {
                continue;
            }
            let fallback_workdir = session_entry
                .file_name()
                .to_str()
                .and_then(|id| indexed_workdirs.get(id));
            if let Some(session) = parse_state(&state_path, fallback_workdir)? {
                sessions.push(session);
            }
        }
    }
    sessions.sort_by(|a, b| crate::model::compare_sessions(a, b, crate::model::SortMode::Time));
    Ok(sessions)
}

fn read_index_workdirs(path: &Path) -> Result<HashMap<String, String>, AgfError> {
    let mut workdirs = HashMap::new();
    if !path.exists() {
        return Ok(workdirs);
    }
    let read_ok = for_each_bounded_line(path, usize::MAX, MAX_INDEX_LINE_BYTES, |line| {
        let Ok(record) = serde_json::from_slice::<Value>(line) else {
            return;
        };
        let Some(id) = record
            .get("sessionId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
        else {
            return;
        };
        if record.get("deleted").and_then(Value::as_bool) == Some(true) {
            workdirs.remove(id);
            return;
        }
        if let Some(workdir) = record
            .get("workDir")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|workdir| !workdir.is_empty())
        {
            workdirs.insert(id.to_string(), workdir.to_string());
        }
    });
    if !read_ok {
        return Err(std::io::Error::other(format!(
            "failed to read Kimi session index {}",
            path.display()
        ))
        .into());
    }
    Ok(workdirs)
}

fn parse_state(
    path: &Path,
    fallback_workdir: Option<&String>,
) -> Result<Option<Session>, AgfError> {
    let metadata = path.metadata()?;
    if metadata.len() > MAX_STATE_BYTES {
        return Ok(None);
    }
    let mut content = String::with_capacity(metadata.len() as usize);
    std::fs::File::open(path)?
        .take(MAX_STATE_BYTES + 1)
        .read_to_string(&mut content)?;
    let Ok(state) = serde_json::from_str::<Value>(&content) else {
        return Ok(None);
    };
    if state.get("archived").and_then(Value::as_bool) == Some(true) {
        return Ok(None);
    }

    let Some(session_id) = path
        .parent()
        .and_then(|parent| {
            if parent.file_name().and_then(|name| name.to_str()) == Some("session-meta") {
                parent.parent()
            } else {
                Some(parent)
            }
        })
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
    else {
        return Ok(None);
    };
    let project_path = ["/cwd", "/workDir", "/custom/cwd"]
        .into_iter()
        .find_map(|pointer| state.pointer(pointer).and_then(Value::as_str))
        .map(str::trim)
        .filter(|cwd| !cwd.is_empty())
        .map(str::to_string)
        .or_else(|| fallback_workdir.cloned());
    let Some(project_path) = project_path else {
        return Ok(None);
    };

    let mut summaries = Vec::new();
    for pointer in ["/title", "/lastPrompt"] {
        let Some(text) = state
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
    let timestamp = parse_timestamp(state.get("updatedAt"))
        .or_else(|| parse_timestamp(state.get("createdAt")))
        .unwrap_or(file_time);
    let is_child = state
        .pointer("/custom/child_session_kind")
        .and_then(Value::as_str)
        == Some("child");

    Ok(Some(Session {
        agent: Agent::Kimi,
        session_id,
        project_name: project_name_from_path(&project_path),
        project_path,
        summaries,
        timestamp,
        git_branch: state
            .get("gitBranch")
            .or_else(|| state.pointer("/custom/gitBranch"))
            .and_then(Value::as_str)
            .map(str::to_string),
        worktree: state
            .get("worktreeLabel")
            .or_else(|| state.pointer("/custom/worktreeLabel"))
            .and_then(Value::as_str)
            .map(str::to_string),
        recap: None,
        interactive: !is_child,
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
    fn parses_v2_state_and_legacy_index_workdir() {
        let root = std::env::temp_dir().join(format!("agf-kimi-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let sessions_root = root.join("sessions");
        let current = sessions_root.join("wd_current").join("kimi-current");
        let legacy = sessions_root.join("wd_legacy").join("kimi-legacy");
        std::fs::create_dir_all(&current).unwrap();
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(
            current.join("state.json"),
            r#"{
              "version":2,
              "cwd":"/work/kimi-current",
              "createdAt":1786000000000,
              "updatedAt":1786000123000,
              "title":"Implement auth",
              "lastPrompt":"Add the refresh-token tests",
              "custom":{}
            }"#,
        )
        .unwrap();
        std::fs::write(
            legacy.join("state.json"),
            r#"{
              "createdAt":"2026-08-01T00:00:00Z",
              "updatedAt":"2026-08-02T00:00:00Z",
              "title":"Legacy session",
              "lastPrompt":"Recover my cwd",
              "custom":{}
            }"#,
        )
        .unwrap();
        std::fs::write(
            root.join("session_index.jsonl"),
            format!(
                "{{\"sessionId\":\"kimi-legacy\",\"sessionDir\":{:?},\"workDir\":\"/work/kimi-legacy\"}}\n",
                legacy.to_string_lossy()
            ),
        )
        .unwrap();

        let sessions = scan_from(&sessions_root, &root.join("session_index.jsonl")).unwrap();
        assert_eq!(sessions.len(), 2);
        let current = sessions
            .iter()
            .find(|session| session.session_id == "kimi-current")
            .unwrap();
        assert_eq!(current.agent, Agent::Kimi);
        assert_eq!(current.project_path, "/work/kimi-current");
        assert_eq!(current.timestamp, 1_786_000_123_000);
        let legacy = sessions
            .iter()
            .find(|session| session.session_id == "kimi-legacy")
            .unwrap();
        assert_eq!(legacy.project_path, "/work/kimi-legacy");
        let _ = std::fs::remove_dir_all(root);
    }
}
