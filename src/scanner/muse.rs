use std::collections::HashMap;
use std::io;
use std::path::Path;

use serde_json::Value;

use crate::error::AgfError;
use crate::model::{Agent, Session, normalize_timestamp};

use super::{collapse_whitespace, project_name_from_path, read_head_tail, truncate};

const WINDOW_BYTES: u64 = 256 * 1024;
const MAX_SUMMARIES: usize = 5;

pub fn scan() -> Result<Vec<Session>, AgfError> {
    scan_from(&crate::config::muse_data_dir()?.join("sessions"))
}

fn scan_from(root: &Path) -> Result<Vec<Session>, AgfError> {
    if !root.try_exists()? {
        return Ok(Vec::new());
    }
    if !root.is_dir() {
        return Err(io::Error::other("Muse session store is not a directory").into());
    }
    let mut sessions = HashMap::<String, Session>::new();
    // YYYY/MM/DD/<uuid>/session.jsonl. Do not descend into subagent logs:
    // those streams cannot be resumed as standalone interactive sessions.
    for entry in walkdir::WalkDir::new(root).max_depth(5) {
        let entry = entry.map_err(io::Error::other)?;
        if entry.depth() != 5 || entry.file_name() != "session.jsonl" {
            continue;
        }
        if !entry.file_type().is_file() {
            continue;
        }
        if let Some(session) = parse_session(entry.path())? {
            match sessions.get(&session.session_id) {
                Some(previous) if previous.timestamp >= session.timestamp => {}
                _ => {
                    sessions.insert(session.session_id.clone(), session);
                }
            }
        }
    }
    let mut sessions: Vec<_> = sessions.into_values().collect();
    sessions.sort_by(|a, b| crate::model::compare_sessions(a, b, crate::model::SortMode::Time));
    Ok(sessions)
}

fn is_session_id(id: &str) -> bool {
    id.len() == 36
        && id.bytes().enumerate().all(|(i, b)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
}

fn parse_session(path: &Path) -> Result<Option<Session>, AgfError> {
    let Some(id) = path
        .parent()
        .and_then(Path::file_name)
        .and_then(|s| s.to_str())
    else {
        return Ok(None);
    };
    if !is_session_id(id) {
        return Ok(None);
    }
    let file_time = path
        .metadata()?
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0);
    let content = read_head_tail(path, WINDOW_BYTES, WINDOW_BYTES).ok_or_else(|| {
        io::Error::other(format!("failed to read Muse session {}", path.display()))
    })?;
    let mut workspace = None;
    let mut summaries = Vec::new();
    let mut recap = None;
    let mut timestamp = 0;
    for line in content.head.lines().chain(content.tail.lines()) {
        let Ok(record) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        // Ignore unrelated records and permission transaction frames. Never
        // infer a resume identity solely from a directory name.
        if record.pointer("/stream/kind").and_then(Value::as_str) != Some("session") {
            continue;
        }
        if record.pointer("/stream/id").and_then(Value::as_str) != Some(id) {
            return Ok(None);
        }
        if let Some(micros) = record.get("recorded_at").and_then(Value::as_i64) {
            // Muse's on-disk envelope uses Unix microseconds, unlike AGF.
            timestamp = timestamp.max(normalize_timestamp(micros / 1000, 0));
        }
        match record.get("payload_type").and_then(Value::as_str) {
            Some("runtime.session.metadata") => {
                // Both forms are recognized by Muse's bundled session reader.
                let metadata = record
                    .pointer("/payload/record")
                    .unwrap_or(&record["payload"]);
                workspace = metadata
                    .get("workspace_root")
                    .and_then(Value::as_str)
                    .filter(|s| !s.trim().is_empty())
                    .map(str::to_string);
            }
            Some("runtime.session")
                if record.pointer("/payload/kind").and_then(Value::as_str) == Some("run") =>
            {
                let event = &record["payload"]["event"];
                match event.get("kind").and_then(Value::as_str) {
                    Some("started") => {
                        if let Some(prompt) = preview(event.get("prompt"))
                            && !summaries.contains(&prompt)
                        {
                            summaries.push(prompt);
                            if summaries.len() > MAX_SUMMARIES {
                                summaries.remove(0);
                            }
                        }
                    }
                    Some("assistant_message_committed") => {
                        if let Some(text) = preview(event.get("text")) {
                            recap = Some(text);
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    let Some(workspace) = workspace else {
        return Ok(None);
    };
    summaries.reverse();
    Ok(Some(Session {
        agent: Agent::Muse,
        session_id: id.to_string(),
        project_name: project_name_from_path(&workspace),
        project_path: workspace,
        summaries,
        timestamp: normalize_timestamp(timestamp, file_time),
        git_branch: None,
        worktree: None,
        recap,
        // Retained root sessions (including exec/serve) support muse resume;
        // the log metadata does not reliably identify the launch surface.
        interactive: true,
    }))
}

fn preview(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(collapse_whitespace)
        .filter(|text| !text.is_empty())
        .map(|text| truncate(&text, 200))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    const ID: &str = "0198f0aa-1111-7000-8000-0000000000aa";
    const FIXTURE: &str = include_str!("../../tests/fixtures/muse/session.jsonl");

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "agf-muse-scan-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&root).unwrap();
            Self(root)
        }
        fn write(&self, contents: &str) -> PathBuf {
            let path = self.0.join("2026/09/01").join(ID).join("session.jsonl");
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, contents).unwrap();
            path
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn event(payload_type: &str, payload: Value, timestamp: i64) -> String {
        format!(
            "{}\n",
            json!({"stream":{"kind":"session","id":ID},"recorded_at":timestamp,"payload_type":payload_type,"payload":payload})
        )
    }

    #[test]
    fn native_log_metadata_prompts_recap_and_microsecond_activity() {
        let fixture = Fixture::new();
        let path = fixture.write(FIXTURE);
        let before = fs::read(&path).unwrap();
        let sessions = scan_from(&fixture.0).unwrap();
        assert_eq!(sessions.len(), 1);
        let session = &sessions[0];
        assert_eq!(session.agent, Agent::Muse);
        assert_eq!(session.session_id, ID);
        assert_eq!(session.project_path, "/work/muse-project");
        assert_eq!(session.project_name, "muse-project");
        assert_eq!(session.summaries, ["AGF fixture prompt 한국어"]);
        assert_eq!(
            session.recap.as_deref(),
            Some("echo: AGF fixture prompt 한국어")
        );
        assert_eq!(session.timestamp, 1_788_220_803_000);
        assert!(session.interactive);
        assert_eq!(fs::read(path).unwrap(), before);
    }

    #[test]
    fn skips_malformed_lines_child_logs_and_untrusted_identities() {
        let fixture = Fixture::new();
        let path = fixture.write(&format!("{{bad json\n\n{FIXTURE}\n{{\"partial\":"));
        let child = path.parent().unwrap().join("subagent").join(ID);
        fs::create_dir_all(&child).unwrap();
        fs::write(child.join("session.jsonl"), FIXTURE).unwrap();
        assert_eq!(scan_from(&fixture.0).unwrap().len(), 1);
        fs::write(
            &path,
            FIXTURE.replace(ID, "0198f0aa-1111-7000-8000-0000000000bb"),
        )
        .unwrap();
        assert!(scan_from(&fixture.0).unwrap().is_empty());
        for id in [
            "--last",
            "../session",
            "x",
            "g198f0aa-1111-7000-8000-0000000000aa",
        ] {
            assert!(!is_session_id(id));
        }
        fs::write(
            &path,
            event(
                "runtime.session",
                json!({"kind":"run","event":{"kind":"started","prompt":"no metadata"}}),
                1_788_220_800_000_000,
            ),
        )
        .unwrap();
        assert!(scan_from(&fixture.0).unwrap().is_empty());
    }

    #[test]
    fn bounded_tail_recovers_latest_workspace_prompt_and_activity() {
        let fixture = Fixture::new();
        let mut content = FIXTURE.to_string();
        content.push_str(&event(
            "runtime.session",
            json!({"kind":"run","event":{"kind":"tool_result","text":"한".repeat(400_000)}}),
            1_788_220_804_000_000,
        ));
        content.push_str(&event(
            "runtime.session.metadata",
            json!({"kind":"metadata","record":{"workspace_root":"/work/renamed"}}),
            1_788_220_805_000_000,
        ));
        for i in 0..8 {
            content.push_str(&event("runtime.session", json!({"kind":"run","event":{"kind":"started","prompt":format!("  Follow-up\n {i}  ")}}), 1_788_220_806_000_000 + i * 1_000_000));
        }
        content.push_str(&event(
            "runtime.session",
            json!({"kind":"task","event":{"kind":"started","prompt":"not a user prompt"}}),
            i64::MAX,
        ));
        let path = fixture.write(&content);
        assert!(
            read_head_tail(&path, WINDOW_BYTES, WINDOW_BYTES)
                .unwrap()
                .truncated
        );
        let session = scan_from(&fixture.0).unwrap().pop().unwrap();
        assert_eq!(session.project_path, "/work/renamed");
        assert_eq!(session.timestamp, 1_788_220_813_000);
        assert_eq!(
            session.summaries,
            [
                "Follow-up 7",
                "Follow-up 6",
                "Follow-up 5",
                "Follow-up 4",
                "Follow-up 3"
            ]
        );
    }

    #[test]
    fn missing_store_is_empty_but_io_failures_are_errors() {
        let fixture = Fixture::new();
        assert!(scan_from(&fixture.0.join("missing")).unwrap().is_empty());
        let path = fixture.write(FIXTURE);
        assert!(scan_from(&path).is_err());
        fs::write(&path, b"\xff\xfe").unwrap();
        assert!(scan_from(&fixture.0).is_err());
    }

    #[test]
    fn duplicate_identity_keeps_the_latest_activity_and_missing_time_uses_mtime() {
        let fixture = Fixture::new();
        let path = fixture.write(FIXTURE);
        let duplicate = fixture.0.join("2026/09/02").join(ID);
        fs::create_dir_all(&duplicate).unwrap();
        fs::write(
            duplicate.join("session.jsonl"),
            FIXTURE.replace("1788220803000000", "1788220809000000"),
        )
        .unwrap();
        let sessions = scan_from(&fixture.0).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].timestamp, 1_788_220_809_000);
        fs::write(
            &path,
            event(
                "runtime.session.metadata",
                json!({"workspace_root":"/work/project"}),
                0,
            ),
        )
        .unwrap();
        let session = parse_session(&path).unwrap().unwrap();
        assert!(session.timestamp > 1_788_220_809_000);
    }
}
