use serde::Deserialize;
use serde_json::Value;
use std::fs::File;
use std::io::{BufRead, BufReader};
use walkdir::WalkDir;

use crate::error::AgfError;
use crate::model::{Agent, Session};
use crate::scanner::{first_line_truncated, project_name_from_path};

const SUMMARY_MAX_CHARS: usize = 120;

/// Cap on bytes parsed per pi session file. Pi transcripts are usually small,
/// but a long-running session can grow to several MB. Reading every byte on a
/// cold scan — which the v5 `CACHE_VERSION` bump forces once on upgrade —
/// repeats the v0.10.1 heavy-log stall that `read_head_tail` was added to
/// prevent for Claude. The header (id/cwd/timestamp) is the first line so it
/// is always read; this only bounds how many prompt summaries are collected
/// from pathologically large sessions.
const MAX_PARSE_BYTES: usize = 512 * 1024;

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
    let mut summaries = Vec::new();
    let mut bytes_read = 0usize;

    for line_result in reader.lines() {
        // A single bad line (invalid UTF-8, transient IO error) must skip just
        // that line — never abort the rest of the file. `map_while(Result::ok)`
        // stops at the first `Err`, which silently truncates summaries (or
        // drops the entire session if the bad line is line 1, since the header
        // is not yet captured). Same bug class as the cursor `.ok()?` fix in
        // v0.11.3 (`extract_first_prompt_skips_invalid_utf8_lines`).
        let Ok(line) = line_result else {
            continue;
        };
        // +1 approximates the newline stripped by `lines()`; we only need a
        // rough byte budget, not an exact count.
        bytes_read += line.len() + 1;

        let line = line.trim();
        if !line.is_empty()
            && let Ok(value) = serde_json::from_str::<Value>(line)
        {
            if header.is_none() && value.get("type").and_then(Value::as_str) == Some("session") {
                header = serde_json::from_value::<PiSessionHeader>(value.clone()).ok();
            }

            if let Some(summary) = extract_user_summary(&value) {
                summaries.push(summary);
            }
        }

        if bytes_read >= MAX_PARSE_BYTES {
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

    let project_name = project_name_from_path(&cwd);

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

    /// Write a standalone pi session JSONL to a unique temp path and return it.
    /// Tests that exercise `parse_session` directly don't need to redirect
    /// `$HOME`, so they stay portable — `dirs::home_dir()` only honors a `HOME`
    /// override on Unix (Windows resolves via the Known Folder API).
    fn temp_session_file(name: &str, lines: &[&str]) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "agf-pi-{name}-{}-{}.jsonl",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let mut file = fs::File::create(&path).unwrap();
        for line in lines {
            writeln!(file, "{line}").unwrap();
        }
        path
    }

    #[test]
    fn parse_session_extracts_user_messages_as_summaries() {
        let path = temp_session_file(
            "user-summary",
            &[
                r#"{"type":"session","id":"session-with-summary","timestamp":"2026-05-01T00:00:00Z","cwd":"/tmp/project"}"#,
                r#"{"type":"model_change","id":"model"}"#,
                r#"{"type":"message","message":{"role":"bashExecution","content":[{"type":"text","text":"cargo test"}]}}"#,
                r#"{"type":"message","message":{"role":"user","content":[{"type":"text","text":"发布pip版本\n后续细节不用进入列表标题"}]}}"#,
                r#"{"type":"message","message":{"role":"assistant","content":[{"type":"text","text":"done"}]}}"#,
                r#"{"type":"message","message":{"role":"user","content":[{"type":"text","text":"再发一个版本"}]}}"#,
            ],
        );

        let session = parse_session(&path);
        let _ = fs::remove_file(&path);

        let session = session.expect("session header should parse");
        assert_eq!(session.session_id, "session-with-summary");
        assert_eq!(session.summaries, vec!["发布pip版本", "再发一个版本"]);
    }

    #[test]
    fn parse_session_stops_at_byte_budget() {
        // Header on line 1, then user messages whose raw bytes far exceed
        // MAX_PARSE_BYTES so the read loop has to break early.
        let big_text = "x".repeat(2000); // ~2 KiB per line
        let user_line = format!(
            r#"{{"type":"message","message":{{"role":"user","content":[{{"type":"text","text":"{big_text}"}}]}}}}"#
        );
        let mut lines = vec![
            r#"{"type":"session","id":"big","timestamp":"2026-05-01T00:00:00Z","cwd":"/tmp/project"}"#
                .to_string(),
        ];
        for _ in 0..400 {
            lines.push(user_line.clone()); // ~800 KiB total
        }
        let line_refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let path = temp_session_file("byte-budget", &line_refs);

        let session = parse_session(&path);
        let _ = fs::remove_file(&path);

        let session = session.expect("header on line 1 should parse within the budget");
        assert_eq!(session.session_id, "big");
        // The 512 KiB budget stops collection before all 400 prompts are read.
        assert!(
            session.summaries.len() < 400,
            "expected the byte budget to cap summaries, got {}",
            session.summaries.len()
        );
    }

    /// Regression: a single line with invalid UTF-8 bytes used to abort the
    /// entire per-line loop via `reader.lines().map_while(Result::ok)`. If the
    /// bad line preceded the session header, the whole session was silently
    /// dropped. The fix skips just the bad line and keeps iterating, so the
    /// header on line 2 is captured and the user prompt on line 3 still
    /// surfaces. Same bug class as the cursor `.ok()?` fix in v0.11.3.
    #[test]
    fn parse_session_skips_invalid_utf8_lines() {
        let pid = std::process::id();
        let ts = chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default();
        let path = std::env::temp_dir().join(format!("agf-pi-badutf8-{pid}-{ts}.jsonl"));
        let _ = fs::remove_file(&path);

        let mut bytes: Vec<u8> = Vec::new();
        // Line 1: a truncated multibyte sequence — `BufReader::lines()` returns
        // `Err(InvalidData)` for this row.
        bytes.extend_from_slice(&[0xC3, 0x28]);
        bytes.push(b'\n');
        // Line 2: valid session header.
        bytes.extend_from_slice(
            br#"{"type":"session","id":"recovered","timestamp":"2026-05-29T00:00:00Z","cwd":"/tmp/project"}"#,
        );
        bytes.push(b'\n');
        // Line 3: valid user message.
        bytes.extend_from_slice(
            br#"{"type":"message","message":{"role":"user","content":[{"type":"text","text":"after garbage"}]}}"#,
        );
        fs::write(&path, &bytes).unwrap();

        let session = parse_session(&path);
        let _ = fs::remove_file(&path);

        let session = session.expect("session should surface despite invalid UTF-8 on line 1");
        assert_eq!(session.session_id, "recovered");
        assert_eq!(session.summaries, vec!["after garbage"]);
    }

    /// `scan()` walks `$HOME/.pi/agent/sessions`; redirecting the home dir via
    /// the `HOME` env var only works on Unix, so the dir-walk + multi-session
    /// (dedup-removal) coverage is gated to Unix.
    #[cfg(unix)]
    #[test]
    fn scan_keeps_multiple_sessions_from_same_project() {
        use std::sync::{Mutex, OnceLock};
        static HOME_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        let _guard = HOME_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();

        let old_home = std::env::var_os("HOME");
        let home = std::env::temp_dir().join(format!(
            "agf-pi-scan-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let session_dir = home.join(".pi/agent/sessions/--tmp-project--");
        fs::create_dir_all(&session_dir).unwrap();
        for (file, id, ts) in [
            ("old.jsonl", "old-session", "2026-05-01T00:00:00Z"),
            ("new.jsonl", "new-session", "2026-05-02T00:00:00Z"),
        ] {
            let mut f = fs::File::create(session_dir.join(file)).unwrap();
            writeln!(
                f,
                r#"{{"type":"session","id":"{id}","timestamp":"{ts}","cwd":"/tmp/project"}}"#
            )
            .unwrap();
        }
        // Serialized by the HOME_LOCK guard above.
        unsafe { std::env::set_var("HOME", &home) };

        let sessions = scan().unwrap();

        if let Some(old_home) = old_home {
            // Serialized by the HOME_LOCK guard above.
            unsafe { std::env::set_var("HOME", old_home) };
        } else {
            // Serialized by the HOME_LOCK guard above.
            unsafe { std::env::remove_var("HOME") };
        }

        let ids: Vec<_> = sessions.iter().map(|s| s.session_id.as_str()).collect();
        assert_eq!(ids, vec!["new-session", "old-session"]);
    }
}
