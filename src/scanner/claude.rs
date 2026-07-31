use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::BufRead;

use rayon::prelude::*;

use serde::Deserialize;
use serde_json::Value;

use crate::error::AgfError;
use crate::model::{Agent, Session};
use crate::scanner::{collapse_whitespace, read_head_tail};

/// Per-file I/O cap for `scan_session_metadata`. Files larger than the sum
/// fall back to head + tail reads; smaller files are read in full. Sized so
/// that:
///   * `cwd` (logged once at session start, ~1 KB into the file) is always
///     in the head slice;
///   * `aiTitle` (emitted while the agent is forming project context, within
///     the first few hundred lines) fits in the head slice;
///   * the latest `away_summary` recap — appended last, so it sits at the very
///     end of the file — is captured by the tail slice.
///
/// `TAIL_BYTES` is deliberately small. `away_summary` lines are appended
/// chronologically and only the most recent one is displayed, so a few dozen KB
/// of tail reliably contains it. A larger tail mainly forces mid-size
/// transcripts (which fall under `head + tail` and are therefore read in FULL)
/// to be slurped end-to-end — the dominant cold-start scan cost on large
/// `~/.claude/projects` trees (tens of MB read for an off-by-default recap).
const HEAD_BYTES: u64 = 16 * 1024;
const TAIL_BYTES: u64 = 32 * 1024;

#[derive(Deserialize)]
struct ClaudeEntry {
    display: Option<String>,
    timestamp: Option<f64>,
    project: Option<String>,
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
}

struct SessionData {
    project: String,
    timestamp: f64,
    summaries: Vec<(f64, String)>, // (timestamp, display) pairs
}

/// Metadata extracted from per-session JSONL files.
struct SessionMeta {
    worktree: Option<String>,
    recap: Option<String>, // most recent away_summary, optionally prefixed with aiTitle
}

/// Walk `~/.claude/projects/*/<sessionId>.jsonl` once and return the list of
/// `(session_id, path)` pairs for every per-session JSONL on disk.
///
/// Callers (orphan filtering + `scan_session_metadata`) share this so the
/// directory tree is only read once per scan.
fn list_session_files(claude_dir: &std::path::Path) -> Vec<(String, std::path::PathBuf)> {
    let projects_dir = claude_dir.join("projects");

    let Ok(proj_entries) = fs::read_dir(&projects_dir) else {
        return Vec::new();
    };

    let mut file_paths: Vec<(String, std::path::PathBuf)> = Vec::new();
    for proj_entry in proj_entries.flatten() {
        let proj_path = proj_entry.path();
        if !proj_path.is_dir() {
            continue;
        }
        let Ok(session_files) = fs::read_dir(&proj_path) else {
            continue;
        };
        for session_file in session_files.flatten() {
            let file_path = session_file.path();
            if file_path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            if let Some(session_id) = file_path
                .file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.to_string())
            {
                file_paths.push((session_id, file_path));
            }
        }
    }

    file_paths
}

/// Scan ~/.claude/projects/*/<sessionId>.jsonl to detect worktree sessions
/// and extract recap (away_summary / aiTitle) metadata.
///
/// `cwd` in the per-session JSONL is the actual working directory, which for
/// worktree sessions looks like `<project>/.claude/worktrees/<name>`.
fn scan_session_metadata(
    file_paths: Vec<(String, std::path::PathBuf)>,
) -> HashMap<String, SessionMeta> {
    file_paths
        .into_par_iter()
        .filter_map(|(session_id, file_path)| {
            let ht = read_head_tail(&file_path, HEAD_BYTES, TAIL_BYTES)?;

            let mut worktree: Option<String> = None;
            let mut ai_title: Option<String> = None;

            // Head slice: scan for worktree (cwd) + aiTitle. First-match
            // semantics for both, matching the pre-cap behavior.
            for line in ht.head.lines() {
                if worktree.is_some() && ai_title.is_some() {
                    break;
                }
                let Ok(val) = serde_json::from_str::<Value>(line) else {
                    continue;
                };
                extract_worktree(&val, &mut worktree);
                extract_ai_title(&val, &mut ai_title);
            }

            // Tail slice: scan for the latest away_summary. For small files
            // (`!truncated`) the head already contains every line, so skip
            // the redundant tail pass.
            let mut latest_recap: Option<String> = None;
            let mut latest_recap_ts: Option<String> = None;
            let scan_tail = if ht.truncated { &ht.tail } else { &ht.head };
            for line in scan_tail.lines() {
                let Ok(val) = serde_json::from_str::<Value>(line) else {
                    continue;
                };
                // Late aiTitle wins on small files; tail-late aiTitle on
                // truncated files is rare but harmless to capture.
                extract_ai_title(&val, &mut ai_title);
                extract_recap(&val, &mut latest_recap, &mut latest_recap_ts);
            }

            // Build recap: prepend "recap: " and optionally aiTitle
            let recap = match (ai_title, latest_recap) {
                (Some(title), Some(summary)) => Some(format!("recap: {title} — {summary}")),
                (None, Some(summary)) => Some(format!("recap: {summary}")),
                (Some(title), None) => Some(title),
                (None, None) => None,
            };

            if worktree.is_some() || recap.is_some() {
                Some((session_id, SessionMeta { worktree, recap }))
            } else {
                None
            }
        })
        .collect()
}

fn extract_worktree(val: &Value, worktree: &mut Option<String>) {
    if worktree.is_some() {
        return;
    }
    if let Some(cwd) = val.get("cwd").and_then(|c| c.as_str())
        && let Some((_, wt)) = cwd.split_once("/.claude/worktrees/")
        && !wt.is_empty()
    {
        *worktree = Some(wt.to_string());
    }
}

fn extract_ai_title(val: &Value, ai_title: &mut Option<String>) {
    if val.get("type").and_then(|t| t.as_str()) == Some("ai-title")
        && let Some(title) = val.get("aiTitle").and_then(|t| t.as_str())
    {
        *ai_title = Some(title.to_string());
    }
}

fn extract_recap(
    val: &Value,
    latest_recap: &mut Option<String>,
    latest_recap_ts: &mut Option<String>,
) {
    if val.get("type").and_then(|t| t.as_str()) != Some("system")
        || val.get("subtype").and_then(|t| t.as_str()) != Some("away_summary")
    {
        return;
    }
    let ts = val
        .get("timestamp")
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_string();
    // Lexicographic comparison of RFC3339 timestamps with a fixed format
    // (e.g. `2026-04-21T12:34:56.789Z`) is monotonic, so string order ==
    // chronological order.
    if latest_recap_ts
        .as_deref()
        .is_none_or(|prev| ts.as_str() > prev)
        && let Some(content) = val.get("content").and_then(|c| c.as_str())
    {
        // Strip the "(disable recaps in /config)" suffix
        let clean = content
            .trim_end_matches("(disable recaps in /config)")
            .trim();
        *latest_recap = Some(clean.to_string());
        *latest_recap_ts = Some(ts);
    }
}

/// Read the current git branch from the project root's `.git/HEAD`.
/// Returns `None` if the directory is not a git repo or is in detached HEAD state.
fn read_git_branch(project_path: &str) -> Option<String> {
    let head_path = std::path::Path::new(project_path).join(".git").join("HEAD");
    // `.git/HEAD` is a small (~30 byte) plain text file; a direct read is
    // fast enough that the earlier thread+channel 100 ms timeout was
    // unnecessary paranoia.
    let content = fs::read_to_string(&head_path).ok()?;
    let branch = content.trim().strip_prefix("ref: refs/heads/")?.to_string();
    if branch.is_empty() {
        None
    } else {
        Some(branch)
    }
}

pub fn scan() -> Result<Vec<Session>, AgfError> {
    let claude_dir = crate::config::claude_dir()?;
    let path = claude_dir.join("history.jsonl");
    if !path.exists() {
        return Ok(Vec::new());
    }

    let session_files = list_session_files(&claude_dir);
    let existing_ids: HashSet<String> = session_files.iter().map(|(id, _)| id.clone()).collect();
    let session_meta = scan_session_metadata(session_files);
    let mut branch_cache: HashMap<String, Option<String>> = HashMap::new();
    let mut sessions_map: HashMap<String, SessionData> = HashMap::new();

    let file = fs::File::open(&path)?;
    for line in std::io::BufReader::new(file).lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let entry: ClaudeEntry = match serde_json::from_str(line) {
            Ok(e) => e,
            Err(_) => continue,
        };
        let session_id = match &entry.session_id {
            Some(id) if !id.is_empty() => id.clone(),
            _ => continue,
        };
        if !existing_ids.contains(&session_id) {
            // Orphans (no per-session JSONL under ~/.claude/projects/) are
            // dropped by the final filter anyway; skip early so unbounded
            // history.jsonl growth (#27) doesn't accumulate dead SessionData
            // and summary tuples for the whole scan. Mirrors the
            // codex::read_history_summaries pre-filter from v0.11.4.
            continue;
        }
        let ts = entry.timestamp.unwrap_or(0.0);

        let data = sessions_map
            .entry(session_id)
            .or_insert_with(|| SessionData {
                project: entry.project.clone().unwrap_or_default(),
                timestamp: ts,
                summaries: Vec::new(),
            });

        // Keep the latest timestamp and project
        if ts >= data.timestamp {
            data.timestamp = ts;
            if let Some(ref proj) = entry.project {
                data.project = proj.clone();
            }
        }

        if let Some(display) = entry.display {
            // Collapse multi-line content (e.g. pasted text) into a single line.
            let display = collapse_whitespace(&display);
            if !display.is_empty() {
                data.summaries.push((ts, display));
            }
        }
    }

    let mut sessions: Vec<Session> = sessions_map
        .into_iter()
        .filter_map(|(session_id, mut data)| {
            if data.project.is_empty() {
                return None;
            }
            // history.jsonl logs `sessionId` on session start, but the actual
            // transcript file is created lazily by `claude --resume` and may
            // be deleted by the user later. `claude --resume <id>` errors out
            // with "No conversation found" for these orphans, so drop them
            // from the listing rather than offering a dead resume target.
            if !existing_ids.contains(&session_id) {
                return None;
            }

            // project in history.jsonl is always the real project root.
            let project_path = data.project.clone();
            let project_name = std::path::Path::new(&project_path)
                .file_name()?
                .to_str()?
                .to_string();
            let timestamp = data.timestamp as i64;

            // Worktree: detected from per-session JSONL cwd field.
            // Branch: live current branch from .git/HEAD of the project root.
            //   - For worktree sessions this shows the root project's branch (e.g. "main"),
            //     which is displayed in the detail view alongside the worktree name.
            //   - For regular sessions this shows the project's current branch.
            let meta = session_meta.get(&session_id);
            let worktree = meta.and_then(|m| m.worktree.clone());
            let git_branch = branch_cache
                .entry(project_path.clone())
                .or_insert_with(|| read_git_branch(&project_path))
                .clone();

            // Sort summaries newest-first
            data.summaries
                .sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
            let summaries: Vec<String> = data.summaries.into_iter().map(|(_, s)| s).collect();

            let recap = meta.and_then(|m| m.recap.clone());

            Some(Session {
                agent: Agent::ClaudeCode,
                session_id,
                project_name,
                project_path,
                summaries,
                timestamp,
                git_branch,
                worktree,
                recap,
            })
        })
        .collect();

    sessions.sort_by_key(|s| std::cmp::Reverse(s.timestamp));
    Ok(sessions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn make_claude_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(name);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("projects")).unwrap();
        dir
    }

    fn touch_session(claude_dir: &std::path::Path, project: &str, session_id: &str) {
        let proj = claude_dir.join("projects").join(project);
        fs::create_dir_all(&proj).unwrap();
        let path = proj.join(format!("{session_id}.jsonl"));
        let mut f = fs::File::create(&path).unwrap();
        f.write_all(b"{}\n").unwrap();
    }

    #[test]
    fn list_session_files_collects_jsonl_session_ids() {
        let claude_dir = make_claude_dir("agf-test-list-sessions");
        touch_session(&claude_dir, "-home-foo", "aaa-111");
        touch_session(&claude_dir, "-home-foo", "bbb-222");
        touch_session(&claude_dir, "-home-bar", "ccc-333");
        // Non-jsonl sibling must be ignored.
        fs::write(claude_dir.join("projects/-home-foo/notes.txt"), "x").unwrap();

        let ids: HashSet<String> = list_session_files(&claude_dir)
            .into_iter()
            .map(|(id, _)| id)
            .collect();

        assert_eq!(
            ids,
            ["aaa-111", "bbb-222", "ccc-333"]
                .into_iter()
                .map(String::from)
                .collect()
        );
    }

    #[test]
    fn list_session_files_returns_empty_when_projects_missing() {
        let dir = std::env::temp_dir().join("agf-test-no-projects");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        assert!(list_session_files(&dir).is_empty());
    }

    #[test]
    fn scan_session_metadata_finds_worktree_in_head_and_latest_recap_in_tail() {
        // A transcript larger than HEAD_BYTES + TAIL_BYTES: `worktree` must come
        // from the head (cwd on line 1) and the latest `away_summary` recap from
        // the tail (recaps are appended last). This locks TAIL_BYTES — shrinking
        // it must never drop the recap, which always sits at the file's end.
        let claude_dir = make_claude_dir("agf-test-recap-tail");
        let proj = claude_dir.join("projects").join("-home-proj");
        fs::create_dir_all(&proj).unwrap();
        let sid = "recap-big-1";
        let path = proj.join(format!("{sid}.jsonl"));
        let mut f = fs::File::create(&path).unwrap();

        // Head: cwd inside a worktree.
        writeln!(
            f,
            r#"{{"type":"user","cwd":"/home/proj/.claude/worktrees/feature-x"}}"#
        )
        .unwrap();
        // Padding to push the file well past HEAD_BYTES + TAIL_BYTES.
        let filler = format!(r#"{{"type":"assistant","pad":"{}"}}"#, "x".repeat(2000));
        let target = (HEAD_BYTES + TAIL_BYTES) as usize + 100 * 1024;
        let mut written = 0usize;
        while written < target {
            writeln!(f, "{filler}").unwrap();
            written += filler.len() + 1;
        }
        // Tail: an older then a newer away_summary — the latest one must win.
        writeln!(
            f,
            r#"{{"type":"system","subtype":"away_summary","timestamp":"2026-05-01T00:00:00.000Z","content":"old recap"}}"#
        )
        .unwrap();
        writeln!(
            f,
            r#"{{"type":"system","subtype":"away_summary","timestamp":"2026-05-02T00:00:00.000Z","content":"latest recap"}}"#
        )
        .unwrap();

        let meta = scan_session_metadata(vec![(sid.to_string(), path)]);
        let m = meta
            .get(sid)
            .expect("metadata should be present for large file");
        assert_eq!(m.worktree.as_deref(), Some("feature-x"));
        assert_eq!(m.recap.as_deref(), Some("recap: latest recap"));

        let _ = fs::remove_dir_all(&claude_dir);
    }
}
