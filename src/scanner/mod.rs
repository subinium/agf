use std::thread;

use crate::model::Session;

pub mod claude;
pub mod codex;
pub mod cursor;

pub fn scan_all() -> Vec<Session> {
    let handles = vec![
        thread::spawn(|| claude::scan().unwrap_or_default()),
        thread::spawn(|| codex::scan().unwrap_or_default()),
        thread::spawn(|| cursor::scan().unwrap_or_default()),
    ];
    let mut sessions: Vec<Session> = handles
        .into_iter()
        .flat_map(|h| h.join().unwrap_or_default())
        .collect();

    // Populate git_dirty in parallel
    let dirty_handles: Vec<_> = sessions
        .iter()
        .map(|s| {
            let path = s.project_path.clone();
            thread::spawn(move || crate::git::is_dirty(&path))
        })
        .collect();
    for (session, handle) in sessions.iter_mut().zip(dirty_handles) {
        session.git_dirty = handle.join().unwrap_or(None);
    }

    sessions.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    sessions
}
