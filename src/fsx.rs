//! Crash-safe file writes.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

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
    let destination = resolve_symlink_target(path)?;
    let (tmp, mut file) = create_unique_temp(&destination)?;
    if let Err(error) = file.write_all(contents).and_then(|()| file.sync_all()) {
        drop(file);
        let _ = fs::remove_file(&tmp);
        return Err(error);
    }
    drop(file);

    // Carry over the destination's mode so a restrictive history file does not
    // widen to whatever the umask grants a freshly created temp.
    if let Ok(meta) = fs::metadata(&destination) {
        let _ = fs::set_permissions(&tmp, meta.permissions());
    }

    fs::rename(&tmp, &destination).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

/// Atomic rename replaces a directory entry, so renaming over a symlink would
/// destroy the link instead of updating its target. Resolve link chains first
/// (including relative dotfile-manager links) and create the sibling temp next
/// to the real destination.
fn resolve_symlink_target(path: &Path) -> io::Result<PathBuf> {
    let mut current = path.to_path_buf();
    for _ in 0..40 {
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                let target = fs::read_link(&current)?;
                current = if target.is_absolute() {
                    target
                } else {
                    current
                        .parent()
                        .unwrap_or_else(|| Path::new("."))
                        .join(target)
                };
            }
            Ok(_) => return Ok(current),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(current),
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        "atomic-write symlink chain is too deep",
    ))
}

fn create_unique_temp(path: &Path) -> io::Result<(PathBuf, fs::File)> {
    static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);
    let file_name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?
        .to_string_lossy();
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty());

    for _ in 0..128 {
        let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
        let name = format!(".{file_name}.agf-tmp-{}-{id}", std::process::id());
        let tmp = parent.map_or_else(|| PathBuf::from(&name), |dir| dir.join(&name));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&tmp) {
            Ok(file) => return Ok((tmp, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate a unique atomic-write temporary file",
    ))
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
            .filter(|n| n.to_string_lossy().contains(".agf-tmp-"))
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

    #[test]
    fn concurrent_atomic_writers_do_not_share_a_temp_file() {
        let dir = temp_dir("concurrent");
        let path = dir.join("cache.json");
        let handles: Vec<_> = (0..16)
            .map(|i| {
                let path = path.clone();
                std::thread::spawn(move || write_atomic(&path, format!("writer-{i}").as_bytes()))
            })
            .collect();
        for handle in handles {
            handle.join().unwrap().unwrap();
        }
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.starts_with("writer-"));
        let leftovers = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().contains(".agf-tmp-"))
            .count();
        assert_eq!(leftovers, 0);
    }

    #[test]
    #[cfg(unix)]
    fn write_atomic_preserves_symlink_and_updates_target() {
        use std::os::unix::fs::symlink;

        let dir = temp_dir("symlink");
        let target_dir = dir.join("dotfiles");
        fs::create_dir_all(&target_dir).unwrap();
        let target = target_dir.join("zshrc");
        fs::write(&target, b"old\n").unwrap();
        let link = dir.join(".zshrc");
        symlink("dotfiles/zshrc", &link).unwrap();

        write_atomic(&link, b"new\n").unwrap();

        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read_to_string(&target).unwrap(), "new\n");
        let _ = fs::remove_dir_all(dir);
    }
}
