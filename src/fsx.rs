//! Crash-safe file writes.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Write `contents` to `path` atomically: a sibling temp file is written and
/// flushed to disk, then renamed over the destination.
///
/// Every caller rewrites a file it does not own the only copy of —
/// `~/.claude/history.jsonl`, `~/.codex/history.jsonl`, the user's shell rc,
/// the session cache — by reading it, transforming it in memory and writing
/// the result back. A plain `fs::write` truncates the destination first, so an
/// interruption (crash, power loss, ENOSPC) between the truncate and the last
/// byte leaves a half-written file and no original to fall back on.
/// `rename(2)` is atomic within a filesystem, so a concurrent reader sees
/// either the old file or the complete new one, never a torn prefix.
pub fn write_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    let mut tmp_name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?
        .to_os_string();
    tmp_name.push(".agf-tmp");
    // Same directory as the destination: `rename` is only atomic within one
    // filesystem, so a temp in /tmp could silently degrade to copy+delete.
    let tmp = match path.parent().filter(|p| !p.as_os_str().is_empty()) {
        Some(dir) => dir.join(&tmp_name),
        None => PathBuf::from(&tmp_name),
    };

    if let Err(e) = fill_temp(&tmp, contents) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }

    // Carry over the destination's mode so a restrictive history file does not
    // widen to whatever the umask grants a freshly created temp.
    if let Ok(meta) = fs::metadata(path) {
        let _ = fs::set_permissions(&tmp, meta.permissions());
    }

    fs::rename(&tmp, path).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

fn fill_temp(tmp: &Path, contents: &[u8]) -> io::Result<()> {
    let mut file = fs::File::create(tmp)?;
    file.write_all(contents)?;
    // `rename` only orders the directory entry; without this the rename can
    // land while the new contents are still sitting in the page cache.
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("agf-fsx-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn write_atomic_replaces_existing_content() {
        let dir = temp_dir("replace");
        let path = dir.join("history.jsonl");
        fs::write(&path, b"old\n").unwrap();

        write_atomic(&path, b"new\n").unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "new\n");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn write_atomic_creates_missing_file_and_leaves_no_temp() {
        let dir = temp_dir("create");
        let path = dir.join("history.jsonl");

        write_atomic(&path, b"fresh\n").unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "fresh\n");
        let leftovers: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .filter(|n| n.to_string_lossy().contains(".agf-tmp"))
            .collect();
        assert!(leftovers.is_empty(), "temp file left behind: {leftovers:?}");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn write_atomic_reports_error_without_touching_destination() {
        let dir = temp_dir("failure");
        let path = dir.join("nested").join("history.jsonl");
        // Parent does not exist: the temp create fails, and crucially the
        // destination is never truncated on the way to that failure.
        assert!(write_atomic(&path, b"x").is_err());
        assert!(!path.exists());
        let _ = fs::remove_dir_all(dir);
    }
}
