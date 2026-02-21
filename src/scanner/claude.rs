use std::collections::HashMap;
use std::fs;

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

/// Scan ~/.claude/projects/*/<sessionId>.jsonl to detect worktree sessions.
/// Returns session_id → worktree_name for sessions started inside a worktree.
///
/// `cwd` in the per-session JSONL is the actual working directory, which for
/// worktree sessions looks like `<project>/.claude/worktrees/<name>`.
fn scan_session_worktrees(claude_dir: &std::path::Path) -> HashMap<String, String> {
    let projects_dir = claude_dir.join("projects");
    let mut map = HashMap::new();

    let Ok(proj_entries) = fs::read_dir(&projects_dir) else {
        return map;
    };

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
            let Some(session_id) = file_path
                .file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.to_string())
            else {
                continue;
            };
            if map.contains_key(&session_id) {
                continue;
            }
            let Ok(content) = fs::read_to_string(&file_path) else {
                continue;
            };
            for line in content.lines().take(20) {
                if let Ok(val) = serde_json::from_str::<Value>(line) {
                    if let Some(cwd) = val.get("cwd").and_then(|c| c.as_str()) {
                        if let Some((_, wt)) = cwd.split_once("/.claude/worktrees/") {
                            if !wt.is_empty() {
                                map.insert(session_id.clone(), wt.to_string());
                                break;
                            }
                        }
                    }
                }
            }
        }
    }

    map
}

/// Read the current git branch from the project root's `.git/HEAD`.
/// Returns `None` if the directory is not a git repo or is in detached HEAD state.
fn read_git_branch(project_path: &str) -> Option<String> {
    let head_path = std::path::Path::new(project_path).join(".git").join("HEAD");
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

    let worktrees = scan_session_worktrees(&claude_dir);
    let content = fs::read_to_string(&path)?;
    let mut sessions_map: HashMap<String, SessionData> = HashMap::new();

    for line in content.lines() {
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
            let worktree = worktrees.get(&session_id).cloned();
            let git_branch = read_git_branch(&project_path);

            // Sort summaries newest-first
            data.summaries
                .sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
            let summaries: Vec<String> = data.summaries.into_iter().map(|(_, s)| s).collect();

            Some(Session {
                agent: Agent::ClaudeCode,
                session_id,
                project_name,
                project_path,
                summaries,
                timestamp,
                git_branch,
                worktree,
                git_dirty: None,
            })
        })
        .collect();

    sessions.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    Ok(sessions)
}
