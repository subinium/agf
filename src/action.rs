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
            // NOTE: Pi/Kiro CLI only resume latest; session_id ignored.
            let cmd = session.agent.resume_cmd(&session.session_id);
            Some(shell.cd_and(&quoted_path, &cmd))
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
    match action {
        Action::Resume => session.agent.resume_cmd(&session.session_id),
        Action::NewSession => "choose agent CLI...".to_string(),
        Action::Open => format!("{} .", detect_editor()),
        Action::Cd => CommandShell::from_env().cd_only(&session.display_path()),
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
    let shell = CommandShell::from_env();
    let quoted_path = shell.quote(&session.project_path);
    // NOTE: Pi/Kiro CLI only resume latest; session_id ignored.
    let base_cmd = session.agent.resume_cmd(&session.session_id);
    shell.cd_and(&quoted_path, &format!("{base_cmd}{flags}"))
}

pub fn new_session_with_flags(session: &Session, agent: Agent, flags: &str) -> Option<String> {
    let shell = CommandShell::from_env();
    let quoted_path = shell.quote(&session.project_path);
    let base = agent.new_session_cmd();
    Some(shell.cd_and(&quoted_path, &format!("{base}{flags}")))
}
