use std::collections::{HashMap, HashSet};

use unicode_width::UnicodeWidthStr;

use crate::action;
use crate::config::installed_agents;
use crate::fuzzy::FuzzyMatcher;
use crate::model::{Action, Agent, Session, SortMode};

// Color constants
const HIGHLIGHT_BG: slt::Color = slt::Color::Rgb(59, 59, 59);
const BRIGHT_WHITE: slt::Color = slt::Color::Rgb(229, 229, 229);
const GRAY_500: slt::Color = slt::Color::Rgb(107, 114, 128);
const GRAY_400: slt::Color = slt::Color::Rgb(163, 163, 163);
const VIOLET: slt::Color = slt::Color::Rgb(139, 92, 246);
const YELLOW: slt::Color = slt::Color::Rgb(245, 158, 11);
const SEPARATOR: slt::Color = slt::Color::Rgb(64, 64, 64);
const RED: slt::Color = slt::Color::Rgb(239, 68, 68);
const GREEN_400: slt::Color = slt::Color::Rgb(52, 211, 153);
const CYAN: slt::Color = slt::Color::Rgb(34, 211, 238);

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mode {
    Browse,
    GroupedBrowse,
    ActionSelect,
    AgentSelect,
    PermissionSelect,
    ResumeSelect,
    DeleteConfirm,
    BulkDelete,
    Preview,
    Help,
}

#[derive(Debug, Clone)]
pub struct ProjectGroup {
    pub project_path: String,
    pub project_name: String,
    pub sessions: Vec<usize>, // indices into App.sessions
}

#[derive(Debug, Clone)]
pub struct NewSessionOption {
    pub agent: Agent,
    pub label: String,
    pub command_suffix: &'static str,
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
    pub delete_index: usize,
    pub new_session_options: Vec<NewSessionOption>,
    pub mode_index: usize,
    pub mode_options: Vec<(&'static str, &'static str)>,
    pub resume_mode_index: usize,
    pub resume_mode_options: Vec<(&'static str, &'static str)>,
    pub scroll_offset: usize,
    pub viewport_height: usize,
    pub sort_mode: SortMode,
    pub selected_set: HashSet<usize>,
    pub summary_offsets: HashMap<String, usize>,
    pub summary_search_count: usize,
    pub include_summaries: bool,
    pub show_recap: bool,
    pub help_selected: usize,
    pub search_textarea: slt::TextareaState,
    pub cwd: Option<String>,
    pub agent_counts: HashMap<Agent, usize>,
    pub pinned_sessions: Vec<String>,
    pub settings: crate::settings::Settings,
    pub groups: Vec<ProjectGroup>,
    pub group_expanded: HashSet<String>,
    pub grouped_selected: usize,
    pub grouped_scroll: usize,
    fuzzy: FuzzyMatcher,
}

impl App {
    pub fn new(
        sessions: Vec<Session>,
        initial_query: Option<String>,
        summary_search_count: usize,
        include_summaries: bool,
        cwd: Option<String>,
        pinned_sessions: Vec<String>,
    ) -> Self {
        let agents = installed_agents();

        let mut agent_counts: HashMap<Agent, usize> = HashMap::new();
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
        let search_textarea = {
            let mut ta = slt::TextareaState::new();
            if !query.is_empty() {
                ta.lines = vec![query.clone()];
                ta.cursor_col = query.chars().count();
            }
            ta
        };
        let settings = crate::settings::Settings::load();
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
            resume_mode_index: 0,
            resume_mode_options: Vec::new(),
            scroll_offset: 0,
            viewport_height: 4,
            sort_mode: SortMode::Time,
            selected_set: HashSet::new(),
            summary_offsets: HashMap::new(),
            summary_search_count,
            include_summaries,
            show_recap: settings.show_recap,
            help_selected: 0,
            search_textarea,
            cwd,
            agent_counts,
            pinned_sessions,
            settings,
            groups: Vec::new(),
            group_expanded: HashSet::new(),
            grouped_selected: 0,
            grouped_scroll: 0,
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

        // Boost: pinned first, then $PWD matches (stable sort preserves primary order within groups)
        let pinned = self.pinned_sessions.clone();
        let cwd = self.cwd.clone();
        self.sessions.sort_by(|a, b| {
            let rank = |s: &Session| -> u8 {
                if pinned.contains(&s.session_id) {
                    0
                } else if cwd.as_ref().is_some_and(|c| s.project_path == *c) {
                    1
                } else {
                    2
                }
            };
            rank(a).cmp(&rank(b))
        });

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
        let mut settings = self.settings.clone();
        settings.summary_search_count = self.summary_search_count;
        settings.search_scope = if self.include_summaries {
            "all".to_string()
        } else {
            "name_path".to_string()
        };
        settings.pinned_sessions = self.pinned_sessions.clone();
        settings.show_recap = self.show_recap;
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
        let max_offset = self.filtered_indices.len().saturating_sub(visible);
        if self.scroll_offset > max_offset {
            self.scroll_offset = max_offset;
        }
    }

    pub fn build_groups(&mut self) {
        let mut map: std::collections::BTreeMap<String, Vec<usize>> =
            std::collections::BTreeMap::new();
        for &idx in &self.filtered_indices {
            let s = &self.sessions[idx];
            map.entry(s.project_path.clone()).or_default().push(idx);
        }
        self.groups = map
            .into_iter()
            .map(|(path, sessions)| {
                let name = std::path::Path::new(&path)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown")
                    .to_string();
                ProjectGroup {
                    project_path: path,
                    project_name: name,
                    sessions,
                }
            })
            .collect();
        // Sort groups: most recent session first
        self.groups.sort_by(|a, b| {
            let a_ts = a
                .sessions
                .first()
                .map(|&i| self.sessions[i].timestamp)
                .unwrap_or(0);
            let b_ts = b
                .sessions
                .first()
                .map(|&i| self.sessions[i].timestamp)
                .unwrap_or(0);
            b_ts.cmp(&a_ts)
        });
    }

    /// Count total visible rows in grouped view (headers + expanded children)
    fn grouped_row_count(&self) -> usize {
        self.groups
            .iter()
            .map(|g| {
                if self.group_expanded.contains(&g.project_path) {
                    1 + g.sessions.len()
                } else {
                    1
                }
            })
            .sum()
    }

    /// Map a flat row index to (group_index, None) for header or (group_index, Some(child_index))
    fn grouped_row_at(&self, row: usize) -> Option<(usize, Option<usize>)> {
        let mut current = 0;
        for (gi, g) in self.groups.iter().enumerate() {
            if current == row {
                return Some((gi, None));
            }
            current += 1;
            if self.group_expanded.contains(&g.project_path) {
                for ci in 0..g.sessions.len() {
                    if current == row {
                        return Some((gi, Some(ci)));
                    }
                    current += 1;
                }
            }
        }
        None
    }

    fn agents_with_sessions(&self) -> Vec<Agent> {
        let mut seen = HashSet::new();
        for s in &self.sessions {
            seen.insert(s.agent);
        }
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
        let mut result: Option<String> = None;
        let app = self;
        slt::run_with(
            slt::RunConfig::default().title("agf").mouse(true),
            |ui: &mut slt::Context| {
                app.viewport_height = (ui.height() as usize).saturating_sub(4).max(1);
                app.adjust_scroll();
                match app.mode {
                    Mode::Browse => ui_browse(ui, app),
                    Mode::GroupedBrowse => ui_grouped_browse(ui, app),
                    Mode::ActionSelect => ui_action_select(ui, app, &mut result),
                    Mode::AgentSelect => ui_agent_select(ui, app, &mut result),
                    Mode::PermissionSelect => ui_permission_select(ui, app, &mut result),
                    Mode::ResumeSelect => ui_resume_select(ui, app, &mut result),
                    Mode::DeleteConfirm => ui_delete_confirm(ui, app),
                    Mode::BulkDelete => ui_bulk_delete(ui, app),
                    Mode::Preview => ui_preview(ui, app),
                    Mode::Help => ui_help(ui, app),
                }
            },
        )?;
        Ok(result)
    }
}

type StyledChunk = (String, slt::Style);

fn agent_color(agent: Agent) -> slt::Color {
    let (r, g, b) = agent.color();
    slt::Color::Rgb(r, g, b)
}

fn ui_browse(ui: &mut slt::Context, app: &mut App) {
    // --- Consume keys that conflict with textarea BEFORE rendering ---
    // Consume Esc/Enter/Up/Down so textarea doesn't process them
    let esc = ui.consume_key_code(slt::KeyCode::Esc);
    let enter = ui.consume_key_code(slt::KeyCode::Enter);
    let up = ui.consume_key_code(slt::KeyCode::Up);
    let down = ui.consume_key_code(slt::KeyCode::Down);
    let right = ui.consume_key_code(slt::KeyCode::Right);
    let tab = ui.consume_key_code(slt::KeyCode::Tab);
    let backtab = ui.consume_key_code(slt::KeyCode::BackTab);

    // Ctrl+letter: consume the char so textarea doesn't insert it
    let ctrl_up =
        ui.key_mod('p', slt::KeyModifiers::CONTROL) || ui.key_mod('k', slt::KeyModifiers::CONTROL);
    let ctrl_down =
        ui.key_mod('n', slt::KeyModifiers::CONTROL) || ui.key_mod('j', slt::KeyModifiers::CONTROL);
    let ctrl_sort = ui.key_mod('s', slt::KeyModifiers::CONTROL);
    let ctrl_bulk = ui.key_mod('d', slt::KeyModifiers::CONTROL);
    let ctrl_clear = ui.key_mod('u', slt::KeyModifiers::CONTROL);
    let ctrl_right = ui.key_mod('l', slt::KeyModifiers::CONTROL);
    let ctrl_group = ui.key_mod('g', slt::KeyModifiers::CONTROL);
    // Consume ctrl chars to prevent textarea insertion
    if ctrl_up {
        ui.consume_key('p');
        ui.consume_key('k');
    }
    if ctrl_down {
        ui.consume_key('n');
        ui.consume_key('j');
    }
    if ctrl_sort {
        ui.consume_key('s');
    }
    if ctrl_bulk {
        ui.consume_key('d');
    }
    if ctrl_clear {
        ui.consume_key('u');
    }
    if ctrl_right {
        ui.consume_key('l');
    }
    if ctrl_group {
        ui.consume_key('g');
    }

    // Consume special chars that have bindings
    let help = ui.consume_key('?');
    let summary_prev = ui.consume_key('[');
    let summary_next = ui.consume_key(']');

    // --- Handle key actions ---
    if esc {
        ui.quit();
    }
    if help {
        app.mode = Mode::Help;
    }
    if summary_prev {
        app.cycle_summary(true);
    }
    if summary_next {
        app.cycle_summary(false);
    }
    if (up || ctrl_up) && app.selected > 0 {
        app.selected -= 1;
        app.adjust_scroll();
    }
    if (down || ctrl_down)
        && !app.filtered_indices.is_empty()
        && app.selected < app.filtered_indices.len() - 1
    {
        app.selected += 1;
        app.adjust_scroll();
    }
    if enter && app.selected_session().is_some() {
        app.action_index = 0;
        app.mode = Mode::ActionSelect;
    }
    if (right || ctrl_right) && app.selected_session().is_some() {
        app.mode = Mode::Preview;
    }
    if ctrl_sort {
        app.sort_mode = app.sort_mode.next();
        app.apply_sort();
    }
    if ctrl_bulk {
        app.selected_set.clear();
        app.mode = Mode::BulkDelete;
    }
    if ctrl_group {
        app.build_groups();
        app.grouped_selected = 0;
        app.grouped_scroll = 0;
        app.mode = Mode::GroupedBrowse;
    }
    if tab {
        app.cycle_agent_filter(true);
    }
    if backtab {
        app.cycle_agent_filter(false);
    }
    if ctrl_clear {
        app.search_textarea.lines = vec![String::new()];
        app.search_textarea.cursor_col = 0;
        app.query.clear();
        app.update_filter();
    }

    // Mouse: scroll
    if ui.scroll_up() && app.selected > 0 {
        app.selected -= 1;
        app.adjust_scroll();
    }
    if ui.scroll_down()
        && !app.filtered_indices.is_empty()
        && app.selected < app.filtered_indices.len() - 1
    {
        app.selected += 1;
        app.adjust_scroll();
    }

    // Mouse: click on session row (search=1 + separator=1, list starts at y=2)
    if let Some((_x, y)) = ui.mouse_down() {
        let y = y as usize;
        if y >= 2 {
            let clicked_vi = app.scroll_offset + (y - 2);
            if clicked_vi < app.filtered_indices.len() {
                app.selected = clicked_vi;
                app.adjust_scroll();
                app.action_index = 0;
                app.mode = Mode::ActionSelect;
            }
        }
    }

    // --- Render ---
    // Consistent 2-char left margin for all sections (matches "> " indicator width)
    let is_compact = matches!(ui.breakpoint(), slt::Breakpoint::Xs);
    let _ = ui.col(|ui| {
        // Top spacing
        ui.text("");

        // Search bar: "  " indent + textarea + badge
        let _ = ui.container().pl(2).pr(1).row(|ui| {
            let _ = ui.container().grow(1).row(|ui| {
                let _ = ui.textarea(&mut app.search_textarea, 1);
            });
            match app.agent_filter {
                Some(agent) => {
                    let count = app.agent_counts.get(&agent).copied().unwrap_or(0);
                    let _ = ui.badge_colored(&format!("{agent} ({count})"), agent_color(agent));
                }
                None => {
                    let total = app.sessions.len();
                    let _ = ui.badge(&format!("All ({total})"));
                }
            };
        });

        ui.separator_colored(SEPARATOR);

        // Session list (rows have "> " or "  " prefix built-in)
        let _ = ui.container().grow(1).pr(1).col(|ui| {
            if app.filtered_indices.is_empty() {
                let _ = ui.container().pl(2).col(|ui| {
                    let _ = ui.empty_state(
                        "No sessions found",
                        "Try a different search or agent filter",
                    );
                });
            } else if is_compact {
                render_session_list_compact(ui, app);
            } else {
                render_session_list(ui, app, false);
            }
        });

        // Sort info (same 2-char indent)
        let total = app.sessions.len();
        let filtered = app.filtered_indices.len();
        let _ = ui.container().pl(2).pr(1).row(|ui| {
            ui.text(format!("{filtered}/{total}")).fg(GRAY_500);
            if let Some(agent) = app.agent_filter {
                ui.text(" ").fg(GRAY_500);
                let _ = ui.badge_colored(&agent.to_string(), agent_color(agent));
            }
            ui.text(format!(" sort:{}", app.sort_mode.label()))
                .fg(GRAY_500);
        });

        // Separator between content and statusbar
        ui.separator_colored(SEPARATOR);

        // Help bar (dim, right-aligned)
        let _ = ui.container().pr(1).row(|ui| {
            ui.spacer();
            let _ = ui.help_colored(
                &[
                    ("↑↓", "nav"),
                    ("Tab", "agent"),
                    ("[/]", "summary"),
                    ("→", "detail"),
                    ("Enter", "select"),
                    ("^S", "sort"),
                    ("^G", "group"),
                    ("^D", "delete"),
                    ("?", "help"),
                    ("Esc", "quit"),
                ],
                GRAY_500,
                SEPARATOR,
            );
        });
    });

    // Sync textarea → query (textarea stores lines, we use first line only)
    let textarea_text = app
        .search_textarea
        .lines
        .first()
        .cloned()
        .unwrap_or_default();
    // Strip newlines in case textarea somehow got multi-line
    let clean_text: String = textarea_text.chars().filter(|c| *c != '\n').collect();
    if clean_text != app.query {
        app.query = clean_text;
        app.update_filter();
    }
    // Keep textarea single-line
    if app.search_textarea.lines.len() > 1 {
        let merged: String = app.search_textarea.lines.join("");
        app.search_textarea.lines = vec![merged.clone()];
        app.search_textarea.cursor_row = 0;
        app.search_textarea.cursor_col = merged.chars().count();
    }
}

fn ui_grouped_browse(ui: &mut slt::Context, app: &mut App) {
    let esc = ui.consume_key_code(slt::KeyCode::Esc);
    let enter = ui.consume_key_code(slt::KeyCode::Enter);
    let up = ui.consume_key_code(slt::KeyCode::Up);
    let down = ui.consume_key_code(slt::KeyCode::Down);
    let space = ui.consume_key(' ');
    let ctrl_up =
        ui.key_mod('p', slt::KeyModifiers::CONTROL) || ui.key_mod('k', slt::KeyModifiers::CONTROL);
    let ctrl_down =
        ui.key_mod('n', slt::KeyModifiers::CONTROL) || ui.key_mod('j', slt::KeyModifiers::CONTROL);
    let ctrl_group = ui.key_mod('g', slt::KeyModifiers::CONTROL);
    if ctrl_up {
        ui.consume_key('p');
        ui.consume_key('k');
    }
    if ctrl_down {
        ui.consume_key('n');
        ui.consume_key('j');
    }
    if ctrl_group {
        ui.consume_key('g');
    }

    if esc || ctrl_group {
        app.mode = Mode::Browse;
        return;
    }

    let total_rows = app.grouped_row_count();
    if (up || ctrl_up) && app.grouped_selected > 0 {
        app.grouped_selected -= 1;
    }
    if (down || ctrl_down) && app.grouped_selected + 1 < total_rows {
        app.grouped_selected += 1;
    }

    // Enter/Space on header: toggle expand. Enter on child: open action menu.
    if enter || space {
        if let Some((gi, child)) = app.grouped_row_at(app.grouped_selected) {
            match child {
                None => {
                    let path = app.groups[gi].project_path.clone();
                    if app.group_expanded.contains(&path) {
                        app.group_expanded.remove(&path);
                    } else {
                        app.group_expanded.insert(path);
                    }
                }
                Some(ci) => {
                    let session_idx = app.groups[gi].sessions[ci];
                    // Find this session in filtered_indices to set app.selected
                    if let Some(vi) = app.filtered_indices.iter().position(|&i| i == session_idx) {
                        app.selected = vi;
                        app.action_index = 0;
                        app.mode = Mode::ActionSelect;
                    }
                }
            }
        }
    }

    // Scroll
    let visible = app.viewport_height.max(1);
    if app.grouped_selected < app.grouped_scroll {
        app.grouped_scroll = app.grouped_selected;
    } else if app.grouped_selected >= app.grouped_scroll + visible {
        app.grouped_scroll = app.grouped_selected - visible + 1;
    }

    // --- Render ---
    let _ = ui.col(|ui| {
        ui.text("");
        let _ = ui.container().pl(2).pr(1).row(|ui| {
            ui.text("Project View").fg(BRIGHT_WHITE).bold();
            ui.spacer();
            let total_projects = app.groups.len();
            let total_sessions = app.filtered_indices.len();
            ui.text(format!(
                "{total_projects} projects, {total_sessions} sessions"
            ))
            .fg(GRAY_500);
        });
        ui.separator_colored(SEPARATOR);

        let _ = ui.container().grow(1).pr(1).col(|ui| {
            if app.groups.is_empty() {
                let _ = ui.container().pl(2).col(|ui| {
                    let _ = ui.empty_state("No projects", "Try a different filter");
                });
                return;
            }

            let total_width = ui.width() as usize;
            let end = (app.grouped_scroll + visible).min(total_rows);
            let mut row_idx = 0;
            for group in app.groups.iter() {
                let expanded = app.group_expanded.contains(&group.project_path);
                let session_count = group.sessions.len();

                // Get most recent timestamp for the group
                let latest_time = group
                    .sessions
                    .first()
                    .map(|&i| app.sessions[i].time_display())
                    .unwrap_or_default();
                // Agents in this group
                let mut agent_set: Vec<Agent> = Vec::new();
                for &idx in &group.sessions {
                    let a = app.sessions[idx].agent;
                    if !agent_set.contains(&a) {
                        agent_set.push(a);
                    }
                }

                // Header row
                if row_idx >= app.grouped_scroll && row_idx < end {
                    let is_selected = row_idx == app.grouped_selected;
                    let bg = if is_selected {
                        HIGHLIGHT_BG
                    } else {
                        slt::Color::Reset
                    };
                    let arrow = if expanded { "\u{25be}" } else { "\u{25b8}" };
                    let display_path = if let Some(home) = dirs::home_dir() {
                        if let Some(rest) =
                            group.project_path.strip_prefix(home.to_str().unwrap_or(""))
                        {
                            format!("~{rest}")
                        } else {
                            group.project_path.clone()
                        }
                    } else {
                        group.project_path.clone()
                    };

                    let _ = ui.row(|ui| {
                        ui.styled(format!(" {arrow} "), slt::Style::new().fg(GRAY_400).bg(bg));
                        ui.styled(
                            group.project_name.clone(),
                            slt::Style::new().fg(BRIGHT_WHITE).bold().bg(bg),
                        );
                        ui.styled(
                            format!(" ({session_count})"),
                            slt::Style::new().fg(YELLOW).bg(bg),
                        );
                        // Show agent badges inline
                        for a in &agent_set {
                            ui.styled(
                                format!(" {a}"),
                                slt::Style::new().fg(agent_color(*a)).bg(bg),
                            );
                        }
                        ui.styled("  ".to_string(), slt::Style::new().bg(bg));
                        ui.styled(display_path, slt::Style::new().fg(GRAY_500).bg(bg));
                        ui.spacer();
                        ui.styled(
                            format!("  {latest_time} "),
                            slt::Style::new().fg(VIOLET).bg(bg),
                        );
                    });
                }
                row_idx += 1;

                // Child rows (if expanded)
                if expanded {
                    for (ci, &session_idx) in group.sessions.iter().enumerate() {
                        if row_idx >= app.grouped_scroll && row_idx < end {
                            let s = &app.sessions[session_idx];
                            let is_selected = row_idx == app.grouped_selected;
                            let bg = if is_selected {
                                HIGHLIGHT_BG
                            } else {
                                slt::Color::Reset
                            };
                            let is_last = ci == group.sessions.len() - 1;
                            let tree_char = if is_last { "  └─ " } else { "  ├─ " };
                            let is_pinned = app.pinned_sessions.contains(&s.session_id);
                            let pin_str = if is_pinned { "*" } else { " " };

                            // Calculate available space for summary
                            let fixed_width = 5 + 1 + 12 + 2 + 16; // tree + pin + agent + gap + time
                            let git_width = s.git_branch.as_ref().map(|b| b.len() + 2).unwrap_or(0);
                            let summary_max =
                                total_width.saturating_sub(fixed_width + git_width + 2);

                            let summary_src = if app.show_recap {
                                s.recap
                                    .as_deref()
                                    .or(s.summaries.first().map(String::as_str))
                            } else {
                                s.summaries.first().map(String::as_str)
                            };
                            let summary = summary_src
                                .map(|t| truncate_str(t, summary_max.max(10)))
                                .unwrap_or_default();

                            let _ = ui.row(|ui| {
                                ui.styled(
                                    tree_char.to_string(),
                                    slt::Style::new().fg(SEPARATOR).bg(bg),
                                );
                                if is_pinned {
                                    ui.styled(
                                        pin_str.to_string(),
                                        slt::Style::new().fg(YELLOW).bold().bg(bg),
                                    );
                                } else {
                                    ui.styled(pin_str.to_string(), slt::Style::new().bg(bg));
                                }
                                ui.styled(
                                    format!("{:<12}", s.agent.to_string()),
                                    slt::Style::new().fg(agent_color(s.agent)).bold().bg(bg),
                                );
                                if !summary.is_empty() {
                                    if let Some(rest) = summary.strip_prefix("recap: ") {
                                        ui.styled(
                                            "  recap: ".to_string(),
                                            slt::Style::new().fg(VIOLET).bg(bg),
                                        );
                                        ui.styled(
                                            rest.to_string(),
                                            slt::Style::new().fg(GRAY_400).bg(bg),
                                        );
                                    } else {
                                        ui.styled(
                                            format!("  {summary}"),
                                            slt::Style::new().fg(GRAY_400).bg(bg),
                                        );
                                    }
                                }
                                ui.spacer();
                                if let Some(branch) = &s.git_branch {
                                    ui.styled(
                                        branch.to_string(),
                                        slt::Style::new().fg(GREEN_400).bg(bg),
                                    );
                                }
                                ui.styled(
                                    format!("  {} ", s.time_display()),
                                    slt::Style::new().fg(GRAY_500).bg(bg),
                                );
                            });
                        }
                        row_idx += 1;
                    }
                }
            }
        });

        ui.separator_colored(SEPARATOR);
        let _ = ui.container().pr(1).row(|ui| {
            ui.spacer();
            let _ = ui.help_colored(
                &[
                    ("↑↓", "nav"),
                    ("Enter/Space", "expand"),
                    ("^G", "flat view"),
                    ("Esc", "back"),
                ],
                GRAY_500,
                SEPARATOR,
            );
        });
    });
}

fn ui_action_select(ui: &mut slt::Context, app: &mut App, result: &mut Option<String>) {
    let actions = Action::MENU;
    let action_count = actions.len();

    if ui.key_code(slt::KeyCode::Esc) {
        app.mode = Mode::Browse;
    }

    // Tab/BackTab: wrap-around navigation
    if ui.consume_key_code(slt::KeyCode::BackTab) {
        app.action_index = (app.action_index + action_count - 1) % action_count;
    } else if (ui.key_code(slt::KeyCode::Up)
        || ui.key_mod('p', slt::KeyModifiers::CONTROL)
        || ui.key_mod('k', slt::KeyModifiers::CONTROL))
        && app.action_index > 0
    {
        app.action_index -= 1;
    }

    if ui.consume_key_code(slt::KeyCode::Tab) {
        app.action_index = (app.action_index + 1) % action_count;
    } else if (ui.key_code(slt::KeyCode::Down)
        || ui.key_mod('n', slt::KeyModifiers::CONTROL)
        || ui.key_mod('j', slt::KeyModifiers::CONTROL))
        && app.action_index < action_count - 1
    {
        app.action_index += 1;
    }

    for i in 0..action_count.min(9) {
        let key = char::from_u32((b'1' + i as u8) as u32).unwrap_or('1');
        if ui.key(key) {
            app.action_index = i;
            dispatch_action(ui, app, actions[app.action_index], result);
        }
    }

    // Mouse: click on action item
    if let Some((_x, y)) = ui.mouse_down() {
        let y = y as usize;
        if y >= 4 && y < 4 + action_count {
            let clicked = y - 4;
            app.action_index = clicked;
            dispatch_action(ui, app, actions[app.action_index], result);
        }
    }

    if ui.key_code(slt::KeyCode::Enter) {
        // Resume → go to mode picker; others → dispatch directly
        if actions[app.action_index] == Action::Resume {
            if let Some(session) = app.selected_session() {
                app.resume_mode_options = session.agent.resume_mode_options().to_vec();
                app.resume_mode_index = 0;
                app.mode = Mode::ResumeSelect;
            }
        } else {
            dispatch_action(ui, app, actions[app.action_index], result);
        }
    }

    let Some(session) = app.selected_session() else {
        app.mode = Mode::Browse;
        return;
    };

    let _ = ui.col(|ui| {
        ui.separator_colored(SEPARATOR);
        ui.line(|ui| {
            ui.text(format!(" {} ", session.agent))
                .fg(agent_color(session.agent))
                .bold();
            ui.text("| ").fg(SEPARATOR);
            ui.text(&session.project_name).fg(BRIGHT_WHITE).bold();
            ui.text(" | ").fg(SEPARATOR);
            ui.text(session.display_path()).fg(GRAY_500);
            if let Some(branch) = &session.git_branch {
                ui.text(" | ").fg(SEPARATOR);
                ui.text(branch).fg(GREEN_400);
            }
            ui.text(" | ").fg(SEPARATOR);
            ui.text(session.time_display()).fg(VIOLET);
        });
        ui.separator_colored(SEPARATOR);
        ui.text("");

        let _ = ui.container().grow(1).col(|ui| {
            let total_width = ui.width() as usize;
            for (i, act) in actions.iter().enumerate() {
                let is_selected = i == app.action_index;
                let bg = if is_selected {
                    HIGHLIGHT_BG
                } else {
                    slt::Color::Reset
                };
                let indicator = format!(" {}) ", i + 1);
                let label = if *act == Action::Pin {
                    let is_pinned = app
                        .selected_session()
                        .map(|s| app.pinned_sessions.contains(&s.session_id))
                        .unwrap_or(false);
                    if is_pinned {
                        "Unpin Session".to_string()
                    } else {
                        "Pin Session".to_string()
                    }
                } else {
                    act.to_string()
                };
                let base_style = if *act == Action::Delete {
                    slt::Style::new().fg(RED).bg(bg)
                } else if *act == Action::Back {
                    slt::Style::new().fg(GRAY_500).bg(bg)
                } else {
                    slt::Style::new().fg(BRIGHT_WHITE).bg(bg)
                };
                let label_style = if is_selected {
                    base_style.bold()
                } else {
                    base_style
                };
                let preview = action::action_preview(session, *act);
                let mut preview_text = format!("    {preview}");
                let used = UnicodeWidthStr::width(indicator.as_str())
                    + UnicodeWidthStr::width(label.as_str())
                    + UnicodeWidthStr::width(preview_text.as_str());
                if used > total_width {
                    let max_preview = total_width.saturating_sub(
                        UnicodeWidthStr::width(indicator.as_str())
                            + UnicodeWidthStr::width(label.as_str())
                            + 4,
                    );
                    preview_text = if max_preview > 0 {
                        format!("    {}", truncate_str(&preview, max_preview))
                    } else {
                        String::new()
                    };
                }
                let pad = total_width.saturating_sub(
                    UnicodeWidthStr::width(indicator.as_str())
                        + UnicodeWidthStr::width(label.as_str())
                        + UnicodeWidthStr::width(preview_text.as_str()),
                );

                let _ = ui.row(|ui| {
                    ui.styled(
                        indicator.clone(),
                        slt::Style::new().fg(slt::Color::White).bg(bg),
                    );
                    ui.styled(label.clone(), label_style);
                    ui.styled(preview_text.clone(), slt::Style::new().fg(GRAY_500).bg(bg));
                    if pad > 0 {
                        ui.styled(" ".repeat(pad), slt::Style::new().bg(bg));
                    }
                });
            }
        });

        ui.text("");
        ui.separator_colored(SEPARATOR);
        let _ = ui.container().pl(1).row(|ui| {
            let _ = ui.help_colored(
                &[("Tab", "nav"), ("Enter", "select"), ("Esc", "back")],
                GRAY_500,
                SEPARATOR,
            );
        });
    });
}

fn dispatch_action(
    ui: &mut slt::Context,
    app: &mut App,
    selected_action: Action,
    result: &mut Option<String>,
) {
    match selected_action {
        Action::Back => {
            app.mode = Mode::Browse;
        }
        Action::NewSession => {
            app.agent_index = 0;
            app.mode = Mode::AgentSelect;
        }
        Action::Delete => {
            app.delete_index = 1;
            app.mode = Mode::DeleteConfirm;
        }
        Action::Pin => {
            if let Some(session) = app.selected_session() {
                let id = session.session_id.clone();
                if let Some(pos) = app.pinned_sessions.iter().position(|s| s == &id) {
                    app.pinned_sessions.remove(pos);
                } else {
                    app.pinned_sessions.push(id);
                }
                app.save_settings();
                app.apply_sort();
            }
            app.mode = Mode::Browse;
        }
        _ => {
            if let Some(session) = app.selected_session().cloned() {
                if let Some(cmd) = action::generate_command(&session, selected_action, None) {
                    result.replace(cmd);
                    ui.quit();
                }
            }
        }
    }
}

fn ui_agent_select(ui: &mut slt::Context, app: &mut App, result: &mut Option<String>) {
    let option_count = app.new_session_options.len();

    if ui.key_code(slt::KeyCode::Esc) {
        app.mode = Mode::ActionSelect;
    }

    // Tab/BackTab: wrap-around navigation
    if ui.consume_key_code(slt::KeyCode::BackTab) && option_count > 0 {
        app.agent_index = (app.agent_index + option_count - 1) % option_count;
    } else if (ui.key_code(slt::KeyCode::Up)
        || ui.key_mod('p', slt::KeyModifiers::CONTROL)
        || ui.key_mod('k', slt::KeyModifiers::CONTROL))
        && app.agent_index > 0
    {
        app.agent_index -= 1;
    }

    if ui.consume_key_code(slt::KeyCode::Tab) && option_count > 0 {
        app.agent_index = (app.agent_index + 1) % option_count;
    } else if (ui.key_code(slt::KeyCode::Down)
        || ui.key_mod('n', slt::KeyModifiers::CONTROL)
        || ui.key_mod('j', slt::KeyModifiers::CONTROL))
        && option_count > 0
        && app.agent_index < option_count - 1
    {
        app.agent_index += 1;
    }

    for i in 0..option_count.min(9) {
        let key = char::from_u32((b'1' + i as u8) as u32).unwrap_or('1');
        if ui.key(key) {
            app.agent_index = i;
            dispatch_agent_option(ui, app, result);
        }
    }

    if ui.key_code(slt::KeyCode::Enter) {
        // Enter → go to permission mode picker
        if let Some(opt) = app.new_session_options.get(app.agent_index) {
            app.mode_options = permission_options_for(opt.agent);
            app.mode_index = 0;
            app.mode = Mode::PermissionSelect;
        }
    }

    let Some(session) = app.selected_session() else {
        app.mode = Mode::Browse;
        return;
    };

    let _ = ui.col(|ui| {
        ui.separator_colored(SEPARATOR);
        ui.line(|ui| {
            ui.text(" New session in ").fg(BRIGHT_WHITE);
            ui.text(session.display_path()).fg(GRAY_500);
            ui.text("  (enter -> permission mode)").fg(GRAY_500);
        });
        ui.separator_colored(SEPARATOR);
        ui.text("");

        let _ = ui.container().grow(1).col(|ui| {
            let total_width = ui.width() as usize;
            for (i, opt) in app.new_session_options.iter().enumerate() {
                let is_selected = i == app.agent_index;
                let bg = if is_selected {
                    HIGHLIGHT_BG
                } else {
                    slt::Color::Reset
                };
                let indicator = format!(" {}) ", i + 1);
                let preview = if let Some(s) = app.selected_session() {
                    let base = opt.agent.new_session_cmd();
                    format!("cd {} && {base}", s.display_path())
                } else {
                    String::new()
                };
                let preview_text = format!("    {preview}");
                let used = UnicodeWidthStr::width(indicator.as_str())
                    + UnicodeWidthStr::width(opt.label.as_str())
                    + UnicodeWidthStr::width(preview_text.as_str());
                let pad = total_width.saturating_sub(used);

                let _ = ui.row(|ui| {
                    ui.styled(indicator.clone(), slt::Style::new().fg(GRAY_400).bg(bg));
                    let base = slt::Style::new().fg(agent_color(opt.agent)).bg(bg);
                    ui.styled(
                        opt.label.clone(),
                        if is_selected { base.bold() } else { base },
                    );
                    ui.styled(preview_text.clone(), slt::Style::new().fg(GRAY_500).bg(bg));
                    if pad > 0 {
                        ui.styled(" ".repeat(pad), slt::Style::new().bg(bg));
                    }
                });
            }
        });

        ui.text("");
        ui.separator_colored(SEPARATOR);
        let _ = ui.container().pl(1).row(|ui| {
            let _ = ui.help_colored(
                &[
                    ("1-9", "select"),
                    ("Tab", "nav"),
                    ("Enter", "mode"),
                    ("Esc", "back"),
                ],
                GRAY_500,
                SEPARATOR,
            );
        });
    });
}

fn permission_options_for(agent: Agent) -> Vec<(&'static str, &'static str)> {
    match agent {
        Agent::ClaudeCode => vec![
            ("default", ""),
            ("acceptEdits", " --permission-mode acceptEdits"),
            ("plan (read-only)", " --permission-mode plan"),
            ("bypass permissions", " --dangerously-skip-permissions"),
        ],
        Agent::Codex => vec![
            ("suggest (default)", ""),
            ("auto-edit", " -a untrusted"),
            ("full-auto", " --full-auto"),
            (
                "bypass sandbox",
                " --dangerously-bypass-approvals-and-sandbox",
            ),
        ],
        Agent::Gemini => vec![
            ("default", ""),
            ("auto_edit", " --approval-mode auto_edit"),
            ("yolo (no approval)", " -y"),
            ("plan (read-only)", " --approval-mode plan"),
            ("sandbox", " -s"),
        ],
        _ => vec![("default", "")],
    }
}

fn dispatch_agent_option(ui: &mut slt::Context, app: &mut App, result: &mut Option<String>) {
    if let Some(opt) = app.new_session_options.get(app.agent_index) {
        let agent = opt.agent;
        let suffix = opt.command_suffix;
        if let Some(session) = app.selected_session().cloned() {
            if let Some(cmd) = action::new_session_with_flags(&session, agent, suffix) {
                result.replace(cmd);
                ui.quit();
            }
        }
    }
}

fn ui_permission_select(ui: &mut slt::Context, app: &mut App, result: &mut Option<String>) {
    let option_count = app.mode_options.len();

    if ui.key_code(slt::KeyCode::Esc) {
        app.mode = Mode::AgentSelect;
    }

    // Tab/BackTab: wrap-around navigation
    if ui.key_code(slt::KeyCode::BackTab) && option_count > 0 {
        app.mode_index = (app.mode_index + option_count - 1) % option_count;
    } else if (ui.key_code(slt::KeyCode::Up)
        || ui.key_mod('p', slt::KeyModifiers::CONTROL)
        || ui.key_mod('k', slt::KeyModifiers::CONTROL))
        && app.mode_index > 0
    {
        app.mode_index -= 1;
    }

    if ui.key_code(slt::KeyCode::Tab) && option_count > 0 {
        app.mode_index = (app.mode_index + 1) % option_count;
    } else if (ui.key_code(slt::KeyCode::Down)
        || ui.key_mod('n', slt::KeyModifiers::CONTROL)
        || ui.key_mod('j', slt::KeyModifiers::CONTROL))
        && option_count > 0
        && app.mode_index < option_count - 1
    {
        app.mode_index += 1;
    }

    for i in 0..option_count.min(9) {
        let key = char::from_u32((b'1' + i as u8) as u32).unwrap_or('1');
        if ui.key(key) {
            app.mode_index = i;
            dispatch_mode_option(ui, app, result);
        }
    }

    if ui.key_code(slt::KeyCode::Enter) {
        dispatch_mode_option(ui, app, result);
    }

    if app.selected_session().is_none() {
        app.mode = Mode::Browse;
        return;
    }

    let agent_label = app
        .new_session_options
        .get(app.agent_index)
        .map(|o| o.label.as_str())
        .unwrap_or("agent");

    let _ = ui.col(|ui| {
        ui.separator_colored(SEPARATOR);
        ui.line(|ui| {
            ui.text(" Select mode for ").fg(BRIGHT_WHITE);
            ui.text(agent_label).fg(YELLOW).bold();
        });
        ui.separator_colored(SEPARATOR);
        ui.text("");

        let _ = ui.container().grow(1).col(|ui| {
            let total_width = ui.width() as usize;
            for (i, (label, flags)) in app.mode_options.iter().enumerate() {
                let is_selected = i == app.mode_index;
                let bg = if is_selected {
                    HIGHLIGHT_BG
                } else {
                    slt::Color::Reset
                };
                let indicator = format!(" {}) ", i + 1);
                let flag_preview = if flags.is_empty() {
                    String::new()
                } else {
                    format!("  {}", flags.trim())
                };
                let pad = total_width.saturating_sub(
                    UnicodeWidthStr::width(indicator.as_str())
                        + UnicodeWidthStr::width(*label)
                        + UnicodeWidthStr::width(flag_preview.as_str()),
                );

                let _ = ui.row(|ui| {
                    ui.styled(indicator.clone(), slt::Style::new().fg(GRAY_400).bg(bg));
                    let base = slt::Style::new().fg(BRIGHT_WHITE).bg(bg);
                    ui.styled(
                        (*label).to_string(),
                        if is_selected { base.bold() } else { base },
                    );
                    ui.styled(flag_preview.clone(), slt::Style::new().fg(GRAY_500).bg(bg));
                    if pad > 0 {
                        ui.styled(" ".repeat(pad), slt::Style::new().bg(bg));
                    }
                });
            }
        });

        ui.text("");
        ui.separator_colored(SEPARATOR);
        let _ = ui.container().pl(1).row(|ui| {
            let _ = ui.help_colored(
                &[("1-9", "select"), ("Enter", "confirm"), ("Esc", "back")],
                GRAY_500,
                SEPARATOR,
            );
        });
    });
}

fn dispatch_mode_option(ui: &mut slt::Context, app: &mut App, result: &mut Option<String>) {
    if let Some((_, flags)) = app.mode_options.get(app.mode_index) {
        if let Some(opt) = app.new_session_options.get(app.agent_index) {
            let agent = opt.agent;
            if let Some(session) = app.selected_session().cloned() {
                if let Some(cmd) = action::new_session_with_flags(&session, agent, flags) {
                    result.replace(cmd);
                    ui.quit();
                }
            }
        }
    }
}

fn ui_resume_select(ui: &mut slt::Context, app: &mut App, result: &mut Option<String>) {
    let option_count = app.resume_mode_options.len();

    if ui.key_code(slt::KeyCode::Esc) {
        app.mode = Mode::ActionSelect;
    }

    // Tab/BackTab: wrap-around navigation
    if ui.key_code(slt::KeyCode::BackTab) && option_count > 0 {
        app.resume_mode_index = (app.resume_mode_index + option_count - 1) % option_count;
    } else if (ui.key_code(slt::KeyCode::Up)
        || ui.key_mod('p', slt::KeyModifiers::CONTROL)
        || ui.key_mod('k', slt::KeyModifiers::CONTROL))
        && app.resume_mode_index > 0
    {
        app.resume_mode_index -= 1;
    }

    if ui.key_code(slt::KeyCode::Tab) && option_count > 0 {
        app.resume_mode_index = (app.resume_mode_index + 1) % option_count;
    } else if (ui.key_code(slt::KeyCode::Down)
        || ui.key_mod('n', slt::KeyModifiers::CONTROL)
        || ui.key_mod('j', slt::KeyModifiers::CONTROL))
        && option_count > 0
        && app.resume_mode_index < option_count - 1
    {
        app.resume_mode_index += 1;
    }

    for i in 0..option_count.min(9) {
        let key = char::from_u32((b'1' + i as u8) as u32).unwrap_or('1');
        if ui.key(key) {
            app.resume_mode_index = i;
            dispatch_resume_mode(ui, app, result);
        }
    }

    if ui.key_code(slt::KeyCode::Enter) {
        dispatch_resume_mode(ui, app, result);
    }

    let Some(session) = app.selected_session() else {
        app.mode = Mode::Browse;
        return;
    };

    let _ = ui.col(|ui| {
        ui.separator_colored(SEPARATOR);
        ui.line(|ui| {
            ui.text(" Resume mode for ").fg(BRIGHT_WHITE);
            ui.text(format!("{}", session.agent))
                .fg(agent_color(session.agent))
                .bold();
        });
        ui.separator_colored(SEPARATOR);
        ui.text("");

        let _ = ui.container().grow(1).col(|ui| {
            let total_width = ui.width() as usize;
            for (i, (label, flags)) in app.resume_mode_options.iter().enumerate() {
                let is_selected = i == app.resume_mode_index;
                let bg = if is_selected {
                    HIGHLIGHT_BG
                } else {
                    slt::Color::Reset
                };
                let indicator = format!(" {}) ", i + 1);
                let flag_preview = if flags.is_empty() {
                    String::new()
                } else {
                    format!("  {}", flags.trim())
                };
                let pad = total_width.saturating_sub(
                    UnicodeWidthStr::width(indicator.as_str())
                        + UnicodeWidthStr::width(*label)
                        + UnicodeWidthStr::width(flag_preview.as_str()),
                );

                let _ = ui.row(|ui| {
                    ui.styled(indicator.clone(), slt::Style::new().fg(GRAY_400).bg(bg));
                    let base = slt::Style::new().fg(BRIGHT_WHITE).bg(bg);
                    ui.styled(
                        (*label).to_string(),
                        if is_selected { base.bold() } else { base },
                    );
                    ui.styled(flag_preview.clone(), slt::Style::new().fg(GRAY_500).bg(bg));
                    if pad > 0 {
                        ui.styled(" ".repeat(pad), slt::Style::new().bg(bg));
                    }
                });
            }
        });

        ui.text("");
        ui.separator_colored(SEPARATOR);
        let _ = ui.container().pl(1).row(|ui| {
            let _ = ui.help_colored(
                &[("1-9", "select"), ("Enter", "confirm"), ("Esc", "back")],
                GRAY_500,
                SEPARATOR,
            );
        });
    });
}

fn dispatch_resume_mode(ui: &mut slt::Context, app: &mut App, result: &mut Option<String>) {
    if let Some((_, flags)) = app.resume_mode_options.get(app.resume_mode_index) {
        if let Some(session) = app.selected_session().cloned() {
            let cmd = action::resume_with_flags(&session, flags);
            result.replace(cmd);
            ui.quit();
        }
    }
}

fn ui_bulk_delete(ui: &mut slt::Context, app: &mut App) {
    if ui.key_code(slt::KeyCode::Esc) {
        app.selected_set.clear();
        app.mode = Mode::Browse;
    }

    if (ui.key_code(slt::KeyCode::Up)
        || ui.key_mod('p', slt::KeyModifiers::CONTROL)
        || ui.key_mod('k', slt::KeyModifiers::CONTROL))
        && app.selected > 0
    {
        app.selected -= 1;
        app.adjust_scroll();
    }

    if (ui.key_code(slt::KeyCode::Down)
        || ui.key_mod('n', slt::KeyModifiers::CONTROL)
        || ui.key_mod('j', slt::KeyModifiers::CONTROL))
        && !app.filtered_indices.is_empty()
        && app.selected < app.filtered_indices.len() - 1
    {
        app.selected += 1;
        app.adjust_scroll();
    }

    if ui.key(' ') {
        if let Some(idx) = app.filtered_indices.get(app.selected).copied() {
            if !app.selected_set.remove(&idx) {
                app.selected_set.insert(idx);
            }
        }
        if !app.filtered_indices.is_empty() && app.selected < app.filtered_indices.len() - 1 {
            app.selected += 1;
            app.adjust_scroll();
        }
    }

    if ui.key_code(slt::KeyCode::Enter) && !app.selected_set.is_empty() {
        app.delete_index = 1;
        app.mode = Mode::DeleteConfirm;
    }

    let _ = ui.col(|ui| {
        let _ = ui
            .bordered(slt::Border::Rounded)
            .border_fg(RED)
            .min_h(3)
            .max_h(3)
            .col(|ui| {
                ui.line(|ui| {
                    ui.text(" DELETE MODE").fg(RED).bold();
                    if !app.selected_set.is_empty() {
                        ui.text(format!("  ({} selected)", app.selected_set.len()))
                            .fg(RED);
                    }
                });
            });

        let _ = ui.container().grow(1).col(|ui| {
            render_session_list(ui, app, true);
        });

        ui.line(|ui| {
            ui.text(format!(" {} selected", app.selected_set.len()))
                .fg(RED)
                .bold();
        });
        let _ = ui.container().pl(1).row(|ui| {
            let _ = ui.help_colored(
                &[("Space", "toggle"), ("Enter", "delete"), ("Esc", "cancel")],
                GRAY_500,
                SEPARATOR,
            );
        });
    });
}

fn ui_delete_confirm(ui: &mut slt::Context, app: &mut App) {
    let is_bulk = !app.selected_set.is_empty();

    if ui.key_code(slt::KeyCode::Esc) {
        if is_bulk {
            app.mode = Mode::BulkDelete;
        } else {
            app.mode = Mode::ActionSelect;
        }
    }

    if ui.key_code(slt::KeyCode::Up)
        || ui.key_code(slt::KeyCode::Down)
        || ui.key_code(slt::KeyCode::Left)
        || ui.key_code(slt::KeyCode::Right)
        || ui.key_mod('p', slt::KeyModifiers::CONTROL)
        || ui.key_mod('n', slt::KeyModifiers::CONTROL)
        || ui.key_mod('k', slt::KeyModifiers::CONTROL)
        || ui.key_mod('j', slt::KeyModifiers::CONTROL)
        || ui.key_mod('h', slt::KeyModifiers::CONTROL)
        || ui.key_mod('l', slt::KeyModifiers::CONTROL)
    {
        app.delete_index = if app.delete_index == 0 { 1 } else { 0 };
    }

    if ui.key_code(slt::KeyCode::Enter) {
        if app.delete_index == 0 {
            if is_bulk {
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
            } else if let Some(idx) = app.filtered_indices.get(app.selected).copied() {
                let _ = crate::delete::delete_session(&app.sessions[idx]);
                app.sessions.remove(idx);
                app.update_filter();
            }
            app.mode = Mode::Browse;
        } else if is_bulk {
            app.mode = Mode::BulkDelete;
        } else {
            app.mode = Mode::Browse;
        }
    }

    if is_bulk {
        render_bulk_delete_confirm(ui, app);
    } else {
        render_single_delete_confirm(ui, app);
    }
}

fn render_single_delete_confirm(ui: &mut slt::Context, app: &App) {
    let Some(session) = app.selected_session() else {
        return;
    };

    let _ = ui.col(|ui| {
        ui.separator_colored(SEPARATOR);
        ui.text(" Delete session?").fg(RED).bold();
        ui.separator_colored(SEPARATOR);
        ui.text("");

        ui.line(|ui| {
            ui.text(format!("  {} ", session.agent))
                .fg(agent_color(session.agent))
                .bold();
            ui.text("| ").fg(SEPARATOR);
            ui.text(&session.project_name).fg(BRIGHT_WHITE);
            ui.text(" | ").fg(SEPARATOR);
            ui.text(&session.session_id).fg(GRAY_500);
        });
        ui.text(format!("  {}", session.display_path()))
            .fg(GRAY_500);
        if let Some(summary) = session.summaries.first() {
            let max_width = (ui.width() as usize).saturating_sub(6);
            let truncated = truncate_str(summary, max_width);
            ui.text(format!("  \"{truncated}\"")).fg(GRAY_400);
        }

        ui.text("");
        let options = ["Yes, delete", "Cancel"];
        for (i, opt) in options.iter().enumerate() {
            let is_selected = i == app.delete_index;
            let bg = if is_selected {
                HIGHLIGHT_BG
            } else {
                slt::Color::Reset
            };
            let indicator = if is_selected { " > " } else { "   " };
            let label_style = if i == 0 {
                slt::Style::new().fg(RED).bold().bg(bg)
            } else {
                slt::Style::new().fg(BRIGHT_WHITE).bg(bg)
            };
            let desc = if i == 0 {
                "removes session data only"
            } else {
                "go back"
            };

            let _ = ui.row(|ui| {
                ui.styled(
                    indicator.to_string(),
                    slt::Style::new().fg(slt::Color::White).bg(bg),
                );
                ui.styled((*opt).to_string(), label_style);
                ui.styled(format!("    {desc}"), slt::Style::new().fg(GRAY_500).bg(bg));
            });
        }

        ui.separator_colored(SEPARATOR);
    });
}

fn render_bulk_delete_confirm(ui: &mut slt::Context, app: &App) {
    let count = app.selected_set.len();
    let mut names: Vec<String> = app
        .selected_set
        .iter()
        .filter_map(|idx| app.sessions.get(*idx))
        .map(|s| s.project_name.clone())
        .collect();
    names.sort();

    let _ = ui.col(|ui| {
        ui.separator_colored(SEPARATOR);
        ui.text(format!(" Delete {count} sessions?")).fg(RED).bold();
        ui.separator_colored(SEPARATOR);
        ui.text("");

        for (i, name) in names.iter().enumerate() {
            if i >= 5 {
                ui.text(format!("  ... and {} more", count.saturating_sub(5)))
                    .fg(GRAY_500);
                break;
            }
            ui.text(format!("  - {name}")).fg(BRIGHT_WHITE);
        }

        ui.text("");
        let options = ["Yes, delete all", "Cancel"];
        for (i, opt) in options.iter().enumerate() {
            let is_selected = i == app.delete_index;
            let bg = if is_selected {
                HIGHLIGHT_BG
            } else {
                slt::Color::Reset
            };
            let indicator = if is_selected { " > " } else { "   " };
            let label_style = if i == 0 {
                slt::Style::new().fg(RED).bold().bg(bg)
            } else {
                slt::Style::new().fg(BRIGHT_WHITE).bg(bg)
            };
            let desc = if i == 0 {
                "removes session data only"
            } else {
                "go back"
            };

            let _ = ui.row(|ui| {
                ui.styled(
                    indicator.to_string(),
                    slt::Style::new().fg(slt::Color::White).bg(bg),
                );
                ui.styled((*opt).to_string(), label_style);
                ui.styled(format!("    {desc}"), slt::Style::new().fg(GRAY_500).bg(bg));
            });
        }

        ui.separator_colored(SEPARATOR);
    });
}

fn ui_preview(ui: &mut slt::Context, app: &mut App) {
    if ui.key_code(slt::KeyCode::Esc)
        || ui.key_code(slt::KeyCode::Left)
        || ui.key_code(slt::KeyCode::Right)
        || ui.key_mod('h', slt::KeyModifiers::CONTROL)
    {
        app.mode = Mode::Browse;
    } else if ui.key_code(slt::KeyCode::Enter) {
        app.action_index = 0;
        app.mode = Mode::ActionSelect;
    } else if any_key_pressed(ui) {
        app.mode = Mode::Browse;
    }

    let Some(session) = app.selected_session() else {
        app.mode = Mode::Browse;
        return;
    };

    let _ = ui.col(|ui| {
        ui.separator_colored(SEPARATOR);
        ui.text(" Session Detail").fg(BRIGHT_WHITE).bold();
        ui.separator_colored(SEPARATOR);
        ui.text("");

        ui.line(|ui| {
            ui.text("  Agent:    ").fg(GRAY_500);
            ui.text(session.agent.to_string())
                .fg(agent_color(session.agent))
                .bold();
        });
        ui.line(|ui| {
            ui.text("  Project:  ").fg(GRAY_500);
            ui.text(&session.project_name).fg(BRIGHT_WHITE).bold();
        });
        ui.line(|ui| {
            ui.text("  Path:     ").fg(GRAY_500);
            ui.text(session.display_path()).fg(GRAY_400);
        });
        ui.line(|ui| {
            ui.text("  Session:  ").fg(GRAY_500);
            ui.text(&session.session_id).fg(GRAY_400);
        });
        ui.line(|ui| {
            ui.text("  Time:     ").fg(GRAY_500);
            ui.text(session.time_display()).fg(VIOLET);
        });

        if let Some(branch) = &session.git_branch {
            ui.line(|ui| {
                ui.text("  Branch:   ").fg(GRAY_500);
                ui.text(branch).fg(GREEN_400);
            });
        }
        if let Some(wt) = &session.worktree {
            ui.line(|ui| {
                ui.text("  Worktree: ").fg(GRAY_500);
                ui.text(wt).fg(CYAN);
            });
        }

        if let Some(recap) = &session.recap {
            ui.line(|ui| {
                ui.text("  Recap:    ").fg(GRAY_500);
            });
            let max_width = (ui.width() as usize).saturating_sub(14);
            let truncated = truncate_str(recap, max_width);
            ui.line(|ui| {
                ui.text("    ").fg(GRAY_500);
                ui.text(truncated.clone()).fg(GRAY_400);
            });
        }

        if !session.summaries.is_empty() {
            ui.line(|ui| {
                ui.text("  History:  ").fg(GRAY_500);
            });
            let max_width = (ui.width() as usize).saturating_sub(14);
            for (i, summary) in session.summaries.iter().enumerate() {
                let truncated = truncate_str(summary, max_width);
                ui.line(|ui| {
                    ui.text(format!("    {:>2}. ", i + 1)).fg(GRAY_500);
                    ui.text(truncated.clone()).fg(GRAY_400);
                });
            }
        }

        ui.text("");
        ui.separator_colored(SEPARATOR);
        let _ = ui.container().pl(1).row(|ui| {
            let _ = ui.help_colored(
                &[("Enter", "actions"), ("Esc", "back"), ("Any", "back")],
                GRAY_500,
                SEPARATOR,
            );
        });
    });
}

fn ui_help(ui: &mut slt::Context, app: &mut App) {
    if ui.key_code(slt::KeyCode::Esc) || ui.key('q') {
        app.mode = Mode::Browse;
    }

    if (ui.key_code(slt::KeyCode::Up) || ui.key_mod('k', slt::KeyModifiers::CONTROL))
        && app.help_selected > 0
    {
        app.help_selected -= 1;
    }

    if (ui.key_code(slt::KeyCode::Down) || ui.key_mod('j', slt::KeyModifiers::CONTROL))
        && app.help_selected < 2
    {
        app.help_selected += 1;
    }

    if app.help_selected == 0
        && (ui.key_code(slt::KeyCode::Enter)
            || ui.key(' ')
            || ui.key_code(slt::KeyCode::Left)
            || ui.key_code(slt::KeyCode::Right))
    {
        app.include_summaries = !app.include_summaries;
        app.save_settings();
        app.update_filter();
    }

    if app.help_selected == 1 && (ui.key('+') || ui.key('=')) {
        app.summary_search_count = app.summary_search_count.saturating_add(1).min(50);
        app.save_settings();
    }

    if app.help_selected == 1 && ui.key('-') {
        app.summary_search_count = app.summary_search_count.saturating_sub(1).max(1);
        app.save_settings();
    }

    if app.help_selected == 2
        && (ui.key_code(slt::KeyCode::Enter)
            || ui.key(' ')
            || ui.key_code(slt::KeyCode::Left)
            || ui.key_code(slt::KeyCode::Right))
    {
        app.show_recap = !app.show_recap;
        app.save_settings();
    }

    let search_scope_label = if app.include_summaries {
        "all (name + path + summaries)"
    } else {
        "name_path (default)"
    };
    let config_path = crate::settings::Settings::config_path();
    let config_path_str = config_path.to_string_lossy().to_string();

    let _ = ui.col(|ui| {
        ui.text("");
        let _ = ui.container().pl(2).pr(1).col(|ui| {
            ui.text("Help & Settings").fg(BRIGHT_WHITE).bold();
        });
        ui.separator_colored(SEPARATOR);

        let _ = ui.container().pl(2).pr(1).grow(1).col(|ui| {
            ui.text("").dim();
            ui.text("Keybindings").fg(GRAY_400).bold();
            ui.text("").dim();
            help_line(ui, "↑ / ↓", "Navigate sessions");
            help_line(ui, "[ / ]", "Cycle summary");
            help_line(ui, "→", "Session detail");
            help_line(ui, "Enter", "Action menu");
            help_line(ui, "Tab", "Cycle agent filter");
            help_line(ui, "^S", "Cycle sort");
            help_line(ui, "^D", "Bulk delete");
            help_line(ui, "?", "Help");
            help_line(ui, "Esc", "Quit");

            ui.text("");
            ui.text("Settings").fg(GRAY_400).bold();
            ui.text("").dim();

            // search_scope setting
            let selected_scope = app.help_selected == 0;
            let scope_bg = if selected_scope {
                HIGHLIGHT_BG
            } else {
                slt::Color::Reset
            };
            let _ = ui.row(|ui| {
                ui.styled(
                    if selected_scope { "> " } else { "  " },
                    slt::Style::new().fg(YELLOW).bg(scope_bg),
                );
                ui.styled(
                    format!("{:<22}", "search_scope"),
                    slt::Style::new().fg(BRIGHT_WHITE).bg(scope_bg),
                );
                ui.styled(
                    search_scope_label,
                    slt::Style::new()
                        .fg(if selected_scope {
                            BRIGHT_WHITE
                        } else {
                            GRAY_400
                        })
                        .bg(scope_bg),
                );
            });

            // summary_search_count setting
            let selected_count = app.help_selected == 1;
            let count_bg = if selected_count {
                HIGHLIGHT_BG
            } else {
                slt::Color::Reset
            };
            let _ = ui.row(|ui| {
                ui.styled(
                    if selected_count { "> " } else { "  " },
                    slt::Style::new().fg(YELLOW).bg(count_bg),
                );
                ui.styled(
                    format!("{:<22}", "summary_search_count"),
                    slt::Style::new().fg(BRIGHT_WHITE).bg(count_bg),
                );
                ui.styled(
                    format!("{}", app.summary_search_count),
                    slt::Style::new()
                        .fg(if selected_count {
                            BRIGHT_WHITE
                        } else {
                            GRAY_400
                        })
                        .bg(count_bg),
                );
            });

            // show_recap setting
            let selected_recap = app.help_selected == 2;
            let recap_bg = if selected_recap {
                HIGHLIGHT_BG
            } else {
                slt::Color::Reset
            };
            let recap_label = if app.show_recap {
                "on (show recap instead of last prompt)"
            } else {
                "off (default)"
            };
            let _ = ui.row(|ui| {
                ui.styled(
                    if selected_recap { "> " } else { "  " },
                    slt::Style::new().fg(YELLOW).bg(recap_bg),
                );
                ui.styled(
                    format!("{:<22}", "show_recap"),
                    slt::Style::new().fg(BRIGHT_WHITE).bg(recap_bg),
                );
                ui.styled(
                    recap_label,
                    slt::Style::new()
                        .fg(if selected_recap {
                            BRIGHT_WHITE
                        } else {
                            GRAY_400
                        })
                        .bg(recap_bg),
                );
            });

            ui.text("");
            ui.text("Config").fg(GRAY_400).bold();
            ui.text("").dim();
            ui.text(format!("  {config_path_str}")).fg(GRAY_500);
        });

        ui.separator_colored(SEPARATOR);
        let _ = ui.container().pr(1).row(|ui| {
            ui.spacer();
            let _ = ui.help_colored(
                &[
                    ("↑↓", "navigate"),
                    ("Enter", "toggle"),
                    ("+/-", "adjust"),
                    ("Esc", "close"),
                ],
                GRAY_500,
                SEPARATOR,
            );
        });
    });
}

fn help_line(ui: &mut slt::Context, key: &str, desc: &str) {
    let _ = ui.row(|ui| {
        ui.styled(format!("  {:<16}", key), slt::Style::new().fg(GRAY_500));
        ui.text(desc).fg(GRAY_400);
    });
}

fn render_session_list(ui: &mut slt::Context, app: &App, bulk_mode: bool) {
    let visible = app.viewport_height.max(1);
    let end = (app.scroll_offset + visible).min(app.filtered_indices.len());
    let total_width = ui.width() as usize;
    let right_margin = 1usize;

    // Compute max project name width across all filtered sessions for table alignment
    let name_col_width = app
        .filtered_indices
        .iter()
        .map(|&i| UnicodeWidthStr::width(app.sessions[i].project_name.as_str()))
        .max()
        .unwrap_or(0)
        .min(30); // cap at 30 chars to leave room for summary

    for vi in app.scroll_offset..end {
        let session_idx = app.filtered_indices[vi];
        let session = &app.sessions[session_idx];
        let is_selected = vi == app.selected;
        let bg = if is_selected {
            HIGHLIGHT_BG
        } else {
            slt::Color::Reset
        };

        if bulk_mode {
            let is_checked = app.selected_set.contains(&session_idx);
            let indicator = match (is_selected, is_checked) {
                (true, true) => ">[x] ",
                (true, false) => ">[ ] ",
                (false, true) => " [x] ",
                (false, false) => " [ ] ",
            };
            let indicator_style = if is_checked {
                slt::Style::new().fg(RED).bold().bg(bg)
            } else {
                slt::Style::new().fg(slt::Color::White).bg(bg)
            };
            let summary_text = if app.show_recap {
                session
                    .recap
                    .as_deref()
                    .or(session.summaries.first().map(String::as_str))
            } else {
                session.summaries.first().map(String::as_str)
            };
            let chunks = build_session_row(
                session,
                bg,
                5,
                total_width,
                right_margin,
                None,
                summary_text,
                name_col_width,
            );

            let _ = ui.row(|ui| {
                ui.styled(indicator.to_string(), indicator_style);
                render_chunks(ui, &chunks);
            });
        } else {
            let is_pinned = app.pinned_sessions.contains(&session.session_id);
            let indicator = match (is_selected, is_pinned) {
                (true, true) => ">*",
                (true, false) => "> ",
                (false, true) => " *",
                (false, false) => "  ",
            };
            let match_positions = app.match_positions.get(vi).map(Vec::as_slice);
            let summary_offset = app
                .summary_offsets
                .get(&session.session_id)
                .copied()
                .unwrap_or(0);
            let summary_text = if app.show_recap && summary_offset == 0 {
                session
                    .recap
                    .as_deref()
                    .or(session.summaries.first().map(String::as_str))
            } else {
                session.summaries.get(summary_offset).map(String::as_str)
            };
            let chunks = build_session_row(
                session,
                bg,
                2,
                total_width,
                right_margin,
                match_positions,
                summary_text,
                name_col_width,
            );

            let _ = ui.row(|ui| {
                let ind_style = if is_pinned {
                    slt::Style::new().fg(YELLOW).bold().bg(bg)
                } else {
                    slt::Style::new().fg(slt::Color::White).bg(bg)
                };
                ui.styled(indicator.to_string(), ind_style);
                render_chunks(ui, &chunks);
            });
        }
    }
}

fn render_session_list_compact(ui: &mut slt::Context, app: &App) {
    let visible = app.viewport_height.max(1);
    let end = (app.scroll_offset + visible).min(app.filtered_indices.len());

    for vi in app.scroll_offset..end {
        let session_idx = app.filtered_indices[vi];
        let session = &app.sessions[session_idx];
        let is_selected = vi == app.selected;
        let bg = if is_selected {
            HIGHLIGHT_BG
        } else {
            slt::Color::Reset
        };
        let indicator = if is_selected { "> " } else { "  " };

        let _ = ui.row(|ui| {
            ui.styled(
                indicator.to_string(),
                slt::Style::new().fg(slt::Color::White).bg(bg),
            );
            ui.styled(
                format!("{:<14}", session.agent.to_string()),
                slt::Style::new()
                    .fg(agent_color(session.agent))
                    .bold()
                    .bg(bg),
            );
            ui.styled(
                format!("{:<20}", truncate_str(&session.project_name, 20)),
                slt::Style::new().fg(BRIGHT_WHITE).bold().bg(bg),
            );
            if let Some(wt) = &session.worktree {
                ui.styled(
                    format!("{:<8}", truncate_str(wt, 8)),
                    slt::Style::new().fg(CYAN).bg(bg),
                );
            } else if let Some(branch) = &session.git_branch {
                ui.styled(
                    format!("{:<8}", truncate_str(branch, 8)),
                    slt::Style::new().fg(GREEN_400).bg(bg),
                );
            } else {
                ui.styled("        ", slt::Style::new().bg(bg));
            }
            ui.styled(
                format!("{:>12}", session.time_display()),
                slt::Style::new().fg(GRAY_500).bg(bg),
            );
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn build_session_row(
    session: &Session,
    bg: slt::Color,
    indicator_width: usize,
    total_width: usize,
    right_margin: usize,
    match_positions: Option<&[u32]>,
    summary_text: Option<&str>,
    name_col_width: usize,
) -> Vec<StyledChunk> {
    let mut chunks: Vec<StyledChunk> = Vec::new();

    let agent_label = format!("{:<14}", session.agent.to_string());
    chunks.push((
        agent_label,
        slt::Style::new()
            .fg(agent_color(session.agent))
            .bold()
            .bg(bg),
    ));

    let time_str = session.time_display();
    let time_width = UnicodeWidthStr::width(time_str.as_str()) + 2;
    let right_display_width = time_width + right_margin;

    let git_info_str = if let Some(wt) = &session.worktree {
        Some(format!("  {wt}"))
    } else {
        session.git_branch.as_ref().map(|b| format!("  {b}"))
    };
    let git_info_width = git_info_str
        .as_deref()
        .map(UnicodeWidthStr::width)
        .unwrap_or(0);

    // Use fixed column width for project name (padded to align columns)
    let fixed_left = indicator_width + 14;
    let max_proj =
        total_width.saturating_sub(fixed_left + right_display_width + git_info_width + 4);
    let col_width = name_col_width.min(max_proj);
    let proj_display = if col_width == 0 {
        String::new()
    } else if UnicodeWidthStr::width(session.project_name.as_str()) > col_width {
        truncate_str(&session.project_name, col_width)
    } else {
        let name_width = UnicodeWidthStr::width(session.project_name.as_str());
        let pad = col_width.saturating_sub(name_width);
        format!("{}{}", session.project_name, " ".repeat(pad))
    };

    if let Some(positions) = match_positions {
        chunks.extend(highlight_text(&proj_display, positions, 0, bg));
    } else {
        chunks.push((
            proj_display,
            slt::Style::new().fg(BRIGHT_WHITE).bold().bg(bg),
        ));
    }

    let left_used = indicator_width + chunk_width(&chunks);
    let available = total_width.saturating_sub(left_used + git_info_width + right_display_width);

    if available > 7 {
        if let Some(summary) = summary_text {
            let sep = "  ";
            let max_summary = available.saturating_sub(sep.len());
            if max_summary > 5 {
                let truncated = truncate_str(summary, max_summary);
                chunks.push((sep.to_string(), slt::Style::new().bg(bg)));
                if let Some(rest) = truncated.strip_prefix("recap: ") {
                    chunks.push(("recap: ".to_string(), slt::Style::new().fg(VIOLET).bg(bg)));
                    chunks.push((rest.to_string(), slt::Style::new().fg(GRAY_400).bg(bg)));
                } else {
                    chunks.push((truncated, slt::Style::new().fg(GRAY_400).bg(bg)));
                }
            }
        }
    }

    let left_width = indicator_width + chunk_width(&chunks);
    let padding = total_width.saturating_sub(left_width + git_info_width + right_display_width);
    if padding > 0 {
        chunks.push((" ".repeat(padding), slt::Style::new().bg(bg)));
    }

    if let Some(git_str) = git_info_str {
        let color = if session.worktree.is_some() {
            CYAN
        } else {
            GREEN_400
        };
        chunks.push((git_str, slt::Style::new().fg(color).bg(bg)));
    }
    chunks.push((
        format!("  {time_str}"),
        slt::Style::new().fg(GRAY_500).bg(bg),
    ));
    if right_margin > 0 {
        chunks.push((" ".repeat(right_margin), slt::Style::new().bg(bg)));
    }

    chunks
}

fn chunk_width(chunks: &[StyledChunk]) -> usize {
    chunks
        .iter()
        .map(|(text, _)| UnicodeWidthStr::width(text.as_str()))
        .sum()
}

fn render_chunks(ui: &mut slt::Context, chunks: &[StyledChunk]) {
    for (text, style) in chunks {
        ui.styled(text.clone(), *style);
    }
}

fn highlight_text(
    text: &str,
    positions: &[u32],
    offset: usize,
    bg: slt::Color,
) -> Vec<StyledChunk> {
    let mut chunks = Vec::new();
    let chars: Vec<char> = text.chars().collect();

    let mut i = 0;
    while i < chars.len() {
        let global_pos = (i + offset) as u32;
        if positions.contains(&global_pos) {
            chunks.push((
                chars[i].to_string(),
                slt::Style::new().fg(YELLOW).bold().underline().bg(bg),
            ));
            i += 1;
        } else {
            let start = i;
            while i < chars.len() && !positions.contains(&((i + offset) as u32)) {
                i += 1;
            }
            let normal: String = chars[start..i].iter().collect();
            chunks.push((normal, slt::Style::new().fg(BRIGHT_WHITE).bold().bg(bg)));
        }
    }

    chunks
}

fn any_key_pressed(ui: &slt::Context) -> bool {
    if ui.key_code(slt::KeyCode::Esc)
        || ui.key_code(slt::KeyCode::Enter)
        || ui.key_code(slt::KeyCode::Backspace)
        || ui.key_code(slt::KeyCode::Tab)
        || ui.key_code(slt::KeyCode::BackTab)
        || ui.key_code(slt::KeyCode::Up)
        || ui.key_code(slt::KeyCode::Down)
        || ui.key_code(slt::KeyCode::Left)
        || ui.key_code(slt::KeyCode::Right)
        || ui.key_code(slt::KeyCode::Home)
        || ui.key_code(slt::KeyCode::End)
        || ui.key_code(slt::KeyCode::PageUp)
        || ui.key_code(slt::KeyCode::PageDown)
        || ui.key_code(slt::KeyCode::Delete)
    {
        return true;
    }
    for code in 1u8..=12u8 {
        if ui.key_code(slt::KeyCode::F(code)) {
            return true;
        }
    }
    for code in 32u8..=126u8 {
        if ui.key(code as char) {
            return true;
        }
    }
    false
}

fn truncate_str(s: &str, max_width: usize) -> String {
    use unicode_width::UnicodeWidthChar;

    let normalized;
    let s = if s.contains(['\n', '\r', '\t']) {
        normalized = s.split_whitespace().collect::<Vec<_>>().join(" ");
        normalized.as_str()
    } else {
        s
    };

    let mut width = 0;
    let mut end = 0;

    for (i, ch) in s.char_indices() {
        let ch_width = ch.width().unwrap_or(0);
        if width + ch_width > max_width {
            break;
        }
        width += ch_width;
        end = i + ch.len_utf8();
    }

    if end >= s.len() {
        s.to_string()
    } else if max_width > 3 {
        let mut w = 0;
        let mut e = 0;
        for (i, ch) in s.char_indices() {
            let ch_width = ch.width().unwrap_or(0);
            if w + ch_width > max_width - 3 {
                break;
            }
            w += ch_width;
            e = i + ch.len_utf8();
        }
        format!("{}...", &s[..e])
    } else {
        s[..end].to_string()
    }
}
