use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{App, Mode};
use crate::action;
use crate::model::Action;

pub enum InputResult {
    Continue,
    Quit,
    Execute(String),
}

pub fn handle_browse(app: &mut App, key: KeyEvent) -> InputResult {
    match (key.code, key.modifiers) {
        (KeyCode::Esc, _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => InputResult::Quit,

        (KeyCode::Up, _) | (KeyCode::Char('p'), KeyModifiers::CONTROL) => {
            if app.selected > 0 {
                app.selected -= 1;
                app.adjust_scroll();
            }
            InputResult::Continue
        }

        (KeyCode::Down, _) | (KeyCode::Char('n'), KeyModifiers::CONTROL) => {
            if !app.filtered_indices.is_empty() && app.selected < app.filtered_indices.len() - 1 {
                app.selected += 1;
                app.adjust_scroll();
            }
            InputResult::Continue
        }

        (KeyCode::Enter, _) => {
            if app.selected_session().is_some() {
                app.action_index = 0;
                app.mode = Mode::ActionSelect;
            }
            InputResult::Continue
        }

        (KeyCode::Right, _) => {
            if app.selected_session().is_some() {
                app.mode = Mode::Preview;
            }
            InputResult::Continue
        }

        (KeyCode::Char('s'), KeyModifiers::CONTROL) => {
            app.sort_mode = app.sort_mode.next();
            app.apply_sort();
            InputResult::Continue
        }

        (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
            app.selected_set.clear();
            app.mode = Mode::BulkDelete;
            InputResult::Continue
        }

        (KeyCode::Tab, _) => {
            app.cycle_agent_filter(true);
            InputResult::Continue
        }

        (KeyCode::BackTab, _) => {
            app.cycle_agent_filter(false);
            InputResult::Continue
        }

        (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
            app.query.clear();
            app.update_filter();
            InputResult::Continue
        }

        (KeyCode::Backspace, _) => {
            app.query.pop();
            app.update_filter();
            InputResult::Continue
        }

        (KeyCode::Char(c), modifiers) if !modifiers.contains(KeyModifiers::CONTROL) => {
            app.query.push(c);
            app.update_filter();
            InputResult::Continue
        }

        _ => InputResult::Continue,
    }
}

pub fn handle_action_select(app: &mut App, key: KeyEvent) -> InputResult {
    let actions = Action::MENU;

    match (key.code, key.modifiers) {
        (KeyCode::Esc, _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
            app.mode = Mode::Browse;
            InputResult::Continue
        }

        (KeyCode::Up, _) | (KeyCode::Char('p'), KeyModifiers::CONTROL) => {
            if app.action_index > 0 {
                app.action_index -= 1;
            }
            InputResult::Continue
        }

        (KeyCode::Down, _) | (KeyCode::Char('n'), KeyModifiers::CONTROL) => {
            if app.action_index < actions.len() - 1 {
                app.action_index += 1;
            }
            InputResult::Continue
        }

        (KeyCode::Char(c @ '1'..='5'), _) => {
            let idx = (c as usize) - ('1' as usize);
            if idx < actions.len() {
                app.action_index = idx;
            }
            dispatch_action(app, actions[app.action_index])
        }

        (KeyCode::Enter, _) => dispatch_action(app, actions[app.action_index]),

        _ => InputResult::Continue,
    }
}

fn dispatch_action(app: &mut App, selected_action: Action) -> InputResult {
    match selected_action {
        Action::Back => {
            app.mode = Mode::Browse;
            InputResult::Continue
        }
        Action::NewSession => {
            app.agent_index = 0;
            app.mode = Mode::AgentSelect;
            InputResult::Continue
        }
        Action::Delete => {
            app.delete_index = 1;
            app.mode = Mode::DeleteConfirm;
            InputResult::Continue
        }
        _ => {
            if let Some(session) = app.selected_session().cloned() {
                if let Some(cmd) = action::generate_command(&session, selected_action, None) {
                    return InputResult::Execute(cmd);
                }
            }
            InputResult::Continue
        }
    }
}

pub fn handle_agent_select(app: &mut App, key: KeyEvent) -> InputResult {
    let option_count = app.new_session_options.len();

    match (key.code, key.modifiers) {
        (KeyCode::Esc, _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
            app.mode = Mode::ActionSelect;
            InputResult::Continue
        }

        (KeyCode::Up, _) | (KeyCode::Char('p'), KeyModifiers::CONTROL) => {
            if app.agent_index > 0 {
                app.agent_index -= 1;
            }
            InputResult::Continue
        }

        (KeyCode::Down, _) | (KeyCode::Char('n'), KeyModifiers::CONTROL) => {
            if option_count > 0 && app.agent_index < option_count - 1 {
                app.agent_index += 1;
            }
            InputResult::Continue
        }

        (KeyCode::Char(c @ '1'..='9'), _) => {
            let idx = (c as usize) - ('1' as usize);
            if idx < option_count {
                app.agent_index = idx;
            }
            dispatch_agent_option(app)
        }

        (KeyCode::Tab, _) => {
            if let Some(opt) = app.new_session_options.get(app.agent_index) {
                app.mode_options = match opt.agent {
                    crate::model::Agent::ClaudeCode => vec![
                        ("default", ""),
                        ("acceptEdits", " --permission-mode acceptEdits"),
                        ("plan (read-only)", " --permission-mode plan"),
                        ("bypass permissions", " --dangerously-skip-permissions"),
                    ],
                    crate::model::Agent::Codex => vec![
                        ("suggest (default)", ""),
                        ("auto-edit", " -a untrusted"),
                        ("full-auto", " --full-auto"),
                        (
                            "bypass sandbox",
                            " --dangerously-bypass-approvals-and-sandbox",
                        ),
                    ],
                    crate::model::Agent::OpenCode => vec![("default", "")],
                };
                app.mode_index = 0;
                app.mode = super::Mode::PermissionSelect;
            }
            InputResult::Continue
        }

        (KeyCode::Enter, _) => dispatch_agent_option(app),

        _ => InputResult::Continue,
    }
}

fn dispatch_agent_option(app: &mut App) -> InputResult {
    if let Some(opt) = app.new_session_options.get(app.agent_index) {
        let agent = opt.agent;
        let suffix = opt.command_suffix;
        if let Some(session) = app.selected_session().cloned() {
            if let Some(cmd) = action::new_session_with_flags(&session, agent, suffix) {
                return InputResult::Execute(cmd);
            }
        }
    }
    InputResult::Continue
}

pub fn handle_mode_select(app: &mut App, key: KeyEvent) -> InputResult {
    let option_count = app.mode_options.len();

    match (key.code, key.modifiers) {
        (KeyCode::Esc, _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
            app.mode = super::Mode::AgentSelect;
            InputResult::Continue
        }

        (KeyCode::Up, _) | (KeyCode::Char('p'), KeyModifiers::CONTROL) => {
            if app.mode_index > 0 {
                app.mode_index -= 1;
            }
            InputResult::Continue
        }

        (KeyCode::Down, _) | (KeyCode::Char('n'), KeyModifiers::CONTROL) => {
            if option_count > 0 && app.mode_index < option_count - 1 {
                app.mode_index += 1;
            }
            InputResult::Continue
        }

        (KeyCode::Char(c @ '1'..='9'), _) => {
            let idx = (c as usize) - ('1' as usize);
            if idx < option_count {
                app.mode_index = idx;
            }
            dispatch_mode_option(app)
        }

        (KeyCode::Enter, _) => dispatch_mode_option(app),

        _ => InputResult::Continue,
    }
}

fn dispatch_mode_option(app: &mut App) -> InputResult {
    if let Some(&(_, flags)) = app.mode_options.get(app.mode_index) {
        if let Some(opt) = app.new_session_options.get(app.agent_index) {
            let agent = opt.agent;
            if let Some(session) = app.selected_session().cloned() {
                if let Some(cmd) = action::new_session_with_flags(&session, agent, flags) {
                    return InputResult::Execute(cmd);
                }
            }
        }
    }
    InputResult::Continue
}

pub fn handle_bulk_delete(app: &mut App, key: KeyEvent) -> InputResult {
    match (key.code, key.modifiers) {
        (KeyCode::Esc, _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
            app.selected_set.clear();
            app.mode = Mode::Browse;
            InputResult::Continue
        }

        (KeyCode::Up, _) | (KeyCode::Char('p'), KeyModifiers::CONTROL) => {
            if app.selected > 0 {
                app.selected -= 1;
                app.adjust_scroll();
            }
            InputResult::Continue
        }

        (KeyCode::Down, _) | (KeyCode::Char('n'), KeyModifiers::CONTROL) => {
            if !app.filtered_indices.is_empty() && app.selected < app.filtered_indices.len() - 1 {
                app.selected += 1;
                app.adjust_scroll();
            }
            InputResult::Continue
        }

        (KeyCode::Char(' '), _) => {
            if let Some(&idx) = app.filtered_indices.get(app.selected) {
                if !app.selected_set.remove(&idx) {
                    app.selected_set.insert(idx);
                }
            }
            // Move cursor down
            if !app.filtered_indices.is_empty() && app.selected < app.filtered_indices.len() - 1 {
                app.selected += 1;
                app.adjust_scroll();
            }
            InputResult::Continue
        }

        (KeyCode::Enter, _) => {
            if !app.selected_set.is_empty() {
                app.delete_index = 1; // Cancel default
                app.mode = Mode::DeleteConfirm;
            }
            InputResult::Continue
        }

        _ => InputResult::Continue,
    }
}

pub fn handle_delete_confirm(app: &mut App, key: KeyEvent) -> InputResult {
    let is_bulk = !app.selected_set.is_empty();

    match (key.code, key.modifiers) {
        (KeyCode::Esc, _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
            if is_bulk {
                app.mode = Mode::BulkDelete;
            } else {
                app.mode = Mode::ActionSelect;
            }
            InputResult::Continue
        }

        (KeyCode::Up, _) | (KeyCode::Down, _) | (KeyCode::Left, _) | (KeyCode::Right, _) => {
            app.delete_index = if app.delete_index == 0 { 1 } else { 0 };
            InputResult::Continue
        }

        (KeyCode::Enter, _) => {
            if app.delete_index == 0 {
                // Yes — delete
                if is_bulk {
                    // Bulk: sort indices descending to avoid invalidation
                    let mut indices: Vec<usize> = app.selected_set.drain().collect();
                    indices.sort_unstable_by(|a, b| b.cmp(a));
                    for idx in indices {
                        if idx < app.sessions.len() {
                            let _ = crate::delete::delete_session(&app.sessions[idx]);
                            app.sessions.remove(idx);
                        }
                    }
                    app.selected_set.clear();
                    app.update_filter();
                } else {
                    // Single delete (ActionSelect → Delete path)
                    if let Some(&idx) = app.filtered_indices.get(app.selected) {
                        let _ = crate::delete::delete_session(&app.sessions[idx]);
                        app.sessions.remove(idx);
                        app.update_filter();
                    }
                }
                app.mode = Mode::Browse;
            } else {
                // Cancel
                if is_bulk {
                    app.mode = Mode::BulkDelete;
                } else {
                    app.mode = Mode::Browse;
                }
            }
            InputResult::Continue
        }

        _ => InputResult::Continue,
    }
}

pub fn handle_preview(app: &mut App, key: KeyEvent) -> InputResult {
    match (key.code, key.modifiers) {
        (KeyCode::Esc, _)
        | (KeyCode::Char('c'), KeyModifiers::CONTROL)
        | (KeyCode::Left, _)
        | (KeyCode::Right, _) => {
            app.mode = Mode::Browse;
            InputResult::Continue
        }

        (KeyCode::Enter, _) => {
            app.action_index = 0;
            app.mode = Mode::ActionSelect;
            InputResult::Continue
        }

        _ => {
            app.mode = Mode::Browse;
            InputResult::Continue
        }
    }
}
