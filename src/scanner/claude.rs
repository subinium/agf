use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};

use rayon::prelude::*;

use serde::Deserialize;
use serde_json::Value;

use crate::error::AgfError;
use crate::model::{Agent, Session};

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

/// Scan ~/.claude/projects/*/<sessionId>.jsonl to detect worktree sessions
/// and extract recap (away_summary / aiTitle) metadata.
///
/// `cwd` in the per-session JSONL is the actual working directory, which for
/// worktree sessions looks like `<project>/.claude/worktrees/<name>`.
fn scan_session_metadata(claude_dir: &std::path::Path) -> HashMap<String, SessionMeta> {
    let projects_dir = claude_dir.join("projects");

    let Ok(proj_entries) = fs::read_dir(&projects_dir) else {
        return HashMap::new();
    };

    // Collect all JSONL file paths first, then process in parallel.
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
        .into_par_iter()
        .filter_map(|(session_id, file_path)| {
            let file = fs::File::open(&file_path).ok()?;
            let reader = BufReader::new(file);
            let mut worktree: Option<String> = None;
            let mut latest_recap: Option<String> = None;
            let mut latest_recap_ts: Option<String> = None;
            let mut ai_title: Option<String> = None;

            for line in reader.lines() {
                let Ok(line) = line else { continue };
                let Ok(val) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };

                // Detect worktree from cwd (only first few lines matter, but we
                // scan the whole file anyway for away_summary)
                if worktree.is_none() {
                    if let Some(cwd) = val.get("cwd").and_then(|c| c.as_str()) {
                        if let Some((_, wt)) = cwd.split_once("/.claude/worktrees/") {
                            if !wt.is_empty() {
                                worktree = Some(wt.to_string());
                            }
                        }
                    }
                }

                // Extract aiTitle
                if val.get("type").and_then(|t| t.as_str()) == Some("ai-title") {
                    if let Some(title) = val.get("aiTitle").and_then(|t| t.as_str()) {
                        ai_title = Some(title.to_string());
                    }
                }

                // Extract the most recent away_summary (recap)
                if val.get("type").and_then(|t| t.as_str()) == Some("system")
                    && val.get("subtype").and_then(|t| t.as_str()) == Some("away_summary")
                {
                    let ts = val
                        .get("timestamp")
                        .and_then(|t| t.as_str())
                        .unwrap_or("")
                        .to_string();
                    // Lexicographic comparison of RFC3339 timestamps with a
                    // fixed format (e.g. `2026-04-21T12:34:56.789Z`) is
                    // monotonic, so string order == chronological order.
                    if latest_recap_ts
                        .as_deref()
                        .is_none_or(|prev| ts.as_str() > prev)
                    {
                        if let Some(content) = val.get("content").and_then(|c| c.as_str()) {
                            // Strip the "(disable recaps in /config)" suffix
                            let clean = content
                                .trim_end_matches("(disable recaps in /config)")
                                .trim();
                            latest_recap = Some(clean.to_string());
                            latest_recap_ts = Some(ts);
                        }
                    }
                }
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

    let session_meta = scan_session_metadata(&claude_dir);
    let mut branch_cache: HashMap<String, Option<String>> = HashMap::new();
    let mut sessions_map: HashMap<String, SessionData> = HashMap::new();

    let file = fs::File::open(&path)?;
    for line in std::io::BufReader::new(file).lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };
        let line = line.trim().to_owned();
        if line.is_empty() {
            continue;
        }
        let entry: ClaudeEntry = match serde_json::from_str(&line) {
            Ok(e) => e,
            Err(_) => continue,
        };
        let session_id = match &entry.session_id {
            Some(id) if !id.is_empty() => id.clone(),
            _ => continue,
        };
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
            let display: String = display.split_whitespace().collect::<Vec<_>>().join(" ");
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
