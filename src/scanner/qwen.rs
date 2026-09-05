use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

use serde_json::Value;

use crate::error::AgfError;
use crate::model::{Agent, Session};

use super::{collapse_whitespace, project_name_from_path, read_head_tail, truncate};

const MAX_HEAD_BYTES: u64 = 2 * 1024 * 1024;
const MAX_HEAD_RECORDS: usize = 10;
const TITLE_WINDOW_BYTES: u64 = 64 * 1024;

pub fn scan() -> Result<Vec<Session>, AgfError> {
    let root = crate::config::qwen_runtime_dir()?;
    scan_from_roots(&[root.join("projects"), root.join("tmp")])
}

fn scan_from_roots(roots: &[std::path::PathBuf]) -> Result<Vec<Session>, AgfError> {
    let mut by_id: HashMap<String, Session> = HashMap::new();
    for root in roots {
        if !root.exists() {
            continue;
        }
        for project_entry in std::fs::read_dir(root)? {
            let project_entry = project_entry?;
            if !project_entry.file_type()?.is_dir() {
                continue;
            }
            let chats = project_entry.path().join("chats");
            if !chats.is_dir() {
                continue;
            }
            for entry in std::fs::read_dir(chats)? {
                let entry = entry?;
                let path = entry.path();
                if !entry.file_type()?.is_file()
                    || path.extension().and_then(|ext| ext.to_str()) != Some("jsonl")
                {
                    continue;
                }
                let Some(stem) = path.file_stem().and_then(|name| name.to_str()) else {
                    continue;
                };
                if !is_session_id(stem) {
                    continue;
                }
                if let Some(session) = parse_session(&path)? {
                    match by_id.get(&session.session_id) {
                        Some(existing) if existing.timestamp >= session.timestamp => {}
                        _ => {
                            by_id.insert(session.session_id.clone(), session);
                        }
                    }
                }
            }
        }
    }
    let mut sessions: Vec<_> = by_id.into_values().collect();
    sessions.sort_by(|a, b| crate::model::compare_sessions(a, b, crate::model::SortMode::Time));
    Ok(sessions)
}

fn parse_session(path: &Path) -> Result<Option<Session>, AgfError> {
    let Some(file_id) = path
        .file_stem()
        .and_then(|name| name.to_str())
        .map(str::to_string)
    else {
        return Ok(None);
    };
    let Some(head) = read_head(path)? else {
        return Ok(None);
    };
    if head.session_id != file_id || head.cwd.is_empty() {
        return Ok(None);
    }

    let metadata = path.metadata()?;
    let file_time = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0);
    let title = latest_title(path)?;
    let mut summaries = Vec::new();
    for text in [title, head.prompt] {
        let Some(text) = text
            .map(|text| collapse_whitespace(&text))
            .filter(|text| !text.is_empty())
            .map(|text| truncate(&text, 200))
        else {
            continue;
        };
        if !summaries.contains(&text) {
            summaries.push(text);
        }
    }
    let timestamp = if file_time != 0 {
        file_time
    } else {
        parse_iso(&head.start_time).unwrap_or(0)
    };
    let interactive = !matches!(
        head.source_type.as_deref(),
        Some("agent" | "subagent" | "background_agent")
    );

    Ok(Some(Session {
        agent: Agent::Qwen,
        session_id: file_id,
        project_name: project_name_from_path(&head.cwd),
        project_path: head.cwd,
        summaries,
        timestamp,
        git_branch: head.git_branch,
        worktree: None,
        recap: None,
        interactive,
    }))
}

#[derive(Default)]
struct HeadInfo {
    session_id: String,
    cwd: String,
    start_time: String,
    git_branch: Option<String>,
    prompt: Option<String>,
    source_type: Option<String>,
}

fn read_head(path: &Path) -> Result<Option<HeadInfo>, AgfError> {
    let file = std::fs::File::open(path)?;
    let exceeds_budget = file.metadata()?.len() > MAX_HEAD_BYTES;
    let mut reader = BufReader::new(file).take(MAX_HEAD_BYTES);
    let mut line_bytes = Vec::new();
    let mut info = HeadInfo::default();
    let mut records_seen = 0usize;
    let mut first_record_seen = false;

    while records_seen < MAX_HEAD_RECORDS {
        line_bytes.clear();
        let bytes = reader.read_until(b'\n', &mut line_bytes)?;
        if bytes == 0 {
            break;
        }
        let line = match std::str::from_utf8(&line_bytes) {
            Ok(line) => line,
            // Only an incomplete code point at the artificial read boundary
            // may be dropped. Malformed input and incomplete real EOF fail.
            Err(error) if exceeds_budget && reader.limit() == 0 && error.error_len().is_none() => {
                std::str::from_utf8(&line_bytes[..error.valid_up_to()])
                    .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?
            }
            Err(error) => {
                return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, error).into());
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        records_seen += 1;
        let parsed = serde_json::from_str::<Value>(line).ok();
        if !first_record_seen {
            first_record_seen = true;
            if let Some(record) = parsed.as_ref() {
                info.session_id = string_at(record, "/sessionId").unwrap_or_default();
                info.cwd = string_at(record, "/cwd").unwrap_or_default();
                info.start_time = string_at(record, "/timestamp").unwrap_or_default();
                info.git_branch = string_at(record, "/gitBranch");
            } else {
                // A pasted multi-megabyte prompt may make the first JSONL row
                // exceed our allocation budget. Qwen writes the resumable
                // envelope before message content, so recover those bounded
                // prefix fields without retaining the payload.
                info.session_id = json_string_from_prefix(line, "sessionId").unwrap_or_default();
                info.cwd = json_string_from_prefix(line, "cwd").unwrap_or_default();
                info.start_time = json_string_from_prefix(line, "timestamp").unwrap_or_default();
                info.git_branch = json_string_from_prefix(line, "gitBranch");
                if info.session_id.is_empty() || info.cwd.is_empty() {
                    return Ok(None);
                }
                info.prompt = Some("(large prompt)".to_string());
            }
        }
        let Some(record) = parsed.as_ref() else {
            continue;
        };
        if info.prompt.is_none()
            && record.get("type").and_then(Value::as_str) == Some("user")
            && record.get("subtype").is_none()
        {
            info.prompt = string_at(record, "/systemPayload/displayText").or_else(|| {
                record
                    .pointer("/message/parts")
                    .and_then(Value::as_array)
                    .and_then(|parts| {
                        parts.iter().find_map(|part| {
                            part.get("text").and_then(Value::as_str).map(str::to_string)
                        })
                    })
            });
        }
        if record.get("type").and_then(Value::as_str) == Some("system")
            && record.get("subtype").and_then(Value::as_str) == Some("session_source")
        {
            info.source_type = string_at(record, "/systemPayload/sourceType");
        }
    }

    if !first_record_seen || info.session_id.is_empty() || info.cwd.is_empty() {
        Ok(None)
    } else {
        Ok(Some(info))
    }
}

fn latest_title(path: &Path) -> Result<Option<String>, AgfError> {
    let windows =
        read_head_tail(path, TITLE_WINDOW_BYTES, TITLE_WINDOW_BYTES).ok_or_else(|| {
            std::io::Error::other(format!("failed to read Qwen session {}", path.display()))
        })?;
    let mut title = None;
    for line in windows.head.lines().chain(windows.tail.lines()) {
        let Ok(record) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if record.get("type").and_then(Value::as_str) == Some("system")
            && record.get("subtype").and_then(Value::as_str) == Some("custom_title")
            && let Some(current) = string_at(&record, "/systemPayload/customTitle")
            && !current.trim().is_empty()
        {
            title = Some(current);
        }
    }
    Ok(title)
}

fn string_at(value: &Value, pointer: &str) -> Option<String> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

fn parse_iso(timestamp: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(timestamp)
        .ok()
        .map(|timestamp| timestamp.timestamp_millis())
}

fn is_session_id(id: &str) -> bool {
    (32..=36).contains(&id.len())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
}

fn json_string_from_prefix(prefix: &str, field: &str) -> Option<String> {
    let key = format!("\"{field}\"");
    let mut search_from = 0usize;
    while let Some(relative) = prefix.get(search_from..)?.find(&key) {
        let after_key = search_from + relative + key.len();
        let mut rest = prefix.get(after_key..)?.trim_start();
        if !rest.starts_with(':') {
            search_from = after_key;
            continue;
        }
        rest = rest.get(1..)?.trim_start();
        let encoded = rest.strip_prefix('"')?;
        let mut escaped = false;
        for (index, character) in encoded.char_indices() {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                return serde_json::from_str::<String>(&rest[..=index + 1]).ok();
            }
        }
        return None;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    #[test]
    fn parses_official_chat_records_and_latest_custom_title() {
        let root =
            std::env::temp_dir().join(format!("agf-qwen-scan-normal-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let chats = root.join("projects/project-id/chats");
        std::fs::create_dir_all(&chats).unwrap();
        let id = "019d1234-1234-7234-8234-123456789abc";
        let path = chats.join(format!("{id}.jsonl"));
        std::fs::write(
            &path,
            format!(
                "{{\"uuid\":\"u1\",\"parentUuid\":null,\"sessionId\":\"{id}\",\"timestamp\":\"2026-08-10T00:00:00Z\",\"type\":\"user\",\"cwd\":\"/work/qwen-project\",\"version\":\"1\",\"gitBranch\":\"feat/qwen\",\"message\":{{\"role\":\"user\",\"parts\":[{{\"text\":\"Implement the session picker\"}}]}}}}\n{{\"uuid\":\"u2\",\"parentUuid\":\"u1\",\"sessionId\":\"{id}\",\"timestamp\":\"2026-08-10T00:01:00Z\",\"type\":\"system\",\"subtype\":\"custom_title\",\"cwd\":\"/work/qwen-project\",\"version\":\"1\",\"systemPayload\":{{\"customTitle\":\"Qwen session support\",\"titleSource\":\"manual\"}}}}\n"
            ),
        )
        .unwrap();
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(SystemTime::UNIX_EPOCH + Duration::from_millis(1_786_000_123_000))
            .unwrap();

        let sessions = scan_from_roots(&[root.join("projects")]).unwrap();
        assert_eq!(sessions.len(), 1);
        let session = &sessions[0];
        assert_eq!(session.agent, Agent::Qwen);
        assert_eq!(session.project_name, "qwen-project");
        assert_eq!(session.summaries[0], "Qwen session support");
        assert_eq!(session.summaries[1], "Implement the session picker");
        assert_eq!(session.git_branch.as_deref(), Some("feat/qwen"));
        assert_eq!(session.timestamp, 1_786_000_123_000);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn oversized_first_prompt_keeps_resume_envelope_with_bounded_memory() {
        let root = std::env::temp_dir().join(format!("agf-qwen-scan-large-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let chats = root.join("projects/project-id/chats");
        std::fs::create_dir_all(&chats).unwrap();
        let id = "019d1234-1234-7234-8234-123456789abd";
        let path = chats.join(format!("{id}.jsonl"));
        let large = "x".repeat(MAX_HEAD_BYTES as usize + 1024);
        std::fs::write(
            &path,
            format!(
                "{{\"uuid\":\"u1\",\"parentUuid\":null,\"sessionId\":\"{id}\",\"timestamp\":\"2026-08-10T00:00:00Z\",\"type\":\"user\",\"cwd\":\"/work/qwen-large\",\"version\":\"1\",\"message\":{{\"role\":\"user\",\"parts\":[{{\"text\":\"{large}\"}}]}}}}\n"
            ),
        )
        .unwrap();

        let sessions = scan_from_roots(&[root.join("projects")]).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, id);
        assert_eq!(sessions[0].project_path, "/work/qwen-large");
        assert_eq!(sessions[0].summaries, ["(large prompt)"]);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn oversized_cjk_header_preserves_session_and_sibling_at_each_byte_boundary() {
        for partial_bytes in 1..=2 {
            let root = std::env::temp_dir().join(format!(
                "agf-qwen-cjk-{partial_bytes}-{}",
                std::process::id()
            ));
            let chats = root.join("project/chats");
            std::fs::create_dir_all(&chats).unwrap();
            let id = "019d1234-1234-7234-8234-123456789abc";
            let sibling = "019d1234-1234-7234-8234-123456789abd";
            let prefix = format!(
                r#"{{"sessionId":"{id}","cwd":"/work/cjk","gitBranch":"feature","type":"user","message":{{"parts":[{{"text":""#
            );
            let padding = (MAX_HEAD_BYTES as usize - prefix.len() - partial_bytes) % 3;
            let record = format!(
                "{prefix}{}{}\"}}]}}}}\n",
                "x".repeat(padding),
                "\u{754c}".repeat(MAX_HEAD_BYTES as usize / 3 + 1024)
            );
            let error =
                std::str::from_utf8(&record.as_bytes()[..MAX_HEAD_BYTES as usize]).unwrap_err();
            assert_eq!(error.error_len(), None);
            assert_eq!(MAX_HEAD_BYTES as usize - error.valid_up_to(), partial_bytes);
            std::fs::write(chats.join(format!("{id}.jsonl")), record).unwrap();
            std::fs::write(
                chats.join(format!("{sibling}.jsonl")),
                format!(r#"{{"sessionId":"{sibling}","cwd":"/work/sibling"}}"#),
            )
            .unwrap();

            let sessions = scan_from_roots(std::slice::from_ref(&root)).unwrap();
            assert_eq!(sessions.len(), 2);
            let session = sessions.iter().find(|s| s.session_id == id).unwrap();
            assert_eq!(session.project_path, "/work/cjk");
            assert_eq!(session.git_branch.as_deref(), Some("feature"));
            assert_eq!(session.summaries, ["(large prompt)"]);
            assert!(sessions.iter().any(|s| s.session_id == sibling));
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn malformed_utf8_and_incomplete_eof_are_not_header_boundary_recovery() {
        let root = std::env::temp_dir().join(format!("agf-qwen-invalid-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("019d1234-1234-7234-8234-123456789abc.jsonl");
        for suffix in [&b"\xff"[..], &b"\xe7\x95"[..]] {
            let mut record = br#"{"sessionId":"019d1234-1234-7234-8234-123456789abc","cwd":"/work/test","text":""#.to_vec();
            record.extend_from_slice(suffix);
            std::fs::write(&path, &record).unwrap();
            assert!(
                matches!(read_head(&path), Err(AgfError::Io(error)) if error.kind() == std::io::ErrorKind::InvalidData)
            );
        }
        let mut malformed =
            br#"{"sessionId":"019d1234-1234-7234-8234-123456789abc","cwd":"/work/test","text":""#
                .to_vec();
        malformed.push(0xff);
        malformed.resize(MAX_HEAD_BYTES as usize + 1, b'x');
        std::fs::write(&path, malformed).unwrap();
        assert!(
            matches!(read_head(&path), Err(AgfError::Io(error)) if error.kind() == std::io::ErrorKind::InvalidData)
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
