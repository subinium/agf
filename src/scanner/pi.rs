use serde::Deserialize;
use walkdir::WalkDir;

use crate::error::AgfError;
use crate::model::{Agent, Session};
use crate::scanner::read_first_line;

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

        // Read just the first line rather than loading the whole JSONL.
        let first_line = match read_first_line(path) {
            Some(line) => line,
            None => continue,
        };

        let header: PiSessionHeader = match serde_json::from_str(first_line.trim()) {
            Ok(h) => h,
            Err(_) => continue,
        };

        if header.entry_type.as_deref() != Some("session") {
            continue;
        }

        let session_id = match header.id {
            Some(id) => id,
            None => continue,
        };

        let cwd = match header.cwd {
            Some(cwd) => cwd,
            None => continue,
        };

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

        sessions.push(Session {
            agent: Agent::Pi,
            session_id,
            project_name,
            project_path: cwd,
            summaries: Vec::new(),
            timestamp,
            git_branch: None,
            worktree: None,
            recap: None,
        });
    }

    // Sort by timestamp desc. Pi supports resuming a specific session by id,
    // so keep older sessions from the same project selectable.
    sessions.sort_by_key(|s| std::cmp::Reverse(s.timestamp));

    Ok(sessions)
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
}
