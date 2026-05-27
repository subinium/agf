use std::path::{Path, PathBuf};

use rusqlite::Connection;
use walkdir::WalkDir;

use crate::error::AgfError;
use crate::model::{Agent, Session};

use super::truncate;

pub fn scan() -> Result<Vec<Session>, AgfError> {
    scan_from(&crate::config::cursor_dir()?)
}

fn scan_from(cursor_dir: &Path) -> Result<Vec<Session>, AgfError> {
    let projects_dir = cursor_dir.join("projects");

    if !projects_dir.exists() {
        return Ok(Vec::new());
    }

    let chats_dir = cursor_dir.join("chats");
    let mut sessions = Vec::new();

    // Walk ~/.cursor/projects/*/agent-transcripts/*/<session>.jsonl
    for entry in WalkDir::new(&projects_dir)
        .min_depth(4)
        .max_depth(4)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();

        if !path.is_file() || path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }

        // Grandparent must be "agent-transcripts"
        let agent_transcripts_dir = match path
            .parent()
            .and_then(|p| p.parent())
            .filter(|p| p.file_name().and_then(|n| n.to_str()) == Some("agent-transcripts"))
        {
            Some(p) => p,
            None => continue,
        };

        let session_id = match path.file_stem().and_then(|n| n.to_str()) {
            Some(id) => id.to_string(),
            None => continue,
        };

        // Parent of agent-transcripts is the dash-encoded project path
        let encoded_dir = match agent_transcripts_dir
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
        {
            Some(name) => name.to_string(),
            None => continue,
        };

        // Skip temp directories
        if encoded_dir.starts_with("var-folders") {
            continue;
        }

        let project_path = match decode_dash_path(&encoded_dir) {
            Some(p) => p,
            None => continue,
        };

        let project_name = project_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        let project_path_str = project_path.to_string_lossy().to_string();

        // Try to get metadata from store.db
        let meta = if chats_dir.exists() {
            find_store_db_metadata(&chats_dir, &session_id)
        } else {
            None
        };

        let (summary, timestamp) = match meta {
            Some(m) => (m.name, m.created_at),
            None => {
                // Fall back to transcript file mtime
                let mtime = path
                    .metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0);
                (None, mtime)
            }
        };

        sessions.push(Session {
            agent: Agent::CursorAgent,
            session_id,
            project_name,
            project_path: project_path_str,
            summaries: summary.into_iter().collect(),
            timestamp,
            git_branch: None,
            worktree: None,
            recap: None,
        });
    }

    Ok(sessions)
}

struct StoreMeta {
    name: Option<String>,
    created_at: i64,
}

/// Search ~/.cursor/chats/*/<session_id>/store.db and extract metadata.
fn find_store_db_metadata(chats_dir: &Path, session_id: &str) -> Option<StoreMeta> {
    // chats_dir contains workspace-hash directories
    let read_dir = std::fs::read_dir(chats_dir).ok()?;
    for workspace_entry in read_dir.filter_map(|e| e.ok()) {
        let store_path = workspace_entry.path().join(session_id).join("store.db");
        if store_path.exists() {
            return read_store_db(&store_path);
        }
    }
    None
}

/// Read the `meta` table from store.db, hex-decode the value, and parse as JSON.
fn read_store_db(store_path: &Path) -> Option<StoreMeta> {
    let conn = Connection::open_with_flags(
        store_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;

    // The meta table typically has key-value rows with hex-encoded JSON
    let mut stmt = conn
        .prepare("SELECT value FROM cursorDiskKV WHERE key = 'composerData'")
        .ok()?;
    let hex_value: String = stmt.query_row([], |row| row.get(0)).ok()?;

    let json_bytes = hex_decode(&hex_value)?;
    let json_str = std::str::from_utf8(&json_bytes).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(json_str).ok()?;

    let name = parsed
        .get("name")
        .and_then(|v| v.as_str())
        .map(|s| truncate(s, 100));

    // createdAt is in milliseconds
    let created_at = parsed
        .get("createdAt")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    Some(StoreMeta { name, created_at })
}

/// Decode a hex-encoded string to bytes.
fn hex_decode(hex: &str) -> Option<Vec<u8>> {
    if !hex.len().is_multiple_of(2) {
        return None;
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
        .collect()
}

/// Backtracking path decoder: dash-encoded path -> filesystem path.
/// e.g. "Users-subinium-Desktop-my-project" -> /Users/subinium/Desktop/my-project
fn decode_dash_path(encoded: &str) -> Option<PathBuf> {
    let parts: Vec<&str> = encoded.split('-').collect();
    solve(&parts, 0, Path::new("/"))
}

fn solve(parts: &[&str], idx: usize, current: &Path) -> Option<PathBuf> {
    if idx >= parts.len() {
        return if current.is_dir() {
            Some(current.to_path_buf())
        } else {
            None
        };
    }
    // Try longest segment first (greedy — fewer filesystem checks)
    for end in (idx + 1..=parts.len()).rev() {
        let segment = parts[idx..end].join("-");
        let candidate = current.join(&segment);
        if end == parts.len() {
            if candidate.is_dir() {
                return Some(candidate);
            }
        } else if candidate.is_dir() {
            if let Some(result) = solve(parts, end, &candidate) {
                return Some(result);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;

    /// Build a (cursor_dir, real_project_dir, dash_encoded_slug) fixture.
    ///
    /// Both directories are created under `temp_dir()`. The project dir is
    /// canonicalized so macOS `/tmp` → `/private/tmp` is handled correctly,
    /// and its path components are joined with `-` to produce the slug that
    /// Cursor writes as the project folder name under `projects/`.
    ///
    /// Labels must be alphanumeric only (no dashes) to avoid ambiguity in
    /// the backtracking decoder.
    fn make_fixture(label: &str) -> (PathBuf, PathBuf, String) {
        let pid = std::process::id();
        let cursor_dir = std::env::temp_dir().join(format!("agfcursor{pid}{label}"));
        let proj_dir = std::env::temp_dir().join(format!("agfproj{pid}{label}"));

        let _ = fs::remove_dir_all(&cursor_dir);
        let _ = fs::remove_dir_all(&proj_dir);
        fs::create_dir_all(cursor_dir.join("projects")).unwrap();
        fs::create_dir_all(&proj_dir).unwrap();

        let real_proj = proj_dir.canonicalize().unwrap();
        let encoded = real_proj
            .to_str()
            .unwrap()
            .trim_start_matches('/')
            .replace('/', "-");

        (cursor_dir, real_proj, encoded)
    }

    /// Write an empty `.jsonl` at the correct depth:
    /// `<cursor_dir>/projects/<slug>/agent-transcripts/<uuid>/<uuid>.jsonl`
    fn place_session(cursor_dir: &Path, slug: &str, uuid: &str) {
        let session_dir = cursor_dir
            .join("projects")
            .join(slug)
            .join("agent-transcripts")
            .join(uuid);
        fs::create_dir_all(&session_dir).unwrap();
        let mut f = fs::File::create(session_dir.join(format!("{uuid}.jsonl"))).unwrap();
        f.write_all(b"{}\n").unwrap();
    }

    #[test]
    fn scan_from_finds_jsonl_session() {
        let (cursor_dir, real_proj, encoded) = make_fixture("finds");
        let uuid = "aaaaaaaa0000000000000000000000a1";

        place_session(&cursor_dir, &encoded, uuid);

        let sessions = scan_from(&cursor_dir).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, uuid);
        assert_eq!(sessions[0].project_path, real_proj.to_str().unwrap());
        assert_eq!(sessions[0].agent, Agent::CursorAgent);

        let _ = fs::remove_dir_all(&cursor_dir);
        let _ = fs::remove_dir_all(&real_proj);
    }

    #[test]
    fn scan_from_ignores_txt_at_wrong_depth() {
        let (cursor_dir, real_proj, encoded) = make_fixture("txtigno");

        // Old (wrong) format: .txt directly inside agent-transcripts/
        let transcripts_dir = cursor_dir
            .join("projects")
            .join(&encoded)
            .join("agent-transcripts");
        fs::create_dir_all(&transcripts_dir).unwrap();
        fs::write(transcripts_dir.join("session.txt"), b"").unwrap();

        assert!(scan_from(&cursor_dir).unwrap().is_empty());

        let _ = fs::remove_dir_all(&cursor_dir);
        let _ = fs::remove_dir_all(&real_proj);
    }

    #[test]
    fn scan_from_ignores_jsonl_directly_in_agent_transcripts() {
        let (cursor_dir, real_proj, encoded) = make_fixture("depth");

        // .jsonl at depth 3 instead of 4 — must be ignored
        let transcripts_dir = cursor_dir
            .join("projects")
            .join(&encoded)
            .join("agent-transcripts");
        fs::create_dir_all(&transcripts_dir).unwrap();
        fs::write(transcripts_dir.join("session.jsonl"), b"{}").unwrap();

        assert!(scan_from(&cursor_dir).unwrap().is_empty());

        let _ = fs::remove_dir_all(&cursor_dir);
        let _ = fs::remove_dir_all(&real_proj);
    }

    #[test]
    fn scan_from_skips_var_folders_encoded_dirs() {
        let (cursor_dir, real_proj, _) = make_fixture("varfold");
        let uuid = "aaaaaaaa0000000000000000000000a2";

        // Use a slug that starts with "var-folders" — must be skipped even if
        // the session file is otherwise valid.
        place_session(&cursor_dir, "var-folders-xx-yyyyyyyy", uuid);

        assert!(scan_from(&cursor_dir).unwrap().is_empty());

        let _ = fs::remove_dir_all(&cursor_dir);
        let _ = fs::remove_dir_all(&real_proj);
    }

    #[test]
    fn decode_dash_path_resolves_existing_directory() {
        let pid = std::process::id();
        let dir = std::env::temp_dir().join(format!("agfdecode{pid}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let real = dir.canonicalize().unwrap();

        let encoded = real
            .to_str()
            .unwrap()
            .trim_start_matches('/')
            .replace('/', "-");
        assert_eq!(decode_dash_path(&encoded), Some(real));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn decode_dash_path_returns_none_for_nonexistent() {
        assert_eq!(
            decode_dash_path("this-path-does-not-exist-anywhere-xyzabc999"),
            None
        );
    }
}
