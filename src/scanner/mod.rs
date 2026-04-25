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

/// Read up to `head_bytes` from the start and `tail_bytes` from the end of
/// `path`, returning UTF-8-safe complete lines (no partial lines on the slice
/// boundary). For files ≤ `head_bytes + tail_bytes`, the whole file is read.
///
/// Used by scanners that only need the first/last entries of large JSONL
/// session logs (e.g. cwd in the head, away_summary/aiTitle in the tail) to
/// avoid scanning multi-MB files line-by-line.
pub(crate) fn read_head_tail(
    path: &std::path::Path,
    head_bytes: u64,
    tail_bytes: u64,
) -> Option<HeadTail> {
    use std::fs::File;
    use std::io::{Read, Seek, SeekFrom};

    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();

    // Small file — read whole, no truncation needed.
    if len <= head_bytes + tail_bytes {
        let mut buf = String::with_capacity(len as usize);
        file.read_to_string(&mut buf).ok()?;
        return Some(HeadTail {
            head: buf,
            tail: String::new(),
            truncated: false,
        });
    }

    // Read head.
    let mut head_buf = vec![0u8; head_bytes as usize];
    file.read_exact(&mut head_buf).ok()?;
    // Drop the last (potentially partial) line in the head slice.
    let head_str = trim_partial_last_line(&head_buf);

    // Read tail.
    file.seek(SeekFrom::End(-(tail_bytes as i64))).ok()?;
    let mut tail_buf = vec![0u8; tail_bytes as usize];
    file.read_exact(&mut tail_buf).ok()?;
    // Drop the first (potentially partial) line in the tail slice.
    let tail_str = trim_partial_first_line(&tail_buf);

    Some(HeadTail {
        head: head_str,
        tail: tail_str,
        truncated: true,
    })
}

pub(crate) struct HeadTail {
    pub head: String,
    pub tail: String,
    pub truncated: bool,
}

/// UTF-8-safe lossy decode that drops bytes after the last newline so the
/// returned string never ends mid-line.
fn trim_partial_last_line(bytes: &[u8]) -> String {
    match bytes.iter().rposition(|&b| b == b'\n') {
        Some(i) => String::from_utf8_lossy(&bytes[..=i]).into_owned(),
        None => String::new(),
    }
}

/// UTF-8-safe lossy decode that drops bytes before the first newline so the
/// returned string never starts mid-line.
fn trim_partial_first_line(bytes: &[u8]) -> String {
    match bytes.iter().position(|&b| b == b'\n') {
        Some(i) => String::from_utf8_lossy(&bytes[i + 1..]).into_owned(),
        None => String::new(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_tmp(name: &str, content: &[u8]) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(name);
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(content).unwrap();
        path
    }

    #[test]
    fn read_head_tail_small_file_returns_full_content() {
        let path = write_tmp("agf-test-small.jsonl", b"{\"a\":1}\n{\"b\":2}\n{\"c\":3}\n");
        let ht = read_head_tail(&path, 1024, 1024).unwrap();
        assert!(!ht.truncated);
        assert!(ht.head.contains("\"a\":1"));
        assert!(ht.head.contains("\"c\":3"));
        assert!(ht.tail.is_empty());
    }

    #[test]
    fn read_head_tail_large_file_skips_middle() {
        // 4 KB head marker + 1 MB padding + 4 KB tail marker
        let head_marker = b"{\"head_marker\":\"yes\"}\n";
        let tail_marker = b"{\"tail_marker\":\"yes\"}\n";
        let mut content = Vec::new();
        content.extend_from_slice(head_marker);
        content.extend_from_slice(&vec![b'x'; 1024 * 1024]);
        content.push(b'\n');
        content.extend_from_slice(tail_marker);

        let path = write_tmp("agf-test-large.jsonl", &content);
        let ht = read_head_tail(&path, 4096, 4096).unwrap();
        assert!(ht.truncated);
        assert!(ht.head.contains("head_marker"));
        assert!(!ht.head.contains("tail_marker"));
        assert!(ht.tail.contains("tail_marker"));
        assert!(!ht.tail.contains("head_marker"));
    }

    #[test]
    fn read_head_tail_drops_partial_lines_at_boundary() {
        // A long single line longer than head_bytes — the head slice cuts
        // mid-line, so trim_partial_last_line should drop it entirely.
        let mut content = Vec::new();
        content.extend_from_slice(&vec![b'x'; 8192]);
        content.push(b'\n');
        content.extend_from_slice(b"{\"tail\":\"ok\"}\n");

        let path = write_tmp("agf-test-partial.jsonl", &content);
        let ht = read_head_tail(&path, 1024, 1024).unwrap();
        assert!(ht.truncated);
        // Head slice (1KB) lands inside the long line → no complete line
        // present, head should be empty.
        assert!(ht.head.is_empty(), "head was: {:?}", ht.head);
        // Tail slice picks up the trailing complete line.
        assert!(ht.tail.contains("tail"));
    }
}
