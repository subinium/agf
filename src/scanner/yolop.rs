use serde_json::Value;
use std::path::Path;

use crate::error::AgfError;
use crate::model::{Agent, Session};
use crate::scanner::{
    collapse_whitespace, first_line_truncated, project_name_from_path, read_head_tail,
};

const SUMMARY_MAX_CHARS: usize = 120;
const EVENT_HEAD_BYTES: u64 = 512 * 1024;
const EVENT_TAIL_BYTES: u64 = 64 * 1024;

pub fn scan() -> Result<Vec<Session>, AgfError> {
    scan_from(&crate::config::yolop_sessions_dir()?)
}

fn scan_from(sessions_dir: &Path) -> Result<Vec<Session>, AgfError> {
    if !sessions_dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut sessions = Vec::new();
    for entry in std::fs::read_dir(sessions_dir)? {
        let entry = entry?;
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
        if let Some(session) = parse_session(&dir, session_id)? {
            sessions.push(session);
        }
    }
    sessions.sort_by_key(|s| std::cmp::Reverse(s.timestamp));
    Ok(sessions)
}

/// A Yolop session directory name: `session_` followed by 32 hex digits.
///
/// Also used by `delete::delete_yolop_session_from`, which joins the id onto
/// the sessions directory and calls `remove_dir_all` — the id reaching that
/// call can come from the on-disk cache rather than from this scanner, so the
/// shape gets re-checked there rather than assumed.
pub(crate) fn valid_session_id(id: &str) -> bool {
    id.strip_prefix("session_")
        .is_some_and(|suffix| suffix.len() == 32 && suffix.bytes().all(|b| b.is_ascii_hexdigit()))
}

fn parse_session(dir: &Path, session_id: String) -> Result<Option<Session>, AgfError> {
    let log_path = dir.join("events.jsonl");
    match std::fs::metadata(&log_path) {
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => return Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    }
    let workspace: Value = serde_json::from_slice(&std::fs::read(dir.join("workspace.json"))?)?;
    let project_path = workspace
        .get("active_root")
        .and_then(Value::as_str)
        .or_else(|| workspace.get("workspace_root").and_then(Value::as_str));
    let Some(project_path) = project_path else {
        return Ok(None);
    };
    let project_path = project_path.to_string();
    let mut project_name = workspace
        .get("canonical_repo_root")
        .and_then(Value::as_str)
        .filter(|root| !root.is_empty())
        .or_else(|| workspace.get("repo_root").and_then(Value::as_str))
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
    let worktree = worktree_label(&workspace);
    let metadata_title = workspace.get("title").and_then(Value::as_str);
    let metadata_summary = workspace.get("summary").and_then(Value::as_str);
    let metadata_timestamp = workspace
        .get("updated_at")
        .and_then(Value::as_str)
        .and_then(parse_timestamp)
        .or_else(|| {
            workspace
                .get("created_at")
                .and_then(Value::as_str)
                .and_then(parse_timestamp)
        })
        .unwrap_or(0);

    // Failed refreshes preserve the prior stale cache. Metadata-only output
    // would instead replace it and be marked fresh despite the missing log.
    let events =
        read_head_tail(&log_path, EVENT_HEAD_BYTES, EVENT_TAIL_BYTES).ok_or_else(|| {
            std::io::Error::other(format!(
                "failed to read Yolop event log {}",
                log_path.display()
            ))
        })?;
    let mut prompt_summaries = Vec::new();
    let mut event_title = None;
    let mut latest_ts = None;
    for line in events.head.lines().chain(events.tail.lines()) {
        if let Ok(event) = serde_json::from_str::<Value>(line) {
            if event.get("session_id").and_then(Value::as_str) != Some(&session_id) {
                continue;
            }
            if let Some(ts) = event
                .get("ts")
                .and_then(Value::as_str)
                .and_then(parse_timestamp)
            {
                latest_ts = Some(latest_ts.map_or(ts, |current: i64| current.max(ts)));
            }
            if event.get("type").and_then(Value::as_str) == Some("session.title.updated")
                && let Some(title) = event.pointer("/data/title").and_then(Value::as_str)
            {
                event_title = Some(title.to_string());
            }
            if event.get("type").and_then(Value::as_str) == Some("input.message")
                && let Some(message) = event.pointer("/data/message")
                && message.get("role").and_then(Value::as_str) == Some("user")
                && let Some(summary) = message.get("content").and_then(extract_text)
            {
                push_unique_summary(&mut prompt_summaries, &summary);
            }
        }
    }
    let mut summaries = Vec::new();
    if let Some(title) = event_title
        .as_deref()
        .filter(|title| !title.trim().is_empty())
        .or(metadata_title)
    {
        push_unique_summary(&mut summaries, title);
    }
    if let Some(summary) = metadata_summary {
        push_unique_summary(&mut summaries, summary);
    }
    for summary in prompt_summaries {
        push_unique_summary(&mut summaries, &summary);
    }

    // Explicit metadata is useful for idle sessions, while the log mtime and
    // event timestamps keep active sessions fresh between metadata writes.
    // Preserve the raw durable maximum until the common scan boundary. That
    // layer both clamps implausible future values and prevents their fallback
    // from being cached as fresh, allowing a small clock skew to become valid
    // later even when the source file itself does not change.
    let timestamp = [
        metadata_timestamp,
        modified_ms(&log_path),
        latest_ts.unwrap_or(0),
    ]
    .into_iter()
    .max()
    .unwrap_or(0);

    Ok(Some(Session {
        agent: Agent::Yolop,
        session_id,
        project_name,
        project_path,
        summaries,
        timestamp,
        git_branch,
        worktree,
        recap: None,
        interactive: true,
    }))
}

fn parse_timestamp(timestamp: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(timestamp)
        .ok()
        .map(|timestamp| timestamp.timestamp_millis())
}

fn push_unique_summary(summaries: &mut Vec<String>, summary: &str) {
    let summary = collapse_whitespace(summary);
    if !summary.is_empty() && !summaries.contains(&summary) {
        summaries.push(summary);
    }
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

fn worktree_label(workspace: &Value) -> Option<String> {
    let worktree = workspace.get("worktree")?;
    if let Some(slug) = worktree
        .get("slug")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|slug| !slug.is_empty())
    {
        return Some(slug.to_string());
    }

    let path_label = worktree
        .get("path")
        .and_then(Value::as_str)
        .map(project_name_from_path)
        .filter(|label| !label.starts_with("session_"));
    path_label.or_else(|| {
        worktree
            .get("branch")
            .and_then(Value::as_str)
            .map(str::to_owned)
    })
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
    let root = workspace
        .get("canonical_repo_root")
        .and_then(Value::as_str)
        .filter(|root| !root.is_empty())
        .or_else(|| workspace.get("repo_root").and_then(Value::as_str))?;
    let name = project_name_from_path(root);
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

        let session = parse_session(&dir, SESSION_ID.to_string())
            .unwrap()
            .unwrap();

        assert_eq!(session.agent, Agent::Yolop);
        assert_eq!(session.project_name, "example");
        assert_eq!(session.project_path, "/tmp/example-wt");
        assert_eq!(session.summaries, ["add support for yolop"]);
        assert_eq!(session.git_branch.as_deref(), Some("feat/yolop"));
        assert_eq!(session.worktree.as_deref(), Some("example-wt"));
        assert!(session.timestamp >= 1_783_684_860_000);
        fs::remove_dir_all(root).unwrap();
    }

    /// Backdate a file's mtime so the metadata timestamp is the newest
    /// candidate (a freshly written file would otherwise carry "now").
    fn backdate(path: &Path, unix_secs: u64) {
        fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(unix_secs))
            .unwrap();
    }

    #[test]
    fn current_metadata_supplies_title_project_time_and_worktree_slug() {
        let root = temp_root("current-metadata");
        let dir = root.join(SESSION_ID);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("workspace.json"),
            r#"{
                "active_root":"/tmp/yolop/worktrees/session_generated",
                "repo_root":"/tmp/yolop/worktrees/session_generated",
                "canonical_repo_root":"/Users/example/Projects/everruns/yolop",
                "title":"Release   Yolop\nand Tuika",
                "summary":"Prepare and publish both releases",
                "created_at":"2026-07-10T11:00:00Z",
                "updated_at":"2026-07-30T12:00:00Z",
                "worktree":{
                    "path":"/tmp/yolop/worktrees/session_generated",
                    "branch":"feat/release",
                    "slug":"release-new-version"
                }
            }"#,
        )
        .unwrap();
        fs::write(
            dir.join("events.jsonl"),
            concat!(
                r#"{"type":"input.message","ts":"2026-07-10T12:00:00Z","session_id":"session_019e3db018a17450aba5407af5777237","data":{"message":{"role":"user","content":"Prepare and publish both releases"}}}"#,
                "\n",
                r#"{"type":"input.message","ts":"2026-07-10T12:01:00Z","session_id":"session_019e3db018a17450aba5407af5777237","data":{"message":{"role":"user","content":"Verify the release artifacts"}}}"#,
                "\n"
            ),
        )
        .unwrap();
        backdate(&dir.join("events.jsonl"), 1_700_000_000);

        let session = parse_session(&dir, SESSION_ID.to_string())
            .unwrap()
            .unwrap();

        assert_eq!(session.project_name, "yolop");
        assert_eq!(
            session.summaries,
            [
                "Release Yolop and Tuika",
                "Prepare and publish both releases",
                "Verify the release artifacts"
            ]
        );
        assert_eq!(session.worktree.as_deref(), Some("release-new-version"));
        // `updated_at` is newer than both the (backdated) log mtime and every
        // event `ts`, so it wins.
        assert_eq!(session.timestamp, rfc3339_ms("2026-07-30T12:00:00Z"));
        fs::remove_dir_all(root).unwrap();
    }

    fn rfc3339_ms(ts: &str) -> i64 {
        chrono::DateTime::parse_from_rfc3339(ts)
            .unwrap()
            .timestamp_millis()
    }

    /// Regression: an `updated_at` far in the future used to win the `max()`
    /// outright and pin the session above every real session forever.
    #[test]
    fn implausible_future_metadata_is_preserved_for_common_normalization() {
        let root = temp_root("future-metadata");
        let dir = root.join(SESSION_ID);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("workspace.json"),
            r#"{"active_root":"/tmp/example","repo_root":"/tmp/example","updated_at":"2087-01-01T00:00:00Z"}"#,
        )
        .unwrap();
        fs::write(dir.join("events.jsonl"), b"").unwrap();
        backdate(&dir.join("events.jsonl"), 1_700_000_000);

        let session = parse_session(&dir, SESSION_ID.to_string())
            .unwrap()
            .unwrap();

        // The scanner boundary owns plausibility and cacheability. Keeping the
        // raw value here lets it detect that normalization occurred.
        assert_eq!(session.timestamp, rfc3339_ms("2087-01-01T00:00:00Z"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unreadable_event_log_fails_refresh_instead_of_yielding_partial_metadata() {
        let root = temp_root("unreadable-log");
        let dir = root.join(SESSION_ID);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("workspace.json"),
            r#"{"active_root":"/tmp/example","repo_root":"/tmp/example","title":"Still here"}"#,
        )
        .unwrap();
        // Invalid UTF-8 makes the bounded reader fail even for privileged CI
        // users. It must not bless metadata-only output as an authoritative scan.
        fs::write(
            dir.join("events.jsonl"),
            b"\xff\xfe not utf-8 and no newline",
        )
        .unwrap();

        assert!(matches!(
            parse_session(&dir, SESSION_ID.to_string()),
            Err(AgfError::Io(_))
        ));
        assert!(matches!(scan_from(&root), Err(AgfError::Io(_))));
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn real_event_log_metadata_error_fails_scan_even_with_a_healthy_sibling() {
        let root = temp_root("io-error");
        write_session(&root);
        let broken_id = "session_019e3db018a17450aba5407af5777238";
        let broken = root.join(broken_id);
        fs::create_dir_all(&broken).unwrap();
        fs::copy(
            root.join(SESSION_ID).join("workspace.json"),
            broken.join("workspace.json"),
        )
        .unwrap();
        // A symlink loop produces a real metadata I/O error without relying
        // on permission bits, timing, or the privileges of the test runner.
        std::os::unix::fs::symlink("events.jsonl", broken.join("events.jsonl")).unwrap();
        let expected = fs::metadata(broken.join("events.jsonl")).unwrap_err();
        assert!(matches!(
            scan_from(&root),
            Err(AgfError::Io(error)) if error.raw_os_error() == expected.raw_os_error()
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn failed_refresh_preserves_stale_cache_until_a_successful_retry() {
        const CHILD_ENV: &str = "AGF_YOLOP_CACHE_FIXTURE_CHILD";
        if std::env::var_os(CHILD_ENV).is_none() {
            use std::os::unix::fs::PermissionsExt;
            let home = temp_root("cache-home");
            fs::create_dir(home.join("bin")).unwrap();
            let executable = home.join("bin/yolop");
            fs::write(&executable, b"").unwrap();
            fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "scanner::yolop::tests::failed_refresh_preserves_stale_cache_until_a_successful_retry",
                    "--nocapture",
                ])
                .env_clear()
                .env("HOME", &home)
                .env("PATH", home.join("bin"))
                .env("XDG_CONFIG_HOME", home.join("config"))
                .env("XDG_DATA_HOME", home.join("data"))
                .env("XDG_CACHE_HOME", home.join("cache"))
                .env(CHILD_ENV, "1")
                .current_dir(&home)
                .output()
                .unwrap();
            fs::remove_dir_all(home).unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }

        let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap());
        let root = crate::config::yolop_sessions_dir().unwrap();
        let cache_path = dirs::cache_dir().unwrap().join("agf/sessions.json");
        assert!(root.starts_with(&home));
        assert!(cache_path.starts_with(&home));
        let dir = write_session(&root);
        let initial = crate::scanner::scan_agent_consistent(Agent::Yolop).unwrap();
        let observed =
            std::collections::HashMap::from([(Agent::Yolop, initial.fingerprint.unwrap())]);
        crate::cache::write_cache(
            &initial.sessions,
            &Default::default(),
            &Default::default(),
            &observed,
        );
        let before: Value = serde_json::from_slice(&fs::read(&cache_path).unwrap()).unwrap();
        assert!(!before["agents"]["Yolop"].is_null());

        fs::write(dir.join("events.jsonl"), b"\xff unreadable update").unwrap();
        let (cached, stale) = crate::cache::load_cache();
        assert!(stale.contains(&Agent::Yolop));
        assert_eq!(cached.len(), 1);
        let refresh = crate::scanner::scan_agent_consistent(Agent::Yolop);
        assert!(refresh.is_err());
        let (tx, rx) = std::sync::mpsc::channel();
        let mut app = crate::tui::App::new(
            cached,
            None,
            5,
            false,
            None,
            Vec::new(),
            crate::settings::Settings::default(),
            Some(rx),
            std::collections::HashSet::from([Agent::Yolop]),
        );
        tx.send(crate::cache::ScanResult {
            agent: Agent::Yolop,
            sessions: refresh,
        })
        .unwrap();
        app.ingest_scan_results();
        assert_eq!(app.sessions.len(), 1);
        assert_eq!(app.sessions[0].summaries, initial.sessions[0].summaries);
        assert!(app.failed_agents.contains(&Agent::Yolop));
        assert!(!app.scan_fingerprints.contains_key(&Agent::Yolop));
        crate::cache::write_cache(
            &app.sessions,
            &app.failed_agents,
            &Default::default(),
            &app.scan_fingerprints,
        );
        let after: Value = serde_json::from_slice(&fs::read(&cache_path).unwrap()).unwrap();
        assert_eq!(after["agents"]["Yolop"], before["agents"]["Yolop"]);
        assert!(crate::cache::load_cache().1.contains(&Agent::Yolop));

        write_session(&root);
        let retry = crate::scanner::scan_agent_consistent(Agent::Yolop).unwrap();
        assert!(retry.fingerprint.is_some());
        tx.send(crate::cache::ScanResult {
            agent: Agent::Yolop,
            sessions: Ok(retry),
        })
        .unwrap();
        app.ingest_scan_results();
        assert!(!app.failed_agents.contains(&Agent::Yolop));
        crate::cache::write_cache(
            &app.sessions,
            &app.failed_agents,
            &Default::default(),
            &app.scan_fingerprints,
        );
        assert!(!crate::cache::load_cache().1.contains(&Agent::Yolop));
    }

    #[test]
    fn latest_title_event_overrides_stale_metadata_title() {
        let root = temp_root("title-event");
        let dir = write_session(&root);
        let workspace_path = dir.join("workspace.json");
        let mut workspace: Value =
            serde_json::from_slice(&fs::read(&workspace_path).unwrap()).unwrap();
        workspace["title"] = Value::String("Old generated title".to_string());
        fs::write(&workspace_path, serde_json::to_vec(&workspace).unwrap()).unwrap();
        fs::write(
            dir.join("events.jsonl"),
            concat!(
                r#"{"type":"session.title.updated","ts":"2026-07-10T12:02:00Z","session_id":"session_019e3db018a17450aba5407af5777237","data":{"title":"First generated title"}}"#,
                "\n",
                r#"{"type":"session.title.updated","ts":"2026-07-10T12:03:00Z","session_id":"session_019e3db018a17450aba5407af5777237","data":{"title":"Useful current title"}}"#,
                "\n",
                r#"{"type":"input.message","ts":"2026-07-10T12:04:00Z","session_id":"session_019e3db018a17450aba5407af5777237","data":{"message":{"role":"user","content":"original request"}}}"#,
                "\n"
            ),
        )
        .unwrap();

        let session = parse_session(&dir, SESSION_ID.to_string())
            .unwrap()
            .unwrap();

        assert_eq!(
            session.summaries,
            ["Useful current title", "original request"]
        );
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

        let session = parse_session(&dir, SESSION_ID.to_string())
            .unwrap()
            .unwrap();

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
