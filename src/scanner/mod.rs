use std::thread;

use crate::model::Session;

pub mod claude;
pub mod codex;
pub mod cursor_agent;
pub mod gemini;
pub mod hermes;
pub mod kiro;
pub mod oh_my_pi;
pub mod opencode;
pub mod pi;
pub mod yolop;

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
    use std::io::{BufRead, BufReader, Read};
    /// Defensive cap matching the 512 KiB budgets used by cursor/pi scanners;
    /// a real first line is ~1 KB, and a truncated line just fails JSON
    /// parse upstream and is skipped.
    const MAX_FIRST_LINE_BYTES: u64 = 512 * 1024;
    let file = File::open(path).ok()?;
    let mut reader = BufReader::new(file).take(MAX_FIRST_LINE_BYTES);
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

/// Read up to `max_lines` lines from the start of a file, reading at most
/// `max_bytes` in total.
///
/// For JSONL session logs whose header record sits within the first handful of
/// lines, this replaces a `read_to_string` of a transcript that can be tens of
/// MB — the caller only ever looks at the head.
pub(crate) fn read_head_lines(
    path: &std::path::Path,
    max_lines: usize,
    max_bytes: u64,
) -> Vec<String> {
    use std::fs::File;
    use std::io::{BufRead, BufReader, Read};

    let Ok(file) = File::open(path) else {
        return Vec::new();
    };
    let mut reader = BufReader::new(file).take(max_bytes);
    let mut lines = Vec::new();
    let mut line = String::new();
    while lines.len() < max_lines {
        line.clear();
        // A non-UTF-8 line is an IO error here; stop rather than skip, since
        // the header we are looking for precedes any binary garbage.
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => lines.push(line.trim().to_string()),
        }
    }
    lines
}

/// Char-safe slice: take first `max` chars (never panics on UTF-8 boundaries).
pub(crate) fn char_prefix(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// Collapse all whitespace runs (incl. newlines/tabs) into single spaces.
/// Equivalent to `s.split_whitespace().collect::<Vec<_>>().join(" ")` without
/// the intermediate Vec.
pub(crate) fn collapse_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for w in s.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(w);
    }
    out
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

/// Last path component as the display project name ("unknown" when absent
/// or non-UTF-8).
pub(crate) fn project_name_from_path(path: impl AsRef<std::path::Path>) -> String {
    path.as_ref()
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string()
}

/// Split a GROUP_CONCAT('|||') blob into trimmed, non-empty titles,
/// deduplicated within the blob only, appended to `out`. Entries already in
/// `out` are intentionally NOT considered: a child title equal to the
/// already-pushed parent title is re-pushed (preserves existing behavior).
pub(crate) fn push_concat_titles(out: &mut Vec<String>, blob: &str) {
    let mut seen = std::collections::HashSet::new();
    for t in blob.split("|||") {
        let t = t.trim();
        if !t.is_empty() && seen.insert(t) {
            out.push(t.to_string());
        }
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

/// `Default` is the "readable file, nothing in it" fallback: scanners that can
/// still describe a session from other metadata use it so an unreadable log
/// degrades the entry instead of dropping it from the listing.
#[derive(Default)]
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
        thread::spawn(|| oh_my_pi::scan().unwrap_or_default()),
        thread::spawn(|| kiro::scan().unwrap_or_default()),
        thread::spawn(|| cursor_agent::scan().unwrap_or_default()),
        thread::spawn(|| gemini::scan().unwrap_or_default()),
        thread::spawn(|| hermes::scan().unwrap_or_default()),
        thread::spawn(|| yolop::scan().unwrap_or_default()),
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
    fn read_first_line_returns_first_non_empty_line() {
        let path = write_tmp("agf-test-first-line.jsonl", b"\n\n{\"a\":1}\n{\"b\":2}\n");
        assert_eq!(read_first_line(&path).unwrap(), "{\"a\":1}\n");
    }

    #[test]
    fn read_first_line_caps_newline_free_files() {
        // A pathological newline-free file must not be slurped whole: the
        // `take` cap returns at most 512 KiB, which then fails JSON parse
        // upstream and is skipped — the defensive outcome.
        let content = vec![b'x'; 600 * 1024];
        let path = write_tmp("agf-test-first-line-cap.jsonl", &content);
        let line = read_first_line(&path).expect("truncated line should be returned");
        assert_eq!(line.len(), 512 * 1024);
    }

    #[test]
    fn collapse_whitespace_matches_split_join_semantics() {
        assert_eq!(collapse_whitespace("  hello\n\tworld  "), "hello world");
        assert_eq!(collapse_whitespace(""), "");
        assert_eq!(collapse_whitespace("   \n\t  "), "");
        assert_eq!(collapse_whitespace("one"), "one");
        // Unicode whitespace (ideographic space) collapses like split_whitespace.
        assert_eq!(collapse_whitespace("a\u{3000}b"), "a b");
    }

    #[test]
    fn project_name_from_path_takes_last_component() {
        assert_eq!(
            project_name_from_path("/home/user/my-project"),
            "my-project"
        );
        assert_eq!(project_name_from_path("relative/dir"), "dir");
        // No final component → "unknown".
        assert_eq!(project_name_from_path("/"), "unknown");
        assert_eq!(project_name_from_path(""), "unknown");
        // Also accepts owned PathBuf (the cursor_agent call site).
        assert_eq!(
            project_name_from_path(std::path::PathBuf::from("/tmp/proj")),
            "proj"
        );
    }

    #[test]
    fn push_concat_titles_dedups_within_blob_only() {
        let mut out = vec!["parent".to_string()];
        push_concat_titles(&mut out, " a ||| b |||a||| ||| c ");
        // "a" deduped within the blob; existing "parent" is not considered.
        assert_eq!(out, vec!["parent", "a", "b", "c"]);

        let mut out2 = vec!["dup".to_string()];
        push_concat_titles(&mut out2, "dup");
        // Blob-only dedup scope: a title equal to the parent is re-pushed.
        assert_eq!(out2, vec!["dup", "dup"]);
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
