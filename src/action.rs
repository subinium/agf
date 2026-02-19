use crate::model::{Action, Agent, Session};

pub fn generate_command(
    session: &Session,
    action: Action,
    new_agent: Option<Agent>,
) -> Option<String> {
    let escaped_path = shell_escape(&session.project_path);

    match action {
        Action::Resume => {
            let cmd = session.agent.resume_cmd(&session.session_id);
            Some(format!("cd {escaped_path} && {cmd}"))
        }
        Action::NewSession => {
            let agent = new_agent.unwrap_or(session.agent);
            let cmd = agent.new_session_cmd();
            Some(format!("cd {escaped_path} && {cmd}"))
        }
        Action::Cd => Some(format!("cd {escaped_path}")),
        Action::Delete | Action::Back => None,
    }
}

pub fn action_preview(session: &Session, action: Action) -> String {
    match action {
        Action::Resume => session.agent.resume_cmd(&session.session_id),
        Action::NewSession => "choose agent CLI...".to_string(),
        Action::Cd => format!("cd {}", session.display_path()),
        Action::Delete => "remove session data".to_string(),
        Action::Back => "return to session list".to_string(),
    }
}

pub fn new_session_with_flags(session: &Session, agent: Agent, flags: &str) -> Option<String> {
    let escaped_path = shell_escape(&session.project_path);
    let base = agent.new_session_cmd();
    Some(format!("cd {escaped_path} && {base}{flags}"))
}

fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}
