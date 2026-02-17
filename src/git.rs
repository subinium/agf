use std::path::Path;
use std::process::Command;

/// Check if a git repo at `project_path` has uncommitted changes.
/// Returns Some(true) if dirty, Some(false) if clean, None if not a git repo or error.
pub fn is_dirty(project_path: &str) -> Option<bool> {
    let path = Path::new(project_path);
    if !path.join(".git").exists() {
        return None;
    }

    let output = Command::new("git")
        .arg("-C")
        .arg(project_path)
        .args(["status", "--porcelain"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    Some(!output.stdout.is_empty())
}
