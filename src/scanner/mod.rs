use std::thread;

use crate::model::Session;

pub mod claude;
pub mod codex;
pub mod cursor_agent;
pub mod kiro;
pub mod opencode;
pub mod pi;

/// Truncate a string to `max` chars, appending "..." if truncated.
pub(crate) fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max).collect();
        format!("{truncated}...")
    }
}

pub fn scan_all() -> Vec<Session> {
    let handles = vec![
        thread::spawn(|| claude::scan().unwrap_or_default()),
        thread::spawn(|| codex::scan().unwrap_or_default()),
        thread::spawn(|| opencode::scan().unwrap_or_default()),
        thread::spawn(|| pi::scan().unwrap_or_default()),
        thread::spawn(|| kiro::scan().unwrap_or_default()),
        thread::spawn(|| cursor_agent::scan().unwrap_or_default()),
    ];
    let mut sessions: Vec<Session> = handles
        .into_iter()
        .flat_map(|h| h.join().unwrap_or_default())
        .collect();

    sessions.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    sessions
}
