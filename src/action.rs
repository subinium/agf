use crate::model::{Action, Agent, Session};
use crate::shell::CommandShell;

pub fn generate_command(
    session: &Session,
    action: Action,
    new_agent: Option<Agent>,
) -> Option<String> {
    let shell = CommandShell::from_env();
    let quoted_path = shell.quote(&session.project_path);

    match action {
        Action::Resume => {
            let cmd = session.agent.resume_cmd(&session.session_id, &shell);
            let resume_path = resume_launch_path(session);
            Some(shell.cd_and(&shell.quote(resume_path), &cmd))
        }
        Action::NewSession => {
            let agent = new_agent.unwrap_or(session.agent);
            let cmd = agent.new_session_cmd();
            Some(shell.cd_and(&quoted_path, cmd))
        }
        Action::Open => {
            let editor = detect_editor();
            Some(shell.cd_and(&quoted_path, &format!("{editor} .")))
        }
        Action::Cd => Some(shell.cd_only(&quoted_path)),
        Action::Delete | Action::Back | Action::Pin => None,
    }
}

pub fn action_preview(session: &Session, action: Action) -> String {
    let shell = CommandShell::from_env();
    match action {
        Action::Resume => session.agent.resume_cmd(&session.session_id, &shell),
        Action::NewSession => "choose agent CLI...".to_string(),
        Action::Open => format!("{} .", detect_editor()),
        Action::Cd if session.project_path.is_empty() => "no project directory".to_string(),
        Action::Cd => shell.cd_only(&shell.quote(&session.display_path())),
        Action::Pin => "toggle pin".to_string(),
        Action::Delete => "remove session data".to_string(),
        Action::Back => "return to session list".to_string(),
    }
}

/// `cd`-prefixed preview of `cmd`, matching what the generated command will
/// actually do.
///
/// Previews use the `~`-shortened `display_path()` for readability but must
/// still *quote* it the way the executed command does, and must drop the `cd`
/// entirely for cwd-independent agents — `display_path()` renders their empty
/// `project_path` as `—`, which `cd_and`'s empty-path check cannot recognise,
/// so the preview used to read `cd — && hermes`.
pub fn preview_cd_and(shell: &CommandShell, session: &Session, cmd: &str) -> String {
    if session.project_path.is_empty() {
        return cmd.to_string();
    }
    shell.cd_and(&shell.quote(&session.display_path()), cmd)
}

/// Detect editor from config, then $EDITOR, then $VISUAL, fallback to "vim".
///
/// Resolved once per process and cached: `save_editable()` never writes the
/// `editor` key, so the value cannot change observably mid-run, and this is
/// called from the action-menu render path every frame (same rationale as
/// `CommandShell::from_env` in shell.rs).
pub fn detect_editor() -> String {
    use std::sync::OnceLock;
    static EDITOR: OnceLock<String> = OnceLock::new();
    EDITOR
        .get_or_init(|| {
            let config = crate::settings::Settings::load();
            if let Some(editor) = config.editor.filter(|e| !e.is_empty()) {
                return editor;
            }
            std::env::var("EDITOR")
                .ok()
                .filter(|e| !e.is_empty())
                .or_else(|| std::env::var("VISUAL").ok().filter(|e| !e.is_empty()))
                .unwrap_or_else(|| "vim".to_string())
        })
        .clone()
}

pub fn resume_with_flags(session: &Session, flags: &str) -> String {
    let shell = CommandShell::from_env();
    let quoted_path = shell.quote(resume_launch_path(session));
    let base_cmd = session.agent.resume_cmd(&session.session_id, &shell);
    shell.cd_and(&quoted_path, &format!("{base_cmd}{flags}"))
}

fn resume_launch_path(session: &Session) -> &str {
    if session.agent == Agent::PrimeAgent
        && !session.project_path.is_empty()
        && !std::path::Path::new(&session.project_path).is_dir()
    {
        // Prime Agent can resolve/fork an ID from another cwd. A guaranteed
        // failing `cd` prevents its own recovery prompt from running at all.
        ""
    } else {
        &session.project_path
    }
}

pub fn new_session_with_flags(session: &Session, agent: Agent, flags: &str) -> String {
    let shell = CommandShell::from_env();
    let quoted_path = shell.quote(&session.project_path);
    let base = agent.new_session_cmd();
    shell.cd_and(&quoted_path, &format!("{base}{flags}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Agent;

    fn session(project_path: &str) -> Session {
        Session {
            agent: Agent::Hermes,
            session_id: "sid".to_string(),
            project_name: "p".to_string(),
            project_path: project_path.to_string(),
            summaries: Vec::new(),
            timestamp: 0,
            git_branch: None,
            worktree: None,
            recap: None,
            interactive: true,
        }
    }

    /// Regression: the preview passed the raw `~`-shortened path straight into
    /// `cd_and`, so a path with a space rendered a command that would not run.
    #[test]
    fn preview_cd_and_quotes_the_path_like_the_real_command() {
        let shell = CommandShell::Posix;
        let s = session("/tmp/my project");
        assert_eq!(
            preview_cd_and(&shell, &s, "hermes"),
            "cd '/tmp/my project' && hermes"
        );
    }

    /// Regression: cwd-independent agents leave `project_path` empty, but
    /// `display_path()` renders that as `—`, which `cd_and`'s empty check
    /// cannot recognise — the preview used to read `cd — && hermes`.
    #[test]
    fn preview_cd_and_drops_the_cd_for_cwd_independent_agents() {
        let shell = CommandShell::Posix;
        assert_eq!(preview_cd_and(&shell, &session(""), "hermes"), "hermes");
    }

    #[test]
    fn cd_preview_reports_missing_directory_instead_of_a_broken_command() {
        assert_eq!(
            action_preview(&session(""), Action::Cd),
            "no project directory"
        );
    }

    #[test]
    fn prime_resume_skips_a_deleted_stored_cwd() {
        let mut s = session("/definitely/missing/agf-prime-project");
        s.agent = Agent::PrimeAgent;
        assert_eq!(
            generate_command(&s, Action::Resume, None).unwrap(),
            "prime-agent --resume 'sid'"
        );
    }
}
