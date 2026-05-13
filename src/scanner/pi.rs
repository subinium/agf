use serde::Deserialize;
use serde_json::Value;
use std::fs::File;
use std::io::{BufRead, BufReader};
use walkdir::WalkDir;

use crate::error::AgfError;
use crate::model::{Agent, Session};
use crate::scanner::first_line_truncated;

const SUMMARY_MAX_CHARS: usize = 120;

#[derive(Deserialize)]
struct PiSessionHeader {
    #[serde(rename = "type")]
    entry_type: Option<String>,
    id: Option<String>,
    timestamp: Option<String>,
    cwd: Option<String>,
}

pub fn scan() -> Result<Vec<Session>, AgfError> {
    let sessions_dir = crate::config::pi_sessions_dir()?;
    if !sessions_dir.exists() {
        return Ok(Vec::new());
    }

    let mut sessions = Vec::new();

    for entry in WalkDir::new(&sessions_dir)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();
        if !path.is_file() || path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }

        if let Some(session) = parse_session(path) {
            sessions.push(session);
        }
    }

    // Sort by timestamp desc. Pi supports resuming a specific session by id,
    // so keep older sessions from the same project selectable.
    sessions.sort_by_key(|s| std::cmp::Reverse(s.timestamp));

    Ok(sessions)
}

fn parse_session(path: &std::path::Path) -> Option<Session> {
    let file = File::open(path).ok()?;
    let reader = BufReader::new(file);
    let mut header = None;
    let mut summary = None;

    for line in reader.lines().map_while(Result::ok) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let value: Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(_) => continue,
        };

        if header.is_none() && value.get("type").and_then(Value::as_str) == Some("session") {
            header = serde_json::from_value::<PiSessionHeader>(value.clone()).ok();
        }

        if summary.is_none() {
            summary = extract_user_summary(&value);
        }

        if header.is_some() && summary.is_some() {
            break;
        }
    }

    let header = header?;
    if header.entry_type.as_deref() != Some("session") {
        return None;
    }

    let session_id = header.id?;
    let cwd = header.cwd?;
    let timestamp = header
        .timestamp
        .and_then(|t| chrono::DateTime::parse_from_rfc3339(&t).ok())
        .map(|dt| dt.timestamp_millis())
        .unwrap_or_else(|| {
            path.metadata()
                .and_then(|m| m.modified())
                .map(|t| {
                    t.duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as i64
                })
                .unwrap_or(0)
        });

    let project_name = std::path::Path::new(&cwd)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();

    let summaries = summary.into_iter().collect();

    Some(Session {
        agent: Agent::Pi,
        session_id,
        project_name,
        project_path: cwd,
        summaries,
        timestamp,
        git_branch: None,
        worktree: None,
        recap: None,
    })
}

fn extract_user_summary(value: &Value) -> Option<String> {
    if value.get("type").and_then(Value::as_str) != Some("message") {
        return None;
    }

    let message = value.get("message")?;
    if message.get("role").and_then(Value::as_str) != Some("user") {
        return None;
    }

    extract_text_content(message.get("content")?)
}

fn extract_text_content(content: &Value) -> Option<String> {
    match content {
        Value::String(text) => first_line_truncated(text, SUMMARY_MAX_CHARS),
        Value::Array(items) => items.iter().find_map(extract_text_content),
        Value::Object(map) => match map.get("text") {
            Some(Value::String(text)) => first_line_truncated(text, SUMMARY_MAX_CHARS),
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;
    use std::sync::{Mutex, OnceLock};

    fn home_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn temp_home(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "agf-pi-scan-{name}-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn write_pi_session(
        home: &std::path::Path,
        dir: &str,
        filename: &str,
        id: &str,
        timestamp: &str,
        cwd: &str,
    ) {
        let session_dir = home.join(".pi/agent/sessions").join(dir);
        fs::create_dir_all(&session_dir).unwrap();
        let mut file = fs::File::create(session_dir.join(filename)).unwrap();
        writeln!(
            file,
            r#"{{"type":"session","id":"{id}","timestamp":"{timestamp}","cwd":"{cwd}"}}"#
        )
        .unwrap();
    }

    fn write_pi_session_lines(home: &std::path::Path, dir: &str, filename: &str, lines: &[&str]) {
        let session_dir = home.join(".pi/agent/sessions").join(dir);
        fs::create_dir_all(&session_dir).unwrap();
        let mut file = fs::File::create(session_dir.join(filename)).unwrap();
        for line in lines {
            writeln!(file, "{line}").unwrap();
        }
    }

    #[test]
    fn scan_keeps_multiple_sessions_from_same_project() {
        let _guard = home_lock().lock().unwrap();
        let old_home = std::env::var_os("HOME");
        let home = temp_home("same-project");
        std::env::set_var("HOME", &home);

        write_pi_session(
            &home,
            "--tmp-project--",
            "old.jsonl",
            "old-session",
            "2026-05-01T00:00:00Z",
            "/tmp/project",
        );
        write_pi_session(
            &home,
            "--tmp-project--",
            "new.jsonl",
            "new-session",
            "2026-05-02T00:00:00Z",
            "/tmp/project",
        );

        let sessions = scan().unwrap();

        if let Some(old_home) = old_home {
            std::env::set_var("HOME", old_home);
        } else {
            std::env::remove_var("HOME");
        }

        let ids: Vec<_> = sessions.iter().map(|s| s.session_id.as_str()).collect();
        assert_eq!(ids, vec!["new-session", "old-session"]);
    }

    #[test]
    fn scan_extracts_first_user_message_as_summary() {
        let _guard = home_lock().lock().unwrap();
        let old_home = std::env::var_os("HOME");
        let home = temp_home("user-summary");
        std::env::set_var("HOME", &home);

        write_pi_session_lines(
            &home,
            "--tmp-project--",
            "session.jsonl",
            &[
                r#"{"type":"session","id":"session-with-summary","timestamp":"2026-05-01T00:00:00Z","cwd":"/tmp/project"}"#,
                r#"{"type":"model_change","id":"model"}"#,
                r#"{"type":"message","message":{"role":"bashExecution","content":[{"type":"text","text":"cargo test"}]}}"#,
                r#"{"type":"message","message":{"role":"user","content":[{"type":"text","text":"发布pip版本\n后续细节不用进入列表标题"}]}}"#,
            ],
        );

        let sessions = scan().unwrap();

        if let Some(old_home) = old_home {
            std::env::set_var("HOME", old_home);
        } else {
            std::env::remove_var("HOME");
        }

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].summaries, vec!["发布pip版本"]);
    }
}
