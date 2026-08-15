use crate::error::AgfError;
use crate::model::{Agent, Session};

pub fn scan() -> Result<Vec<Session>, AgfError> {
    let sessions_dir = crate::config::oh_my_pi_sessions_dir()?;
    // OMP stores task/subagent transcripts below each top-level session.
    // Its own global resume lookup only scans `*/*.jsonl`, so match that
    // boundary and don't surface nested files that `omp --resume` can't find.
    super::pi::scan_from(&sessions_dir, Agent::OhMyPi, Some(2))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;

    #[test]
    fn scans_oh_my_pi_session_with_title_slot() {
        let root = std::env::temp_dir().join(format!(
            "agf-omp-scan-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let session_dir = root.join("-tmp-project");
        fs::create_dir_all(&session_dir).unwrap();

        let mut file = fs::File::create(session_dir.join("session.jsonl")).unwrap();
        writeln!(
            file,
            r#"{{"type":"title","v":1,"title":"Fix scanner","updatedAt":"2026-07-27T10:00:00Z","pad":""}}"#
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"type":"session","version":3,"id":"omp-session","timestamp":"2026-07-27T10:00:00Z","cwd":"/tmp/project"}}"#
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"type":"message","message":{{"role":"user","content":[{{"type":"text","text":"Add OMP support"}}]}}}}"#
        )
        .unwrap();

        let nested_dir = session_dir.join("session");
        fs::create_dir_all(&nested_dir).unwrap();
        fs::write(
            nested_dir.join("subagent.jsonl"),
            r#"{"type":"session","id":"subagent","timestamp":"2026-07-27T10:01:00Z","cwd":"/tmp/project"}"#,
        )
        .unwrap();

        let sessions = super::super::pi::scan_from(&root, Agent::OhMyPi, Some(2)).unwrap();
        let _ = fs::remove_dir_all(&root);

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].agent, Agent::OhMyPi);
        assert_eq!(sessions[0].session_id, "omp-session");
        assert_eq!(sessions[0].project_path, "/tmp/project");
        assert_eq!(sessions[0].summaries, vec!["Add OMP support"]);
    }
}
