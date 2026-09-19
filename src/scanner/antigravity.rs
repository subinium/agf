use std::collections::HashMap;
use std::fs;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use rusqlite::Connection;

use crate::error::AgfError;
use crate::model::{Agent, Session};

use super::{collapse_whitespace, project_name_from_path, truncate};

const MAX_SUMMARY_CHARS: usize = 160;
const HEAD_LOG_BYTES: u64 = 128 * 1024;
const TAIL_LOG_BYTES: u64 = 64 * 1024;
const MAX_DB_TEXT_CHARS: i64 = 4096;
const MAX_WORKSPACE_BYTES: usize = 64 * 1024;

pub fn scan() -> Result<Vec<Session>, AgfError> {
    let antigravity_dir = crate::config::antigravity_dir()?;
    scan_from(&antigravity_dir)
}

pub(crate) fn scan_from(base_dir: &Path) -> Result<Vec<Session>, AgfError> {
    let Some(metadata) = optional_metadata(base_dir)? else {
        return Ok(Vec::new());
    };
    require_directory(&metadata)?;

    let brain_dir = base_dir.join("brain");
    let db_path = base_dir.join("conversation_summaries.db");

    let mut sessions: HashMap<String, Session> = HashMap::new();

    if let Some(metadata) = optional_metadata(&db_path)? {
        require_file(&metadata)?;
        let conn = Connection::open_with_flags(
            &db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        scan_db(&conn, &mut sessions)?;
    }

    if let Some(metadata) = optional_metadata(&brain_dir)? {
        require_directory(&metadata)?;
        for entry in fs::read_dir(&brain_dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let Ok(dir_name) = entry.file_name().into_string() else {
                continue;
            };
            if !is_valid_uuid(&dir_name) {
                continue;
            }
            let key = dir_name.to_ascii_lowercase();
            if !sessions.contains_key(&key) && !has_conversation_store(base_dir, &dir_name)? {
                continue;
            }

            let transcript_path = entry
                .path()
                .join(".system_generated")
                .join("logs")
                .join("transcript.jsonl");

            let Some(transcript) = read_transcript(&transcript_path)? else {
                continue;
            };
            if let Some(existing) = sessions.get_mut(&key) {
                if let Some(prompt) = transcript.prompt
                    && !prompt.is_empty()
                    && !existing.summaries.contains(&prompt)
                {
                    existing.summaries.push(prompt);
                }
                if existing.timestamp <= 0 {
                    existing.timestamp = transcript.timestamp;
                }
            } else {
                sessions.insert(
                    key,
                    Session {
                        agent: Agent::Antigravity,
                        session_id: dir_name,
                        project_name: "unknown".to_string(),
                        project_path: String::new(),
                        summaries: transcript
                            .prompt
                            .filter(|prompt| !prompt.is_empty())
                            .into_iter()
                            .collect(),
                        timestamp: transcript.timestamp,
                        git_branch: None,
                        worktree: None,
                        recap: None,
                        // Native storage proves existence, not root/subagent lineage.
                        interactive: false,
                    },
                );
            }
        }
    }

    for session in sessions.values_mut() {
        if session.timestamp <= 0 {
            session.timestamp = folder_mtime_ms(&brain_dir.join(&session.session_id))?.unwrap_or(0);
        }
    }
    Ok(sessions.into_values().collect())
}

fn has_conversation_store(base_dir: &Path, id: &str) -> io::Result<bool> {
    if !is_valid_uuid(id) {
        return Ok(false);
    }
    for suffix in [".db", ".db-wal"] {
        let path = base_dir.join("conversations").join(format!("{id}{suffix}"));
        if optional_metadata(&path)?.is_some_and(|metadata| metadata.is_file()) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn scan_db(conn: &Connection, sessions: &mut HashMap<String, Session>) -> Result<(), AgfError> {
    let mut stmt = conn.prepare(
        "SELECT CASE WHEN length(CAST(conversation_id AS BLOB)) <= 36 \
                     THEN conversation_id END, \
                substr(title, 1, ?1), \
                substr(preview, 1, ?1), \
                CASE WHEN length(CAST(workspace_uris AS BLOB)) <= ?2 \
                     THEN workspace_uris END, \
                substr(last_modified_time, 1, 128), \
                CASE WHEN length(CAST(parent_conversation_id AS BLOB)) > 128 \
                     THEN 'oversized-parent' ELSE parent_conversation_id END, \
                nesting_depth \
         FROM conversation_summaries",
    )?;

    let rows = stmt.query_map([MAX_DB_TEXT_CHARS, MAX_WORKSPACE_BYTES as i64], |row| {
        let conversation_id: Option<String> = row.get(0)?;
        let title = row.get::<_, Option<String>>(1)?.unwrap_or_default();
        let preview = row.get::<_, Option<String>>(2)?.unwrap_or_default();
        let workspace_uris = row.get::<_, Option<String>>(3)?.unwrap_or_default();
        let last_modified_time = row.get::<_, Option<String>>(4)?.unwrap_or_default();
        let parent_conversation_id = row.get::<_, Option<String>>(5)?.unwrap_or_default();
        let nesting_depth = row.get::<_, Option<i64>>(6)?.unwrap_or(0);
        Ok((
            conversation_id,
            title,
            preview,
            workspace_uris,
            last_modified_time,
            parent_conversation_id,
            nesting_depth,
        ))
    })?;

    for row in rows {
        let (
            conversation_id,
            title,
            preview,
            workspace_uris,
            last_modified_time,
            parent_conversation_id,
            nesting_depth,
        ) = row?;
        let Some(conversation_id) = conversation_id.filter(|id| is_valid_uuid(id)) else {
            continue;
        };

        let project_path = parse_workspace_uri(&workspace_uris);
        let project_name = if project_path.is_empty() {
            "unknown".to_string()
        } else {
            project_name_from_path(&project_path)
        };

        let timestamp = parse_timestamp(&last_modified_time);

        let mut summaries = Vec::new();
        let clean_title = collapse_whitespace(&title);
        if !clean_title.is_empty() {
            summaries.push(truncate(&clean_title, MAX_SUMMARY_CHARS));
        }
        let clean_preview = collapse_whitespace(&preview);
        let preview = truncate(&clean_preview, MAX_SUMMARY_CHARS);
        if !preview.is_empty() && !summaries.contains(&preview) {
            summaries.push(preview);
        }

        let interactive = parent_conversation_id.trim().is_empty() && nesting_depth == 0;

        let key = conversation_id.to_ascii_lowercase();
        if sessions
            .get(&key)
            .is_some_and(|session| session.timestamp >= timestamp)
        {
            continue;
        }
        sessions.insert(
            key,
            Session {
                agent: Agent::Antigravity,
                session_id: conversation_id,
                project_name,
                project_path,
                summaries,
                timestamp,
                git_branch: None,
                worktree: None,
                recap: None,
                interactive,
            },
        );
    }

    Ok(())
}

#[derive(Default)]
struct Transcript {
    prompt: Option<String>,
    timestamp: i64,
}

fn read_transcript(path: &Path) -> Result<Option<Transcript>, AgfError> {
    let Some(metadata) = optional_metadata(path)? else {
        return Ok(None);
    };
    require_file(&metadata)?;
    let mut file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let metadata = file.metadata()?;
    require_file(&metadata)?;
    Ok(Some(read_transcript_windows(
        &mut file,
        metadata.len(),
        mtime_ms(&metadata)?,
    )?))
}

/// Like the shared head/tail reader, but preserve read/seek errors for cache freshness.
fn read_transcript_windows(
    reader: &mut (impl Read + Seek),
    len: u64,
    mtime: i64,
) -> io::Result<Transcript> {
    let split = len > HEAD_LOG_BYTES + TAIL_LOG_BYTES;
    let head_len = if split { HEAD_LOG_BYTES } else { len };
    let mut head = vec![0; head_len as usize];
    reader.read_exact(&mut head)?;
    if split {
        head.truncate(
            head.iter()
                .rposition(|byte| *byte == b'\n')
                .map_or(0, |i| i + 1),
        );
    }
    let mut transcript = Transcript::default();
    read_transcript_records(&head, &mut transcript);
    if split {
        reader.seek(SeekFrom::Start(len - TAIL_LOG_BYTES - 1))?;
        let mut tail = vec![0; TAIL_LOG_BYTES as usize + 1];
        reader.read_exact(&mut tail)?;
        let start = tail
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(tail.len(), |i| i + 1);
        let head_timestamp = transcript.timestamp;
        transcript.timestamp = 0;
        read_transcript_records(&tail[start..], &mut transcript);
        // A giant/incomplete tail can hide all recent events in the omitted middle.
        if transcript.timestamp <= 0 {
            transcript.timestamp = mtime.max(head_timestamp);
        } else {
            transcript.timestamp = transcript.timestamp.max(head_timestamp);
        }
    }
    if transcript.timestamp <= 0 {
        transcript.timestamp = mtime;
    }
    Ok(transcript)
}

fn read_transcript_records(bytes: &[u8], transcript: &mut Transcript) {
    for line in bytes.split(|byte| *byte == b'\n') {
        let Ok(val) = serde_json::from_slice::<serde_json::Value>(line) else {
            continue;
        };
        if let Some(timestamp) = val
            .get("created_at")
            .and_then(serde_json::Value::as_str)
            .and_then(parse_iso8601_ms)
        {
            transcript.timestamp = transcript.timestamp.max(timestamp);
        }
        let step_type = val.get("type").and_then(serde_json::Value::as_str);
        if step_type == Some("USER_INPUT") && transcript.prompt.is_none() {
            let content = val
                .get("content")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            transcript.prompt = Some(truncate(&clean_user_prompt(content), MAX_SUMMARY_CHARS));
        }
    }
}

/// Clean `<USER_REQUEST>...</USER_REQUEST>` and discard metadata wrappers
pub(crate) fn clean_user_prompt(raw: &str) -> String {
    let mut text = raw;

    // Strip <USER_REQUEST> tags if present
    if let Some(start) = text.find("<USER_REQUEST>") {
        let after_start = &text[start + "<USER_REQUEST>".len()..];
        text = if let Some(end) = after_start.find("</USER_REQUEST>") {
            &after_start[..end]
        } else {
            after_start
        };
    } else if let Some(start) = text.find("<ADDITIONAL_METADATA>") {
        text = &text[..start];
    }

    // Strip any trailing ADDITIONAL_METADATA or USER_SETTINGS_CHANGE
    if let Some(idx) = text.find("<ADDITIONAL_METADATA>") {
        text = &text[..idx];
    }
    if let Some(idx) = text.find("<USER_SETTINGS_CHANGE>") {
        text = &text[..idx];
    }

    collapse_whitespace(text.trim())
}

/// Select the first safe native file URI, without probing the workspace filesystem.
pub(crate) fn parse_workspace_uri(raw: &str) -> String {
    if raw.len() > MAX_WORKSPACE_BYTES {
        return String::new();
    }
    let trimmed = raw.trim();
    let uris: Vec<String> = if trimmed.starts_with('[') {
        match serde_json::from_str(trimmed) {
            Ok(uris) => uris,
            Err(_) => return String::new(),
        }
    } else {
        vec![trimmed.to_string()]
    };
    uris.iter()
        .find_map(|uri| native_workspace_path(uri))
        .unwrap_or_default()
}

fn native_workspace_path(uri: &str) -> Option<String> {
    let url = validated_file_url(uri, cfg!(windows))?;
    // Native conversion owns drive/UNC handling. Never build Windows paths on Unix.
    let path = url.to_file_path().ok()?;
    path.is_absolute().then_some(())?;
    path.into_os_string().into_string().ok()
}

/// Validate before WHATWG parsing can normalize traversal, controls, or backslashes.
fn validated_file_url(uri: &str, windows: bool) -> Option<url::Url> {
    if !uri.get(..7)?.eq_ignore_ascii_case("file://")
        || uri.chars().any(|c| c.is_control() || c == '\\')
        || uri.trim() != uri
    {
        return None;
    }
    let (authority, raw_path) = uri[7..].split_once('/')?;
    if authority.contains(['%', ':', '@']) {
        return None;
    }
    let raw_path = strict_percent_decode(raw_path)?;
    if raw_path.starts_with('/')
        || raw_path.as_bytes().get(1) == Some(&b'|')
        || raw_path.split('/').any(|part| part == "." || part == "..")
    {
        return None;
    }
    let url = url::Url::parse(uri).ok()?;
    if url.scheme() != "file"
        || url.query().is_some()
        || url.fragment().is_some()
        || (!authority.is_empty()
            && !authority.eq_ignore_ascii_case("localhost")
            && url.host_str().is_none())
    {
        return None;
    }
    let decoded = strict_percent_decode(url.path())?;
    if !decoded.starts_with('/') || decoded.starts_with("//") {
        return None;
    }
    let has_drive = decoded
        .as_bytes()
        .get(1)
        .is_some_and(u8::is_ascii_alphabetic)
        && decoded.as_bytes().get(2) == Some(&b':');
    if windows {
        let components = if url.host_str().is_some() {
            if decoded == "/" || has_drive {
                return None;
            }
            &decoded[1..]
        } else {
            if !has_drive || decoded.as_bytes().get(3) != Some(&b'/') {
                return None;
            }
            &decoded[4..]
        };
        if components
            .split('/')
            .any(|part| !safe_windows_component(part))
        {
            return None;
        }
    } else if url.host_str().is_some() || has_drive {
        return None;
    }
    Some(url)
}

fn safe_windows_component(part: &str) -> bool {
    if part.contains([':', '*', '?', '"', '<', '>', '|']) || part.ends_with([' ', '.']) {
        return false;
    }
    let stem = part.split('.').next().unwrap_or("").to_ascii_uppercase();
    !matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        && !(stem.starts_with("COM") || stem.starts_with("LPT"))
            .then(|| &stem[3..])
            .is_some_and(|suffix| {
                matches!(
                    suffix,
                    "1" | "2"
                        | "3"
                        | "4"
                        | "5"
                        | "6"
                        | "7"
                        | "8"
                        | "9"
                        | "\u{b9}"
                        | "\u{b2}"
                        | "\u{b3}"
                )
            })
}

fn strict_percent_decode(s: &str) -> Option<String> {
    let mut bytes = Vec::with_capacity(s.len());
    let mut chars = s.as_bytes().iter().copied();
    while let Some(b) = chars.next() {
        if b == b'%' {
            let high = char::from(chars.next()?).to_digit(16)?;
            let low = char::from(chars.next()?).to_digit(16)?;
            let decoded = ((high << 4) | low) as u8;
            if decoded == b'/' || decoded == b'\\' {
                return None;
            }
            bytes.push(decoded);
        } else {
            bytes.push(b);
        }
    }
    let decoded = String::from_utf8(bytes).ok()?;
    (!decoded.chars().any(|c| c.is_control() || c == '\\')).then_some(decoded)
}

fn is_valid_uuid(s: &str) -> bool {
    s.len() == 36
        && s.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

fn parse_timestamp(s: &str) -> i64 {
    parse_iso8601_ms(s).unwrap_or(0)
}

fn parse_iso8601_ms(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.timestamp_millis())
        .or_else(|| {
            // Try parsing "YYYY-MM-DD HH:MM:SS.ssssss+00:00"
            chrono::DateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f%:z")
                .ok()
                .map(|dt| dt.timestamp_millis())
        })
}

fn optional_metadata(path: &Path) -> io::Result<Option<fs::Metadata>> {
    match fs::metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn require_directory(metadata: &fs::Metadata) -> io::Result<()> {
    if metadata.is_dir() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "expected an Antigravity directory",
        ))
    }
}

fn require_file(metadata: &fs::Metadata) -> io::Result<()> {
    if metadata.is_file() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "expected an Antigravity file",
        ))
    }
}

fn mtime_ms(metadata: &fs::Metadata) -> io::Result<i64> {
    Ok(metadata
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_millis()).ok())
        .unwrap_or(0))
}

fn folder_mtime_ms(path: &Path) -> io::Result<Option<i64>> {
    optional_metadata(path)?
        .map(|metadata| mtime_ms(&metadata))
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, SystemTime};

    const ID: &str = "12345678-abcd-4abc-8abc-123456789abc";
    const CHILD_ID: &str = "22345678-abcd-4abc-8abc-123456789abc";
    const ORPHAN_ID: &str = "32345678-abcd-4abc-8abc-123456789abc";
    const FIRST: &str = "2026-09-01T01:00:00Z";
    const LATEST: &str = "2026-09-18T05:31:00Z";

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let root = std::env::temp_dir().join(format!(
                "agf-antigravity-{}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                NEXT.fetch_add(1, Ordering::Relaxed),
            ));
            fs::create_dir(&root).unwrap();
            Self(root)
        }

        fn db(&self) -> Connection {
            let conn = Connection::open(self.0.join("conversation_summaries.db")).unwrap();
            conn.execute_batch(
                "CREATE TABLE conversation_summaries (
                    conversation_id TEXT, title TEXT, preview TEXT, workspace_uris TEXT,
                    last_modified_time TEXT, parent_conversation_id TEXT, nesting_depth INTEGER
                 );",
            )
            .unwrap();
            conn
        }

        fn transcript(&self, id: &str, content: impl AsRef<[u8]>) -> PathBuf {
            let path = self
                .0
                .join("brain")
                .join(id)
                .join(".system_generated/logs/transcript.jsonl");
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, content).unwrap();
            path
        }

        fn conversation(&self, id: &str, wal: bool) -> PathBuf {
            let path = self
                .0
                .join("conversations")
                .join(format!("{id}{}", if wal { ".db-wal" } else { ".db" }));
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, b"opaque provider conversation store").unwrap();
            path
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    fn insert_row(conn: &Connection, id: &str, title: &str, timestamp: &str) {
        conn.execute(
            "INSERT INTO conversation_summaries VALUES (?1, ?2, NULL, NULL, ?3, NULL, NULL)",
            rusqlite::params![id, title, timestamp],
        )
        .unwrap();
    }

    fn record(kind: &str, content: &str, timestamp: &str) -> String {
        format!(
            "{}\n",
            serde_json::json!({"type": kind, "content": content, "created_at": timestamp})
        )
    }

    fn native_uri() -> &'static str {
        if cfg!(windows) {
            "file:///C:/Users/test/my%20project"
        } else {
            "file:///home/test/my%20project"
        }
    }

    #[test]
    fn parse_workspace_uri_handles_json_and_plain() {
        let expected = if cfg!(windows) {
            "C:\\Users\\test\\my project"
        } else {
            "/home/test/my project"
        };
        assert_eq!(parse_workspace_uri(native_uri()), expected);
        assert_eq!(
            parse_workspace_uri(&serde_json::to_string(&[native_uri()]).unwrap()),
            expected
        );
        assert_eq!(
            parse_workspace_uri(&native_uri().replacen("file://", "file://localhost", 1)),
            expected
        );
        assert_eq!(parse_workspace_uri(""), "");
        assert_eq!(parse_workspace_uri("[]"), "");
    }

    #[test]
    fn file_uri_validation_covers_both_platforms_without_filesystem_access() {
        for (uri, unix, windows) in [
            ("file:///home/test/project", true, false),
            ("file://localhost/home/test/project", true, false),
            ("file:///C:/Users/test/project", false, true),
            ("file://localhost/C:/Users/test/project", false, true),
            ("file://server/share/my%20project", false, true),
            ("file://server/", false, false),
            ("file:///C:relative", false, false),
            ("file:///C|/legacy", false, false),
            ("file:////server/share", false, false),
            ("file://server/C:/project", false, false),
            ("file:///C:/project:stream", false, false),
            ("file:///C:/NUL.txt", false, false),
            ("file:///C:/COM1/project", false, false),
            ("file:///C:/COM%C2%B9/project", false, false),
            ("file:///C:/project%20", false, false),
            ("file:///C:/project.", false, false),
        ] {
            assert_eq!(
                validated_file_url(uri, false).is_some(),
                unix,
                "Unix: {uri}"
            );
            assert_eq!(
                validated_file_url(uri, true).is_some(),
                windows,
                "Windows: {uri}"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn native_windows_paths_use_drives_and_unc() {
        assert_eq!(
            parse_workspace_uri("file:///C:/Users/my%20project"),
            "C:\\Users\\my project"
        );
        assert_eq!(
            parse_workspace_uri("file://localhost/C:/Users/my%20project"),
            "C:\\Users\\my project"
        );
        assert_eq!(
            parse_workspace_uri("file://server/share/my%20project"),
            "\\\\server\\share\\my project"
        );
        assert_eq!(parse_workspace_uri("file:///home/unix/project"), "");
    }

    #[test]
    fn malformed_foreign_and_unsafe_workspace_values_are_not_cwds() {
        for uri in [
            "https://example.com/project",
            "vscode-remote://ssh-remote+server/home/test",
            "/home/test/project",
            "../project",
            "C:\\project",
            "file://relative",
            "file:relative",
            "file:///tmp/%",
            "file:///tmp/%2",
            "file:///tmp/%GG",
            "file:///tmp/%FF",
            "file:///tmp/%C3%28",
            "file:///tmp/%ED%A0%80",
            "file:///tmp/%00",
            "file:///tmp/%0A",
            "file:///tmp/%C2%85",
            "file:///tmp/a%2Fb",
            "file:///tmp/a%5Cb",
            "file:///tmp/a\\b",
            "file:///tmp/a\nb",
            "file:///tmp/../outside",
            "file:///tmp/%2e%2e/outside",
            "file:///tmp/./project",
            "file:///tmp/project?query",
            "file:///tmp/project#fragment",
            "file://user@localhost/tmp/project",
            "file://localhost:80/tmp/project",
            "file://%6cocalhost/tmp/project",
            "[\"file:///tmp/project\",null]",
            "[\"file:///tmp/project\"",
            "{\"uri\":\"file:///tmp/project\"}",
        ] {
            assert_eq!(parse_workspace_uri(uri), "", "{uri:?}");
        }
        assert_eq!(
            parse_workspace_uri(&"a".repeat(MAX_WORKSPACE_BYTES + 1)),
            ""
        );
        let values = serde_json::to_string(&["https://example.com", native_uri()]).unwrap();
        assert_eq!(
            parse_workspace_uri(&values),
            parse_workspace_uri(native_uri())
        );
    }

    #[test]
    fn unicode_paths_percent_signs_and_plus_are_lossless() {
        let uri = format!(
            "{}/%ED%95%9C%EA%B8%80/%F0%9F%9A%80/a+b%2520%23%3F",
            native_uri()
        );
        // '?' is a valid Unix filename, but not a Windows filename.
        if cfg!(windows) {
            assert_eq!(parse_workspace_uri(&uri), "");
        } else {
            assert!(parse_workspace_uri(&uri).ends_with("/\u{d55c}\u{ae00}/\u{1f680}/a+b%20#?"));
        }
        assert_eq!(
            strict_percent_decode("%ED%95%9C%EA%B8%80"),
            Some("\u{d55c}\u{ae00}".into())
        );
        assert_eq!(
            strict_percent_decode("%F0%9F%9A%80"),
            Some("\u{1f680}".into())
        );
    }

    #[test]
    fn uuid_requires_exact_hyphen_positions() {
        assert!(is_valid_uuid(ID));
        assert!(is_valid_uuid(&ID.to_ascii_uppercase()));
        for id in [
            "------------------------------------",
            "12345678abcd-4abc-8abc-123456789abc-",
            "../12345678-abcd-4abc-8abc-123456789abc",
            "12345678-abcd-4abc-8abc-123456789abg",
        ] {
            assert!(!is_valid_uuid(id), "{id}");
        }
    }

    #[test]
    fn missing_roots_stores_and_transcripts_are_normal_and_create_nothing() {
        let fixture = Fixture::new();
        assert!(scan_from(&fixture.0.join("missing")).unwrap().is_empty());
        assert!(scan_from(&fixture.0).unwrap().is_empty());
        assert_eq!(fs::read_dir(&fixture.0).unwrap().count(), 0);
        fs::create_dir_all(fixture.0.join("brain").join(ID)).unwrap();
        assert!(scan_from(&fixture.0).unwrap().is_empty());
        let conn = fixture.db();
        insert_row(&conn, ID, "DB-only title", "");
        let sessions = scan_from(&fixture.0).unwrap();
        assert_eq!(sessions.len(), 1);
        assert!(sessions[0].timestamp > 0);
        assert_eq!(sessions[0].summaries, ["DB-only title"]);
        assert!(
            read_transcript(&fixture.0.join("missing.jsonl"))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn invalid_store_types_and_corrupt_databases_return_errors() {
        let fixture = Fixture::new();
        let root_file = fixture.0.join("not-a-root");
        fs::write(&root_file, b"").unwrap();
        assert!(matches!(scan_from(&root_file), Err(AgfError::Io(_))));
        let db_path = fixture.0.join("conversation_summaries.db");
        fs::create_dir(&db_path).unwrap();
        assert!(matches!(scan_from(&fixture.0), Err(AgfError::Io(_))));
        fs::remove_dir(&db_path).unwrap();
        fs::write(&db_path, b"not a SQLite database").unwrap();
        assert!(matches!(scan_from(&fixture.0), Err(AgfError::Sqlite(_))));
        fs::remove_file(&db_path).unwrap();
        fs::write(fixture.0.join("brain"), b"not a directory").unwrap();
        assert!(matches!(scan_from(&fixture.0), Err(AgfError::Io(_))));
    }

    #[test]
    fn schema_and_row_errors_do_not_return_partial_success() {
        let fixture = Fixture::new();
        let conn = fixture.db();
        insert_row(&conn, ID, "Good row before bad row", FIRST);
        conn.execute("INSERT INTO conversation_summaries (conversation_id, nesting_depth) VALUES (?1, 'broken')", [CHILD_ID]).unwrap();
        assert!(matches!(scan_from(&fixture.0), Err(AgfError::Sqlite(_))));
        conn.execute(
            "DELETE FROM conversation_summaries WHERE conversation_id = ?1",
            [CHILD_ID],
        )
        .unwrap();
        conn.execute("UPDATE conversation_summaries SET title = x'FF'", [])
            .unwrap();
        assert!(matches!(scan_from(&fixture.0), Err(AgfError::Sqlite(_))));
        conn.execute_batch("DROP TABLE conversation_summaries;")
            .unwrap();
        assert!(matches!(scan_from(&fixture.0), Err(AgfError::Sqlite(_))));
    }

    #[test]
    fn nullable_fields_remain_compatible() {
        let fixture = Fixture::new();
        let conn = fixture.db();
        conn.execute(
            "INSERT INTO conversation_summaries (conversation_id) VALUES (?1)",
            [ID],
        )
        .unwrap();
        let sessions = scan_from(&fixture.0).unwrap();
        assert_eq!(sessions.len(), 1);
        assert!(sessions[0].summaries.is_empty());
        assert_eq!(sessions[0].project_path, "");
        assert_eq!(sessions[0].timestamp, 0);
        assert!(sessions[0].interactive);
    }

    #[test]
    fn sql_bounds_large_fields_without_truncating_workspace_into_a_valid_cwd() {
        let fixture = Fixture::new();
        let conn = fixture.db();
        let title = "\u{754c}".repeat(MAX_DB_TEXT_CHARS as usize + 100);
        let workspace = format!("{}{}", native_uri(), "x".repeat(MAX_WORKSPACE_BYTES));
        conn.execute(
            "INSERT INTO conversation_summaries VALUES (?1, ?2, ?2, ?3, ?4, ?5, 0)",
            rusqlite::params![ID, title, workspace, LATEST, "x".repeat(200_000)],
        )
        .unwrap();
        let sessions = scan_from(&fixture.0).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(
            sessions[0].summaries,
            [format!("{}...", "\u{754c}".repeat(MAX_SUMMARY_CHARS))]
        );
        assert_eq!(sessions[0].project_path, "");
        assert!(!sessions[0].interactive);
        conn.execute(
            "UPDATE conversation_summaries SET conversation_id = ?1",
            ["x".repeat(200_000)],
        )
        .unwrap();
        assert!(scan_from(&fixture.0).unwrap().is_empty());
    }

    #[test]
    fn db_brain_dedup_enrichment_orphans_and_subagents() {
        let fixture = Fixture::new();
        let conn = fixture.db();
        insert_row(&conn, ID, "Older title", FIRST);
        insert_row(&conn, &ID.to_ascii_uppercase(), "Latest title", LATEST);
        conn.execute(
            "UPDATE conversation_summaries SET preview = 'Latest title', workspace_uris = ?1",
            [serde_json::to_string(&[native_uri()]).unwrap()],
        )
        .unwrap();
        insert_row(&conn, CHILD_ID, "Child", FIRST);
        conn.execute("UPDATE conversation_summaries SET parent_conversation_id = ?1 WHERE conversation_id = ?2", [ID, CHILD_ID]).unwrap();
        fixture.transcript(ID, record("USER_INPUT", "<USER_REQUEST>Actual prompt</USER_REQUEST><ADDITIONAL_METADATA>ignore</ADDITIONAL_METADATA>", FIRST));
        fixture.transcript(CHILD_ID, record("USER_INPUT", "Child", FIRST));
        fixture.transcript(
            ORPHAN_ID,
            record("USER_INPUT", "Orphan", FIRST) + &record("PLANNER_RESPONSE", "Done", LATEST),
        );
        fixture.transcript("not-a-uuid", record("USER_INPUT", "Ignored", LATEST));
        fixture.conversation(ORPHAN_ID, false);
        let sessions = scan_from(&fixture.0).unwrap();
        assert_eq!(sessions.len(), 3);
        let root = sessions
            .iter()
            .find(|s| s.session_id.eq_ignore_ascii_case(ID))
            .unwrap();
        assert_eq!(root.summaries, ["Latest title", "Actual prompt"]);
        assert_eq!(root.project_path, parse_workspace_uri(native_uri()));
        assert_eq!(root.project_name, "my project");
        assert_eq!(root.timestamp, parse_timestamp(LATEST));
        assert!(root.interactive);
        let child = sessions.iter().find(|s| s.session_id == CHILD_ID).unwrap();
        assert!(!child.interactive);
        assert_eq!(child.summaries, ["Child"]);
        let orphan = sessions.iter().find(|s| s.session_id == ORPHAN_ID).unwrap();
        assert_eq!(orphan.timestamp, parse_timestamp(LATEST));
        assert_eq!(orphan.project_path, "");
        assert!(!orphan.interactive);
        conn.execute("UPDATE conversation_summaries SET parent_conversation_id = NULL, nesting_depth = 1 WHERE conversation_id = ?1", [CHILD_ID]).unwrap();
        assert!(
            !scan_from(&fixture.0)
                .unwrap()
                .iter()
                .find(|s| s.session_id == CHILD_ID)
                .unwrap()
                .interactive
        );
    }

    #[test]
    fn wal_rows_are_visible_without_changing_database_wal_or_transcript_bytes() {
        let fixture = Fixture::new();
        let conn = fixture.db();
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;")
            .unwrap();
        insert_row(&conn, ID, "Uncheckpointed title", LATEST);
        let transcript = fixture.transcript(ID, record("USER_INPUT", "WAL prompt", FIRST));
        let paths = [
            fixture.0.join("conversation_summaries.db"),
            fixture.0.join("conversation_summaries.db-wal"),
            transcript,
        ];
        let before: Vec<_> = paths.iter().map(|path| fs::read(path).unwrap()).collect();
        assert!(!before[1].is_empty());
        for _ in 0..2 {
            let sessions = scan_from(&fixture.0).unwrap();
            assert_eq!(sessions.len(), 1);
            assert_eq!(
                sessions[0].summaries,
                ["Uncheckpointed title", "WAL prompt"]
            );
        }
        let after: Vec<_> = paths.iter().map(|path| fs::read(path).unwrap()).collect();
        assert_eq!(before, after);
    }

    #[test]
    fn transcript_recovers_after_malformed_rows_and_ignores_incomplete_tail() {
        let fixture = Fixture::new();
        let mut bytes = b"not JSON\n\xff\n{broken}\n".to_vec();
        bytes.extend(record("USER_INPUT", "first prompt", FIRST).as_bytes());
        bytes.extend(record("PLANNER_RESPONSE", "latest reply", LATEST).as_bytes());
        bytes.extend(record("USER_INPUT", "not the first prompt", FIRST).as_bytes());
        bytes.extend(
            b"{\"type\":\"USER_INPUT\",\"created_at\":\"2099-01-01T00:00:00Z\",\"content\":\"",
        );
        let path = fixture.transcript(ID, bytes);
        fixture.conversation(ID, false);
        let transcript = read_transcript(&path).unwrap().unwrap();
        assert_eq!(transcript.prompt.as_deref(), Some("first prompt"));
        assert_eq!(transcript.timestamp, parse_timestamp(LATEST));
        assert_eq!(scan_from(&fixture.0).unwrap().len(), 1);
    }

    #[test]
    fn complete_final_record_without_newline_is_retained() {
        let bytes = record("USER_INPUT", "Complete", LATEST);
        let bytes = bytes.trim_end().as_bytes();
        let result =
            read_transcript_windows(&mut Cursor::new(bytes), bytes.len() as u64, 0).unwrap();
        assert_eq!(result.prompt.as_deref(), Some("Complete"));
        assert_eq!(result.timestamp, parse_timestamp(LATEST));
    }

    #[test]
    fn transcript_head_tail_bounds_recover_latest_activity() {
        let fixture = Fixture::new();
        let large = "\u{754c}".repeat((HEAD_LOG_BYTES + TAIL_LOG_BYTES) as usize);
        let bytes = record("USER_INPUT", "Bounded prompt", FIRST)
            + &record("TOOL_RESULT", &large, FIRST)
            + &record("PLANNER_RESPONSE", "Latest", LATEST);
        let path = fixture.transcript(ID, bytes);
        let result = read_transcript(&path).unwrap().unwrap();
        assert_eq!(result.prompt.as_deref(), Some("Bounded prompt"));
        assert_eq!(result.timestamp, parse_timestamp(LATEST));
    }

    #[test]
    fn timestamp_fallback_uses_latest_durable_event_or_mtime() {
        let fixture = Fixture::new();
        let conn = fixture.db();
        insert_row(&conn, ID, "No DB date", "not a timestamp");
        fixture.transcript(
            ID,
            record("USER_INPUT", "Early prompt", FIRST)
                + &record("PLANNER_RESPONSE", "Recent", LATEST),
        );
        assert_eq!(
            scan_from(&fixture.0).unwrap()[0].timestamp,
            parse_timestamp(LATEST)
        );
        let path = fixture.transcript(ID, record("USER_INPUT", "No date", ""));
        let expected = 1_786_000_123_000;
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(SystemTime::UNIX_EPOCH + Duration::from_millis(expected as u64))
            .unwrap();
        assert_eq!(scan_from(&fixture.0).unwrap()[0].timestamp, expected);
        let bytes = record("USER_INPUT", "Early prompt", FIRST)
            + &"x".repeat((HEAD_LOG_BYTES + TAIL_LOG_BYTES) as usize + 1);
        let mtime = parse_timestamp(LATEST);
        let result = read_transcript_windows(
            &mut Cursor::new(bytes.as_bytes()),
            bytes.len() as u64,
            mtime,
        )
        .unwrap();
        assert_eq!(result.timestamp, mtime);
    }

    struct FaultyReader {
        data: Cursor<Vec<u8>>,
        fail_read_at: u64,
        fail_seek: bool,
        bytes_read: usize,
    }

    impl Read for FaultyReader {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            if self.data.position() >= self.fail_read_at {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "injected read failure",
                ));
            }
            let limit =
                (self.fail_read_at - self.data.position()).min(buffer.len() as u64) as usize;
            let count = self.data.read(&mut buffer[..limit])?;
            self.bytes_read += count;
            Ok(count)
        }
    }

    impl Seek for FaultyReader {
        fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
            if self.fail_seek {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "injected seek failure",
                ))
            } else {
                self.data.seek(position)
            }
        }
    }

    #[test]
    fn actual_read_seek_and_short_read_failures_are_not_malformed_records() {
        let bytes = record("USER_INPUT", "Good before failure", FIRST).into_bytes();
        let len = bytes.len() as u64;
        for fail_at in [0, len - 1] {
            let mut reader = FaultyReader {
                data: Cursor::new(bytes.clone()),
                fail_read_at: fail_at,
                fail_seek: false,
                bytes_read: 0,
            };
            assert_eq!(
                read_transcript_windows(&mut reader, len, 0)
                    .err()
                    .unwrap()
                    .kind(),
                io::ErrorKind::PermissionDenied
            );
        }
        assert_eq!(
            read_transcript_windows(&mut Cursor::new(bytes), len + 1, 0)
                .err()
                .unwrap()
                .kind(),
            io::ErrorKind::UnexpectedEof
        );
        let len = HEAD_LOG_BYTES + TAIL_LOG_BYTES + 100;
        for (fail_read_at, fail_seek) in [(u64::MAX, true), (HEAD_LOG_BYTES, false)] {
            let mut reader = FaultyReader {
                data: Cursor::new(vec![b'x'; len as usize]),
                fail_read_at,
                fail_seek,
                bytes_read: 0,
            };
            assert_eq!(
                read_transcript_windows(&mut reader, len, 0)
                    .err()
                    .unwrap()
                    .kind(),
                io::ErrorKind::PermissionDenied
            );
        }
    }

    #[test]
    fn transcript_reader_never_reads_omitted_middle() {
        let len = 8 * (HEAD_LOG_BYTES + TAIL_LOG_BYTES);
        let mut reader = FaultyReader {
            data: Cursor::new(vec![b'x'; len as usize]),
            fail_read_at: u64::MAX,
            fail_seek: false,
            bytes_read: 0,
        };
        read_transcript_windows(&mut reader, len, 123).unwrap();
        assert_eq!(
            reader.bytes_read as u64,
            HEAD_LOG_BYTES + TAIL_LOG_BYTES + 1
        );
    }

    #[test]
    fn invalid_transcript_type_fails_db_enrichment_too() {
        let fixture = Fixture::new();
        let conn = fixture.db();
        insert_row(&conn, ID, "Valid DB row", LATEST);
        let path = fixture.transcript(ID, b"");
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(matches!(scan_from(&fixture.0), Err(AgfError::Io(_))));
    }

    #[cfg(unix)]
    #[test]
    fn metadata_failures_are_not_absence_and_invalid_ids_are_never_joined() {
        use std::os::unix::fs::symlink;
        let fixture = Fixture::new();
        let conn = fixture.db();
        let invalid_id = "------------------------------------";
        fs::create_dir(fixture.0.join("brain")).unwrap();
        let loop_path = fixture.0.join("brain").join(invalid_id);
        symlink(&loop_path, &loop_path).unwrap();
        insert_row(&conn, invalid_id, "Ignored invalid ID", "");
        insert_row(&conn, "../escape", "Ignored traversal", "");
        assert!(scan_from(&fixture.0).unwrap().is_empty());
        let root_loop = fixture.0.join("loop");
        symlink(&root_loop, &root_loop).unwrap();
        assert!(matches!(scan_from(&root_loop), Err(AgfError::Io(_))));
        let path = fixture.transcript(ID, b"");
        fs::remove_file(&path).unwrap();
        symlink(&path, &path).unwrap();
        fixture.conversation(ID, false);
        assert!(matches!(scan_from(&fixture.0), Err(AgfError::Io(_))));
    }

    #[test]
    fn orphan_artifacts_require_a_regular_conversation_db_or_wal() {
        let fixture = Fixture::new();
        fixture.transcript(ID, record("USER_INPUT", "Artifact", FIRST));
        assert!(scan_from(&fixture.0).unwrap().is_empty());
        let db = fixture.conversation(ID, false);
        assert_eq!(scan_from(&fixture.0).unwrap().len(), 1);
        fs::remove_file(&db).unwrap();
        fs::create_dir(&db).unwrap();
        assert!(scan_from(&fixture.0).unwrap().is_empty());
        fixture.conversation(ID, true);
        assert_eq!(scan_from(&fixture.0).unwrap().len(), 1);
        assert!(!has_conversation_store(&fixture.0, "../escape").unwrap());
    }

    #[test]
    fn backed_orphans_survive_missing_or_oversized_prompts_without_assuming_lineage() {
        let fixture = Fixture::new();
        fixture.conversation(ID, false);
        for content in [
            String::new(),
            record("PLANNER_RESPONSE", "No prompt", LATEST),
            record(
                "USER_INPUT",
                &"x".repeat((HEAD_LOG_BYTES + TAIL_LOG_BYTES) as usize),
                FIRST,
            ),
        ] {
            let path = fixture.transcript(ID, content);
            let sessions = scan_from(&fixture.0).unwrap();
            assert_eq!(sessions.len(), 1);
            assert!(sessions[0].summaries.is_empty());
            assert!(!sessions[0].interactive);
            assert!(sessions[0].timestamp > 0);
            assert!(
                sessions[0].timestamp == parse_timestamp(LATEST)
                    || sessions[0].timestamp == mtime_ms(&fs::metadata(path).unwrap()).unwrap()
            );
        }
    }

    #[test]
    fn transcript_compaction_rewrites_replace_previous_prompt_and_activity() {
        let fixture = Fixture::new();
        fixture.conversation(ID, false);
        fixture.transcript(ID, record("USER_INPUT", "Old prompt", FIRST));
        assert_eq!(scan_from(&fixture.0).unwrap()[0].summaries, ["Old prompt"]);
        fixture.transcript(ID, record("USER_INPUT", "Compacted prompt", LATEST));
        let sessions = scan_from(&fixture.0).unwrap();
        assert_eq!(sessions[0].summaries, ["Compacted prompt"]);
        assert_eq!(sessions[0].timestamp, parse_timestamp(LATEST));
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_db_brain_and_transcripts_are_errors() {
        use std::os::unix::fs::PermissionsExt;
        struct RestorePermissions(PathBuf, fs::Permissions);
        impl Drop for RestorePermissions {
            fn drop(&mut self) {
                fs::set_permissions(&self.0, self.1.clone()).unwrap();
            }
        }
        let fixture = Fixture::new();
        let conn = fixture.db();
        insert_row(&conn, ID, "Title", LATEST);
        drop(conn);
        let transcript = fixture.transcript(ID, record("USER_INPUT", "Prompt", FIRST));
        for path in [
            fixture.0.join("conversation_summaries.db"),
            fixture.0.join("brain"),
            transcript,
        ] {
            let restore =
                RestorePermissions(path.clone(), fs::metadata(&path).unwrap().permissions());
            fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
            // Root can bypass Unix permission bits; injected I/O tests cover that runner.
            let denied = if path.is_dir() {
                fs::read_dir(&path).is_err()
            } else {
                fs::File::open(&path).is_err()
            };
            if denied {
                assert!(scan_from(&fixture.0).is_err(), "{}", path.display());
            }
            drop(restore);
        }
    }

    #[test]
    fn clean_user_prompt_strips_metadata_tags() {
        let raw = "<USER_REQUEST>\nFix the bug in login\n</USER_REQUEST>\n<ADDITIONAL_METADATA>\ntime\n</ADDITIONAL_METADATA>";
        assert_eq!(clean_user_prompt(raw), "Fix the bug in login");

        let plain = "Simple prompt without tags";
        assert_eq!(clean_user_prompt(plain), "Simple prompt without tags");
    }

    #[test]
    fn parse_iso8601_ms_handles_rfc3339_and_sqlite_formats() {
        let rfc = "2026-09-18T05:31:00Z";
        assert!(parse_iso8601_ms(rfc).unwrap() > 0);

        let sqlite_dt = "2026-09-18 05:31:00.884634923+00:00";
        assert!(parse_iso8601_ms(sqlite_dt).unwrap() > 0);
    }
}
