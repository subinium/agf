use std::collections::HashMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use serde::Deserializer as _;
use serde::de::{DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::error::AgfError;
use crate::model::{Agent, Session};

use super::{collapse_whitespace, truncate};

/// Maximum bytes to read from a legacy JSON session file.
/// Gemini session files can balloon to 28 MB+ when tool calls embed full file
/// contents. The JSON header (sessionId, timestamps) fits in the first few
/// hundred bytes; the first user message almost always lands in the first 64 KB.
const MAX_FILE_BYTES: usize = 64 * 1024;
const JSONL_HEAD_BYTES: usize = 128 * 1024;
const JSONL_TAIL_BYTES: usize = 64 * 1024;

pub fn scan() -> Result<Vec<Session>, AgfError> {
    let gemini_dir = crate::config::gemini_dir()?;
    scan_from(&gemini_dir)
}

fn scan_from(gemini_dir: &Path) -> Result<Vec<Session>, AgfError> {
    let tmp_dir = gemini_dir.join("tmp");

    if !tmp_dir.exists() {
        return Ok(Vec::new());
    }

    let path_map = build_path_map(gemini_dir)?;

    // Dedup by sessionId: keep the entry with the latest `lastUpdated`.
    // The same session can appear in both a hash dir (old) and a named dir
    // (new) when Gemini CLI migrates a project to projects.json.
    let mut by_id: HashMap<String, (bool, Session)> = HashMap::new();

    let entries = fs::read_dir(&tmp_dir)?;

    for entry in entries {
        let entry = entry?;
        let dir_name = entry.file_name().to_string_lossy().to_string();
        let chats_dir = entry.path().join("chats");

        if !chats_dir.is_dir() {
            continue;
        }

        let (project_path, project_name) = resolve_project(&dir_name, &path_map);

        if project_path.is_empty() {
            continue;
        }

        let chat_entries = fs::read_dir(&chats_dir)?;

        for chat_entry in chat_entries {
            let chat_entry = chat_entry?;
            let fname = chat_entry.file_name().to_string_lossy().to_string();
            if !chat_entry.file_type()?.is_file()
                || !fname.starts_with("session-")
                || !(fname.ends_with(".json") || fname.ends_with(".jsonl"))
            {
                continue;
            }

            if let Some(session) = parse_session(&chat_entry.path(), &project_path, &project_name)?
            {
                let current_format = fname.ends_with(".jsonl");
                if by_id
                    .get(&session.session_id)
                    .is_none_or(|(current, existing)| {
                        (session.timestamp, current_format) > (existing.timestamp, *current)
                    })
                {
                    by_id.insert(session.session_id.clone(), (current_format, session));
                }
            }
        }
    }

    Ok(by_id.into_values().map(|(_, session)| session).collect())
}

/// Build a map: dir_name → full project path.
///
/// Named dirs (e.g. "github") come directly from `projects.json` values.
/// Hash dirs (e.g. "e0dc5a91...") are matched by computing SHA256 of each known path.
fn build_path_map(gemini_dir: &Path) -> Result<HashMap<String, String>, AgfError> {
    let mut map = HashMap::new();

    let projects_file = gemini_dir.join("projects.json");
    if !projects_file.exists() {
        return Ok(map);
    }
    let content = fs::read_to_string(projects_file)?;
    let json = serde_json::from_str::<serde_json::Value>(&content)?;
    let Some(projects) = json.get("projects").and_then(|v| v.as_object()) else {
        return Ok(map);
    };

    for (path, name) in projects {
        if let Some(name_str) = name.as_str() {
            // Named dir → path
            map.insert(name_str.to_string(), path.clone());
            // Hash dir → path (SHA256 of the full path string)
            map.insert(sha256_hex(path.as_bytes()), path.clone());
        }
    }

    Ok(map)
}

/// Resolve a project dir name to (project_path, project_name).
fn resolve_project(dir_name: &str, path_map: &HashMap<String, String>) -> (String, String) {
    if let Some(full_path) = path_map.get(dir_name) {
        let name = Path::new(full_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(dir_name)
            .to_string();
        return (full_path.clone(), name);
    }

    // Unknown hash: no path available, use truncated hash as display name
    let short = format!("{}…", crate::scanner::char_prefix(dir_name, 8));
    (String::new(), short)
}

/// The filename contains only an ID prefix; the metadata owns the resume ID.
fn parse_session(
    path: &Path,
    project_path: &str,
    project_name: &str,
) -> Result<Option<Session>, AgfError> {
    let mut file = fs::File::open(path)?;
    let metadata = file.metadata()?;
    let fallback_time = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|time| i64::try_from(time.as_millis()).ok());
    let fields = if path.extension().is_some_and(|ext| ext == "jsonl") {
        read_jsonl(&mut file, metadata.len(), fallback_time)?
    } else {
        let mut bytes = Vec::new();
        file.take(MAX_FILE_BYTES as u64).read_to_end(&mut bytes)?;
        parse_legacy(&bytes)
    };
    let Some(fields) = fields else {
        return Ok(None);
    };
    let Some(session_id) = fields.session_id else {
        return Ok(None);
    };
    let Some(timestamp) = fields.activity.or(fallback_time) else {
        return Ok(None);
    };
    let mut summaries: Vec<String> = fields.summary.into_iter().collect();
    if let Some(prompt) = fields.prompt
        && !summaries.contains(&prompt)
    {
        summaries.push(prompt);
    }

    Ok(Some(Session {
        agent: Agent::Gemini,
        session_id,
        project_name: project_name.to_string(),
        project_path: project_path.to_string(),
        summaries,
        timestamp,
        git_branch: None,
        worktree: None,
        recap: None,
        interactive: !fields.subagent,
    }))
}

#[derive(Default)]
struct SessionFields {
    session_id: Option<String>,
    project_hash: Option<String>,
    activity: Option<i64>,
    summary: Option<String>,
    prompt: Option<String>,
    subagent: bool,
}

impl SessionFields {
    fn update_time(&mut self, value: &Value) {
        if let Some(time) = value.as_str().and_then(parse_iso8601_ms) {
            self.activity = Some(self.activity.map_or(time, |previous| previous.max(time)));
        }
    }

    fn metadata_field(&mut self, key: &str, value: &Value) {
        match key {
            "sessionId" | "projectHash" => {
                if let Some(text) = value
                    .as_str()
                    .filter(|s| !s.trim().is_empty() && !s.chars().any(char::is_control))
                {
                    let field = if key == "sessionId" {
                        &mut self.session_id
                    } else {
                        &mut self.project_hash
                    };
                    *field = Some(text.to_string());
                }
            }
            "startTime" | "lastUpdated" => self.update_time(value),
            "summary" => self.summary = value.as_str().and_then(summary_text),
            "kind" => self.subagent = value.as_str() == Some("subagent"),
            "messages" => {
                if let Some(messages) = value.as_array() {
                    for message in messages {
                        self.message(message);
                    }
                }
            }
            _ => {}
        }
    }

    fn message(&mut self, value: &Value) {
        let kind = value.get("type").and_then(Value::as_str);
        if !matches!(kind, Some("user" | "gemini" | "info" | "error" | "warning")) {
            return;
        }
        if let Some(timestamp) = value.get("timestamp") {
            self.update_time(timestamp);
        }
        if self.prompt.is_none() && kind == Some("user") {
            self.prompt = value.get("content").and_then(|content| {
                if let Some(text) = content.as_str() {
                    summary_text(text)
                } else {
                    let text: String = content
                        .as_array()?
                        .iter()
                        .filter_map(|part| part.get("text").and_then(Value::as_str))
                        .collect();
                    summary_text(&text)
                }
            });
        }
    }

    fn jsonl_record(&mut self, value: &Value) {
        if value.get("$rewindTo").and_then(Value::as_str).is_some() {
            return;
        }
        if value.get("id").and_then(Value::as_str).is_some() {
            self.message(value);
            return;
        }
        let metadata = if let Some(updates) = value.get("$set").and_then(Value::as_object) {
            if self.session_id.is_none() || self.project_hash.is_none() {
                return;
            }
            updates
        } else if value.get("sessionId").and_then(Value::as_str).is_some()
            && value.get("projectHash").and_then(Value::as_str).is_some()
        {
            let Some(metadata) = value.as_object() else {
                return;
            };
            metadata
        } else {
            return;
        };
        for (key, value) in metadata {
            self.metadata_field(key, value);
        }
    }
}

fn summary_text(text: &str) -> Option<String> {
    let text = collapse_whitespace(text);
    (!text.is_empty()).then(|| truncate(&text, 100))
}

/// v0.58.0 writes initial metadata, message records, and `$set` patches.
/// Read bounded head/tail windows so large tool records cannot hide recent activity.
fn read_jsonl(
    file: &mut fs::File,
    len: u64,
    fallback: Option<i64>,
) -> Result<Option<SessionFields>, AgfError> {
    let split = len > (JSONL_HEAD_BYTES + JSONL_TAIL_BYTES) as u64;
    let head_size = if split { JSONL_HEAD_BYTES as u64 } else { len };
    let mut head = Vec::new();
    file.take(head_size).read_to_end(&mut head)?;
    if split {
        head.truncate(
            head.iter()
                .rposition(|byte| *byte == b'\n')
                .map_or(0, |idx| idx + 1),
        );
    }
    let mut fields = SessionFields::default();
    read_records(&head, &mut fields);
    if fields.session_id.is_none() || fields.project_hash.is_none() {
        return Ok(None);
    }
    if split {
        file.seek(SeekFrom::Start(len - JSONL_TAIL_BYTES as u64 - 1))?;
        let mut tail = Vec::new();
        file.take(JSONL_TAIL_BYTES as u64 + 1)
            .read_to_end(&mut tail)?;
        let start = tail
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(tail.len(), |idx| idx + 1);
        let head_activity = fields.activity.take();
        read_records(&tail[start..], &mut fields);
        // If the tail only contains an oversized/unfinished record or a title
        // update, the omitted middle may hold the most recent timestamp.
        fields.activity = fields
            .activity
            .or(fallback)
            .into_iter()
            .chain(head_activity)
            .max();
    }
    Ok(Some(fields))
}

fn read_records(bytes: &[u8], fields: &mut SessionFields) {
    for line in bytes.split(|byte| *byte == b'\n') {
        if let Ok(value) = serde_json::from_slice::<Value>(line) {
            fields.jsonl_record(&value);
        }
    }
}

/// Preserve completed top-level fields at EOF without matching keys inside
/// prompts/tool payloads. Serde also handles whitespace and JSON escapes.
fn parse_legacy(bytes: &[u8]) -> Option<SessionFields> {
    let mut fields = SessionFields::default();
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    match deserializer.deserialize_map(LegacyFields(&mut fields)) {
        Ok(()) => deserializer.end().ok()?,
        Err(error) if error.is_eof() => {}
        Err(_) => return None,
    }
    Some(fields)
}

struct LegacyFields<'a>(&'a mut SessionFields);

impl<'de> Visitor<'de> for LegacyFields<'_> {
    type Value = ();

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Gemini session metadata")
    }

    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<(), M::Error> {
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "messages" => map.next_value_seed(LegacyMessages(self.0))?,
                "sessionId" | "projectHash" | "startTime" | "lastUpdated" | "summary" | "kind" => {
                    self.0.metadata_field(&key, &map.next_value::<Value>()?);
                }
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(())
    }
}

struct LegacyMessages<'a>(&'a mut SessionFields);

impl<'de> DeserializeSeed<'de> for LegacyMessages<'_> {
    type Value = ();

    fn deserialize<D: serde::Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        deserializer.deserialize_seq(self)
    }
}

impl<'de> Visitor<'de> for LegacyMessages<'_> {
    type Value = ();

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Gemini messages")
    }

    fn visit_seq<S: SeqAccess<'de>>(self, mut sequence: S) -> Result<(), S::Error> {
        while let Some(message) = sequence.next_element::<Value>()? {
            self.0.message(&message);
        }
        Ok(())
    }
}

/// Parse an ISO 8601 / RFC 3339 string to Unix milliseconds.
fn parse_iso8601_ms(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.timestamp_millis())
}

/// SHA256 hex digest.
fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_file(label: &str, extension: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "agf-gemini-{}-{label}.{extension}",
            std::process::id()
        ));
        fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn jsonl_metadata_updates_preserve_full_identity_and_latest_activity() {
        let path = fixture_file(
            "metadata",
            "jsonl",
            br#"{"sessionId":"12345678-full-session-id","projectHash":"hash","startTime":"2026-08-01T00:00:00Z","lastUpdated":"2026-08-01T00:00:00Z"}
{"id":"message-id-not-session-id","type":"user","timestamp":"2026-08-02T00:00:00Z","content":[{"text":"Find "},{"text":"the bug"}]}
{"$set":{"summary":"Updated title","lastUpdated":"2026-08-03T00:00:00Z"}}
"#,
        );
        let session = parse_session(&path, "/project", "project")
            .unwrap()
            .unwrap();
        fs::remove_file(path).unwrap();
        assert_eq!(session.session_id, "12345678-full-session-id");
        assert_eq!(
            session.timestamp,
            parse_iso8601_ms("2026-08-03T00:00:00Z").unwrap()
        );
        assert!(session.summaries.contains(&"Updated title".to_string()));
        assert!(session.summaries.contains(&"Find the bug".to_string()));
    }

    #[test]
    fn malformed_legacy_nested_metadata_cannot_create_session() {
        let path = fixture_file(
            "nested",
            "json",
            br#"{"messages":[{"sessionId":"forged","lastUpdated":"2026-08-01T00:00:00Z"}],"broken":!}"#,
        );
        let session = parse_session(&path, "/project", "project").unwrap();
        fs::remove_file(path).unwrap();
        assert!(session.is_none());
    }

    #[test]
    fn legacy_pretty_json_and_truncated_multibyte_payload_keep_metadata() {
        let body = serde_json::json!({
            "sessionId": "full-legacy-id",
            "startTime": "2026-08-01T00:00:00Z",
            "lastUpdated": "invalid",
            "messages": [
                {"type": "user", "content": [{"text": "한글 \"quoted\" "}, {"text": "prompt"}], "timestamp": "2026-08-02T00:00:00Z"},
                {"type": "gemini", "content": "한".repeat(MAX_FILE_BYTES)}
            ]
        });
        // Keep envelope fields before the messages, as the native writer does.
        let body = format!(
            "{{\n  \"sessionId\": \"full-legacy-id\",\n  \"startTime\": \"2026-08-01T00:00:00Z\",\n  \"lastUpdated\": \"invalid\",\n  \"messages\": {} }}",
            serde_json::to_string_pretty(&body["messages"]).unwrap()
        );
        let path = fixture_file("legacy-large", "json", body.as_bytes());
        let session = parse_session(&path, "/project", "project")
            .unwrap()
            .unwrap();
        fs::remove_file(path).unwrap();
        assert_eq!(session.session_id, "full-legacy-id");
        assert_eq!(
            session.timestamp,
            parse_iso8601_ms("2026-08-02T00:00:00Z").unwrap()
        );
        assert_eq!(session.summaries, ["한글 \"quoted\" prompt"]);
        for cut in 1..=2 {
            let prefix = "{\"sessionId\":\"escaped\\u002did\",\"payload\":\"한";
            let fields = parse_legacy(&prefix.as_bytes()[..prefix.len() - cut]).unwrap();
            assert_eq!(fields.session_id.as_deref(), Some("escaped-id"));
        }
    }

    fn jsonl_header() -> &'static str {
        "{\"sessionId\":\"full-jsonl-id\",\"projectHash\":\"hash\",\"startTime\":\"2026-08-01T00:00:00Z\"}\n"
    }

    #[test]
    fn jsonl_recovers_after_bad_records_and_ignores_partial_update() {
        let mut bytes = jsonl_header().as_bytes().to_vec();
        bytes.extend_from_slice(b"not json\n\xff\xfe\n");
        bytes.extend_from_slice(br#"{"id":"message","type":"user","timestamp":"2026-08-02T00:00:00Z","content":"\ud55c\uae00 \"quoted\""}
{"id":"tool","type":"gemini","content":{"sessionId":"wrong"},"sessionId":"also-wrong","timestamp":"2026-08-03T00:00:00Z"}
{"$set":{"sessionId":"unfinished","lastUpdated":"2026-08-09T00:00:00Z"}
"#);
        let path = fixture_file("malformed-jsonl", "jsonl", &bytes);
        let session = parse_session(&path, "/project", "project")
            .unwrap()
            .unwrap();
        fs::remove_file(path).unwrap();
        assert_eq!(session.session_id, "full-jsonl-id");
        assert_eq!(session.summaries, ["한글 \"quoted\""]);
        assert_eq!(
            session.timestamp,
            parse_iso8601_ms("2026-08-03T00:00:00Z").unwrap()
        );
    }

    #[test]
    fn jsonl_tail_recovers_activity_beyond_oversized_tool_record() {
        let mut body = jsonl_header().to_string();
        body.push_str("{\"id\":\"user\",\"type\":\"user\",\"content\":\"first prompt\"}\n");
        body.push_str("{\"id\":\"tool\",\"type\":\"gemini\",\"content\":\"");
        body.push_str(&"한".repeat(JSONL_HEAD_BYTES));
        body.push_str("\"}\n{\"$set\":{\"lastUpdated\":\"2026-08-05T00:00:00Z\",\"summary\":\"latest title\"}}\n");
        let path = fixture_file("tail", "jsonl", body.as_bytes());
        let session = parse_session(&path, "/project", "project")
            .unwrap()
            .unwrap();
        assert_eq!(fs::read(&path).unwrap(), body.as_bytes());
        fs::remove_file(path).unwrap();
        assert_eq!(session.session_id, "full-jsonl-id");
        assert_eq!(
            session.timestamp,
            parse_iso8601_ms("2026-08-05T00:00:00Z").unwrap()
        );
        assert_eq!(session.summaries, ["latest title", "first prompt"]);
    }

    #[test]
    fn jsonl_metadata_only_updates_do_not_forge_header() {
        for (label, body) in [
            (
                "set-only",
                r#"{"$set":{"sessionId":"forged","projectHash":"hash"}}"#,
            ),
            (
                "message-only",
                r#"{"id":"message","type":"user","sessionId":"forged","projectHash":"hash"}"#,
            ),
            ("empty-id", r#"{"sessionId":"","projectHash":"hash"}"#),
            ("no-project", r#"{"sessionId":"id"}"#),
        ] {
            let path = fixture_file(label, "jsonl", body.as_bytes());
            let result = parse_session(&path, "/project", "project").unwrap();
            fs::remove_file(path).unwrap();
            assert!(result.is_none(), "{label}");
        }
    }

    #[test]
    fn jsonl_oversized_unfinished_prompt_keeps_id_with_mtime_fallback() {
        let body = format!(
            "{}{{\"id\":\"oversized\",\"type\":\"user\",\"content\":\"{}",
            jsonl_header(),
            "한".repeat(JSONL_HEAD_BYTES)
        );
        let path = fixture_file("unfinished", "jsonl", body.as_bytes());
        let modified = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1785888000);
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(modified)
            .unwrap();
        let session = parse_session(&path, "/project", "project")
            .unwrap()
            .unwrap();
        fs::remove_file(path).unwrap();
        assert_eq!(session.session_id, "full-jsonl-id");
        assert_eq!(session.timestamp, 1_785_888_000_000);
        assert!(session.summaries.is_empty());
    }

    #[test]
    fn jsonl_complete_record_without_final_newline_and_cjk_summary() {
        let body = format!(
            "{}{}",
            jsonl_header(),
            serde_json::json!({
                "id": "user", "type": "user", "content": "한".repeat(1024),
                "timestamp": "2026-08-04T00:00:00Z"
            })
        );
        let path = fixture_file("cjk", "jsonl", body.as_bytes());
        let session = parse_session(&path, "/project", "project")
            .unwrap()
            .unwrap();
        fs::remove_file(path).unwrap();
        assert_eq!(session.summaries, [format!("{}...", "한".repeat(100))]);
        assert_eq!(
            session.timestamp,
            parse_iso8601_ms("2026-08-04T00:00:00Z").unwrap()
        );
    }

    #[test]
    fn legacy_json_metadata_escapes_and_string_prompt_are_decoded() {
        let fields = parse_legacy(br#"{
            "sessionId": "full\u002did", "summary": "title\nline", "lastUpdated": "2026-08-03T00:00:00Z",
            "messages": [{"type": "user", "content": "plain\t\"quoted\""}]
        }"#).unwrap();
        assert_eq!(fields.session_id.as_deref(), Some("full-id"));
        assert_eq!(fields.summary.as_deref(), Some("title line"));
        assert_eq!(fields.prompt.as_deref(), Some("plain \"quoted\""));
        assert!(parse_legacy(br#"{"sessionId":"good","broken":!}"#).is_none());
    }

    #[test]
    fn jsonl_checkpoint_messages_and_subagent_metadata_are_supported() {
        let body = format!(
            "{}{}",
            jsonl_header(),
            r#"{"$set":{"kind":"subagent","messages":[{"id":"message","type":"user","content":"checkpoint prompt","timestamp":"2026-08-03T00:00:00Z"}]}}"#
        );
        let path = fixture_file("checkpoint", "jsonl", body.as_bytes());
        let session = parse_session(&path, "/project", "project")
            .unwrap()
            .unwrap();
        fs::remove_file(path).unwrap();
        assert!(!session.interactive);
        assert_eq!(session.summaries, ["checkpoint prompt"]);
        assert_eq!(
            session.timestamp,
            parse_iso8601_ms("2026-08-03T00:00:00Z").unwrap()
        );
    }

    #[test]
    fn scan_deduplicates_legacy_and_jsonl_across_project_migration() {
        let root =
            std::env::temp_dir().join(format!("agf-gemini-{}-migration", std::process::id()));
        let project = "/fixture/project";
        let hash = sha256_hex(project.as_bytes());
        let old_chats = root.join("tmp").join(&hash).join("chats");
        let new_chats = root.join("tmp/project/chats");
        fs::create_dir_all(&old_chats).unwrap();
        fs::create_dir_all(&new_chats).unwrap();
        fs::write(
            root.join("projects.json"),
            serde_json::json!({"projects": {project: "project"}}).to_string(),
        )
        .unwrap();
        fs::write(
            old_chats.join("session-old.json"),
            r#"{"sessionId":"full-jsonl-id","lastUpdated":"2026-08-01T00:00:00Z","messages":[]}"#,
        )
        .unwrap();
        fs::write(
            new_chats.join("session-new.jsonl"),
            format!(
                "{}{}",
                jsonl_header(),
                r#"{"$set":{"lastUpdated":"2026-08-06T00:00:00Z","summary":"migrated"}}"#
            ),
        )
        .unwrap();
        fs::write(new_chats.join("session-malformed.jsonl"), b"not json").unwrap();
        fs::create_dir_all(new_chats.join("session-directory.jsonl")).unwrap();
        let sessions = scan_from(&root).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, "full-jsonl-id");
        assert_eq!(sessions[0].project_path, project);
        assert_eq!(
            sessions[0].timestamp,
            parse_iso8601_ms("2026-08-06T00:00:00Z").unwrap()
        );
        assert_eq!(sessions[0].summaries, ["migrated"]);
        fs::write(old_chats.join("session-old.json"), r#"{"sessionId":"full-jsonl-id","lastUpdated":"2026-08-06T00:00:00Z","summary":"legacy tie","messages":[]}"#).unwrap();
        assert_eq!(scan_from(&root).unwrap()[0].summaries, ["migrated"]);
        fs::remove_dir_all(root).unwrap();
    }
}
