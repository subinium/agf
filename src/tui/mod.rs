use std::collections::{HashMap, HashSet};
use std::io;

use crossterm::event::{self, Event};
use crossterm::execute;
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::prelude::*;

use crate::config::installed_agents;
use crate::fuzzy::FuzzyMatcher;
use crate::model::{Agent, Session, SortMode};

pub mod input;
pub mod render;

use input::InputResult;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mode {
    Browse,
    ActionSelect,
    AgentSelect,
    PermissionSelect, // permission/approval mode picker
    DeleteConfirm,
    BulkDelete,
    Preview,
    Help,
}

/// An option in the "New Session" agent picker.
#[derive(Debug, Clone)]
pub struct NewSessionOption {
    pub agent: Agent,
    pub label: String,
    pub command_suffix: &'static str, // extra flags appended to the base command
}

pub struct App {
    pub sessions: Vec<Session>,
    pub filtered_indices: Vec<usize>,
    pub match_positions: Vec<Vec<u32>>,
    pub selected: usize,
    pub query: String,
    pub mode: Mode,
    pub agent_filter: Option<Agent>,
    pub action_index: usize,
    pub agent_index: usize,
    pub delete_index: usize, // 0 = Yes, 1 = Cancel
    pub new_session_options: Vec<NewSessionOption>,
    pub mode_index: usize,
    pub mode_options: Vec<(&'static str, &'static str)>, // (label, flag)
    pub scroll_offset: usize,
    pub viewport_height: usize, // actual visible item count, set during render
    pub sort_mode: SortMode,
    pub selected_set: HashSet<usize>,
    pub summary_offsets: HashMap<String, usize>, // session_id -> summary offset
    pub summary_search_count: usize,
    pub include_summaries: bool,
    pub help_selected: usize, // 0 = search_scope, 1 = summary_search_count
    fuzzy: FuzzyMatcher,
}

impl App {
    pub fn new(
        sessions: Vec<Session>,
        initial_query: Option<String>,
        summary_search_count: usize,
        include_summaries: bool,
    ) -> Self {
        let agents = installed_agents();

        // Build new-session options: one per installed agent, sorted by session count (most used first)
        let mut agent_counts: std::collections::HashMap<Agent, usize> =
            std::collections::HashMap::new();
        for s in &sessions {
            *agent_counts.entry(s.agent).or_insert(0) += 1;
        }
        let mut sorted_agents = agents.clone();
        sorted_agents.sort_by(|a, b| {
            agent_counts
                .get(b)
                .unwrap_or(&0)
                .cmp(agent_counts.get(a).unwrap_or(&0))
        });
        let mut new_session_options = Vec::new();
        for agent in &sorted_agents {
            new_session_options.push(NewSessionOption {
                agent: *agent,
                label: format!("{agent}"),
                command_suffix: "",
            });
        }

        let filtered_indices: Vec<usize> = (0..sessions.len()).collect();
        let match_positions: Vec<Vec<u32>> = vec![Vec::new(); sessions.len()];
        let query = initial_query.unwrap_or_default();
        let mut app = Self {
            sessions,
            filtered_indices,
            match_positions,
            selected: 0,
            query,
            mode: Mode::Browse,
            agent_filter: None,
            action_index: 0,
            agent_index: 0,
            delete_index: 1,
            new_session_options,
            mode_index: 0,
            mode_options: Vec::new(),
            scroll_offset: 0,
            viewport_height: 4,
            sort_mode: SortMode::Time,
            selected_set: HashSet::new(),
            summary_offsets: HashMap::new(),
            summary_search_count,
            include_summaries,
            help_selected: 0,
            fuzzy: FuzzyMatcher::new(),
        };
        if !app.query.is_empty() {
            app.update_filter();
        }
        app
    }

    pub fn apply_sort(&mut self) {
        match self.sort_mode {
            SortMode::Time => self.sessions.sort_by(|a, b| b.timestamp.cmp(&a.timestamp)),
            SortMode::Name => self.sessions.sort_by(|a, b| {
                a.project_name
                    .to_lowercase()
                    .cmp(&b.project_name.to_lowercase())
            }),
            SortMode::Agent => self.sessions.sort_by(|a, b| {
                a.agent
                    .to_string()
                    .cmp(&b.agent.to_string())
                    .then(b.timestamp.cmp(&a.timestamp))
            }),
        }
        self.update_filter();
    }

    pub fn update_filter(&mut self) {
        let agent_filtered: Vec<usize> = self
            .sessions
            .iter()
            .enumerate()
            .filter(|(_, s)| match self.agent_filter {
                Some(agent) => s.agent == agent,
                None => true,
            })
            .map(|(i, _)| i)
            .collect();

        if self.query.is_empty() {
            self.match_positions = vec![Vec::new(); agent_filtered.len()];
            self.filtered_indices = agent_filtered;
        } else {
            let subset: Vec<Session> = agent_filtered
                .iter()
                .map(|&i| self.sessions[i].clone())
                .collect();

            let results = self.fuzzy.filter(
                &subset,
                &self.query,
                self.summary_search_count,
                self.include_summaries,
            );

            self.filtered_indices = results.iter().map(|r| agent_filtered[r.index]).collect();
            self.match_positions = results.iter().map(|r| r.positions.clone()).collect();
        }

        if self.filtered_indices.is_empty() {
            self.selected = 0;
        } else if self.selected >= self.filtered_indices.len() {
            self.selected = self.filtered_indices.len() - 1;
        }

        self.adjust_scroll();
    }

    pub fn selected_session(&self) -> Option<&Session> {
        self.filtered_indices
            .get(self.selected)
            .and_then(|&i| self.sessions.get(i))
    }

    /// Cycle the summary offset for the currently selected session.
    /// `forward` = true means go to an older summary (higher index).
    pub fn cycle_summary(&mut self, forward: bool) {
        let session = match self.selected_session() {
            Some(s) => s,
            None => return,
        };
        let count = session.summaries.len();
        if count <= 1 {
            return;
        }
        let id = session.session_id.clone();
        let offset = self.summary_offsets.get(&id).copied().unwrap_or(0);
        let new_offset = if forward {
            if offset + 1 < count {
                offset + 1
            } else {
                offset
            }
        } else {
            offset.saturating_sub(1)
        };
        self.summary_offsets.insert(id, new_offset);
    }

    pub fn save_settings(&self) {
        let settings = crate::settings::Settings {
            sort_by: None,
            max_sessions: None,
            summary_search_count: self.summary_search_count,
            search_scope: if self.include_summaries {
                "all".to_string()
            } else {
                "name_path".to_string()
            },
        };
        settings.save_editable();
    }

    pub fn adjust_scroll(&mut self) {
        if self.filtered_indices.is_empty() {
            self.scroll_offset = 0;
            return;
        }
        let visible = self.viewport_height.max(1);
        if self.selected < self.scroll_offset {
            self.scroll_offset = self.selected;
        } else if self.selected >= self.scroll_offset + visible {
            self.scroll_offset = self.selected - visible + 1;
        }
        // Clamp scroll offset
        let max_offset = self.filtered_indices.len().saturating_sub(visible);
        if self.scroll_offset > max_offset {
            self.scroll_offset = max_offset;
        }
    }

    /// Returns agents that have at least one session in the current list.
    fn agents_with_sessions(&self) -> Vec<Agent> {
        let mut seen = HashSet::new();
        for s in &self.sessions {
            seen.insert(s.agent);
        }
        // Preserve ordering from Agent::all()
        Agent::all()
            .iter()
            .copied()
            .filter(|a| seen.contains(a))
            .collect()
    }

    pub fn cycle_agent_filter(&mut self, forward: bool) {
        let available = self.agents_with_sessions();
        if forward {
            self.agent_filter = match self.agent_filter {
                None => available.first().copied(),
                Some(current) => {
                    let pos = available.iter().position(|a| *a == current).unwrap_or(0);
                    if pos + 1 < available.len() {
                        Some(available[pos + 1])
                    } else {
                        None
                    }
                }
            };
        } else {
            self.agent_filter = match self.agent_filter {
                None => available.last().copied(),
                Some(current) => {
                    let pos = available.iter().position(|a| *a == current).unwrap_or(0);
                    if pos > 0 {
                        Some(available[pos - 1])
                    } else {
                        None
                    }
                }
            };
        }
        self.update_filter();
    }

    pub fn run(&mut self) -> anyhow::Result<Option<String>> {
        crossterm::terminal::enable_raw_mode()?;
        let mut stderr = io::stderr();
        execute!(stderr, EnterAlternateScreen)?;

        let backend = CrosstermBackend::new(stderr);
        let mut terminal = Terminal::new(backend)?;

        let result = self.event_loop(&mut terminal);

        crossterm::terminal::disable_raw_mode()?;
        execute!(terminal.backend_mut(), LeaveAlternateScreen)?;

        result
    }

    fn event_loop(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<io::Stderr>>,
    ) -> anyhow::Result<Option<String>> {
        loop {
            terminal.draw(|f| {
                // Update viewport_height based on actual terminal size
                let area = f.area();
                // header(3) + footer(1) = 4 rows overhead, 1 line per session
                let list_height = area.height.saturating_sub(4) as usize;
                self.viewport_height = list_height.max(1);

                match self.mode {
                    Mode::Browse => render::render_browse(f, self),
                    Mode::ActionSelect => render::render_action_select(f, self),
                    Mode::AgentSelect => render::render_agent_select(f, self),
                    Mode::PermissionSelect => render::render_mode_select(f, self),
                    Mode::DeleteConfirm => render::render_delete_confirm(f, self),
                    Mode::BulkDelete => render::render_bulk_delete(f, self),
                    Mode::Preview => render::render_preview(f, self),
                    Mode::Help => render::render_help(f, self),
                }
            })?;

            if let Event::Key(key) = event::read()? {
                let result = match self.mode {
                    Mode::Browse => input::handle_browse(self, key),
                    Mode::ActionSelect => input::handle_action_select(self, key),
                    Mode::AgentSelect => input::handle_agent_select(self, key),
                    Mode::PermissionSelect => input::handle_mode_select(self, key),
                    Mode::DeleteConfirm => input::handle_delete_confirm(self, key),
                    Mode::BulkDelete => input::handle_bulk_delete(self, key),
                    Mode::Preview => input::handle_preview(self, key),
                    Mode::Help => input::handle_help(self, key),
                };

                match result {
                    InputResult::Continue => {}
                    InputResult::Quit => return Ok(None),
                    InputResult::Execute(cmd) => return Ok(Some(cmd)),
                }
            }
        }
    }
}
