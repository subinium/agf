use crate::model::{Action, Agent, Session};

pub fn generate_command(
    session: &Session,
    action: Action,
    new_agent: Option<Agent>,
) -> Option<String> {
    let escaped_path = shell_escape(&session.project_path);

    match action {
        Action::Resume => {
            // NOTE: Pi/Kiro CLI only resume latest; session_id ignored.
            let cmd = session.agent.resume_cmd(&session.session_id);
            Some(format!("cd {escaped_path} && {cmd}"))
        }
        Action::NewSession => {
            let agent = new_agent.unwrap_or(session.agent);
            let cmd = agent.new_session_cmd();
            Some(format!("cd {escaped_path} && {cmd}"))
        }
        Action::Open => {
            let editor = detect_editor();
            Some(format!("cd {escaped_path} && {editor} ."))
        }
        Action::Cd => Some(format!("cd {escaped_path}")),
        Action::Delete | Action::Back | Action::Pin => None,
    }
}

pub fn action_preview(session: &Session, action: Action) -> String {
    match action {
        Action::Resume => session.agent.resume_cmd(&session.session_id),
        Action::NewSession => "choose agent CLI...".to_string(),
        Action::Open => format!("{} .", detect_editor()),
        Action::Cd => format!("cd {}", session.display_path()),
        Action::Pin => "toggle pin".to_string(),
        Action::Delete => "remove session data".to_string(),
        Action::Back => "return to session list".to_string(),
    }
}

/// Detect editor from config, then $EDITOR, then $VISUAL, fallback to "vim".
pub fn detect_editor() -> String {
    let config = crate::settings::Settings::load();
    if let Some(ref editor) = config.editor {
        if !editor.is_empty() {
            return editor.clone();
        }
    }
    if let Ok(editor) = std::env::var("EDITOR") {
        if !editor.is_empty() {
            return editor;
        }
    }
    if let Ok(editor) = std::env::var("VISUAL") {
        if !editor.is_empty() {
            return editor;
        }
    }
    "vim".to_string()
}

pub fn resume_with_flags(session: &Session, flags: &str) -> String {
    let escaped_path = shell_escape(&session.project_path);
    // NOTE: Pi/Kiro CLI only resume latest; session_id ignored.
    let base_cmd = session.agent.resume_cmd(&session.session_id);
    format!("cd {escaped_path} && {base_cmd}{flags}")
}

pub fn new_session_with_flags(session: &Session, agent: Agent, flags: &str) -> Option<String> {
    let escaped_path = shell_escape(&session.project_path);
    let base = agent.new_session_cmd();
    Some(format!("cd {escaped_path} && {base}{flags}"))
}

fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}
