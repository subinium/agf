use std::path::{Path, PathBuf};

use rusqlite::Connection;
use walkdir::WalkDir;

use crate::error::AgfError;
use crate::model::{Agent, Session};

use super::{project_name_from_path, truncate};

/// Max chars stored per session summary.
const SUMMARY_MAX_CHARS: usize = 100;

/// Max bytes read from a single JSONL transcript when extracting the first
/// prompt. Cursor transcripts can include multi-MB tool-result blobs, and the
/// CACHE_VERSION bump in this release forces a cold rescan for every upgrader,
/// so an unbounded read repeats the v0.10.1 heavy-log stall pattern that
/// `read_head_tail` was added to prevent for Claude.
const MAX_PARSE_BYTES: usize = 512 * 1024;

/// Max lines scanned when extracting the first prompt. The first user message
/// almost always lands within the first few lines; the cap bounds work on
/// pathological transcripts that pad with non-`role=user` rows.
const MAX_PARSE_LINES: usize = 50;

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

    // Single walk covers both storage formats:
    //
    //   Legacy  (Cursor ≤ 2.4.7):  projects/<slug>/agent-transcripts/<uuid>.txt      (depth 3)
    //   Current (Composer 2 / 3+): projects/<slug>/agent-transcripts/<uuid>/<uuid>.jsonl (depth 4)
    for entry in WalkDir::new(&projects_dir)
        .min_depth(3)
        .max_depth(4)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let ext = path.extension().and_then(|e| e.to_str());

        // Resolve (agent_transcripts_dir, session_id) for each format.
        let (agent_transcripts_dir, session_id) = match ext {
            Some("txt") => {
                // Legacy: parent must be "agent-transcripts"
                let Some(parent) = path.parent().filter(|p| {
                    p.file_name().and_then(|n| n.to_str()) == Some("agent-transcripts")
                }) else {
                    continue;
                };
                let Some(id) = path.file_stem().and_then(|n| n.to_str()) else {
                    continue;
                };
                (parent, id.to_string())
            }
            Some("jsonl") => {
                // Current layout: agent-transcripts/<uuid>/<uuid>.jsonl
                // The parent dir name must equal the file stem (both are the
                // same session UUID). Without this invariant a stray jsonl
                // would produce a session_id that mismatches both the
                // store.db lookup key and what `cursor-agent --resume` expects.
                let Some(parent) = path.parent() else {
                    continue;
                };
                let Some(parent_name) = parent.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                let Some(id) = path.file_stem().and_then(|n| n.to_str()) else {
                    continue;
                };
                if parent_name != id {
                    continue;
                }
                let Some(grandparent) = parent.parent().filter(|p| {
                    p.file_name().and_then(|n| n.to_str()) == Some("agent-transcripts")
                }) else {
                    continue;
                };
                (grandparent, id.to_string())
            }
            _ => continue,
        };

        // Parent of agent-transcripts is the dash-encoded project path
        let Some(encoded_dir) = agent_transcripts_dir
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
        else {
            continue;
        };

        // Skip macOS temp directories
        if encoded_dir.starts_with("var-folders") {
            continue;
        }

        let Some(project_path) = decode_dash_path(encoded_dir) else {
            continue;
        };

        let project_name = project_name_from_path(&project_path);

        let project_path_str = project_path.to_string_lossy().to_string();

        // Locate the matching store.db (if any). cursor-agent's `/resume` only
        // surfaces sessions that have BOTH a transcript and a store.db entry
        // under ~/.cursor/chats/<workspace>/<session_id>/, so for the .jsonl
        // (Composer 2+) format we treat the absence of store.db as "orphaned"
        // and skip the session — otherwise agf would show sessions that
        // cursor-agent itself refuses to resume.
        let store_db_path = if chats_dir.exists() {
            find_store_db_path(&chats_dir, &session_id)
        } else {
            None
        };

        if ext == Some("jsonl") && store_db_path.is_none() {
            continue;
        }

        let meta = store_db_path.as_deref().and_then(read_store_db);

        let (summary, timestamp) = match meta {
            Some(m) => (m.name, m.created_at),
            None => {
                let mtime = path
                    .metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0);
                // Prompt extraction only applies to JSONL; .txt format is unknown
                let prompt = if ext == Some("jsonl") {
                    extract_first_prompt(path)
                } else {
                    None
                };
                (prompt, mtime)
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

/// Locate the first existing ~/.cursor/chats/<workspace>/<session_id>/store.db.
///
/// Presence of this file means cursor-agent considers the session resumable;
/// the file is the source of truth for `/resume`.
fn find_store_db_path(chats_dir: &Path, session_id: &str) -> Option<PathBuf> {
    let read_dir = std::fs::read_dir(chats_dir).ok()?;
    for workspace_entry in read_dir.filter_map(|e| e.ok()) {
        let store_path = workspace_entry.path().join(session_id).join("store.db");
        if store_path.exists() {
            return Some(store_path);
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

    // Cursor CLI stores session metadata as hex-encoded JSON in meta WHERE key='0'
    let mut stmt = conn
        .prepare("SELECT value FROM meta WHERE key = '0'")
        .ok()?;
    let hex_value: String = stmt.query_row([], |row| row.get(0)).ok()?;

    let json_bytes = hex_decode(&hex_value)?;
    let json_str = std::str::from_utf8(&json_bytes).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(json_str).ok()?;

    let name = parsed
        .get("name")
        .and_then(|v| v.as_str())
        .map(|s| truncate(s, SUMMARY_MAX_CHARS));

    // createdAt is in milliseconds
    let created_at = parsed
        .get("createdAt")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    Some(StoreMeta { name, created_at })
}

/// Decode a hex-encoded string to bytes.
fn hex_decode(hex: &str) -> Option<Vec<u8>> {
    // Non-ASCII input would make `&hex[i..i + 2]` panic mid-codepoint; a
    // corrupt store.db must yield None, not kill the scanner thread (a panic
    // here would silently erase every Cursor session via scan_all's
    // join-swallow).
    if !hex.len().is_multiple_of(2) || !hex.is_ascii() {
        return None;
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
        .collect()
}

/// Extract the first user-visible prompt from a Cursor agent transcript JSONL.
///
/// Each line is a JSON object with `{"role":"user"|"assistant","message":{"content":[...]}}`.
/// User turns may contain a `<user_info>` system injection (always first) followed by
/// the actual `<user_query>` prompt. We skip info blocks and strip the query tags.
fn extract_first_prompt(jsonl_path: &Path) -> Option<String> {
    use std::fs::File;
    use std::io::{BufRead, BufReader};

    let file = File::open(jsonl_path).ok()?;
    let reader = BufReader::new(file);

    let mut bytes_read = 0usize;
    for (lines_seen, line_result) in reader.lines().enumerate() {
        if lines_seen >= MAX_PARSE_LINES || bytes_read >= MAX_PARSE_BYTES {
            break;
        }

        // A single bad line (invalid UTF-8, transient IO error, malformed
        // JSON, partial flush from a crashed session) must skip just that
        // line — never disable extraction for the rest of the file. The
        // previous `.ok()?` pattern silently defeated this fallback for any
        // transcript whose first 50 lines had even one unparseable row.
        let Ok(line) = line_result else {
            continue;
        };
        bytes_read += line.len() + 1; // +1 approximates the stripped newline
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if value.get("role").and_then(|v| v.as_str()) != Some("user") {
            continue;
        }
        let Some(parts) = value
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_array())
        else {
            continue;
        };

        for part in parts {
            if part.get("type").and_then(|t| t.as_str()) != Some("text") {
                continue;
            }
            let Some(text) = part.get("text").and_then(|t| t.as_str()) else {
                continue;
            };
            if text.trim_start().starts_with("<user_info>") {
                continue;
            }
            // Strip the `<user_query>...</user_query>` wrapper if present.
            // `str::find` returns the FIRST occurrence of each substring
            // independently, so we must search for the closing tag AFTER
            // the opening one — otherwise a text like
            // `"</user_query>foo<user_query>...</user_query>"` (which can
            // occur in a pasted log or AI-generated code sample) gives
            // s > e and `text[s+12..e]` panics with `begin > end`.
            let prompt = match text.find("<user_query>") {
                Some(s) => {
                    let after = s + "<user_query>".len();
                    match text[after..].find("</user_query>") {
                        Some(rel) => text[after..after + rel].trim(),
                        None => text.trim(),
                    }
                }
                None => text.trim(),
            };
            if !prompt.is_empty() {
                return Some(truncate(prompt, SUMMARY_MAX_CHARS));
            }
        }
    }
    None
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
        } else if candidate.is_dir()
            && let Some(result) = solve(parts, end, &candidate)
        {
            return Some(result);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    // Only the unix-gated fixture helpers use the Write trait; gate the
    // import the same way so Windows clippy doesn't flag it as unused.
    #[cfg(unix)]
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
    ///
    /// Gated to Unix: the slug encoding (`canonical.to_str()` →
    /// `trim_start_matches('/')` → `replace('/', "-")`) assumes POSIX paths.
    /// On Windows, `canonicalize()` returns `\\?\C:\…` with backslashes that
    /// the dash decoder can't round-trip — every fixture-based test would
    /// fail. The scanner code itself is platform-agnostic; only this test
    /// fixture is Unix-shaped.
    #[cfg(unix)]
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
    #[cfg(unix)]
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

    /// Create a stub store.db at `<cursor_dir>/chats/<workspace>/<uuid>/store.db`.
    /// The file content is irrelevant; only its existence is checked.
    #[cfg(unix)]
    fn place_store_db(cursor_dir: &Path, workspace: &str, uuid: &str) {
        let dir = cursor_dir.join("chats").join(workspace).join(uuid);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("store.db"), b"").unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn scan_from_finds_jsonl_session_with_store_db() {
        let (cursor_dir, real_proj, encoded) = make_fixture("finds");
        let uuid = "aaaaaaaa0000000000000000000000a1";

        place_session(&cursor_dir, &encoded, uuid);
        place_store_db(&cursor_dir, "anyworkspacehash", uuid);

        let sessions = scan_from(&cursor_dir).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, uuid);
        assert_eq!(sessions[0].project_path, real_proj.to_str().unwrap());
        assert_eq!(sessions[0].agent, Agent::CursorAgent);

        let _ = fs::remove_dir_all(&cursor_dir);
        let _ = fs::remove_dir_all(&real_proj);
    }

    #[cfg(unix)]
    #[test]
    fn scan_from_skips_orphan_jsonl_without_store_db() {
        let (cursor_dir, real_proj, encoded) = make_fixture("orphan");
        let uuid = "aaaaaaaa0000000000000000000000ad";

        // Transcript exists but no store.db → cursor-agent /resume would NOT
        // surface it, so agf must hide it too.
        place_session(&cursor_dir, &encoded, uuid);

        assert!(scan_from(&cursor_dir).unwrap().is_empty());

        let _ = fs::remove_dir_all(&cursor_dir);
        let _ = fs::remove_dir_all(&real_proj);
    }

    #[cfg(unix)]
    #[test]
    fn scan_from_finds_legacy_txt_session() {
        let (cursor_dir, real_proj, encoded) = make_fixture("txtigno");

        // Legacy format: <uuid>.txt directly inside agent-transcripts/
        let transcripts_dir = cursor_dir
            .join("projects")
            .join(&encoded)
            .join("agent-transcripts");
        fs::create_dir_all(&transcripts_dir).unwrap();
        let uuid = "bbbbbbbb0000000000000000000000b1";
        fs::write(transcripts_dir.join(format!("{uuid}.txt")), b"").unwrap();

        let sessions = scan_from(&cursor_dir).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, uuid);
        assert_eq!(sessions[0].project_path, real_proj.to_str().unwrap());
        assert_eq!(sessions[0].agent, Agent::CursorAgent);

        let _ = fs::remove_dir_all(&cursor_dir);
        let _ = fs::remove_dir_all(&real_proj);
    }

    #[cfg(unix)]
    #[test]
    fn scan_from_ignores_txt_nested_in_uuid_subdir() {
        let (cursor_dir, real_proj, encoded) = make_fixture("txtwrong");

        // .txt at depth 4 (inside a UUID subdir) is NOT the legacy format and must be ignored
        let uuid = "cccccccc0000000000000000000000c1";
        let session_dir = cursor_dir
            .join("projects")
            .join(&encoded)
            .join("agent-transcripts")
            .join(uuid);
        fs::create_dir_all(&session_dir).unwrap();
        fs::write(session_dir.join(format!("{uuid}.txt")), b"").unwrap();

        assert!(scan_from(&cursor_dir).unwrap().is_empty());

        let _ = fs::remove_dir_all(&cursor_dir);
        let _ = fs::remove_dir_all(&real_proj);
    }

    #[cfg(unix)]
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

    #[cfg(unix)]
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

    #[cfg(unix)]
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

    /// Regression: `&hex[i..i + 2]` slices at byte offsets, so a multibyte
    /// char spanning an even offset (e.g. '€' at 0, or a 2-byte char at an
    /// odd offset) panicked on a char boundary before `from_str_radix`
    /// could reject it. Corrupt store.db content must yield None — a panic
    /// in this thread silently erases every Cursor session.
    #[test]
    fn hex_decode_returns_none_for_non_ascii_input() {
        assert_eq!(hex_decode("\u{20AC}a"), None); // 3-byte char spans even offset
        assert_eq!(hex_decode("a\u{0100}b"), None); // 2-byte char at odd offset
        assert_eq!(hex_decode("\u{0100}ab"), None); // aligned non-ASCII: None before and after
        assert_eq!(hex_decode("abc"), None); // odd length still rejected
        assert_eq!(hex_decode("48656c6c6f"), Some(b"Hello".to_vec()));
    }

    #[test]
    fn decode_dash_path_returns_none_for_nonexistent() {
        assert_eq!(
            decode_dash_path("this-path-does-not-exist-anywhere-xyzabc999"),
            None
        );
    }

    #[test]
    fn extract_first_prompt_reads_user_query_tags() {
        let pid = std::process::id();
        let path = std::env::temp_dir().join(format!("agf-prompt-test-{pid}.jsonl"));
        fs::write(
            &path,
            r#"{"role":"user","message":{"content":[{"type":"text","text":"<user_query>\nFix the bug\n</user_query>"}]}}"#,
        )
        .unwrap();
        assert_eq!(extract_first_prompt(&path).as_deref(), Some("Fix the bug"));
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn extract_first_prompt_skips_user_info_block() {
        let pid = std::process::id();
        let path = std::env::temp_dir().join(format!("agf-prompt-info-{pid}.jsonl"));
        fs::write(
            &path,
            "{\"role\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"<user_info>OS: darwin</user_info>\"}]}}\n\
             {\"role\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"<user_query>\\nActual prompt\\n</user_query>\"}]}}",
        )
        .unwrap();
        assert_eq!(
            extract_first_prompt(&path).as_deref(),
            Some("Actual prompt")
        );
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn extract_first_prompt_returns_none_for_empty_jsonl() {
        let pid = std::process::id();
        let path = std::env::temp_dir().join(format!("agf-prompt-empty-{pid}.jsonl"));
        fs::write(&path, b"{}").unwrap();
        assert_eq!(extract_first_prompt(&path), None);
        let _ = fs::remove_file(&path);
    }

    #[cfg(unix)]
    #[test]
    fn scan_from_falls_back_to_prompt_when_store_db_lacks_meta() {
        let (cursor_dir, real_proj, encoded) = make_fixture("nodb");
        let uuid = "aaaaaaaa0000000000000000000000a3";

        // Real JSONL with a user prompt …
        let session_dir = cursor_dir
            .join("projects")
            .join(&encoded)
            .join("agent-transcripts")
            .join(uuid);
        fs::create_dir_all(&session_dir).unwrap();
        fs::write(
            session_dir.join(format!("{uuid}.jsonl")),
            r#"{"role":"user","message":{"content":[{"type":"text","text":"<user_query>\nhello world\n</user_query>"}]}}"#,
        )
        .unwrap();
        // … plus a stub store.db (not a valid sqlite file, so read_store_db
        // returns None) — the session is "resumable" so it must surface, and
        // the summary must fall back to the extracted prompt.
        place_store_db(&cursor_dir, "anyworkspacehash", uuid);

        let sessions = scan_from(&cursor_dir).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].summaries, vec!["hello world"]);

        let _ = fs::remove_dir_all(&cursor_dir);
        let _ = fs::remove_dir_all(&real_proj);
    }

    /// Write a standalone `.jsonl` to a unique temp path for tests that drive
    /// `extract_first_prompt` directly (no `make_fixture` / `scan_from`, so
    /// they stay portable — the existing fixture encodes paths Unix-style).
    fn temp_jsonl(label: &str, body: &[u8]) -> PathBuf {
        let pid = std::process::id();
        let path = std::env::temp_dir().join(format!("agf-cursor-{label}-{pid}.jsonl"));
        let _ = fs::remove_file(&path);
        fs::write(&path, body).unwrap();
        path
    }

    /// Regression: a text part containing `</user_query>` BEFORE `<user_query>`
    /// (e.g. a pasted log or AI-generated code sample) used to compute
    /// `start > end` and panic with "begin > end" when slicing. The fix
    /// searches for the closing tag AFTER the opening one.
    #[test]
    fn extract_first_prompt_does_not_panic_on_inverted_tags() {
        // The text has the CLOSING tag first, then a normal opening+closing pair.
        let path = temp_jsonl(
            "invtag",
            br#"{"role":"user","message":{"content":[{"type":"text","text":"</user_query>noise<user_query>real prompt</user_query>"}]}}"#,
        );
        let result = extract_first_prompt(&path);
        let _ = fs::remove_file(&path);
        assert_eq!(result.as_deref(), Some("real prompt"));
    }

    /// Regression: previously, the FIRST malformed JSON line aborted the whole
    /// per-line loop via `.ok()?`, so a single garbage line at the top of a
    /// JSONL silently disabled the blank-summary fallback for the entire file.
    /// Now each bad line is skipped individually.
    #[test]
    fn extract_first_prompt_skips_malformed_json_lines() {
        // Line 1: garbage; Line 2: empty; Line 3: valid user prompt.
        let body = b"not-json-at-all\n\n\
            {\"role\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"<user_query>recovered</user_query>\"}]}}\n";
        let path = temp_jsonl("malformed", body);
        let result = extract_first_prompt(&path);
        let _ = fs::remove_file(&path);
        assert_eq!(result.as_deref(), Some("recovered"));
    }

    /// Regression: a single line containing invalid UTF-8 bytes used to abort
    /// the loop via `let line = line.ok()?;`. It should now be skipped like
    /// any other bad line.
    #[test]
    fn extract_first_prompt_skips_invalid_utf8_lines() {
        let mut bytes: Vec<u8> = Vec::new();
        // Line 1: a truncated multibyte sequence — `BufReader::lines` returns
        // Err(InvalidData) for this row.
        bytes.extend_from_slice(&[0xC3, 0x28]); // invalid 2-byte UTF-8 start
        bytes.push(b'\n');
        // Line 2: well-formed JSON with a user prompt.
        bytes.extend_from_slice(
            br#"{"role":"user","message":{"content":[{"type":"text","text":"<user_query>after garbage</user_query>"}]}}"#,
        );
        let path = temp_jsonl("badutf8", &bytes);
        let result = extract_first_prompt(&path);
        let _ = fs::remove_file(&path);
        assert_eq!(result.as_deref(), Some("after garbage"));
    }

    /// Invariant: a jsonl whose file stem mismatches its parent directory name
    /// is silently dropped. Real Cursor always writes them equal; the guard
    /// prevents a stray jsonl from producing a session_id that mismatches both
    /// the store.db lookup key and what `cursor-agent --resume` expects.
    ///
    /// Gated to Unix because `make_fixture`'s slug encoding (joining canonical
    /// path components with `-`) assumes Unix-style paths; Windows canonical
    /// paths carry `\\?\C:\…` and backslashes that the dash decoder can't
    /// round-trip.
    #[cfg(unix)]
    #[test]
    fn scan_from_rejects_jsonl_with_stem_mismatched_to_parent() {
        let (cursor_dir, real_proj, encoded) = make_fixture("stemmismatch");
        let parent_uuid = "aaaaaaaa0000000000000000000000c1";
        let stem_uuid = "bbbbbbbb0000000000000000000000c1";

        let session_dir = cursor_dir
            .join("projects")
            .join(&encoded)
            .join("agent-transcripts")
            .join(parent_uuid);
        fs::create_dir_all(&session_dir).unwrap();
        // stem != parent_uuid → must be skipped.
        fs::write(session_dir.join(format!("{stem_uuid}.jsonl")), b"{}\n").unwrap();
        // Even with a store.db at the stem id, the scanner shouldn't surface it.
        place_store_db(&cursor_dir, "ws", stem_uuid);

        let sessions = scan_from(&cursor_dir).unwrap();
        assert!(sessions.is_empty());

        let _ = fs::remove_dir_all(&cursor_dir);
        let _ = fs::remove_dir_all(&real_proj);
    }

    /// Real-world load-bearing case: project paths regularly contain hyphens
    /// (this very repo is `agent-tui-finder`). The backtracking decoder must
    /// pick the segment that actually exists on disk when multiple
    /// hyphen-split prefixes are valid directory candidates.
    ///
    /// Gated to Unix for the same reason as `make_fixture` — the encoded slug
    /// is built from a canonical path with `'/'`-separated components.
    #[cfg(unix)]
    #[test]
    fn decode_dash_path_resolves_hyphenated_segments() {
        let pid = std::process::id();
        let base = std::env::temp_dir()
            .join(format!("agfdecode{pid}hyphen"))
            .canonicalize()
            .unwrap_or_else(|_| {
                let p = std::env::temp_dir().join(format!("agfdecode{pid}hyphen"));
                let _ = fs::remove_dir_all(&p);
                fs::create_dir_all(&p).unwrap();
                p.canonicalize().unwrap()
            });
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        // Decoy siblings the greedy-longest-first decoder must reject.
        fs::create_dir_all(base.join("agent")).unwrap();
        fs::create_dir_all(base.join("agent-tui")).unwrap();
        // The real target.
        let target = base.join("agent-tui-finder");
        fs::create_dir_all(&target).unwrap();

        let encoded = base
            .to_str()
            .unwrap()
            .trim_start_matches('/')
            .replace('/', "-")
            + "-agent-tui-finder";

        let resolved = decode_dash_path(&encoded).unwrap();
        assert_eq!(resolved, target);

        let _ = fs::remove_dir_all(&base);
    }
}
