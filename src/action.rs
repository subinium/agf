use crate::model::{Action, Agent, Session};

pub fn generate_command(
    session: &Session,
    action: Action,
    new_agent: Option<Agent>,
) -> Option<String> {
    let escaped_path = shell_escape(&session.project_path);

    match action {
        Action::Resume => {
            let cmd = match session.agent {
                Agent::ClaudeCode => format!("claude --resume '{}'", session.session_id),
                Agent::Codex => format!("codex resume '{}'", session.session_id),
            };
            Some(format!("cd {escaped_path} && {cmd}"))
        }
        Action::NewSession => {
            let agent = new_agent.unwrap_or(session.agent);
            let cmd = match agent {
                Agent::ClaudeCode => "claude".to_string(),
                Agent::Codex => "codex".to_string(),
            };
            Some(format!("cd {escaped_path} && {cmd}"))
        }
        Action::Cd => Some(format!("cd {escaped_path}")),
        Action::Delete | Action::Back => None,
    }
}

pub fn action_preview(session: &Session, action: Action) -> String {
    match action {
        Action::Resume => match session.agent {
            Agent::ClaudeCode => format!("claude --resume '{}'", session.session_id),
            Agent::Codex => format!("codex resume '{}'", session.session_id),
        },
        Action::NewSession => "choose agent CLI...".to_string(),
        Action::Cd => format!("cd {}", session.display_path()),
        Action::Delete => "remove session data".to_string(),
        Action::Back => "return to session list".to_string(),
    }
}

pub fn new_session_with_flags(session: &Session, agent: Agent, flags: &str) -> Option<String> {
    let escaped_path = shell_escape(&session.project_path);
    let base = match agent {
        Agent::ClaudeCode => "claude",
        Agent::Codex => "codex",
    };
    Some(format!("cd {escaped_path} && {base}{flags}"))
}

fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}
