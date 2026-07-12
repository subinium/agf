use serde_json::Value;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use crate::error::AgfError;
use crate::model::{Agent, Session};
use crate::scanner::{first_line_truncated, project_name_from_path};

const SUMMARY_MAX_CHARS: usize = 120;
const MAX_PARSE_BYTES: usize = 512 * 1024;

pub fn scan() -> Result<Vec<Session>, AgfError> {
    scan_from(&crate::config::yolop_sessions_dir()?)
}

fn scan_from(sessions_dir: &Path) -> Result<Vec<Session>, AgfError> {
    if !sessions_dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut sessions = Vec::new();
    for entry in std::fs::read_dir(sessions_dir)?.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let Some(session_id) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if !valid_session_id(&session_id) {
            continue;
        }
        if let Some(session) = parse_session(&dir, session_id) {
            sessions.push(session);
        }
    }
    sessions.sort_by_key(|s| std::cmp::Reverse(s.timestamp));
    Ok(sessions)
}

fn valid_session_id(id: &str) -> bool {
    id.strip_prefix("session_")
        .is_some_and(|suffix| suffix.len() == 32 && suffix.bytes().all(|b| b.is_ascii_hexdigit()))
}

fn parse_session(dir: &Path, session_id: String) -> Option<Session> {
    let log_path = dir.join("events.jsonl");
    if !log_path.is_file() {
        return None;
    }
    let workspace: Value =
        serde_json::from_slice(&std::fs::read(dir.join("workspace.json")).ok()?).ok()?;
    let project_path = workspace
        .get("active_root")
        .or_else(|| workspace.get("workspace_root"))?
        .as_str()?
        .to_string();
    let mut project_name = workspace
        .get("repo_root")
        .and_then(Value::as_str)
        .map(project_name_from_path)
        .unwrap_or_else(|| project_name_from_path(&project_path));
    if project_name.starts_with("session_")
        && let Some(repo_name) = linked_session_repo_name(dir, Path::new(&project_path))
            .or_else(|| worktree_repo_name(Path::new(&project_path)))
    {
        project_name = repo_name;
    }
    let git_branch = workspace
        .pointer("/worktree/branch")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let worktree = workspace
        .pointer("/worktree/path")
        .and_then(Value::as_str)
        .map(str::to_owned);

    let file = File::open(&log_path).ok()?;
    let mut summaries = Vec::new();
    let mut latest_ts = None;
    let mut bytes_read = 0usize;
    for line in BufReader::new(file).lines() {
        let Ok(line) = line else { continue };
        bytes_read += line.len() + 1;
        if let Ok(event) = serde_json::from_str::<Value>(&line) {
            if event.get("session_id").and_then(Value::as_str) != Some(&session_id) {
                continue;
            }
            if let Some(ts) = event
                .get("ts")
                .and_then(Value::as_str)
                .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
                .map(|ts| ts.timestamp_millis())
            {
                latest_ts = Some(latest_ts.map_or(ts, |current: i64| current.max(ts)));
            }
            if event.get("type").and_then(Value::as_str) == Some("input.message")
                && let Some(message) = event.pointer("/data/message")
                && message.get("role").and_then(Value::as_str) == Some("user")
                && let Some(summary) = message.get("content").and_then(extract_text)
            {
                summaries.push(summary);
            }
        }
        if bytes_read >= MAX_PARSE_BYTES {
            break;
        }
    }
    // The prompt scan is intentionally bounded, so the newest event may be
    // beyond the read window. The log mtime tracks the latest append.
    let timestamp = modified_ms(&log_path).max(latest_ts.unwrap_or(0));

    Some(Session {
        agent: Agent::Yolop,
        session_id,
        project_name,
        project_path,
        summaries,
        timestamp,
        git_branch,
        worktree,
        recap: None,
    })
}

fn extract_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => first_line_truncated(text, SUMMARY_MAX_CHARS),
        Value::Array(items) => items.iter().find_map(extract_text),
        Value::Object(map) => map
            .get("text")
            .and_then(Value::as_str)
            .and_then(|text| first_line_truncated(text, SUMMARY_MAX_CHARS)),
        _ => None,
    }
}

fn modified_ms(path: &Path) -> i64 {
    path.metadata()
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Older Yolop sessions created from inside an existing worktree recorded the
/// worktree itself as `repo_root`. Its `.git` indirection still points at the
/// original repository, which provides a useful project label.
fn worktree_repo_name(project_path: &Path) -> Option<String> {
    let git_file = std::fs::read_to_string(project_path.join(".git")).ok()?;
    let git_dir = Path::new(git_file.trim().strip_prefix("gitdir: ")?);
    let common_git_dir = git_dir
        .ancestors()
        .find(|path| path.file_name().and_then(|name| name.to_str()) == Some(".git"))?;
    Some(project_name_from_path(common_git_dir.parent()?))
}

/// Nested sessions can point at another Yolop session's worktree. That parent
/// session retains the original repo metadata even after its worktree is gone.
fn linked_session_repo_name(session_dir: &Path, project_path: &Path) -> Option<String> {
    let linked_id = project_path.file_name()?.to_str()?;
    if !valid_session_id(linked_id) {
        return None;
    }
    let workspace: Value = serde_json::from_slice(
        &std::fs::read(session_dir.parent()?.join(linked_id).join("workspace.json")).ok()?,
    )
    .ok()?;
    let name = project_name_from_path(workspace.get("repo_root")?.as_str()?);
    (!name.starts_with("session_")).then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const SESSION_ID: &str = "session_019e3db018a17450aba5407af5777237";

    fn temp_root(name: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "agf-yolop-{name}-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn write_session(root: &Path) -> std::path::PathBuf {
        let dir = root.join(SESSION_ID);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("workspace.json"),
            r#"{"active_root":"/tmp/example-wt","repo_root":"/tmp/example","worktree":{"path":"/tmp/example-wt","branch":"feat/yolop","base_ref":"main"}}"#,
        )
        .unwrap();
        fs::write(
            dir.join("events.jsonl"),
            concat!(
                r#"{"type":"input.message","ts":"2026-07-10T12:00:00Z","session_id":"session_019e3db018a17450aba5407af5777237","data":{"message":{"role":"user","content":[{"type":"text","text":"add support for yolop\nwith tests"}]}}}"#,
                "\n",
                r#"{"type":"output.message.completed","ts":"2026-07-10T12:01:00Z","session_id":"session_019e3db018a17450aba5407af5777237","data":{"message":{"role":"assistant","content":"done"}}}"#,
                "\n"
            ),
        )
        .unwrap();
        dir
    }

    #[test]
    fn parse_session_extracts_workspace_prompt_and_timestamp() {
        let root = temp_root("parse");
        let dir = write_session(&root);

        let session = parse_session(&dir, SESSION_ID.to_string()).unwrap();

        assert_eq!(session.agent, Agent::Yolop);
        assert_eq!(session.project_name, "example");
        assert_eq!(session.project_path, "/tmp/example-wt");
        assert_eq!(session.summaries, ["add support for yolop"]);
        assert_eq!(session.git_branch.as_deref(), Some("feat/yolop"));
        assert_eq!(session.worktree.as_deref(), Some("/tmp/example-wt"));
        assert!(session.timestamp >= 1_783_684_860_000);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn scan_ignores_non_session_directories() {
        let root = temp_root("scan");
        write_session(&root);
        fs::create_dir(root.join("outputs")).unwrap();

        let sessions = scan_from(&root).unwrap();

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, SESSION_ID);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn valid_session_id_rejects_path_traversal() {
        assert!(valid_session_id(SESSION_ID));
        assert!(!valid_session_id(
            "../session_019e3db018a17450aba5407af5777237"
        ));
        assert!(!valid_session_id("session_not-hex"));
    }

    #[test]
    fn worktree_repo_name_resolves_original_repository() {
        let root = temp_root("worktree-name");
        let worktree = root.join("session_019f4fec10e370b2be16cca7debb6ab1");
        fs::create_dir(&worktree).unwrap();
        fs::write(
            worktree.join(".git"),
            "gitdir: /Users/example/Projects/everruns/yolop/.git/worktrees/session_test\n",
        )
        .unwrap();

        assert_eq!(worktree_repo_name(&worktree).as_deref(), Some("yolop"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn malformed_input_event_does_not_drop_session() {
        let root = temp_root("malformed-event");
        let dir = write_session(&root);
        fs::write(
            dir.join("events.jsonl"),
            concat!(
                r#"{"type":"input.message","ts":"2026-07-10T12:00:00Z","session_id":"session_019e3db018a17450aba5407af5777237","data":{"message":{"role":"user"}}}"#,
                "\n",
                r#"{"type":"input.message","ts":"2026-07-10T12:01:00Z","session_id":"session_019e3db018a17450aba5407af5777237","data":{"message":{"role":"user","content":"valid prompt"}}}"#,
                "\n"
            ),
        )
        .unwrap();

        let session = parse_session(&dir, SESSION_ID.to_string()).unwrap();

        assert_eq!(session.summaries, ["valid prompt"]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn linked_session_repo_name_uses_parent_metadata() {
        let root = temp_root("linked-name");
        let child = root.join(SESSION_ID);
        fs::create_dir(&child).unwrap();
        let linked_id = "session_019f4fec10e370b2be16cca7debb6ab1";
        let linked = root.join(linked_id);
        fs::create_dir(&linked).unwrap();
        fs::write(
            linked.join("workspace.json"),
            r#"{"active_root":"/tmp/generated","repo_root":"/Users/example/Projects/everruns/yolop"}"#,
        )
        .unwrap();

        let name = linked_session_repo_name(
            &child,
            Path::new("/tmp/worktrees").join(linked_id).as_path(),
        );

        assert_eq!(name.as_deref(), Some("yolop"));
        fs::remove_dir_all(root).unwrap();
    }
}
