use std::path::Path;

use serde_json::Value;

use crate::error::AgfError;
use crate::model::{Agent, Session};
use crate::scanner::{
    collapse_whitespace, for_each_bounded_line_with_overflow, project_name_from_path, truncate,
};

const MAX_LINE_BYTES: usize = 2 * 1024 * 1024;

pub fn scan() -> Result<Vec<Session>, AgfError> {
    scan_from(&crate::config::prime_sessions_dir()?)
}

fn scan_from(dir: &Path) -> Result<Vec<Session>, AgfError> {
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut sessions = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if !entry.file_type()?.is_file()
            || path.extension().and_then(|ext| ext.to_str()) != Some("jsonl")
        {
            continue;
        }
        if let Some(session) = parse_session(&path)? {
            sessions.push(session);
        }
    }
    sessions.sort_by(|a, b| crate::model::compare_sessions(a, b, crate::model::SortMode::Time));
    Ok(sessions)
}

fn parse_session(path: &Path) -> Result<Option<Session>, AgfError> {
    let metadata = path.metadata()?;
    let mtime = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::SystemTime::UNIX_EPOCH).ok())
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0);
    let mut session_id = None;
    let mut cwd = None;
    let mut header_timestamp = 0;
    let mut last_activity = 0;
    let mut summaries = Vec::new();
    let mut name = None;
    let mut git_branch = None;
    let mut message_count = 0u64;
    let mut status = None;
    let mut first_valid_seen = false;
    let mut invalid_header = false;

    // Prime's durable activity/name/status can be appended arbitrarily late in
    // a long transcript. Stream to EOF while retaining the per-line allocation
    // ceiling; a total-byte cap would silently sort active long sessions as old.
    let read_ok = for_each_bounded_line_with_overflow(
        path,
        usize::MAX,
        MAX_LINE_BYTES,
        |line, _, oversized| {
            if invalid_header {
                return;
            }
            if oversized {
                let entry_type = json_string_from_prefix(line, "type");
                if !first_valid_seen {
                    first_valid_seen = true;
                    if entry_type.as_deref() != Some("session") {
                        invalid_header = true;
                        return;
                    }
                }
                if entry_type.as_deref() == Some("message") {
                    message_count = message_count.saturating_add(1);
                    let role = json_string_from_prefix(line, "role");
                    if matches!(role.as_deref(), Some("user" | "assistant"))
                        && let Some(timestamp) = json_string_from_prefix(line, "timestamp")
                            .and_then(|timestamp| {
                                chrono::DateTime::parse_from_rfc3339(&timestamp).ok()
                            })
                            .map(|timestamp| timestamp.timestamp_millis())
                    {
                        last_activity = last_activity.max(timestamp);
                    }
                    if role.as_deref() == Some("user") && summaries.len() < 10 {
                        let preview = json_string_from_prefix(line, "text")
                            .or_else(|| json_string_from_prefix(line, "content"));
                        let preview = preview
                            .map(|preview| truncate(&collapse_whitespace(&preview), 200))
                            .filter(|preview| !preview.is_empty())
                            .unwrap_or_else(|| "(large message)".to_string());
                        summaries.push(preview);
                    }
                }
                return;
            }
            let Ok(value) = serde_json::from_slice::<Value>(line) else {
                return;
            };
            let entry_type = value.get("type").and_then(Value::as_str);
            let is_first = !first_valid_seen;
            if is_first {
                first_valid_seen = true;
                if entry_type != Some("session") {
                    invalid_header = true;
                    return;
                }
            }
            match entry_type {
                Some("session") if is_first => {
                    let id = value.get("id").and_then(Value::as_str).map(str::trim);
                    let project = value.get("cwd").and_then(Value::as_str).map(str::trim);
                    if let (Some(id), Some(project)) = (id, project)
                        && !id.is_empty()
                        && !project.is_empty()
                    {
                        session_id = Some(id.to_string());
                        cwd = Some(project.to_string());
                        header_timestamp = iso_timestamp(&value);
                        git_branch = value
                            .pointer("/git/branch")
                            .and_then(Value::as_str)
                            .map(str::to_string);
                    }
                }
                Some("message") => {
                    message_count = message_count.saturating_add(1);
                    let Some(message) = value.get("message") else {
                        return;
                    };
                    let role = message.get("role").and_then(Value::as_str);
                    if matches!(role, Some("user" | "assistant")) {
                        let message_time = message
                            .get("timestamp")
                            .and_then(Value::as_i64)
                            .unwrap_or_else(|| iso_timestamp(&value));
                        last_activity = last_activity.max(message_time);
                    }
                    if role == Some("user")
                        && summaries.len() < 10
                        && let Some(text) = content_text(message.get("content"))
                    {
                        summaries.push(truncate(&collapse_whitespace(&text), 200));
                    }
                }
                Some("session_info") => {
                    name = value
                        .get("name")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|name| !name.is_empty())
                        .map(str::to_string);
                }
                Some("git_state") => {
                    if let Some(branch) = value.pointer("/git/branch").and_then(Value::as_str) {
                        git_branch = Some(branch.to_string());
                    }
                }
                Some("agent_status") => {
                    let basis = value
                        .pointer("/status/basedOnMessageCount")
                        .and_then(Value::as_u64);
                    let summary = value
                        .pointer("/status/summary")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|summary| !summary.is_empty())
                        .map(str::to_string);
                    if let (Some(basis), Some(summary)) = (basis, summary) {
                        status = Some((basis, summary));
                    }
                }
                _ => {}
            }
        },
    );
    if !read_ok {
        return Err(std::io::Error::other(format!(
            "failed to read Prime Agent session {}",
            path.display()
        ))
        .into());
    }

    let (Some(session_id), Some(project_path)) = (session_id, cwd) else {
        return Ok(None);
    };
    if let Some(name) = name {
        summaries.insert(0, name);
    }
    let recap = status
        .filter(|(basis, _)| *basis == message_count)
        .map(|(_, summary)| format!("recap: {summary}"));

    Ok(Some(Session {
        agent: Agent::PrimeAgent,
        session_id,
        project_name: project_name_from_path(&project_path),
        project_path,
        summaries,
        timestamp: if last_activity != 0 {
            last_activity
        } else if header_timestamp != 0 {
            header_timestamp
        } else {
            mtime
        },
        git_branch,
        worktree: None,
        recap,
        interactive: true,
    }))
}

fn json_string_from_prefix(prefix: &[u8], field: &str) -> Option<String> {
    let text = match std::str::from_utf8(prefix) {
        Ok(text) => text,
        // Oversized rows can end the retained prefix mid-code-point. Do not
        // recover across an actual malformed sequence anywhere in the prefix.
        Err(error) if error.error_len().is_none() => {
            std::str::from_utf8(&prefix[..error.valid_up_to()]).ok()?
        }
        Err(_) => return None,
    };
    let key = format!("\"{field}\"");
    let mut search_from = 0usize;
    while let Some(relative) = text.get(search_from..)?.find(&key) {
        let after_key = search_from + relative + key.len();
        let mut rest = text.get(after_key..)?.trim_start();
        if !rest.starts_with(':') {
            search_from = after_key;
            continue;
        }
        rest = rest.get(1..)?.trim_start();
        let encoded = rest.strip_prefix('"')?;
        let mut escaped = false;
        for (index, ch) in encoded.char_indices() {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                return serde_json::from_str::<String>(&rest[..=index + 1]).ok();
            }
        }
        return None;
    }
    None
}

fn iso_timestamp(value: &Value) -> i64 {
    value
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(|timestamp| chrono::DateTime::parse_from_rfc3339(timestamp).ok())
        .map(|timestamp| timestamp.timestamp_millis())
        .unwrap_or(0)
}

fn content_text(content: Option<&Value>) -> Option<String> {
    match content? {
        Value::String(text) => Some(text.clone()),
        Value::Array(blocks) => {
            let mut output = String::new();
            for text in blocks.iter().filter_map(|block| {
                (block.get("type").and_then(Value::as_str) == Some("text"))
                    .then(|| block.get("text").and_then(Value::as_str))
                    .flatten()
            }) {
                if !output.is_empty() {
                    output.push(' ');
                }
                output.push_str(text);
            }
            (!output.is_empty()).then_some(output)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;

    fn fixture(name: &str, lines: &[&str]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("agf-prime-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("different-file-stem.jsonl");
        let mut file = File::create(&path).unwrap();
        for line in lines {
            writeln!(file, "{line}").unwrap();
        }
        path
    }

    #[test]
    fn parses_v3_activity_name_git_and_current_recap() {
        let path = fixture(
            "v3",
            &[
                r#"{"type":"session","version":3,"id":"prime-id","timestamp":"2026-08-01T00:00:00Z","cwd":"/tmp/project","git":{"branch":"old"}}"#,
                r#"{"type":"message","id":"a","parentId":null,"timestamp":"2026-08-01T00:00:01Z","message":{"role":"user","content":"first prompt","timestamp":1785542401000}}"#,
                r#"{"type":"message","id":"b","parentId":"a","timestamp":"2026-08-01T00:00:02Z","message":{"role":"assistant","content":[{"type":"text","text":"ok"}],"timestamp":1785542402000}}"#,
                r#"{"type":"session_info","id":"c","parentId":"b","timestamp":"2026-08-01T00:00:03Z","name":"Named session"}"#,
                r#"{"type":"git_state","id":"d","parentId":"c","timestamp":"2026-08-01T00:00:04Z","git":{"branch":"feature"}}"#,
                r#"{"type":"agent_status","id":"e","parentId":"d","timestamp":"2026-08-01T00:00:05Z","status":{"summary":"finished","basedOnMessageCount":2}}"#,
            ],
        );

        let session = parse_session(&path).unwrap().unwrap();
        assert_eq!(session.session_id, "prime-id");
        assert_eq!(session.summaries, ["Named session", "first prompt"]);
        assert_eq!(session.git_branch.as_deref(), Some("feature"));
        assert_eq!(session.recap.as_deref(), Some("recap: finished"));
        assert_eq!(session.timestamp, 1_785_542_402_000);
    }

    #[test]
    fn bookkeeping_does_not_bump_activity_and_stale_recap_is_hidden() {
        let path = fixture(
            "activity",
            &[
                r#"{"type":"session","version":3,"id":"prime-id","timestamp":"2026-08-01T00:00:00Z","cwd":"/tmp/project"}"#,
                r#"{"type":"message","id":"a","parentId":null,"timestamp":"2026-08-01T00:00:01Z","message":{"role":"user","content":"prompt","timestamp":1785542401000}}"#,
                r#"{"type":"git_state","id":"b","parentId":"a","timestamp":"2026-08-15T00:00:00Z","git":{"branch":"main"}}"#,
                r#"{"type":"agent_status","id":"c","parentId":"b","timestamp":"2026-08-15T00:00:01Z","status":{"summary":"stale","basedOnMessageCount":0}}"#,
            ],
        );

        let session = parse_session(&path).unwrap().unwrap();
        assert_eq!(session.timestamp, 1_785_542_401_000);
        assert_eq!(session.recap, None);
    }

    #[test]
    fn oversized_message_preserves_count_role_and_activity_from_prefix() {
        let huge = "x".repeat(MAX_LINE_BYTES + 1024);
        let message = format!(
            r#"{{"type":"message","timestamp":"2026-08-01T00:00:05Z","message":{{"role":"user","content":"{huge}"}}}}"#
        );
        let path = fixture(
            "oversized",
            &[
                r#"{"type":"session","version":3,"id":"prime-big","timestamp":"2026-08-01T00:00:00Z","cwd":"/tmp/project"}"#,
                &message,
                r#"{"type":"agent_status","status":{"basedOnMessageCount":1,"summary":"current"}}"#,
            ],
        );

        let session = parse_session(&path).unwrap().unwrap();
        assert_eq!(session.timestamp, 1_785_542_405_000);
        assert_eq!(session.recap.as_deref(), Some("recap: current"));
        assert_eq!(session.summaries, ["(large message)"]);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn rejects_a_late_header_after_a_valid_non_session_record() {
        let path = fixture(
            "late-header",
            &[
                "not json",
                r#"{"type":"message","timestamp":"2026-08-01T00:00:01Z","message":{"role":"user","content":"prompt"}}"#,
                r#"{"type":"session","version":3,"id":"prime-late","timestamp":"2026-08-01T00:00:00Z","cwd":"/tmp/project"}"#,
            ],
        );

        assert!(parse_session(&path).unwrap().is_none());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn oversized_cjk_message_updates_activity_invalidates_recap_and_keeps_sibling() {
        for partial_bytes in 1..=2 {
            let prefix = r#"{"type":"message","timestamp":"2026-08-01T00:00:05Z","message":{"role":"user","content":""#;
            let padding = (MAX_LINE_BYTES - prefix.len() - partial_bytes) % 3;
            let message = format!(
                "{prefix}{}{}\"}}}}",
                "x".repeat(padding),
                "\u{754c}".repeat(MAX_LINE_BYTES / 3 + 1024)
            );
            let error = std::str::from_utf8(&message.as_bytes()[..MAX_LINE_BYTES]).unwrap_err();
            assert_eq!(error.error_len(), None);
            assert_eq!(MAX_LINE_BYTES - error.valid_up_to(), partial_bytes);
            let path = fixture(
                &format!("cjk-{partial_bytes}"),
                &[
                    r#"{"type":"session","id":"prime-cjk","timestamp":"2026-08-01T00:00:00Z","cwd":"/tmp/project"}"#,
                    r#"{"type":"agent_status","status":{"basedOnMessageCount":0,"summary":"stale"}}"#,
                    &message,
                    r#"{"type":"session_info","name":"Latest name"}"#,
                ],
            );
            let root = path.parent().unwrap();
            std::fs::write(
                root.join("sibling.jsonl"),
                r#"{"type":"session","id":"prime-sibling","cwd":"/tmp/sibling"}"#,
            )
            .unwrap();

            let sessions = scan_from(root).unwrap();
            assert_eq!(sessions.len(), 2);
            let session = sessions
                .iter()
                .find(|s| s.session_id == "prime-cjk")
                .unwrap();
            assert_eq!(session.timestamp, 1_785_542_405_000);
            assert_eq!(session.recap, None);
            assert_eq!(session.summaries, ["Latest name", "(large message)"]);
            assert!(sessions.iter().any(|s| s.session_id == "prime-sibling"));
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn prefix_recovery_rejects_malformed_utf8_before_or_at_the_boundary() {
        for suffix in [&b"\xff"[..], &b"\xe7x"[..], &b"\x80"[..]] {
            let mut prefix = br#"{"type":"message","content":""#.to_vec();
            prefix.extend_from_slice(suffix);
            assert_eq!(json_string_from_prefix(&prefix, "type"), None);
        }
    }
}
