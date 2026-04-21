use std::thread;

use crate::model::Session;

pub mod claude;
pub mod codex;
pub mod cursor_agent;
pub mod gemini;
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

/// Read only the first non-empty line of a file without loading the rest.
pub(crate) fn read_first_line(path: &std::path::Path) -> Option<String> {
    use std::fs::File;
    use std::io::{BufRead, BufReader};
    let file = File::open(path).ok()?;
    let mut reader = BufReader::new(file);
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line).ok()?;
        if n == 0 {
            return None;
        }
        if !line.trim().is_empty() {
            return Some(line);
        }
    }
}

/// Char-safe slice: take first `max` chars (never panics on UTF-8 boundaries).
pub(crate) fn char_prefix(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// Extract first non-empty line, truncated to `max_len` chars with '…' suffix.
pub(crate) fn first_line_truncated(s: &str, max_len: usize) -> Option<String> {
    let line = s.lines().next().unwrap_or("").trim();
    if line.is_empty() {
        return None;
    }
    if line.chars().count() > max_len {
        Some(format!("{}…", char_prefix(line, max_len)))
    } else {
        Some(line.to_string())
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
        thread::spawn(|| gemini::scan().unwrap_or_default()),
    ];
    let mut sessions: Vec<Session> = handles
        .into_iter()
        .flat_map(|h| match h.join() {
            Ok(v) => v,
            Err(_) => {
                if std::env::var("AGF_DEBUG").is_ok() {
                    eprintln!("[agf] scanner thread panicked");
                }
                Vec::new()
            }
        })
        .collect();

    sessions.sort_by_key(|s| std::cmp::Reverse(s.timestamp));
    sessions
}
