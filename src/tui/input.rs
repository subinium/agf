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

        (KeyCode::Enter, _) => {
            let selected_action = actions[app.action_index];
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
                        if let Some(cmd) = action::generate_command(&session, selected_action, None)
                        {
                            return InputResult::Execute(cmd);
                        }
                    }
                    InputResult::Continue
                }
            }
        }

        _ => InputResult::Continue,
    }
}

pub fn handle_agent_select(app: &mut App, key: KeyEvent) -> InputResult {
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
            if !app.installed_agents.is_empty() && app.agent_index < app.installed_agents.len() - 1
            {
                app.agent_index += 1;
            }
            InputResult::Continue
        }

        (KeyCode::Enter, _) => {
            if let Some(&agent) = app.installed_agents.get(app.agent_index) {
                if let Some(session) = app.selected_session().cloned() {
                    if let Some(cmd) =
                        action::generate_command(&session, Action::NewSession, Some(agent))
                    {
                        return InputResult::Execute(cmd);
                    }
                }
            }
            InputResult::Continue
        }

        _ => InputResult::Continue,
    }
}

pub fn handle_delete_confirm(app: &mut App, key: KeyEvent) -> InputResult {
    match (key.code, key.modifiers) {
        (KeyCode::Esc, _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
            app.mode = Mode::ActionSelect;
            InputResult::Continue
        }

        (KeyCode::Up, _) | (KeyCode::Down, _) | (KeyCode::Left, _) | (KeyCode::Right, _) => {
            app.delete_index = if app.delete_index == 0 { 1 } else { 0 };
            InputResult::Continue
        }

        (KeyCode::Enter, _) => {
            if app.delete_index == 0 {
                // Yes - delete the session data, then remove from list
                if let Some(&idx) = app.filtered_indices.get(app.selected) {
                    let _ = crate::delete::delete_session(&app.sessions[idx]);
                    app.sessions.remove(idx);
                    app.update_filter();
                }
            }
            app.mode = Mode::Browse;
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
