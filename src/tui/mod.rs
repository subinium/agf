use std::collections::{HashMap, HashSet};
use std::sync::mpsc::Receiver;

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::action;
use crate::cache::ScanResult;
use crate::config::installed_agents;
use crate::fuzzy::FuzzyMatcher;
use crate::model::{Action, Agent, Session, SessionIdentity, SortMode, compare_sessions};
use crate::text::{self, truncate_flat as truncate_str};

/// Width of the agent-name column, shared by the row builder and the layout
/// arithmetic that reserves space for it.
const AGENT_COL_WIDTH: usize = 14;

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
const BROWSE_FIRST_SESSION_ROW: usize = 3;

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
    pub sessions: Vec<SessionIdentity>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum GroupSelection {
    Header(String),
    Session(SessionIdentity),
}

#[derive(Debug, Clone)]
pub struct NewSessionOption {
    pub agent: Agent,
    pub label: String,
    pub command_suffix: &'static str,
}

pub struct App {
    pub sessions: Vec<Session>,
    session_index: HashMap<SessionIdentity, usize>,
    pub filtered_indices: Vec<usize>,
    pub match_positions: Vec<Vec<u32>>,
    pub selected: usize,
    pub query: String,
    pub mode: Mode,
    pub agent_filter: Option<Agent>,
    pub action_index: usize,
    pub agent_index: usize,
    pub delete_index: usize,
    /// Identity captured when a single-session confirmation opens. Background
    /// refreshes can replace/reorder the session Vec while the dialog is
    /// visible, so a numeric cursor is not a safe deletion target.
    pending_delete: Option<SessionIdentity>,
    /// Session whose action/preview flow is open. Unlike the browse cursor,
    /// this identity cannot drift to a neighbor when a streaming scan removes
    /// or reorders rows.
    active_session: Option<SessionIdentity>,
    pub new_session_options: Vec<NewSessionOption>,
    pub mode_index: usize,
    pub mode_options: Vec<(&'static str, &'static str)>,
    pub resume_mode_index: usize,
    pub resume_mode_options: Vec<(&'static str, &'static str)>,
    pub scroll_offset: usize,
    pub viewport_height: usize,
    pub sort_mode: SortMode,
    /// Multi-select for bulk delete, keyed by session identity — NOT by
    /// `sessions` Vec index. A background scan can reorder/replace `sessions`
    /// between selection and delete (every render frame drains scan results),
    /// so an index-keyed set would resolve to the wrong sessions at delete
    /// time and destroy the wrong data.
    ///
    /// Grouped by agent so membership tests borrow (`HashSet<String>::contains`
    /// accepts `&str`) instead of allocating a key per row per frame, and so
    /// the delete pass gets its per-agent batches for free.
    pub selected_set: HashMap<Agent, HashSet<String>>,
    pub summary_offsets: HashMap<Agent, HashMap<String, usize>>,
    pub summary_search_count: usize,
    pub include_summaries: bool,
    pub show_recap: bool,
    pub help_selected: usize,
    pub search_textarea: slt::TextareaState,
    /// Current working directory at TUI launch. Previously drove a cwd-match
    /// boost in `apply_sort` (removed in v0.11.0); kept on the struct so the
    /// surrounding wiring (CLI plumbing in `main.rs`, `App::new` signature)
    /// stays stable for a future settings-gated reintroduction.
    #[allow(dead_code)]
    pub cwd: Option<String>,
    pub agent_counts: HashMap<Agent, usize>,
    pub pinned_sessions: Vec<String>,
    pub settings: crate::settings::Settings,
    pub groups: Vec<ProjectGroup>,
    pub group_expanded: HashSet<String>,
    pub grouped_selected: usize,
    pub grouped_scroll: usize,
    /// Cached max project-name column width across the current filtered list.
    /// Computed in `update_filter()`; invalidated in `apply_sort()`.
    pub name_col_width_cache: Option<usize>,
    /// Channel from background scan workers. `None` means no scan in flight.
    /// Drained on every render tick; replaced with `None` once the senders
    /// have all dropped (i.e. every stale agent has reported in).
    pub scan_rx: Option<Receiver<ScanResult>>,
    /// Agents whose background scan is still running. Drives the
    /// "Refreshing N agents…" footer indicator.
    pub scanning_agents: HashSet<Agent>,
    /// Failed/panicked workers are no longer shown as scanning, but their
    /// previous cache entries must be preserved on exit.
    pub failed_agents: HashSet<Agent>,
    /// Source snapshot paired with each successfully ingested worker result.
    /// Cache writes must use this snapshot, never a later unrelated mtime.
    pub scan_fingerprints: HashMap<Agent, crate::cache::SourceFingerprint>,
    /// Successful deletes made after workers started. Late results from those
    /// workers are filtered so they cannot resurrect a deleted row.
    deleted_tombstones: HashMap<Agent, HashSet<String>>,
    fuzzy: FuzzyMatcher,
}

impl App {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        sessions: Vec<Session>,
        initial_query: Option<String>,
        summary_search_count: usize,
        include_summaries: bool,
        cwd: Option<String>,
        pinned_sessions: Vec<String>,
        settings: crate::settings::Settings,
        scan_rx: Option<Receiver<ScanResult>>,
        scanning_agents: HashSet<Agent>,
    ) -> Self {
        let mut agent_counts: HashMap<Agent, usize> = HashMap::new();
        for s in &sessions {
            *agent_counts.entry(s.agent).or_insert(0) += 1;
        }
        let mut sorted_agents = installed_agents();
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

        let session_index = sessions
            .iter()
            .enumerate()
            .map(|(index, session)| (session.identity(), index))
            .collect();
        let filtered_indices: Vec<usize> = (0..sessions.len()).collect();
        let match_positions: Vec<Vec<u32>> = vec![Vec::new(); sessions.len()];
        let query = initial_query.unwrap_or_default();
        let search_textarea = {
            let mut ta = slt::TextareaState::new();
            if !query.is_empty() {
                ta.lines = vec![query.clone()];
                ta.cursor_col = query.graphemes(true).count();
            }
            ta
        };
        let show_recap = settings.show_recap;
        let mut app = Self {
            sessions,
            session_index,
            filtered_indices,
            match_positions,
            selected: 0,
            query,
            mode: Mode::Browse,
            agent_filter: None,
            action_index: 0,
            agent_index: 0,
            delete_index: 1,
            pending_delete: None,
            active_session: None,
            new_session_options,
            mode_index: 0,
            mode_options: Vec::new(),
            resume_mode_index: 0,
            resume_mode_options: Vec::new(),
            scroll_offset: 0,
            viewport_height: 4,
            sort_mode: SortMode::Time,
            selected_set: HashMap::new(),
            summary_offsets: HashMap::new(),
            summary_search_count,
            include_summaries,
            show_recap,
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
            name_col_width_cache: None,
            scan_rx,
            scanning_agents,
            failed_agents: HashSet::new(),
            scan_fingerprints: HashMap::new(),
            deleted_tombstones: HashMap::new(),
            fuzzy: FuzzyMatcher::new(),
        };
        if !app.query.is_empty() {
            app.update_filter();
        }
        app
    }

    pub fn apply_sort(&mut self) {
        // Snapshot the selected session's identity so we can restore the
        // cursor position after the underlying Vec is reordered.
        let pivot = self
            .selected_session()
            .map(|s| (s.agent, s.session_id.clone()));
        self.apply_sort_preserving(pivot);
    }

    fn apply_sort_preserving(&mut self, pivot: Option<(Agent, String)>) {
        self.sessions
            .sort_by(|a, b| compare_sessions(a, b, self.sort_mode));

        // Boost: pinned first; everything else keeps the primary sort order
        // (stable sort preserves it within rank). The previous cwd-match
        // boost was removed in v0.11.0 because it implicitly grouped the
        // current project's sessions above older sessions from elsewhere,
        // which made the listing look "out of time order" without any
        // visible cause — `agf` has no UI hint that cwd boost is active.
        // Pinning is still honored because it's an explicit user action.
        let pinned = &self.pinned_sessions;
        self.sessions
            .sort_by_key(|session| !is_pinned_in(pinned, session));
        self.rebuild_session_index();

        // Sessions reordered → cached column width no longer valid.
        self.name_col_width_cache = None;

        self.update_filter();

        // Restore the selection to the same session after reordering.
        if let Some((agent, id)) = pivot
            && let Some(new_pos) = self
                .filtered_indices
                .iter()
                .position(|&i| self.sessions[i].agent == agent && self.sessions[i].session_id == id)
        {
            self.selected = new_pos;
            self.adjust_scroll();
        }
    }

    pub fn update_filter(&mut self) {
        let agent_filtered: Vec<usize> = self
            .sessions
            .iter()
            .enumerate()
            .filter(|(_, session)| {
                (self.settings.include_non_interactive || session.interactive)
                    && self.agent_filter.is_none_or(|agent| session.agent == agent)
            })
            .map(|(i, _)| i)
            .collect();

        if self.query.is_empty() {
            self.match_positions = vec![Vec::new(); agent_filtered.len()];
            self.filtered_indices = agent_filtered;
        } else {
            // No-clone fuzzy search: fuzzy::filter iterates `sessions` via the
            // provided indices slice so we don't need to materialize a subset.
            let results = self.fuzzy.filter(
                &self.sessions,
                &agent_filtered,
                &self.query,
                self.summary_search_count,
                self.include_summaries,
            );

            self.filtered_indices = results.iter().map(|r| agent_filtered[r.index]).collect();
            self.match_positions = results.into_iter().map(|r| r.positions).collect();
        }

        if let Some(max) = self.settings.max_sessions {
            self.filtered_indices.truncate(max);
            self.match_positions.truncate(max);
        }

        if self.filtered_indices.is_empty() {
            self.selected = 0;
        } else if self.selected >= self.filtered_indices.len() {
            self.selected = self.filtered_indices.len() - 1;
        }

        // Recompute the cached project-name column width for the new filter set.
        let name_col_width = self
            .filtered_indices
            .iter()
            .map(|&i| text::width(&text::sanitize_terminal(&self.sessions[i].project_name)))
            .max()
            .unwrap_or(0)
            .min(30); // cap at 30 chars to leave room for summary
        self.name_col_width_cache = Some(name_col_width);

        self.adjust_scroll();
    }

    pub fn selected_session(&self) -> Option<&Session> {
        self.filtered_indices
            .get(self.selected)
            .and_then(|&i| self.sessions.get(i))
    }

    fn capture_active_session(&mut self) -> bool {
        self.active_session = self
            .selected_session()
            .filter(|session| crate::model::valid_resume_id(&session.session_id))
            .map(Session::identity);
        self.active_session.is_some()
    }

    fn action_session(&self) -> Option<&Session> {
        self.active_session
            .as_ref()
            .and_then(|identity| self.session_by_identity(identity))
    }

    fn is_pinned(&self, session: &Session) -> bool {
        is_pinned_in(&self.pinned_sessions, session)
    }

    /// Total number of sessions checked for bulk delete.
    fn selection_count(&self) -> usize {
        self.selected_set.values().map(HashSet::len).sum()
    }

    /// Is this session checked for bulk delete? Borrows the id — no allocation
    /// on the per-row render path.
    fn is_checked(&self, session: &Session) -> bool {
        self.selected_set
            .get(&session.agent)
            .is_some_and(|ids| ids.contains(session.session_id.as_str()))
    }

    fn toggle_checked(&mut self, agent: Agent, session_id: &str) {
        if !agent.supports_delete() {
            return;
        }
        let ids = self.selected_set.entry(agent).or_default();
        if !ids.remove(session_id) {
            ids.insert(session_id.to_string());
        }
        if ids.is_empty() {
            self.selected_set.remove(&agent);
        }
    }

    /// Rebuild the per-agent counts from `sessions`.
    ///
    /// Cheaper to be right than to be incremental: `max_sessions` truncation
    /// and failed deletes both make an adjust-by-delta count drift away from
    /// the list it labels.
    fn recount_agents(&mut self) {
        self.agent_counts.clear();
        for session in &self.sessions {
            *self.agent_counts.entry(session.agent).or_insert(0) += 1;
        }
    }

    pub fn cycle_summary(&mut self, forward: bool) {
        let Some(session) = self.selected_session() else {
            return;
        };
        let count = session.summaries.len();
        if count <= 1 {
            return;
        }
        let agent = session.agent;
        let session_id = session.session_id.clone();
        let offset = self
            .summary_offsets
            .entry(agent)
            .or_default()
            .entry(session_id)
            .or_insert(0);
        *offset = if forward {
            (*offset + 1) % count
        } else if *offset == 0 {
            count - 1
        } else {
            *offset - 1
        };
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
        let margin = 3usize.min(visible.saturating_sub(1));
        if self.selected < self.scroll_offset {
            self.scroll_offset = self.selected;
        } else if self.selected >= self.scroll_offset + visible.saturating_sub(margin) {
            // Saturating form: the margin branch can trigger while
            // selected < visible (e.g. viewport 10, margin 3, selected 7),
            // where `selected - visible` would underflow usize.
            self.scroll_offset = (self.selected + margin + 1).saturating_sub(visible);
        }
        let max_offset = self.filtered_indices.len().saturating_sub(visible);
        if self.scroll_offset > max_offset {
            self.scroll_offset = max_offset;
        }
    }

    pub fn build_groups(&mut self) {
        let mut map: std::collections::BTreeMap<String, Vec<SessionIdentity>> =
            std::collections::BTreeMap::new();
        for &idx in &self.filtered_indices {
            let s = &self.sessions[idx];
            map.entry(s.project_path.clone())
                .or_default()
                .push(s.identity());
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
                    project_name: text::sanitize_terminal(&name),
                    sessions,
                }
            })
            .collect();
        // Sort groups: most recent session first. Use the MAX timestamp across
        // each group's sessions, not `.first()` — `.first()` is only the newest
        // when the list is in Time sort; in Name/Agent sort it is not, which
        // ordered the groups incorrectly.
        let session_index = &self.session_index;
        let all_sessions = &self.sessions;
        self.groups.sort_by(|a, b| {
            let a_ts = a
                .sessions
                .iter()
                .filter_map(|identity| session_index.get(identity))
                .filter_map(|index| all_sessions.get(*index))
                .map(|session| session.timestamp)
                .max()
                .unwrap_or(0);
            let b_ts = b
                .sessions
                .iter()
                .filter_map(|identity| session_index.get(identity))
                .filter_map(|index| all_sessions.get(*index))
                .map(|session| session.timestamp)
                .max()
                .unwrap_or(0);
            b_ts.cmp(&a_ts)
        });
    }

    fn rebuild_session_index(&mut self) {
        self.session_index = self
            .sessions
            .iter()
            .enumerate()
            .map(|(index, session)| (session.identity(), index))
            .collect();
    }

    fn session_by_identity(&self, identity: &SessionIdentity) -> Option<&Session> {
        self.session_index
            .get(identity)
            .and_then(|index| self.sessions.get(*index))
    }

    fn filtered_position_for_identity(&self, identity: &SessionIdentity) -> Option<usize> {
        let index = *self.session_index.get(identity)?;
        self.filtered_indices
            .iter()
            .position(|candidate| *candidate == index)
    }

    fn grouped_selection_identity(&self) -> Option<GroupSelection> {
        let (group_index, child) = self.grouped_row_at(self.grouped_selected)?;
        let group = self.groups.get(group_index)?;
        Some(match child {
            None => GroupSelection::Header(group.project_path.clone()),
            Some(child_index) => GroupSelection::Session(group.sessions.get(child_index)?.clone()),
        })
    }

    fn restore_grouped_selection(&mut self, selection: Option<GroupSelection>) {
        let Some(selection) = selection else {
            self.grouped_selected = self
                .grouped_selected
                .min(self.grouped_row_count().saturating_sub(1));
            return;
        };
        let mut row = 0;
        for group in &self.groups {
            if selection == GroupSelection::Header(group.project_path.clone()) {
                self.grouped_selected = row;
                return;
            }
            row += 1;
            if self.group_expanded.contains(&group.project_path) {
                for identity in &group.sessions {
                    if selection == GroupSelection::Session(identity.clone()) {
                        self.grouped_selected = row;
                        return;
                    }
                    row += 1;
                }
            }
        }
        self.grouped_selected = self
            .grouped_selected
            .min(self.grouped_row_count().saturating_sub(1));
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
        // Reuse the pre-built agent_counts map: an agent is "present" iff it
        // has at least one session (count > 0).
        Agent::all()
            .iter()
            .copied()
            .filter(|a| self.agent_counts.get(a).is_some_and(|&n| n > 0))
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

    /// Drain any pending background scan results into `self.sessions`.
    /// Called once per render frame so freshly-scanned agents appear in the
    /// list as soon as their worker thread finishes.
    pub fn ingest_scan_results(&mut self) {
        // Take the receiver out so we can mutate self while polling.
        let Some(rx) = self.scan_rx.take() else {
            return;
        };

        // At the top of the browse list, follow the top-ranked session as
        // streaming results arrive. Once the user has moved lower (or opened
        // another mode), keep the chosen session anchored across refreshes.
        // Capture before merging because filtered_indices still refer to the
        // pre-merge Vec.
        let pivot = if self.mode == Mode::Browse && self.selected == 0 {
            None
        } else {
            self.selected_session()
                .map(|s| (s.agent, s.session_id.clone()))
        };
        let grouped_pivot = (self.mode == Mode::GroupedBrowse)
            .then(|| self.grouped_selection_identity())
            .flatten();
        let mut merged_any = false;
        let mut channel_open = true;
        loop {
            match rx.try_recv() {
                Ok(result) => {
                    self.scanning_agents.remove(&result.agent);
                    match result.sessions {
                        Ok(scan) => {
                            self.failed_agents.remove(&result.agent);
                            if let Some(fingerprint) = scan.fingerprint {
                                self.scan_fingerprints.insert(result.agent, fingerprint);
                            } else {
                                self.scan_fingerprints.remove(&result.agent);
                            }
                            self.merge_agent_sessions(result.agent, scan.sessions);
                            merged_any = true;
                        }
                        Err(error) => {
                            self.failed_agents.insert(result.agent);
                            if std::env::var("AGF_DEBUG").is_ok() {
                                eprintln!("[agf] {} refresh failed: {error}", result.agent);
                            }
                        }
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    channel_open = false;
                    break;
                }
            }
        }
        if channel_open {
            // More results may arrive — keep polling next frame.
            self.scan_rx = Some(rx);
        } else if !self.scanning_agents.is_empty() {
            self.failed_agents.extend(self.scanning_agents.drain());
        }
        if merged_any {
            // Re-apply current sort + filter so new sessions land in the
            // correct order and the cached column width is recomputed.
            self.apply_sort_preserving(pivot);
            if self.mode == Mode::GroupedBrowse {
                self.build_groups();
                self.restore_grouped_selection(grouped_pivot);
            }
        }
    }

    /// Replace all sessions for `agent` with `new_sessions`. Caller is
    /// responsible for re-sorting / re-filtering.
    fn merge_agent_sessions(&mut self, agent: Agent, new_sessions: Vec<Session>) {
        self.sessions.retain(|s| s.agent != agent);
        let tombstones = self.deleted_tombstones.get(&agent);
        self.sessions
            .extend(new_sessions.into_iter().filter(|session| {
                !tombstones.is_some_and(|ids| ids.contains(session.session_id.as_str()))
            }));
        self.recount_agents();
    }

    pub fn cache_skip_agents(&self) -> HashSet<Agent> {
        self.scanning_agents
            .union(&self.failed_agents)
            .copied()
            .collect()
    }

    pub fn cache_invalidate_agents(&self) -> HashSet<Agent> {
        self.deleted_tombstones.keys().copied().collect()
    }

    pub fn run(&mut self) -> anyhow::Result<Option<String>> {
        let mut result: Option<String> = None;
        let app = self;
        slt::run_with(
            slt::RunConfig::default().title("agf").mouse(true),
            |ui: &mut slt::Context| {
                app.ingest_scan_results();
                app.viewport_height = list_viewport_height(ui.height() as usize, app.mode);
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

fn is_pinned_in(pins: &[String], session: &Session) -> bool {
    pins.iter().any(|saved| {
        saved == &session.session_id
            || saved
                .strip_prefix(session.agent.slug())
                .and_then(|rest| rest.strip_prefix(':'))
                == Some(session.session_id.as_str())
    })
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
    let ctrl_left = ui.key_mod('h', slt::KeyModifiers::CONTROL);
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
    if ctrl_left {
        ui.consume_key('h');
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
    if enter && app.capture_active_session() {
        app.action_index = 0;
        app.mode = Mode::ActionSelect;
    }
    if (right || ctrl_right) && app.capture_active_session() {
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

    // Mouse: the blank top row, search row, and separator occupy y=0..=2.
    if let Some((x, y)) = ui.mouse_down()
        && x < ui.width()
        && let Some(clicked_vi) = browse_click_index(
            y as usize,
            app.scroll_offset,
            app.filtered_indices.len(),
            app.viewport_height,
        )
    {
        app.selected = clicked_vi;
        app.adjust_scroll();
        app.action_index = 0;
        if app.capture_active_session() {
            app.mode = Mode::ActionSelect;
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

        let _ = ui.separator_colored(SEPARATOR);

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
            // Background-scan progress: appears while stale agents refresh
            // and disappears once every worker has reported in.
            if !app.scanning_agents.is_empty() {
                ui.text(format!(" • scanning {}…", app.scanning_agents.len()))
                    .fg(YELLOW);
            }
        });

        // Separator between content and statusbar
        let _ = ui.separator_colored(SEPARATOR);

        render_footer(
            ui,
            &[
                ("↑↓", "nav"),
                ("Tab", "agent"),
                ("[ or ]", "summary"),
                ("→", "detail"),
                ("Enter", "select"),
                ("^S", "sort"),
                ("^G", "group"),
                ("^D", "delete"),
                ("?", "help"),
                ("Esc", "quit"),
            ],
        );
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
        app.search_textarea.cursor_col = merged.graphemes(true).count();
    }
}

fn list_viewport_height(height: usize, mode: Mode) -> usize {
    let reserved = match mode {
        Mode::GroupedBrowse | Mode::BulkDelete => 5,
        _ => 6,
    };
    height.saturating_sub(reserved)
}

fn browse_click_index(
    y: usize,
    scroll_offset: usize,
    session_count: usize,
    visible: usize,
) -> Option<usize> {
    let row = y.checked_sub(BROWSE_FIRST_SESSION_ROW)?;
    if row >= visible {
        return None;
    }
    let index = scroll_offset.checked_add(row)?;
    (index < session_count).then_some(index)
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
    let ctrl_left = ui.key_mod('h', slt::KeyModifiers::CONTROL);
    let ctrl_right = ui.key_mod('l', slt::KeyModifiers::CONTROL);
    let ctrl_group = ui.key_mod('g', slt::KeyModifiers::CONTROL);
    if ctrl_up {
        ui.consume_key('p');
        ui.consume_key('k');
    }
    if ctrl_down {
        ui.consume_key('n');
        ui.consume_key('j');
    }
    if ctrl_left {
        ui.consume_key('h');
    }
    if ctrl_right {
        ui.consume_key('l');
    }
    if ctrl_group {
        ui.consume_key('g');
    }

    if esc || ctrl_group {
        app.mode = Mode::Browse;
        return;
    }
    if ctrl_right && let Some((gi, Some(ci))) = app.grouped_row_at(app.grouped_selected) {
        let identity = app.groups[gi].sessions[ci].clone();
        if let Some(vi) = app.filtered_position_for_identity(&identity) {
            app.selected = vi;
            if app.capture_active_session() {
                app.mode = Mode::Preview;
            }
            return;
        }
    }

    let total_rows = app.grouped_row_count();
    if (up || ctrl_up) && app.grouped_selected > 0 {
        app.grouped_selected -= 1;
    }
    if (down || ctrl_down) && app.grouped_selected + 1 < total_rows {
        app.grouped_selected += 1;
    }

    // Enter/Space on header: toggle expand. Enter on child: open action menu.
    if (enter || space)
        && let Some((gi, child)) = app.grouped_row_at(app.grouped_selected)
    {
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
                let identity = app.groups[gi].sessions[ci].clone();
                if let Some(vi) = app.filtered_position_for_identity(&identity) {
                    app.selected = vi;
                    app.action_index = 0;
                    if app.capture_active_session() {
                        app.mode = Mode::ActionSelect;
                    }
                }
            }
        }
    }

    // Scroll
    let visible = app.viewport_height.max(1);
    let margin = 3usize.min(visible.saturating_sub(1));
    if app.grouped_selected < app.grouped_scroll {
        app.grouped_scroll = app.grouped_selected;
    } else if app.grouped_selected >= app.grouped_scroll + visible.saturating_sub(margin) {
        app.grouped_scroll = (app.grouped_selected + margin + 1).saturating_sub(visible);
    }
    let max_grouped_offset = total_rows.saturating_sub(visible);
    if app.grouped_scroll > max_grouped_offset {
        app.grouped_scroll = max_grouped_offset;
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
        let _ = ui.separator_colored(SEPARATOR);

        let _ = ui.container().grow(1).pr(1).col(|ui| {
            if app.groups.is_empty() {
                let _ = ui.container().pl(2).col(|ui| {
                    let _ = ui.empty_state("No projects", "Try a different filter");
                });
                return;
            }

            let total_width = ui.width() as usize;
            let end = (app.grouped_scroll + app.viewport_height).min(total_rows);
            let mut row_idx = 0;
            for group in app.groups.iter() {
                let expanded = app.group_expanded.contains(&group.project_path);
                let session_count = group.sessions.len();

                // Get most recent timestamp for the group
                let latest_time = group
                    .sessions
                    .iter()
                    .filter_map(|identity| app.session_by_identity(identity))
                    .max_by_key(|session| session.timestamp)
                    .map(Session::time_display)
                    .unwrap_or_default();
                // Agents in this group
                let mut agent_set: Vec<Agent> = Vec::new();
                for identity in &group.sessions {
                    if let Some(session) = app.session_by_identity(identity)
                        && !agent_set.contains(&session.agent)
                    {
                        agent_set.push(session.agent);
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
                        if let Ok(rest) =
                            std::path::Path::new(&group.project_path).strip_prefix(&home)
                        {
                            if rest.as_os_str().is_empty() {
                                "~".to_string()
                            } else {
                                format!("~/{}", rest.to_string_lossy())
                            }
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
                        ui.styled(
                            text::sanitize_terminal(&display_path),
                            slt::Style::new().fg(GRAY_500).bg(bg),
                        );
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
                    for (ci, identity) in group.sessions.iter().enumerate() {
                        if row_idx >= app.grouped_scroll && row_idx < end {
                            let Some(s) = app.session_by_identity(identity) else {
                                row_idx += 1;
                                continue;
                            };
                            let is_selected = row_idx == app.grouped_selected;
                            let bg = if is_selected {
                                HIGHLIGHT_BG
                            } else {
                                slt::Color::Reset
                            };
                            let is_last = ci == group.sessions.len() - 1;
                            let tree_char = if is_last { "  └─ " } else { "  ├─ " };
                            let is_pinned = app.is_pinned(s);
                            let pin_str = if is_pinned { "*" } else { " " };

                            // Calculate available space for summary
                            let fixed_width = 5 + 1 + 12 + 2 + 16; // tree + pin + agent + gap + time
                            let git_width = s
                                .git_branch
                                .as_ref()
                                .map_or(0, |b| text::width(&text::sanitize_terminal(b)) + 2);
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
                                        text::sanitize_terminal(branch),
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

        let _ = ui.separator_colored(SEPARATOR);
        render_footer(
            ui,
            &[
                ("↑↓", "nav"),
                ("Enter/Space", "expand"),
                ("^G", "flat view"),
                ("Esc", "back"),
            ],
        );
    });
}

fn available_actions(session: &Session) -> Vec<Action> {
    Action::MENU
        .into_iter()
        .filter(|action| {
            (*action != Action::Delete || session.agent.supports_delete())
                && (*action != Action::Cd || !session.project_path.is_empty())
        })
        .collect()
}

fn ui_action_select(ui: &mut slt::Context, app: &mut App, result: &mut Option<String>) {
    let Some(actions) = app.action_session().map(available_actions) else {
        app.active_session = None;
        app.mode = Mode::Browse;
        return;
    };
    let action_count = actions.len();
    app.action_index = app.action_index.min(action_count.saturating_sub(1));

    if ui.key_code(slt::KeyCode::Esc) {
        app.active_session = None;
        app.mode = Mode::Browse;
    }

    if ui.consume_key_code(slt::KeyCode::BackTab)
        || ui.key_code(slt::KeyCode::Up)
        || ui.key_mod('p', slt::KeyModifiers::CONTROL)
        || ui.key_mod('k', slt::KeyModifiers::CONTROL)
        || ui.key('k')
    {
        app.action_index = (app.action_index + action_count - 1) % action_count;
    } else if ui.consume_key_code(slt::KeyCode::Tab)
        || ui.key_code(slt::KeyCode::Down)
        || ui.key_mod('n', slt::KeyModifiers::CONTROL)
        || ui.key_mod('j', slt::KeyModifiers::CONTROL)
        || ui.key('j')
    {
        app.action_index = (app.action_index + 1) % action_count;
    }

    // Iterate the digit chars themselves: deriving them via `b'1' + i as u8`
    // needs a lossy usize->u8 cast to say something the range already states.
    for (i, key) in ('1'..='9').enumerate().take(action_count.min(9)) {
        if ui.key(key) {
            app.action_index = i;
            // Number-key Resume should mirror the Enter flow: open the mode
            // picker instead of dispatching Resume directly. Other actions
            // dispatch immediately.
            if actions[app.action_index] == Action::Resume {
                if let Some(session) = app.action_session() {
                    app.resume_mode_options = session.agent.resume_mode_options().to_vec();
                    app.resume_mode_index = 0;
                    app.mode = Mode::ResumeSelect;
                }
            } else {
                dispatch_action(ui, app, actions[app.action_index], result);
            }
        }
    }

    // Mouse: click on action item
    if let Some((_x, y)) = ui.mouse_down() {
        let y = y as usize;
        if y >= 4 && y < 4 + action_count {
            let clicked = y - 4;
            app.action_index = clicked;
            if actions[app.action_index] == Action::Resume {
                if let Some(session) = app.action_session() {
                    app.resume_mode_options = session.agent.resume_mode_options().to_vec();
                    app.resume_mode_index = 0;
                    app.mode = Mode::ResumeSelect;
                }
            } else {
                dispatch_action(ui, app, actions[app.action_index], result);
            }
        }
    }

    if ui.key_code(slt::KeyCode::Enter) {
        // Resume → go to mode picker; others → dispatch directly
        if actions[app.action_index] == Action::Resume {
            if let Some(session) = app.action_session() {
                app.resume_mode_options = session.agent.resume_mode_options().to_vec();
                app.resume_mode_index = 0;
                app.mode = Mode::ResumeSelect;
            }
        } else {
            dispatch_action(ui, app, actions[app.action_index], result);
        }
    }

    let Some(session) = app.action_session() else {
        app.active_session = None;
        app.mode = Mode::Browse;
        return;
    };

    let _ = ui.col(|ui| {
        let _ = ui.separator_colored(SEPARATOR);
        ui.line(|ui| {
            ui.text(format!(" {} ", session.agent))
                .fg(agent_color(session.agent))
                .bold();
            ui.text("| ").fg(SEPARATOR);
            ui.text(text::sanitize_terminal(&session.project_name))
                .fg(BRIGHT_WHITE)
                .bold();
            ui.text(" | ").fg(SEPARATOR);
            ui.text(session.display_path()).fg(GRAY_500);
            if let Some(branch) = &session.git_branch {
                ui.text(" | ").fg(SEPARATOR);
                ui.text(text::sanitize_terminal(branch)).fg(GREEN_400);
            }
            ui.text(" | ").fg(SEPARATOR);
            ui.text(session.time_display()).fg(VIOLET);
        });
        let _ = ui.separator_colored(SEPARATOR);
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
                        .action_session()
                        .is_some_and(|session| app.is_pinned(session));
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
                let preview = truncate_str(&action::action_preview(session, *act), total_width);
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
        let _ = ui.separator_colored(SEPARATOR);
        render_footer(
            ui,
            &[("Tab/jk", "nav"), ("Enter", "select"), ("Esc", "back")],
        );
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
            app.active_session = None;
            app.mode = Mode::Browse;
        }
        Action::NewSession => {
            app.agent_index = 0;
            app.mode = Mode::AgentSelect;
        }
        Action::Delete => {
            app.delete_index = 1;
            app.pending_delete = app.active_session.clone();
            if app.pending_delete.is_some() {
                app.mode = Mode::DeleteConfirm;
            }
        }
        Action::Pin => {
            if let Some(session) = app.action_session() {
                let key = session.settings_key();
                let legacy_id = session.session_id.clone();
                if let Some(pos) = app
                    .pinned_sessions
                    .iter()
                    .position(|saved| saved == &key || saved == &legacy_id)
                {
                    app.pinned_sessions.remove(pos);
                } else {
                    app.pinned_sessions.push(key);
                }
                app.save_settings();
                app.apply_sort();
            }
            app.active_session = None;
            app.mode = Mode::Browse;
        }
        _ => {
            if let Some(session) = app.action_session().cloned()
                && let Some(cmd) = action::generate_command(&session, selected_action, None)
            {
                result.replace(cmd);
                ui.quit();
            }
        }
    }
}

fn ui_agent_select(ui: &mut slt::Context, app: &mut App, result: &mut Option<String>) {
    let option_count = app.new_session_options.len();

    if ui.key_code(slt::KeyCode::Esc) {
        app.mode = Mode::ActionSelect;
    }

    if option_count > 0
        && (ui.consume_key_code(slt::KeyCode::BackTab)
            || ui.key_code(slt::KeyCode::Up)
            || ui.key_mod('p', slt::KeyModifiers::CONTROL)
            || ui.key_mod('k', slt::KeyModifiers::CONTROL))
    {
        app.agent_index = (app.agent_index + option_count - 1) % option_count;
    } else if option_count > 0
        && (ui.consume_key_code(slt::KeyCode::Tab)
            || ui.key_code(slt::KeyCode::Down)
            || ui.key_mod('n', slt::KeyModifiers::CONTROL)
            || ui.key_mod('j', slt::KeyModifiers::CONTROL))
    {
        app.agent_index = (app.agent_index + 1) % option_count;
    }

    // Iterate the digit chars themselves: deriving them via `b'1' + i as u8`
    // needs a lossy usize->u8 cast to say something the range already states.
    for (i, key) in ('1'..='9').enumerate().take(option_count.min(9)) {
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

    let Some(session) = app.action_session() else {
        app.active_session = None;
        app.mode = Mode::Browse;
        return;
    };

    let _ = ui.col(|ui| {
        let _ = ui.separator_colored(SEPARATOR);
        ui.line(|ui| {
            ui.text(" New session in ").fg(BRIGHT_WHITE);
            ui.text(session.display_path()).fg(GRAY_500);
            ui.text("  (enter -> permission mode)").fg(GRAY_500);
        });
        let _ = ui.separator_colored(SEPARATOR);
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
                let preview = if let Some(s) = app.action_session() {
                    let shell = crate::shell::CommandShell::from_env();
                    action::preview_cd_and(&shell, s, &opt.agent.new_session_command(&shell))
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
        let _ = ui.separator_colored(SEPARATOR);
        render_footer(
            ui,
            &[
                ("1-9", "select"),
                ("Tab", "nav"),
                ("Enter", "mode"),
                ("Esc", "back"),
            ],
        );
    });
}

fn permission_options_for(agent: Agent) -> Vec<(&'static str, &'static str)> {
    agent.resume_mode_options().to_vec()
}

fn dispatch_agent_option(ui: &mut slt::Context, app: &mut App, result: &mut Option<String>) {
    if let Some(opt) = app.new_session_options.get(app.agent_index)
        && let Some(session) = app.action_session().cloned()
    {
        let cmd = action::new_session_with_flags(&session, opt.agent, opt.command_suffix);
        result.replace(cmd);
        ui.quit();
    }
}

fn ui_permission_select(ui: &mut slt::Context, app: &mut App, result: &mut Option<String>) {
    let option_count = app.mode_options.len();

    if ui.key_code(slt::KeyCode::Esc) {
        app.mode = Mode::AgentSelect;
    }

    if option_count > 0
        && (ui.key_code(slt::KeyCode::BackTab)
            || ui.key_code(slt::KeyCode::Up)
            || ui.key_mod('p', slt::KeyModifiers::CONTROL)
            || ui.key_mod('k', slt::KeyModifiers::CONTROL))
    {
        app.mode_index = (app.mode_index + option_count - 1) % option_count;
    } else if option_count > 0
        && (ui.key_code(slt::KeyCode::Tab)
            || ui.key_code(slt::KeyCode::Down)
            || ui.key_mod('n', slt::KeyModifiers::CONTROL)
            || ui.key_mod('j', slt::KeyModifiers::CONTROL))
    {
        app.mode_index = (app.mode_index + 1) % option_count;
    }

    // Iterate the digit chars themselves: deriving them via `b'1' + i as u8`
    // needs a lossy usize->u8 cast to say something the range already states.
    for (i, key) in ('1'..='9').enumerate().take(option_count.min(9)) {
        if ui.key(key) {
            app.mode_index = i;
            dispatch_mode_option(ui, app, result);
        }
    }

    if ui.key_code(slt::KeyCode::Enter) {
        dispatch_mode_option(ui, app, result);
    }

    if app.action_session().is_none() {
        app.active_session = None;
        app.mode = Mode::Browse;
        return;
    }

    let agent_label = app
        .new_session_options
        .get(app.agent_index)
        .map_or("agent", |o| o.label.as_str());

    let _ = ui.col(|ui| {
        let _ = ui.separator_colored(SEPARATOR);
        ui.line(|ui| {
            ui.text(" Select mode for ").fg(BRIGHT_WHITE);
            ui.text(agent_label).fg(YELLOW).bold();
        });
        let _ = ui.separator_colored(SEPARATOR);
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
        let _ = ui.separator_colored(SEPARATOR);
        render_footer(
            ui,
            &[("1-9", "select"), ("Enter", "confirm"), ("Esc", "back")],
        );
    });
}

fn dispatch_mode_option(ui: &mut slt::Context, app: &mut App, result: &mut Option<String>) {
    if let Some((_, flags)) = app.mode_options.get(app.mode_index)
        && let Some(opt) = app.new_session_options.get(app.agent_index)
        && let Some(session) = app.action_session().cloned()
    {
        let cmd = action::new_session_with_flags(&session, opt.agent, flags);
        result.replace(cmd);
        ui.quit();
    }
}

fn ui_resume_select(ui: &mut slt::Context, app: &mut App, result: &mut Option<String>) {
    let option_count = app.resume_mode_options.len();

    if ui.key_code(slt::KeyCode::Esc) {
        app.mode = Mode::ActionSelect;
    }

    if option_count > 0
        && (ui.key_code(slt::KeyCode::BackTab)
            || ui.key_code(slt::KeyCode::Up)
            || ui.key_mod('p', slt::KeyModifiers::CONTROL)
            || ui.key_mod('k', slt::KeyModifiers::CONTROL))
    {
        app.resume_mode_index = (app.resume_mode_index + option_count - 1) % option_count;
    } else if option_count > 0
        && (ui.key_code(slt::KeyCode::Tab)
            || ui.key_code(slt::KeyCode::Down)
            || ui.key_mod('n', slt::KeyModifiers::CONTROL)
            || ui.key_mod('j', slt::KeyModifiers::CONTROL))
    {
        app.resume_mode_index = (app.resume_mode_index + 1) % option_count;
    }

    // Iterate the digit chars themselves: deriving them via `b'1' + i as u8`
    // needs a lossy usize->u8 cast to say something the range already states.
    for (i, key) in ('1'..='9').enumerate().take(option_count.min(9)) {
        if ui.key(key) {
            app.resume_mode_index = i;
            dispatch_resume_mode(ui, app, result);
        }
    }

    if ui.key_code(slt::KeyCode::Enter) {
        dispatch_resume_mode(ui, app, result);
    }

    let Some(session) = app.action_session() else {
        app.active_session = None;
        app.mode = Mode::Browse;
        return;
    };

    let _ = ui.col(|ui| {
        let _ = ui.separator_colored(SEPARATOR);
        ui.line(|ui| {
            ui.text(" Resume mode for ").fg(BRIGHT_WHITE);
            ui.text(format!("{}", session.agent))
                .fg(agent_color(session.agent))
                .bold();
        });
        let _ = ui.separator_colored(SEPARATOR);
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
        let _ = ui.separator_colored(SEPARATOR);
        render_footer(
            ui,
            &[("1-9", "select"), ("Enter", "confirm"), ("Esc", "back")],
        );
    });
}

fn dispatch_resume_mode(ui: &mut slt::Context, app: &mut App, result: &mut Option<String>) {
    if let Some((_, flags)) = app.resume_mode_options.get(app.resume_mode_index)
        && let Some(session) = app.action_session().cloned()
    {
        let cmd = action::resume_with_flags(&session, flags);
        result.replace(cmd);
        ui.quit();
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
        if let Some((agent, id)) = app
            .filtered_indices
            .get(app.selected)
            .and_then(|&i| app.sessions.get(i))
            .map(|s| (s.agent, s.session_id.clone()))
        {
            app.toggle_checked(agent, &id);
        }
        if !app.filtered_indices.is_empty() && app.selected < app.filtered_indices.len() - 1 {
            app.selected += 1;
            app.adjust_scroll();
        }
    }

    if ui.key_code(slt::KeyCode::Enter) && !app.selected_set.is_empty() {
        app.delete_index = 1;
        app.pending_delete = None;
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
                        ui.text(format!("  ({} selected)", app.selection_count()))
                            .fg(RED);
                    }
                });
            });

        let _ = ui.container().grow(1).col(|ui| {
            render_session_list(ui, app, true);
        });

        ui.line(|ui| {
            ui.text(format!(" {} selected", app.selection_count()))
                .fg(RED)
                .bold();
        });
        render_footer(
            ui,
            &[("Space", "toggle"), ("Enter", "delete"), ("Esc", "cancel")],
        );
    });
}

fn ui_delete_confirm(ui: &mut slt::Context, app: &mut App) {
    let is_bulk = !app.selected_set.is_empty();

    if ui.key_code(slt::KeyCode::Esc) {
        app.pending_delete = None;
        if is_bulk {
            app.mode = Mode::BulkDelete;
        } else {
            app.mode = Mode::ActionSelect;
        }
    }

    if ui.key_code(slt::KeyCode::Left)
        || ui.key_code(slt::KeyCode::Right)
        || ui.key_code(slt::KeyCode::Up)
        || ui.key_code(slt::KeyCode::Down)
        || ui.key('h')
        || ui.key('l')
        || ui.key('j')
        || ui.key('k')
        || ui.key_mod('h', slt::KeyModifiers::CONTROL)
        || ui.key_mod('l', slt::KeyModifiers::CONTROL)
        || ui.key_mod('j', slt::KeyModifiers::CONTROL)
        || ui.key_mod('k', slt::KeyModifiers::CONTROL)
    {
        app.delete_index = if app.delete_index == 0 { 1 } else { 0 };
    }

    if ui.key_code(slt::KeyCode::Enter) {
        if app.delete_index == 0 {
            if is_bulk {
                // Resolve identities to sessions at delete time. Keying by
                // identity — not by a Vec index captured at selection time — is
                // what keeps this correct when a background scan reordered
                // `sessions` in between.
                //
                // Narrowed to what is currently listed first: a session that
                // was scanned away since it was checked is no longer something
                // the user can see, so it is not something we delete.
                let mut targets: HashMap<Agent, HashSet<String>> = HashMap::new();
                for session in &app.sessions {
                    if app.is_checked(session) {
                        targets
                            .entry(session.agent)
                            .or_default()
                            .insert(session.session_id.clone());
                    }
                }
                app.selected_set.clear();

                // One filesystem/database pass per agent, not per session.
                // Only agents whose pass succeeded come back, so a failed
                // delete leaves its rows visible.
                let deleted = crate::delete::delete_selection(&targets);
                for (agent, ids) in &deleted {
                    app.deleted_tombstones
                        .entry(*agent)
                        .or_default()
                        .extend(ids.iter().cloned());
                }
                app.sessions.retain(|s| {
                    !deleted
                        .get(&s.agent)
                        .is_some_and(|ids| ids.contains(s.session_id.as_str()))
                });
                app.recount_agents();
                app.rebuild_session_index();
                app.update_filter();
            } else if let Some(identity) = app.pending_delete.take()
                && let Some(idx) = app.session_index.get(&identity).copied()
            {
                // Only drop the row from the UI when the on-disk delete
                // actually succeeded; a failed delete stays visible.
                if crate::delete::delete_session(&app.sessions[idx]).is_ok() {
                    app.deleted_tombstones
                        .entry(identity.agent)
                        .or_default()
                        .insert(identity.session_id.clone());
                    app.sessions.remove(idx);
                    app.recount_agents();
                    app.rebuild_session_index();
                }
                app.update_filter();
            }
            app.active_session = None;
            app.mode = Mode::Browse;
        } else if is_bulk {
            app.mode = Mode::BulkDelete;
        } else {
            app.pending_delete = None;
            app.active_session = None;
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
    let Some(session) = app
        .pending_delete
        .as_ref()
        .and_then(|identity| app.session_by_identity(identity))
    else {
        return;
    };

    let _ = ui.col(|ui| {
        let _ = ui.separator_colored(SEPARATOR);
        ui.text(" Delete session?").fg(RED).bold();
        let _ = ui.separator_colored(SEPARATOR);
        ui.text("");

        ui.line(|ui| {
            ui.text(format!("  {} ", session.agent))
                .fg(agent_color(session.agent))
                .bold();
            ui.text("| ").fg(SEPARATOR);
            ui.text(text::sanitize_terminal(&session.project_name))
                .fg(BRIGHT_WHITE);
            ui.text(" | ").fg(SEPARATOR);
            ui.text(text::sanitize_terminal(&session.session_id))
                .fg(GRAY_500);
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

        let _ = ui.separator_colored(SEPARATOR);
    });
}

fn render_bulk_delete_confirm(ui: &mut slt::Context, app: &App) {
    // Names come from the listed sessions, so the confirmation shows exactly
    // the rows the delete will act on (checked sessions that have since been
    // scanned away are skipped by both).
    let mut names: Vec<String> = app
        .sessions
        .iter()
        .filter(|s| app.is_checked(s))
        .map(|s| text::sanitize_terminal(&s.project_name))
        .collect();
    let count = names.len();
    names.sort();

    let _ = ui.col(|ui| {
        let _ = ui.separator_colored(SEPARATOR);
        ui.text(format!(" Delete {count} sessions?")).fg(RED).bold();
        let _ = ui.separator_colored(SEPARATOR);
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

        let _ = ui.separator_colored(SEPARATOR);
    });
}

fn ui_preview(ui: &mut slt::Context, app: &mut App) {
    // Only Esc dismisses the preview. Enter opens the action menu.
    // Left (or Ctrl-h) also goes back so users have a symmetrical "exit"
    // gesture to the Right-to-enter they used to get here.
    if ui.key_code(slt::KeyCode::Esc)
        || ui.key_code(slt::KeyCode::Left)
        || ui.key_mod('h', slt::KeyModifiers::CONTROL)
    {
        app.active_session = None;
        app.mode = Mode::Browse;
        return;
    }
    if ui.key_code(slt::KeyCode::Enter) {
        app.action_index = 0;
        app.mode = Mode::ActionSelect;
        return;
    }

    // Up/Down (and Ctrl-p/n, Ctrl-k/j) cycle to the previous/next session
    // within the current filter, keeping the preview open.
    let up = ui.key_code(slt::KeyCode::Up)
        || ui.key_mod('p', slt::KeyModifiers::CONTROL)
        || ui.key_mod('k', slt::KeyModifiers::CONTROL);
    let down = ui.key_code(slt::KeyCode::Down)
        || ui.key_mod('n', slt::KeyModifiers::CONTROL)
        || ui.key_mod('j', slt::KeyModifiers::CONTROL);
    if up && app.selected > 0 {
        app.selected -= 1;
        app.adjust_scroll();
        app.capture_active_session();
    }
    if down && !app.filtered_indices.is_empty() && app.selected < app.filtered_indices.len() - 1 {
        app.selected += 1;
        app.adjust_scroll();
        app.capture_active_session();
    }

    let Some(session) = app.action_session() else {
        app.active_session = None;
        app.mode = Mode::Browse;
        return;
    };

    let _ = ui.col(|ui| {
        let _ = ui.separator_colored(SEPARATOR);
        ui.text(" Session Detail").fg(BRIGHT_WHITE).bold();
        let _ = ui.separator_colored(SEPARATOR);
        ui.text("");

        ui.line(|ui| {
            ui.text("  Agent:    ").fg(GRAY_500);
            ui.text(session.agent.to_string())
                .fg(agent_color(session.agent))
                .bold();
        });
        ui.line(|ui| {
            ui.text("  Project:  ").fg(GRAY_500);
            ui.text(text::sanitize_terminal(&session.project_name))
                .fg(BRIGHT_WHITE)
                .bold();
        });
        ui.line(|ui| {
            ui.text("  Path:     ").fg(GRAY_500);
            ui.text(session.display_path()).fg(GRAY_400);
        });
        ui.line(|ui| {
            ui.text("  Session:  ").fg(GRAY_500);
            ui.text(text::sanitize_terminal(&session.session_id))
                .fg(GRAY_400);
        });
        ui.line(|ui| {
            ui.text("  Time:     ").fg(GRAY_500);
            ui.text(session.time_display()).fg(VIOLET);
        });

        if let Some(branch) = &session.git_branch {
            ui.line(|ui| {
                ui.text("  Branch:   ").fg(GRAY_500);
                ui.text(text::sanitize_terminal(branch)).fg(GREEN_400);
            });
        }
        if let Some(wt) = &session.worktree {
            ui.line(|ui| {
                ui.text("  Worktree: ").fg(GRAY_500);
                ui.text(text::sanitize_terminal(wt)).fg(CYAN);
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
        let _ = ui.separator_colored(SEPARATOR);
        render_footer(
            ui,
            &[("↑↓", "cycle"), ("Enter", "actions"), ("Esc/←", "back")],
        );
    });
}

fn ui_help(ui: &mut slt::Context, app: &mut App) {
    if ui.key_code(slt::KeyCode::Esc) || ui.key('q') {
        app.mode = Mode::Browse;
    }

    let ctrl_up =
        ui.key_mod('p', slt::KeyModifiers::CONTROL) || ui.key_mod('k', slt::KeyModifiers::CONTROL);
    let ctrl_down =
        ui.key_mod('n', slt::KeyModifiers::CONTROL) || ui.key_mod('j', slt::KeyModifiers::CONTROL);
    if ctrl_up {
        ui.consume_key('p');
        ui.consume_key('k');
    }
    if ctrl_down {
        ui.consume_key('n');
        ui.consume_key('j');
    }

    if (ui.key_code(slt::KeyCode::Up) || ctrl_up) && app.help_selected > 0 {
        app.help_selected -= 1;
    }

    if (ui.key_code(slt::KeyCode::Down) || ctrl_down) && app.help_selected < 2 {
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
        app.update_filter();
    }

    if app.help_selected == 1 && ui.key('-') {
        app.summary_search_count = app.summary_search_count.saturating_sub(1).max(1);
        app.save_settings();
        app.update_filter();
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
        let _ = ui.separator_colored(SEPARATOR);

        let _ = ui.container().pl(2).pr(1).grow(1).col(|ui| {
            ui.text("").dim();
            ui.text("Keybindings").fg(GRAY_400).bold();
            ui.text("").dim();
            help_line(ui, "↑ / ↓", "Navigate sessions");
            let _ = ui.row(|ui| {
                ui.styled("  [", slt::Style::new().fg(GRAY_500));
                ui.styled(" or ", slt::Style::new().fg(GRAY_400));
                ui.styled("]", slt::Style::new().fg(GRAY_500));
                ui.styled("          ", slt::Style::new());
                ui.text("Cycle summary").fg(GRAY_400);
            });
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

        let _ = ui.separator_colored(SEPARATOR);
        render_footer(
            ui,
            &[
                ("↑↓", "navigate"),
                ("Enter", "toggle"),
                ("+/-", "adjust"),
                ("Esc", "close"),
            ],
        );
    });
}

fn help_line(ui: &mut slt::Context, key: &str, desc: &str) {
    let _ = ui.row(|ui| {
        ui.styled(format!("  {key:<16}"), slt::Style::new().fg(GRAY_500));
        ui.text(desc).fg(GRAY_400);
    });
}

fn render_footer(ui: &mut slt::Context, hints: &[(&str, &str)]) {
    let _ = ui.container().px(1).row(|ui| {
        ui.text(concat!("agf v", env!("CARGO_PKG_VERSION")))
            .fg(GRAY_500);
        ui.spacer();
        let _ = ui.help_colored(hints, GRAY_500, SEPARATOR);
    });
}

fn render_session_list(ui: &mut slt::Context, app: &App, bulk_mode: bool) {
    let visible = app.viewport_height;
    let end = (app.scroll_offset + visible).min(app.filtered_indices.len());
    let total_width = ui.width() as usize;
    let right_margin = 1usize;

    // Use the cached project-name column width computed in update_filter().
    // Fallback path only runs if the cache was never populated (first frame
    // before any filter), which is cheap since the list is small.
    let name_col_width = app.name_col_width_cache.unwrap_or_else(|| {
        app.filtered_indices
            .iter()
            .map(|&i| UnicodeWidthStr::width(app.sessions[i].project_name.as_str()))
            .max()
            .unwrap_or(0)
            .min(30)
    });

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
            let deletable = session.agent.supports_delete();
            let is_checked = app.is_checked(session);
            let indicator = match (is_selected, is_checked, deletable) {
                (true, _, false) => ">[—] ",
                (false, _, false) => " [—] ",
                (true, true, true) => ">[x] ",
                (true, false, true) => ">[ ] ",
                (false, true, true) => " [x] ",
                (false, false, true) => " [ ] ",
            };
            let indicator_style = if !deletable {
                slt::Style::new().fg(GRAY_500).bg(bg)
            } else if is_checked {
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
                render_chunks(ui, chunks);
            });
        } else {
            let is_pinned = app.is_pinned(session);
            let indicator = match (is_selected, is_pinned) {
                (true, true) => ">*",
                (true, false) => "> ",
                (false, true) => " *",
                (false, false) => "  ",
            };
            let match_positions = app.match_positions.get(vi).map(Vec::as_slice);
            let summary_offset = app
                .summary_offsets
                .get(&session.agent)
                .and_then(|offsets| offsets.get(session.session_id.as_str()))
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
                render_chunks(ui, chunks);
            });
        }
    }
}

fn render_session_list_compact(ui: &mut slt::Context, app: &App) {
    let visible = app.viewport_height;
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
        let indicator = match (is_selected, app.is_pinned(session)) {
            (true, true) => ">*",
            (true, false) => "> ",
            (false, true) => " *",
            (false, false) => "  ",
        };

        let _ = ui.row(|ui| {
            ui.styled(
                indicator.to_string(),
                slt::Style::new().fg(slt::Color::White).bg(bg),
            );
            // `text::fit`, not `{:<n$}`: these are terminal columns, and
            // `{:<n$}` pads by char count, so a CJK project name here skewed
            // every column to its right.
            ui.styled(
                text::fit(&session.agent.to_string(), AGENT_COL_WIDTH),
                slt::Style::new()
                    .fg(agent_color(session.agent))
                    .bold()
                    .bg(bg),
            );
            ui.styled(
                text::fit(&session.project_name, 20),
                slt::Style::new().fg(BRIGHT_WHITE).bold().bg(bg),
            );
            if let Some(wt) = &session.worktree {
                ui.styled(text::fit(wt, 8), slt::Style::new().fg(CYAN).bg(bg));
            } else if let Some(branch) = &session.git_branch {
                ui.styled(text::fit(branch, 8), slt::Style::new().fg(GREEN_400).bg(bg));
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

    let agent_label = text::fit(&session.agent.to_string(), AGENT_COL_WIDTH);
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
        Some(format!("  {}", text::sanitize_terminal(wt)))
    } else {
        session
            .git_branch
            .as_ref()
            .map(|b| format!("  {}", text::sanitize_terminal(b)))
    };
    let git_info_width = git_info_str.as_deref().map_or(0, UnicodeWidthStr::width);

    // Use fixed column width for project name (padded to align columns)
    let fixed_left = indicator_width + AGENT_COL_WIDTH;
    let max_proj =
        total_width.saturating_sub(fixed_left + right_display_width + git_info_width + 4);
    let col_width = name_col_width.min(max_proj);
    let project_name = text::sanitize_terminal(&session.project_name);
    let proj_display = if col_width == 0 {
        String::new()
    } else if text::width(&project_name) > col_width {
        truncate_str(&project_name, col_width)
    } else {
        text::pad(&project_name, col_width)
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

    if available > 7
        && let Some(summary) = summary_text
    {
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

/// Emit pre-built row chunks. Takes the Vec by value: chunks are built fresh
/// per visible row each frame, so consuming them avoids a String clone per
/// chunk on the hot render path.
fn render_chunks(ui: &mut slt::Context, chunks: Vec<StyledChunk>) {
    for (text, style) in chunks {
        ui.styled(text, style);
    }
}

fn highlight_text(
    source: &str,
    positions: &[u32],
    offset: usize,
    bg: slt::Color,
) -> Vec<StyledChunk> {
    // `fuzzy::filter` hands back sorted, deduplicated positions, so probe them
    // with a binary search rather than the linear `contains` this used to do
    // once per character, per row, per frame.
    let is_match =
        |i: usize| u32::try_from(i + offset).is_ok_and(|pos| positions.binary_search(&pos).is_ok());

    let mut chunks = Vec::new();
    let chars: Vec<char> = source.chars().collect();

    let mut i = 0;
    while i < chars.len() {
        if is_match(i) {
            chunks.push((
                chars[i].to_string(),
                slt::Style::new().fg(YELLOW).bold().underline().bg(bg),
            ));
            i += 1;
        } else {
            let start = i;
            while i < chars.len() && !is_match(i) {
                i += 1;
            }
            let normal: String = chars[start..i].iter().collect();
            chunks.push((normal, slt::Style::new().fg(BRIGHT_WHITE).bold().bg(bg)));
        }
    }

    chunks
}

#[cfg(test)]
mod scroll_margin_tests {
    use super::*;

    fn make_app(n: usize) -> App {
        let sessions = (0..n)
            .map(|i| crate::model::Session {
                agent: crate::model::Agent::all()[0],
                session_id: format!("s{i}"),
                project_name: "p".into(),
                project_path: "/tmp/p".into(),
                summaries: Vec::new(),
                timestamp: i as i64,
                git_branch: None,
                worktree: None,
                recap: None,
                interactive: true,
            })
            .collect();
        App::new(
            sessions,
            None,
            5,
            false,
            None,
            Vec::new(),
            crate::settings::Settings::default(),
            None,
            HashSet::new(),
        )
    }

    /// The margin branch triggers while `selected < visible` (viewport 10,
    /// margin 3 → at selected == 7); the pre-fix `selected - visible + 1 +
    /// margin` underflowed usize and panicked in debug builds.
    #[test]
    fn adjust_scroll_does_not_underflow_when_margin_branch_fires_early() {
        let mut app = make_app(8);
        app.viewport_height = 10;
        app.selected = 7;
        app.adjust_scroll();
        // All 8 rows fit in a 10-row viewport: no scrolling at all.
        assert_eq!(app.scroll_offset, 0);
    }

    #[test]
    fn adjust_scroll_keeps_margin_rows_below_cursor_mid_list() {
        let mut app = make_app(30);
        app.viewport_height = 10;
        app.selected = 12;
        app.adjust_scroll();
        // offset = selected + margin + 1 - visible → rows 6..=15 visible,
        // cursor at 12 leaves margin (3) rows below it.
        assert_eq!(app.scroll_offset, 6);
    }

    #[test]
    fn adjust_scroll_clamps_to_list_end_instead_of_overscrolling() {
        let mut app = make_app(15);
        app.viewport_height = 10;
        app.selected = 14;
        app.adjust_scroll();
        // Margin would push offset to 8, but max_offset = 15 - 10 = 5.
        assert_eq!(app.scroll_offset, 5);
    }

    #[test]
    fn browse_footer_and_separator_are_not_session_rows() {
        let visible = list_viewport_height(24, Mode::Browse);
        assert_eq!(visible, 18);
        assert_eq!(browse_click_index(3, 7, 100, visible), Some(7));
        assert_eq!(browse_click_index(20, 7, 100, visible), Some(24));
        for y in [0, 1, 2, 21, 22, 23, usize::MAX] {
            assert_eq!(browse_click_index(y, 7, 100, visible), None);
        }
        assert_eq!(browse_click_index(3, 0, 100, 0), None);
    }

    #[test]
    fn actual_browse_footer_click_does_not_open_an_offscreen_session() {
        let mut app = make_app(100);
        let mut backend = slt::TestBackend::new(100, 24);
        app.viewport_height = list_viewport_height(24, Mode::Browse);
        backend.render(|ui| ui_browse(ui, &mut app));
        for y in [21, 22, 23] {
            backend.run_with_events(slt::EventBuilder::new().click(1, y).build(), |ui| {
                ui_browse(ui, &mut app)
            });
            assert_eq!(app.mode, Mode::Browse);
            assert_eq!(app.selected, 0);
        }
        backend.run_with_events(slt::EventBuilder::new().click(1, 4).build(), |ui| {
            ui_browse(ui, &mut app)
        });
        assert_eq!(app.mode, Mode::ActionSelect);
        assert_eq!(app.selected, 1);
    }

    #[test]
    fn tiny_and_resized_browse_frames_have_bounded_viewports() {
        for width in [1, 8, 39, 80, 120] {
            for height in [1, 5, 6, 7, 24] {
                let mut app = make_app(100);
                let mut backend = slt::TestBackend::new(width, height);
                app.viewport_height = list_viewport_height(height as usize, Mode::Browse);
                backend.render(|ui| ui_browse(ui, &mut app));
                assert_eq!(app.viewport_height, (height as usize).saturating_sub(6));
                assert_eq!(app.mode, Mode::Browse);
            }
        }
    }

    #[test]
    fn initial_search_caret_uses_graphemes() {
        let app = App::new(
            vec![],
            Some("e\u{301}한👩‍💻".into()),
            5,
            false,
            None,
            vec![],
            crate::settings::Settings::default(),
            None,
            HashSet::new(),
        );
        assert_eq!(app.search_textarea.cursor_col, 3);
    }
}

#[cfg(test)]
mod streaming_selection_tests {
    use super::*;
    use std::sync::mpsc;

    fn session(agent: Agent, id: &str, timestamp: i64) -> Session {
        Session {
            agent,
            session_id: id.into(),
            project_name: "p".into(),
            project_path: "/tmp/p".into(),
            summaries: Vec::new(),
            timestamp,
            git_branch: None,
            worktree: None,
            recap: None,
            interactive: true,
        }
    }

    fn completed(sessions: Vec<Session>) -> crate::scanner::CompletedScan {
        crate::scanner::CompletedScan {
            sessions,
            fingerprint: Some(crate::cache::SourceFingerprint::default()),
        }
    }

    fn make_app(
        sessions: Vec<Session>,
        scan_rx: mpsc::Receiver<ScanResult>,
        scanning_agents: HashSet<Agent>,
    ) -> App {
        App::new(
            sessions,
            None,
            5,
            false,
            None,
            Vec::new(),
            crate::settings::Settings::default(),
            Some(scan_rx),
            scanning_agents,
        )
    }

    #[test]
    fn streaming_newer_sessions_keep_initial_cursor_at_top() {
        let (tx, rx) = mpsc::channel();
        let mut app = make_app(
            Vec::new(),
            rx,
            HashSet::from([Agent::OpenCode, Agent::ClaudeCode]),
        );

        tx.send(ScanResult {
            agent: Agent::OpenCode,
            sessions: Ok(completed(vec![session(Agent::OpenCode, "opencode", 10)])),
        })
        .unwrap();
        app.ingest_scan_results();
        assert_eq!(app.selected_session().unwrap().session_id, "opencode");

        tx.send(ScanResult {
            agent: Agent::ClaudeCode,
            sessions: Ok(completed(vec![session(Agent::ClaudeCode, "claude", 20)])),
        })
        .unwrap();
        app.ingest_scan_results();

        assert_eq!(app.selected, 0);
        assert_eq!(app.selected_session().unwrap().session_id, "claude");
    }

    #[test]
    fn streaming_newer_sessions_preserve_non_top_selection() {
        let (tx, rx) = mpsc::channel();
        let mut app = make_app(
            vec![
                session(Agent::OpenCode, "newer-opencode", 20),
                session(Agent::OpenCode, "chosen-opencode", 10),
            ],
            rx,
            HashSet::from([Agent::ClaudeCode]),
        );
        app.apply_sort();
        app.selected = 1;

        tx.send(ScanResult {
            agent: Agent::ClaudeCode,
            sessions: Ok(completed(vec![session(Agent::ClaudeCode, "claude", 30)])),
        })
        .unwrap();
        app.ingest_scan_results();

        assert_eq!(
            app.selected_session().unwrap().session_id,
            "chosen-opencode"
        );
    }

    #[test]
    fn failed_stream_preserves_stale_rows_and_marks_agent_failed() {
        let (tx, rx) = mpsc::channel();
        let mut app = make_app(
            vec![session(Agent::Codex, "cached", 10)],
            rx,
            HashSet::from([Agent::Codex]),
        );
        tx.send(ScanResult {
            agent: Agent::Codex,
            sessions: Err("database is locked".into()),
        })
        .unwrap();
        app.ingest_scan_results();
        assert_eq!(app.sessions.len(), 1);
        assert_eq!(app.sessions[0].session_id, "cached");
        assert!(app.failed_agents.contains(&Agent::Codex));
        assert!(!app.scanning_agents.contains(&Agent::Codex));
    }

    #[test]
    fn action_target_never_drifts_when_refresh_removes_it() {
        let (tx, rx) = mpsc::channel();
        let mut app = make_app(
            vec![
                session(Agent::Codex, "chosen", 20),
                session(Agent::Codex, "neighbor", 10),
            ],
            rx,
            HashSet::from([Agent::Codex]),
        );
        app.apply_sort();
        assert!(app.capture_active_session());
        app.mode = Mode::ActionSelect;

        tx.send(ScanResult {
            agent: Agent::Codex,
            sessions: Ok(completed(vec![session(Agent::Codex, "neighbor", 30)])),
        })
        .unwrap();
        app.ingest_scan_results();

        assert!(app.action_session().is_none());
        assert_eq!(app.selected_session().unwrap().session_id, "neighbor");
    }

    #[test]
    fn late_scan_cannot_resurrect_a_deleted_identity() {
        let (tx, rx) = mpsc::channel();
        let mut app = make_app(Vec::new(), rx, HashSet::from([Agent::Codex]));
        app.deleted_tombstones
            .entry(Agent::Codex)
            .or_default()
            .insert("deleted".to_string());
        tx.send(ScanResult {
            agent: Agent::Codex,
            sessions: Ok(completed(vec![
                session(Agent::Codex, "deleted", 30),
                session(Agent::Codex, "keep", 20),
            ])),
        })
        .unwrap();

        app.ingest_scan_results();

        assert!(
            !app.sessions
                .iter()
                .any(|session| session.session_id == "deleted")
        );
        assert!(
            app.sessions
                .iter()
                .any(|session| session.session_id == "keep")
        );
    }

    #[test]
    fn grouped_streaming_rebuilds_identity_rows_without_stale_indices() {
        let (tx, rx) = mpsc::channel();
        let mut chosen = session(Agent::OpenCode, "chosen", 10);
        chosen.project_path = "/tmp/chosen".into();
        chosen.project_name = "chosen".into();
        let mut app = make_app(vec![chosen], rx, HashSet::from([Agent::ClaudeCode]));
        app.mode = Mode::GroupedBrowse;
        app.build_groups();
        app.group_expanded.insert("/tmp/chosen".into());
        app.grouped_selected = 1;

        let mut incoming = session(Agent::ClaudeCode, "new", 20);
        incoming.project_path = "/tmp/new".into();
        incoming.project_name = "new".into();
        tx.send(ScanResult {
            agent: Agent::ClaudeCode,
            sessions: Ok(completed(vec![incoming])),
        })
        .unwrap();
        app.ingest_scan_results();

        assert_eq!(
            app.grouped_selection_identity(),
            Some(GroupSelection::Session(SessionIdentity {
                agent: Agent::OpenCode,
                session_id: "chosen".into(),
            }))
        );
        assert!(app.groups.iter().all(|group| {
            group
                .sessions
                .iter()
                .all(|identity| app.session_by_identity(identity).is_some())
        }));
    }
}

#[cfg(test)]
mod bulk_selection_tests {
    use super::*;

    fn session(agent: Agent, id: &str, timestamp: i64) -> Session {
        Session {
            agent,
            session_id: id.into(),
            project_name: "p".into(),
            project_path: "/tmp/p".into(),
            summaries: Vec::new(),
            timestamp,
            git_branch: None,
            worktree: None,
            recap: None,
            interactive: true,
        }
    }

    fn app_with(sessions: Vec<Session>, settings: crate::settings::Settings) -> App {
        App::new(
            sessions,
            None,
            5,
            false,
            None,
            Vec::new(),
            settings,
            None,
            HashSet::new(),
        )
    }

    #[test]
    fn toggling_checks_and_unchecks_by_identity() {
        let mut app = app_with(
            vec![
                session(Agent::ClaudeCode, "shared-id", 30),
                session(Agent::Codex, "shared-id", 20),
            ],
            crate::settings::Settings::default(),
        );

        app.toggle_checked(Agent::Codex, "shared-id");

        // Same session_id, different agent: only the Codex row is checked.
        assert!(!app.is_checked(&app.sessions[0]));
        assert!(app.is_checked(&app.sessions[1]));
        assert_eq!(app.selection_count(), 1);

        app.toggle_checked(Agent::Codex, "shared-id");
        assert!(!app.is_checked(&app.sessions[1]));
        assert_eq!(app.selection_count(), 0);
        // The now-empty per-agent bucket is dropped, so `is_empty()` (which
        // drives "am I in bulk mode") stays truthful.
        assert!(app.selected_set.is_empty());
    }

    #[test]
    fn native_managed_sessions_cannot_enter_bulk_delete_selection() {
        let mut app = app_with(
            vec![session(Agent::Grok, "grok", 30)],
            crate::settings::Settings::default(),
        );
        app.toggle_checked(Agent::Grok, "grok");
        assert!(app.selected_set.is_empty());
        assert_eq!(app.selection_count(), 0);
    }

    #[test]
    fn native_managed_sessions_hide_single_delete_action() {
        for agent in [
            Agent::Grok,
            Agent::Kimi,
            Agent::Qwen,
            Agent::PrimeAgent,
            Agent::Gemini,
        ] {
            assert!(!available_actions(&session(agent, "id", 1)).contains(&Action::Delete));
        }
        assert!(available_actions(&session(Agent::Codex, "id", 1)).contains(&Action::Delete));
    }

    #[test]
    fn cwd_independent_session_does_not_offer_cd() {
        let mut hermes = session(Agent::Hermes, "id", 1);
        hermes.project_path.clear();
        assert!(!available_actions(&hermes).contains(&Action::Cd));
        assert!(available_actions(&hermes).contains(&Action::Resume));
    }

    #[test]
    fn cached_option_like_identity_cannot_become_an_active_session() {
        let mut app = app_with(
            vec![session(
                Agent::Codex,
                "--dangerously-bypass-approvals-and-sandbox",
                1,
            )],
            crate::settings::Settings::default(),
        );
        assert!(!app.capture_active_session());
        assert!(app.action_session().is_none());
    }

    #[test]
    fn selection_count_sums_across_agents() {
        let mut app = app_with(
            vec![
                session(Agent::ClaudeCode, "a", 30),
                session(Agent::ClaudeCode, "b", 20),
                session(Agent::Codex, "c", 10),
            ],
            crate::settings::Settings::default(),
        );

        app.toggle_checked(Agent::ClaudeCode, "a");
        app.toggle_checked(Agent::ClaudeCode, "b");
        app.toggle_checked(Agent::Codex, "c");

        assert_eq!(app.selection_count(), 3);
    }

    /// `max_sessions` is a presentation limit. It must not truncate the source
    /// vector that is later persisted to cache.
    #[test]
    fn max_sessions_limits_visible_rows_without_dropping_cache_rows() {
        let settings = crate::settings::Settings {
            max_sessions: Some(2),
            ..crate::settings::Settings::default()
        };
        let mut app = app_with(vec![session(Agent::Codex, "codex-new", 100)], settings);

        app.merge_agent_sessions(
            Agent::ClaudeCode,
            vec![
                session(Agent::ClaudeCode, "claude-1", 90),
                session(Agent::ClaudeCode, "claude-2", 80),
                session(Agent::ClaudeCode, "claude-3", 70),
            ],
        );
        app.apply_sort();

        assert_eq!(app.sessions.len(), 4);
        assert_eq!(app.filtered_indices.len(), 2);
        assert_eq!(app.agent_counts.get(&Agent::ClaudeCode), Some(&3));
        assert_eq!(app.agent_counts.get(&Agent::Codex), Some(&1));
        assert_eq!(
            app.agent_counts.values().sum::<usize>(),
            app.sessions.len(),
            "counts must always sum to the list they label"
        );
    }

    #[test]
    fn recount_drops_agents_with_no_remaining_sessions() {
        let mut app = app_with(
            vec![
                session(Agent::ClaudeCode, "a", 30),
                session(Agent::Codex, "b", 20),
            ],
            crate::settings::Settings::default(),
        );
        app.sessions.retain(|s| s.agent != Agent::Codex);
        app.recount_agents();

        // `agents_with_sessions` filters on presence, so a zero-count entry
        // would leave Codex selectable in the Tab filter cycle.
        assert_eq!(app.agent_counts.get(&Agent::Codex), None);
        assert_eq!(app.agents_with_sessions(), vec![Agent::ClaudeCode]);
    }
}
