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

mod inspect;
pub(crate) mod palette;
mod presentation;
use palette::Palette;

/// Width of the agent-name column, shared by the row builder and the layout
/// arithmetic that reserves space for it.
const AGENT_COL_WIDTH: usize = 14;

const BROWSE_FIRST_SESSION_ROW: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NoticeKind {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Debug, Clone)]
struct Notice {
    message: String,
    kind: NoticeKind,
}

impl Notice {
    fn new(kind: NoticeKind, message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind,
        }
    }

    fn color(&self, palette: Palette) -> slt::Color {
        match self.kind {
            NoticeKind::Info => palette.secondary,
            NoticeKind::Success => palette.success,
            NoticeKind::Warning => palette.warning,
            NoticeKind::Error => palette.danger,
        }
    }
}

impl std::fmt::Display for Notice {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let marker = match self.kind {
            NoticeKind::Info => "i",
            NoticeKind::Success => "+",
            NoticeKind::Warning | NoticeKind::Error => "!",
        };
        write!(formatter, "{marker} {}", self.message)
    }
}

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
    pub help_scroll: usize,
    pub help_settings: bool,
    pub help_return_mode: Mode,
    pub preview_scroll: usize,
    notice: Option<Notice>,
    palette: Palette,
    system_theme: Option<slt::Theme>,
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
            help_scroll: 0,
            help_settings: false,
            help_return_mode: Mode::Browse,
            preview_scroll: 0,
            notice: None,
            palette: Palette::dark(),
            system_theme: None,
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

    pub fn save_settings(&mut self) {
        let mut settings = self.settings.clone();
        settings.summary_search_count = self.summary_search_count;
        settings.search_scope = if self.include_summaries {
            "all".to_string()
        } else {
            "name_path".to_string()
        };
        settings.pinned_sessions = self.pinned_sessions.clone();
        settings.show_recap = self.show_recap;
        self.notice = Some(match settings.save_editable() {
            Ok(()) => Notice::new(NoticeKind::Success, "Settings saved"),
            Err(error) => Notice::new(
                NoticeKind::Error,
                format!("Settings not saved: {}", error.kind()),
            ),
        });
    }

    fn restart_search(&mut self) {
        self.selected = 0;
        self.scroll_offset = 0;
        self.preview_scroll = 0;
        self.update_filter();
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
        self.restart_search();
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
        let depth = slt::ColorDepth::detect();
        slt::run_with(
            slt::RunConfig::default()
                .title("agf")
                .mouse(true)
                .color_depth(depth)
                .theme(terminal_theme(std::env::var("COLORFGBG").ok().as_deref())),
            |ui: &mut slt::Context| {
                app.ingest_scan_results();
                if depth == slt::ColorDepth::Basic {
                    ui.provide(depth, |ui| render_frame(ui, app, &mut result));
                } else {
                    render_frame(ui, app, &mut result);
                }
            },
        )?;
        Ok(result)
    }
}

type StyledChunk = (String, slt::Style);

pub(crate) fn terminal_theme(colorfgbg: Option<&str>) -> slt::Theme {
    let light = colorfgbg
        .and_then(|value| value.rsplit(';').next())
        .and_then(|background| background.trim().parse::<u8>().ok())
        .is_some_and(|background| slt::Color::Indexed(background).luminance_f64() > 0.5);
    if light {
        slt::Theme::light()
    } else {
        slt::Theme::dark()
    }
}

fn render_frame(ui: &mut slt::Context, app: &mut App, result: &mut Option<String>) {
    let system_theme = app.system_theme.get_or_insert_with(|| *ui.theme());
    let theme = match app.settings.appearance {
        crate::settings::Appearance::Auto => *system_theme,
        crate::settings::Appearance::Dark => slt::Theme::dark(),
        crate::settings::Appearance::Light => slt::Theme::light(),
    };
    ui.set_dark_mode(theme.is_dark);
    ui.set_theme(theme);
    app.palette = Palette::from_ui(ui);
    let palette = app.palette;
    let mut theme = *ui.theme();
    theme.bg = palette.background;
    theme.text = palette.text;
    theme.text_dim = palette.muted;
    theme.primary = palette.marker(true);
    theme.accent = palette.accent;
    theme.selected_bg = palette.selection_bg;
    theme.selected_fg = palette.selection_text;
    ui.set_theme(theme);
    let (width, height) = (ui.width(), ui.height());
    let _ = ui
        .container()
        .w(width)
        .h(height)
        .text_color(palette.text)
        .bg(palette.background)
        .col(|ui| {
            render_frame_content(ui, app, result);
        });
}

fn render_frame_content(ui: &mut slt::Context, app: &mut App, result: &mut Option<String>) {
    let palette = app.palette;
    if ui.width() < 20 || ui.height() < 8 {
        if ui.consume_key_code(slt::KeyCode::Esc) {
            ui.quit();
        }
        let width = ui.width() as usize;
        let height = ui.height();
        let _ = ui.container().h(height).col(|ui| {
            let _ = ui.container().grow(1).col(|ui| {
                ui.text(text::truncate("Resize terminal", width))
                    .fg(palette.warning);
            });
            if height > 1 {
                render_footer(ui, &[("Esc", "Quit")]);
            }
        });
        return;
    }
    if app.mode != Mode::Help && ui.consume_key_code(slt::KeyCode::F(1)) {
        app.help_return_mode = app.mode;
        app.help_settings = false;
        app.help_scroll = 0;
        app.mode = Mode::Help;
        return;
    }
    app.viewport_height = list_viewport_height(ui.height() as usize, app.mode);
    app.adjust_scroll();
    match app.mode {
        Mode::Browse => ui_browse(ui, app),
        Mode::GroupedBrowse => ui_grouped_browse(ui, app),
        Mode::ActionSelect => ui_action_select(ui, app, result),
        Mode::AgentSelect => ui_agent_select(ui, app, result),
        Mode::PermissionSelect => ui_permission_select(ui, app, result),
        Mode::ResumeSelect => ui_resume_select(ui, app, result),
        Mode::DeleteConfirm => ui_delete_confirm(ui, app),
        Mode::BulkDelete => ui_bulk_delete(ui, app),
        Mode::Preview => ui_preview(ui, app, result),
        Mode::Help => ui_help(ui, app),
    }
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

fn consume_control(ui: &mut slt::Context, character: char) -> bool {
    let indices: Vec<_> = ui
        .key_presses_when(true)
        .filter(|(_, key)| {
            key.code == slt::KeyCode::Char(character)
                && key.modifiers.contains(slt::KeyModifiers::CONTROL)
        })
        .map(|(index, _)| index)
        .collect();
    for &index in &indices {
        ui.consume_event(index);
    }
    !indices.is_empty()
}

fn ui_browse(ui: &mut slt::Context, app: &mut App) {
    let palette = app.palette;
    // Browse is the first input consumer. Preserve text before an action key,
    // but never apply trailing text after the user has left the search field.
    let event_count = ui.raw_events().count();
    if ui.consume_key_code(slt::KeyCode::Esc) {
        ui.quit();
        return;
    }
    let action_index = ui.key_presses_when(true).find_map(|(index, key)| {
        (key.code == slt::KeyCode::Enter
            || (key.modifiers.contains(slt::KeyModifiers::CONTROL)
                && matches!(key.code, slt::KeyCode::Char('l' | 'g' | 'd'))))
        .then_some(index)
    });
    if let Some(index) = action_index {
        for after in index + 1..event_count {
            ui.consume_event(after);
        }
    }
    let enter = ui.consume_key_code(slt::KeyCode::Enter);
    let up = ui.consume_key_code(slt::KeyCode::Up);
    let down = ui.consume_key_code(slt::KeyCode::Down);
    let tab = ui.consume_key_code(slt::KeyCode::Tab);
    let backtab = ui.consume_key_code(slt::KeyCode::BackTab);
    let scope = ui.consume_key_code(slt::KeyCode::F(2));
    let summary_prev = ui.consume_key_code(slt::KeyCode::F(3));
    let summary_next = ui.consume_key_code(slt::KeyCode::F(4));
    let ctrl_p = consume_control(ui, 'p');
    let ctrl_k = consume_control(ui, 'k');
    let ctrl_n = consume_control(ui, 'n');
    let ctrl_j = consume_control(ui, 'j');
    let sort = consume_control(ui, 's');
    let bulk = consume_control(ui, 'd');
    let clear = consume_control(ui, 'u');
    let details = consume_control(ui, 'l');
    let grouped = consume_control(ui, 'g');
    let wheel_up = ui.scroll_up();
    let wheel_down = ui.scroll_down();
    let mouse_target = ui
        .mouse_down()
        .filter(|(x, _)| *x < ui.width())
        .and_then(|(_, y)| {
            browse_click_index(
                y as usize,
                app.scroll_offset,
                app.filtered_indices.len(),
                app.viewport_height,
            )
        })
        .and_then(|index| app.filtered_indices.get(index))
        .map(|&index| app.sessions[index].identity());

    if tab {
        app.cycle_agent_filter(true);
    }
    if backtab {
        app.cycle_agent_filter(false);
    }
    if scope {
        app.include_summaries = !app.include_summaries;
        app.restart_search();
        app.save_settings();
    }
    if clear {
        app.search_textarea.lines = vec![String::new()];
        app.search_textarea.cursor_row = 0;
        app.search_textarea.cursor_col = 0;
        app.query.clear();
        app.restart_search();
    }

    let width = ui.width() as usize;
    let height = ui.height();
    let _ = ui.container().h(height).col(|ui| {
        ui.text("");
        let _ = ui.container().pl(2).pr(1).h(1).row(|ui| {
            let (name, count) = match app.agent_filter {
                Some(agent) => (
                    agent.to_string(),
                    app.agent_counts.get(&agent).copied().unwrap_or(0),
                ),
                None => ("All".to_string(), app.sessions.len()),
            };
            let badge = text::truncate(
                &format!("{name} ({count})"),
                width.saturating_sub(3).min(width / 2),
            );
            let _ = ui.container().grow(1).h(1).col(|ui| {
                let _ = ui.textarea(&mut app.search_textarea, 1);
            });
            let (label, count) = badge.split_at_checked(name.len()).unwrap_or((&badge, ""));
            ui.styled(
                label,
                slt::Style::new()
                    .fg(app
                        .agent_filter
                        .map(|agent| palette.agent(agent))
                        .unwrap_or(palette.secondary))
                    .bg(palette.background),
            );
            ui.styled(
                count,
                slt::Style::new().fg(palette.muted).bg(palette.background),
            );
        });
        // Synchronize before selecting a result: Paste + Enter uses the new query.
        if app.search_textarea.lines.len() > 1 {
            let merged = app.search_textarea.lines.join("");
            app.search_textarea.cursor_row = 0;
            app.search_textarea.cursor_col = merged.graphemes(true).count();
            app.search_textarea.lines = vec![merged];
        }
        let query = app
            .search_textarea
            .lines
            .first()
            .cloned()
            .unwrap_or_default();
        if query != app.query {
            app.query = query;
            app.notice = None;
            app.restart_search();
        }
        if (up || ctrl_p || ctrl_k || wheel_up) && app.selected > 0 {
            app.selected -= 1;
            app.adjust_scroll();
        }
        if (down || ctrl_n || ctrl_j || wheel_down) && app.selected + 1 < app.filtered_indices.len()
        {
            app.selected += 1;
            app.adjust_scroll();
        }
        if sort {
            app.sort_mode = app.sort_mode.next();
            app.apply_sort();
            app.notice = None;
        }
        if summary_prev {
            app.cycle_summary(false);
        }
        if summary_next {
            app.cycle_summary(true);
        }
        if let Some(identity) = mouse_target
            && let Some(position) = app.filtered_position_for_identity(&identity)
        {
            app.selected = position;
            app.adjust_scroll();
            app.action_index = 0;
            if app.capture_active_session() {
                app.mode = Mode::ActionSelect;
            }
        } else if enter && app.capture_active_session() {
            app.action_index = 0;
            app.mode = Mode::ActionSelect;
        } else if details && app.capture_active_session() {
            app.preview_scroll = 0;
            app.mode = Mode::Preview;
        } else if bulk {
            app.selected_set.clear();
            app.mode = Mode::BulkDelete;
        } else if grouped {
            app.build_groups();
            app.grouped_selected = 0;
            app.grouped_scroll = 0;
            app.mode = Mode::GroupedBrowse;
        }

        let _ = ui.separator_colored(palette.border);
        let _ = ui
            .container()
            .h(app.viewport_height as u32)
            .pr(1)
            .col(|ui| {
                if app.filtered_indices.is_empty() {
                    let (title, detail) = browse_empty_state(app);
                    ui.text(text::truncate(
                        &format!("  {title}"),
                        width.saturating_sub(1),
                    ))
                    .fg(palette.text);
                    if app.viewport_height > 1 {
                        ui.text(text::truncate(
                            &format!("  {detail}"),
                            width.saturating_sub(1),
                        ))
                        .fg(palette.secondary);
                    }
                } else if width < 60 {
                    render_session_list_compact(ui, app);
                } else {
                    render_session_list(ui, app, false);
                }
            });
        let (prefix, message, color) = browse_status_parts(app);
        let urgent = !app.failed_agents.is_empty()
            || app.notice.as_ref().is_some_and(|notice| {
                matches!(notice.kind, NoticeKind::Warning | NoticeKind::Error)
            });
        let prefix = if urgent && text::width(&prefix) + 2 + text::width(&message) > width {
            String::new()
        } else {
            text::truncate(&format!("  {prefix}"), width)
        };
        let message = text::truncate(&message, width.saturating_sub(text::width(&prefix)));
        let _ = ui.container().h(1).row(|ui| {
            ui.styled(
                prefix,
                slt::Style::new().fg(palette.muted).bg(palette.background),
            );
            ui.styled(message, slt::Style::new().fg(color).bg(palette.background));
        });
        let _ = ui.separator_colored(palette.border);
        render_footer(
            ui,
            &[
                ("Up/Down", "Move"),
                ("Enter", "Actions"),
                ("Tab", "Agent"),
                ("F2", "Scope"),
                ("F3/F4", "Summary"),
                ("Ctrl+L", "Details"),
                ("Ctrl+S", "Sort"),
                ("Ctrl+G", "Projects"),
                ("Ctrl+D", "Delete"),
                ("F1", "Help"),
                ("Esc", "Quit"),
            ],
        );
    });
}

fn failed_provider_names(app: &App) -> String {
    Agent::all()
        .iter()
        .filter(|agent| app.failed_agents.contains(agent))
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn browse_empty_state(app: &App) -> (String, String) {
    if !app.scanning_agents.is_empty() {
        return (
            "Scanning sessions".into(),
            format!("{} providers pending", app.scanning_agents.len()),
        );
    }
    if !app.failed_agents.is_empty() {
        return (
            "Sessions unavailable".into(),
            format!("Refresh failed: {}", failed_provider_names(app)),
        );
    }
    if app.sessions.is_empty() {
        return (
            "No saved sessions".into(),
            "No local session data found".into(),
        );
    }
    (
        "No matches".into(),
        "Change the query, provider, or search scope".into(),
    )
}

fn browse_status_parts(app: &App) -> (String, String, slt::Color) {
    let palette = app.palette;
    let count = format!("{}/{}", app.filtered_indices.len(), app.sessions.len());
    let scope = if app.include_summaries {
        "All text"
    } else {
        "Name/path"
    };
    if let Some(notice) = &app.notice
        && matches!(notice.kind, NoticeKind::Warning | NoticeKind::Error)
    {
        let mut message = notice.to_string();
        if !app.failed_agents.is_empty() {
            message.push_str(&format!(
                " | Refresh failed: {}",
                failed_provider_names(app)
            ));
        }
        return (
            format!("{count} | {scope} | "),
            message,
            notice.color(palette),
        );
    }
    if !app.failed_agents.is_empty() {
        let cached = app
            .sessions
            .iter()
            .any(|session| app.failed_agents.contains(&session.agent));
        return (
            format!("{count} | "),
            format!(
                "Refresh failed: {}{}",
                failed_provider_names(app),
                if cached { " | Cached results" } else { "" }
            ),
            palette.warning,
        );
    }
    if !app.scanning_agents.is_empty() {
        return (
            format!("{count} | "),
            format!("Scanning {} providers", app.scanning_agents.len()),
            palette.secondary,
        );
    }
    if let Some(notice) = &app.notice {
        return (
            format!("{count} | {scope} | "),
            notice.to_string(),
            notice.color(palette),
        );
    }
    (
        format!("{count} | {scope} | "),
        format!("Sort: {}", app.sort_mode.label()),
        palette.muted,
    )
}

#[cfg(test)]
fn browse_status(app: &App) -> String {
    let (prefix, message, _) = browse_status_parts(app);
    format!("{prefix}{message}")
}

fn list_viewport_height(height: usize, mode: Mode) -> usize {
    let reserved = match mode {
        Mode::BulkDelete => 5,
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
    let palette = app.palette;
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
                app.preview_scroll = 0;
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

    let width = ui.width() as usize;
    let height = ui.height();
    let context = format!(
        "{} | {}",
        app.agent_filter
            .map(|agent| agent.to_string())
            .unwrap_or_else(|| "All agents".into()),
        if app.query.is_empty() {
            "All sessions".into()
        } else {
            format!("Search: {}", text::sanitize_terminal(&app.query))
        }
    );
    let child_selected = matches!(app.grouped_row_at(app.grouped_selected), Some((_, Some(_))));
    let _ = ui.container().h(height).col(|ui| {
        presentation::header(
            ui,
            "Project View",
            Some(&format!("{} projects", app.groups.len())),
        );
        ui.text(text::truncate(&format!(" {context}"), width))
            .fg(palette.secondary);
        let _ = ui.container().h(app.viewport_height as u32).col(|ui| {
            if app.groups.is_empty() {
                let (title, _) = browse_empty_state(app);
                ui.text(text::truncate(&format!(" {title}"), width))
                    .fg(palette.secondary);
                return;
            }
            let end = (app.grouped_scroll + app.viewport_height).min(total_rows);
            let mut row_index = 0;
            for group in &app.groups {
                let expanded = app.group_expanded.contains(&group.project_path);
                if (app.grouped_scroll..end).contains(&row_index) {
                    let bg = if row_index == app.grouped_selected {
                        palette.selection_bg
                    } else {
                        palette.background
                    };
                    let arrow = if expanded { "\u{25be}" } else { "\u{25b8}" };
                    let marker = if row_index == app.grouped_selected {
                        ">"
                    } else {
                        " "
                    };
                    let mut title = format!(
                        "{marker}{arrow} {} ({})",
                        group.project_name,
                        group.sessions.len()
                    );
                    if width >= 80 {
                        title.push_str(&format!("  {}", group.project_path));
                    }
                    let time = group
                        .sessions
                        .iter()
                        .filter_map(|id| app.session_by_identity(id))
                        .max_by_key(|session| session.timestamp)
                        .map(Session::time_display)
                        .unwrap_or_default();
                    render_edge_row(
                        ui,
                        &title,
                        &time,
                        palette.marker(row_index == app.grouped_selected),
                        bg,
                    );
                }
                row_index += 1;
                if expanded {
                    for (index, identity) in group.sessions.iter().enumerate() {
                        if (app.grouped_scroll..end).contains(&row_index)
                            && let Some(session) = app.session_by_identity(identity)
                        {
                            let tree = if index + 1 == group.sessions.len() {
                                "  └─"
                            } else {
                                "  ├─"
                            };
                            let summary = if app.show_recap {
                                session
                                    .recap
                                    .as_deref()
                                    .or_else(|| session.summaries.first().map(String::as_str))
                            } else {
                                session.summaries.first().map(String::as_str)
                            }
                            .unwrap_or("");
                            let marker = if row_index == app.grouped_selected {
                                ">"
                            } else {
                                " "
                            };
                            let prefix = format!(
                                "{marker}{}{} ",
                                &tree[1..],
                                if app.is_pinned(session) { "*" } else { " " }
                            );
                            render_grouped_session(
                                ui,
                                &prefix,
                                session,
                                summary,
                                row_index == app.grouped_selected,
                            );
                        }
                        row_index += 1;
                    }
                }
            }
        });
        let _ = ui.separator_colored(palette.border);
        render_footer(
            ui,
            &[
                ("Up/Down", "Move"),
                ("Enter", if child_selected { "Actions" } else { "Expand" }),
                ("Ctrl+L", "Details"),
                ("Ctrl+G", "List"),
                ("F1", "Help"),
                ("Esc", "Back"),
            ],
        );
    });
}

fn render_edge_row(
    ui: &mut slt::Context,
    left: &str,
    right: &str,
    color: slt::Color,
    bg: slt::Color,
) {
    let palette = Palette::from_ui(ui);
    let width = ui.width() as usize;
    let right = if width >= 40 {
        text::truncate(right, 14)
    } else {
        String::new()
    };
    let reserved = text::width(&right) + usize::from(!right.is_empty());
    let left = text::fit(left, width.saturating_sub(reserved));
    let _ = ui.container().h(1).row(|ui| {
        ui.styled(left, slt::Style::new().fg(color).bg(bg));
        if !right.is_empty() {
            ui.styled(
                format!(" {right}"),
                slt::Style::new()
                    .fg(palette.row_muted(bg == palette.selection_bg))
                    .bg(bg),
            );
        }
    });
}

fn render_grouped_session(
    ui: &mut slt::Context,
    prefix: &str,
    session: &Session,
    summary: &str,
    selected: bool,
) {
    let palette = Palette::from_ui(ui);
    let width = ui.width() as usize;
    let bg = if selected {
        palette.selection_bg
    } else {
        palette.background
    };
    let time = if width >= 40 {
        format!(" {}", text::truncate(&session.time_display(), 14))
    } else {
        String::new()
    };
    let left_width = width.saturating_sub(text::width(&time));
    let prefix = text::truncate(prefix, left_width);
    let label = text::truncate(
        session.agent.cli_name(),
        left_width.saturating_sub(text::width(&prefix)),
    );
    let remaining = left_width.saturating_sub(text::width(&prefix) + text::width(&label));
    let summary = text::pad(
        &text::truncate(
            &format!("  {}", text::sanitize_terminal(summary)),
            remaining,
        ),
        remaining,
    );
    let _ = ui.container().h(1).row(|ui| {
        ui.styled(
            prefix,
            slt::Style::new().fg(palette.marker(selected)).bg(bg),
        );
        ui.styled(
            label,
            slt::Style::new().fg(palette.agent(session.agent)).bg(bg),
        );
        ui.styled(
            summary,
            slt::Style::new().fg(palette.row_text(selected)).bg(bg),
        );
        ui.styled(
            time,
            slt::Style::new().fg(palette.row_muted(selected)).bg(bg),
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

fn menu_range(count: usize, selected: usize, height: u32) -> std::ops::Range<usize> {
    // Four header rows and three footer rows surround each picker.
    let visible = (height as usize).saturating_sub(7);
    let start = selected
        .saturating_sub(visible.saturating_sub(1))
        .min(count.saturating_sub(visible));
    start..(start + visible).min(count)
}

fn render_menu_row(
    ui: &mut slt::Context,
    index: usize,
    label: &str,
    preview: &str,
    selected: bool,
    color: slt::Color,
) {
    let palette = Palette::from_ui(ui);
    let width = ui.width() as usize;
    let bg = if selected {
        palette.selection_bg
    } else {
        palette.background
    };
    let key = format!("{}{:>2}) ", if selected { ">" } else { " " }, index + 1);
    let key = text::truncate(&key, width);
    let label = text::truncate(
        &text::sanitize_terminal(label),
        width.saturating_sub(text::width(&key)),
    );
    let left = text::width(&key) + text::width(&label);
    let preview = if width.saturating_sub(left) > 4 {
        format!(
            "  {}",
            text::truncate(&text::sanitize_terminal(preview), width - left - 2)
        )
    } else {
        String::new()
    };
    let padding = width.saturating_sub(left + text::width(&preview));
    let _ = ui.container().h(1).row(|ui| {
        ui.styled(key, slt::Style::new().fg(palette.marker(selected)).bg(bg));
        let style = slt::Style::new().fg(color).bg(bg);
        ui.styled(label, if selected { style.bold() } else { style });
        ui.styled(
            preview,
            slt::Style::new().fg(palette.row_muted(selected)).bg(bg),
        );
        if padding > 0 {
            ui.styled(" ".repeat(padding), slt::Style::new().bg(bg));
        }
    });
}

fn ui_action_select(ui: &mut slt::Context, app: &mut App, result: &mut Option<String>) {
    let palette = app.palette;
    let Some(actions) = app.action_session().map(available_actions) else {
        app.active_session = None;
        app.mode = Mode::Browse;
        return;
    };
    let action_count = actions.len();
    app.action_index = app.action_index.min(action_count.saturating_sub(1));
    let mouse_actions = menu_range(action_count, app.action_index, ui.height());

    if ui.key_code(slt::KeyCode::Esc) {
        app.active_session = None;
        app.mode = Mode::Browse;
        return;
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

    // Keyboard input may scroll this frame; a click still targets the painted rows.
    if let Some((x, y)) = ui.mouse_down()
        && x < ui.width()
        && app.mode == Mode::ActionSelect
    {
        let y = y as usize;
        if y >= 4 && y < 4 + mouse_actions.len() {
            let clicked = mouse_actions.start + y - 4;
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

    let visible_actions = menu_range(action_count, app.action_index, ui.height());
    let Some(session) = app.action_session() else {
        app.active_session = None;
        app.mode = Mode::Browse;
        return;
    };

    let height = ui.height();
    let _ = ui.container().h(height).col(|ui| {
        presentation::header(
            ui,
            &format!("{} | {}", session.agent, session.project_name),
            Some(&session.display_path()),
        );
        ui.text(format!("  {}/{}", app.action_index + 1, action_count))
            .fg(palette.muted);
        let _ = ui.container().h(height.saturating_sub(7)).col(|ui| {
            for (i, act) in actions
                .iter()
                .enumerate()
                .skip(visible_actions.start)
                .take(visible_actions.len())
            {
                let label = if *act == Action::Pin {
                    if app.is_pinned(session) {
                        "Unpin Session".to_string()
                    } else {
                        "Pin Session".to_string()
                    }
                } else {
                    act.to_string()
                };
                render_menu_row(
                    ui,
                    i,
                    &label,
                    &action::action_preview(session, *act),
                    app.action_index == i,
                    if *act == Action::Delete {
                        palette.danger
                    } else {
                        palette.text
                    },
                );
            }
        });
        ui.text("");
        let _ = ui.separator_colored(palette.border);
        render_footer(
            ui,
            &[
                ("Up/Down", "Move"),
                ("1-9", "Choose"),
                ("Enter", "Select"),
                ("F1", "Help"),
                ("Esc", "Back"),
            ],
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
    let palette = app.palette;
    let option_count = app.new_session_options.len();

    if ui.key_code(slt::KeyCode::Esc) {
        app.mode = Mode::ActionSelect;
        return;
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

    let height = ui.height();
    let _ = ui.container().h(height).col(|ui| {
        presentation::header(ui, "New session in", Some(&session.display_path()));
        ui.text(format!(
            "  {}/{} providers",
            usize::from(option_count > 0) + app.agent_index,
            option_count
        ))
        .fg(palette.muted);
        let visible = menu_range(option_count, app.agent_index, height);
        let _ = ui.container().h(height.saturating_sub(7)).col(|ui| {
            if option_count == 0 {
                ui.text("  No detected agent CLIs").fg(palette.secondary);
            }
            for (i, opt) in app
                .new_session_options
                .iter()
                .enumerate()
                .skip(visible.start)
                .take(visible.len())
            {
                let shell = crate::shell::CommandShell::from_env();
                let command = opt.agent.new_session_command(&shell);
                let preview = action::preview_cd_and(&shell, session, &command);
                render_menu_row(
                    ui,
                    i,
                    &opt.label,
                    &preview,
                    i == app.agent_index,
                    palette.agent(opt.agent),
                );
            }
        });
        ui.text("");
        let _ = ui.separator_colored(palette.border);
        render_footer(
            ui,
            &[
                ("Up/Down", "Move"),
                ("1-9", "Choose"),
                ("Enter", "Mode"),
                ("F1", "Help"),
                ("Esc", "Back"),
            ],
        );
    });
}

fn permission_options_for(agent: Agent) -> Vec<(&'static str, &'static str)> {
    agent.resume_mode_options().to_vec()
}

fn dispatch_agent_option(_ui: &mut slt::Context, app: &mut App, _result: &mut Option<String>) {
    if let Some(opt) = app.new_session_options.get(app.agent_index) {
        app.mode_options = permission_options_for(opt.agent);
        app.mode_index = 0;
        app.mode = Mode::PermissionSelect;
    }
}

fn ui_permission_select(ui: &mut slt::Context, app: &mut App, result: &mut Option<String>) {
    let palette = app.palette;
    let option_count = app.mode_options.len();

    if ui.key_code(slt::KeyCode::Esc) {
        app.mode = Mode::AgentSelect;
        return;
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

    let height = ui.height();
    let title = "Select mode for";
    let agent = agent_label;
    let selected = app.mode_index;
    let options = &app.mode_options;
    let _ = ui.container().h(height).col(|ui| {
        presentation::header(ui, &format!("{title} {agent}"), None);
        ui.text(format!(
            "  {}/{} modes",
            selected + usize::from(!options.is_empty()),
            options.len()
        ))
        .fg(palette.muted);
        let visible = menu_range(options.len(), selected, height);
        let _ = ui.container().h(height.saturating_sub(7)).col(|ui| {
            for (i, (label, flags)) in options
                .iter()
                .enumerate()
                .skip(visible.start)
                .take(visible.len())
            {
                let dangerous = flags.contains("dangerously") || label.contains("yolo");
                render_menu_row(
                    ui,
                    i,
                    label,
                    flags.trim(),
                    i == selected,
                    if dangerous {
                        palette.danger
                    } else {
                        palette.text
                    },
                );
            }
        });
        ui.text("");
        let _ = ui.separator_colored(palette.border);
        render_footer(
            ui,
            &[
                ("Up/Down", "Move"),
                ("1-9", "Start"),
                ("Enter", "Start"),
                ("F1", "Help"),
                ("Esc", "Back"),
            ],
        );
    });
}

fn dispatch_mode_option(ui: &mut slt::Context, app: &mut App, result: &mut Option<String>) {
    if let Some((_, flags)) = app.mode_options.get(app.mode_index)
        && let Some(opt) = app.new_session_options.get(app.agent_index)
        && let Some(session) = app.action_session().cloned()
    {
        let cmd = action::new_session_with_flags(
            &session,
            opt.agent,
            &format!("{}{flags}", opt.command_suffix),
        );
        result.replace(cmd);
        ui.quit();
    }
}

fn ui_resume_select(ui: &mut slt::Context, app: &mut App, result: &mut Option<String>) {
    let palette = app.palette;
    let option_count = app.resume_mode_options.len();

    if ui.key_code(slt::KeyCode::Esc) {
        app.mode = Mode::ActionSelect;
        return;
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

    let height = ui.height();
    let title = "Resume mode for";
    let agent = session.agent;
    let selected = app.resume_mode_index;
    let options = &app.resume_mode_options;
    let _ = ui.container().h(height).col(|ui| {
        presentation::header(ui, &format!("{title} {agent}"), None);
        ui.text(format!(
            "  {}/{} modes",
            selected + usize::from(!options.is_empty()),
            options.len()
        ))
        .fg(palette.muted);
        let visible = menu_range(options.len(), selected, height);
        let _ = ui.container().h(height.saturating_sub(7)).col(|ui| {
            for (i, (label, flags)) in options
                .iter()
                .enumerate()
                .skip(visible.start)
                .take(visible.len())
            {
                let dangerous = flags.contains("dangerously") || label.contains("yolo");
                render_menu_row(
                    ui,
                    i,
                    label,
                    flags.trim(),
                    i == selected,
                    if dangerous {
                        palette.danger
                    } else {
                        palette.text
                    },
                );
            }
        });
        ui.text("");
        let _ = ui.separator_colored(palette.border);
        render_footer(
            ui,
            &[
                ("Up/Down", "Move"),
                ("1-9", "Resume"),
                ("Enter", "Resume"),
                ("F1", "Help"),
                ("Esc", "Back"),
            ],
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
    let palette = app.palette;
    if ui.key_code(slt::KeyCode::Esc) {
        app.selected_set.clear();
        app.mode = Mode::Browse;
        return;
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

    let height = ui.height();
    let _ = ui.container().h(height).col(|ui| {
        let _ = ui
            .bordered(slt::Border::Rounded)
            .border_fg(palette.danger)
            .min_h(3)
            .max_h(3)
            .col(|ui| {
                ui.line(|ui| {
                    ui.text(" DELETE MODE").fg(palette.danger).bold();
                    if !app.selected_set.is_empty() {
                        ui.text(format!("  ({} selected)", app.selection_count()))
                            .fg(palette.danger);
                    }
                });
            });

        let _ = ui.container().h(app.viewport_height as u32).col(|ui| {
            render_session_list(ui, app, true);
        });

        let selected = format!("{} selected", app.selection_count());
        let status = match app.selected_session() {
            Some(session) if !session.agent.supports_delete() => {
                format!("{selected} | Use {} to delete", session.agent.cli_name())
            }
            _ => selected,
        };
        ui.text(text::truncate(&format!(" {status}"), ui.width() as usize))
            .fg(palette.danger);
        render_footer(
            ui,
            &[
                ("Up/Down", "Move"),
                ("Space", "Select"),
                ("Enter", "Delete"),
                ("F1", "Help"),
                ("Esc", "Cancel"),
            ],
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
        return;
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
                let requested: usize = targets.values().map(HashSet::len).sum();
                let removed: usize = deleted.values().map(HashSet::len).sum();
                app.notice = Some(if requested == 0 {
                    Notice::new(NoticeKind::Info, "No remaining sessions to delete")
                } else if removed == requested {
                    Notice::new(NoticeKind::Success, format!("Deleted {removed} sessions"))
                } else {
                    Notice::new(
                        NoticeKind::Warning,
                        format!(
                            "Deleted {removed}/{requested}; {} not removed",
                            requested - removed
                        ),
                    )
                });
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
                let deletion = crate::delete::delete_session(&app.sessions[idx]);
                app.notice = Some(match &deletion {
                    Ok(()) => Notice::new(NoticeKind::Success, "Session deleted"),
                    Err(error) => Notice::new(
                        NoticeKind::Error,
                        format!("Session not deleted: {}", error.kind()),
                    ),
                });
                if deletion.is_ok() {
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
        .and_then(|id| app.session_by_identity(id))
    else {
        return;
    };
    let suffix = format!(
        " [{}]",
        text::truncate(&text::sanitize_terminal(&session.session_id), 8)
    );
    let target = text::truncate(
        &text::sanitize_terminal(&session.project_name),
        (ui.width() as usize).saturating_sub(2 + text::width(&suffix)),
    );
    let details = vec![
        format!("{target}{suffix}"),
        format!("{} | {}", session.agent, session.display_path()),
        session.session_id.clone(),
    ];
    render_delete_dialog(ui, app, "Delete session?", &details, false);
}

fn render_bulk_delete_confirm(ui: &mut slt::Context, app: &App) {
    let mut names: Vec<_> = app
        .sessions
        .iter()
        .filter(|session| app.is_checked(session))
        .map(|session| format!("{} | {}", session.project_name, session.agent))
        .collect();
    names.sort();
    render_delete_dialog(
        ui,
        app,
        &format!("Delete {} sessions?", names.len()),
        &names,
        true,
    );
}

fn render_delete_dialog(
    ui: &mut slt::Context,
    app: &App,
    title: &str,
    details: &[String],
    bulk: bool,
) {
    let palette = app.palette;
    let height = ui.height();
    let width = ui.width() as usize;
    let body_height = height.saturating_sub(7) as usize;
    let _ = ui.container().h(height).col(|ui| {
        presentation::header(ui, title, None);
        let _ = ui.container().h(body_height as u32).col(|ui| {
            for (index, detail) in details.iter().take(body_height).enumerate() {
                let value = if index > 0 && index + 1 == body_height && details.len() > body_height
                {
                    format!("... {} more", details.len() - index)
                } else {
                    detail.clone()
                };
                ui.text(text::truncate(&format!("  {value}"), width))
                    .fg(palette.secondary);
            }
        });
        let delete = if bulk {
            "Yes, delete all"
        } else {
            "Yes, delete"
        };
        for (index, label) in [delete, "Cancel"].iter().enumerate() {
            let selected = app.delete_index == index;
            let bg = if selected {
                palette.selection_bg
            } else {
                palette.background
            };
            let prefix = text::truncate(if selected { "> " } else { "  " }, width);
            let value = text::fit(label, width.saturating_sub(text::width(&prefix)));
            let _ = ui.container().h(1).row(|ui| {
                ui.styled(
                    prefix,
                    slt::Style::new().fg(palette.marker(selected)).bg(bg).bold(),
                );
                ui.styled(
                    value,
                    slt::Style::new()
                        .fg(if index == 0 {
                            palette.danger
                        } else {
                            palette.row_text(selected)
                        })
                        .bg(bg)
                        .bold(),
                );
            });
        }
        let _ = ui.separator_colored(palette.border);
        render_footer(
            ui,
            &[
                ("Up/Down", "Choose"),
                ("Enter", "Confirm"),
                ("F1", "Help"),
                ("Esc", "Cancel"),
            ],
        );
    });
}

fn ui_preview(ui: &mut slt::Context, app: &mut App, result: &mut Option<String>) {
    inspect::preview(ui, app, result);
}

fn ui_help(ui: &mut slt::Context, app: &mut App) {
    inspect::help(ui, app);
}

pub(crate) fn render_footer(ui: &mut slt::Context, hints: &[(&str, &str)]) {
    presentation::footer(ui, hints);
}

fn render_session_list(ui: &mut slt::Context, app: &App, bulk_mode: bool) {
    let palette = app.palette;
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
            palette.selection_bg
        } else {
            palette.background
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
                slt::Style::new().fg(palette.row_muted(is_selected)).bg(bg)
            } else if is_checked {
                slt::Style::new().fg(palette.danger).bold().bg(bg)
            } else {
                slt::Style::new().fg(palette.text).bg(bg)
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
                palette,
                bg,
                5,
                total_width,
                right_margin,
                None,
                summary_text,
                name_col_width,
            );

            let _ = ui.row(|ui| {
                ui.styled(
                    &indicator[..1],
                    slt::Style::new().fg(palette.marker(is_selected)).bg(bg),
                );
                ui.styled(&indicator[1..], indicator_style);
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
                palette,
                bg,
                2,
                total_width,
                right_margin,
                match_positions,
                summary_text,
                name_col_width,
            );

            let _ = ui.row(|ui| {
                let ind_style = slt::Style::new().fg(palette.marker(is_selected)).bg(bg);
                let ind_style = if is_pinned || is_selected {
                    ind_style.bold()
                } else {
                    ind_style
                };
                ui.styled(indicator.to_string(), ind_style);
                render_chunks(ui, chunks);
            });
        }
    }
}

fn render_session_list_compact(ui: &mut slt::Context, app: &App) {
    let palette = app.palette;
    let width = ui.width() as usize;
    let end = (app.scroll_offset + app.viewport_height).min(app.filtered_indices.len());
    let agent_width = if width >= 48 {
        12
    } else if width >= 32 {
        9
    } else {
        5
    };
    let time_width = if width >= 48 { 14 } else { 4 };
    let name_width = width.saturating_sub(2 + agent_width + time_width + 3);
    for vi in app.scroll_offset..end {
        let session = &app.sessions[app.filtered_indices[vi]];
        let selected = vi == app.selected;
        let bg = if selected {
            palette.selection_bg
        } else {
            palette.background
        };
        let indicator = match (vi == app.selected, app.is_pinned(session)) {
            (true, true) => ">*",
            (true, false) => "> ",
            (false, true) => " *",
            (false, false) => "  ",
        };
        let time = session.time_display();
        let time = if width >= 48 {
            time.as_str()
        } else {
            time.split(" · ").next().unwrap_or(&time)
        };
        let _ = ui.container().h(1).row(|ui| {
            ui.styled(
                indicator,
                slt::Style::new().fg(palette.marker(selected)).bg(bg).bold(),
            );
            ui.styled(
                text::fit(session.agent.cli_name(), agent_width),
                slt::Style::new().fg(palette.agent(session.agent)).bg(bg),
            );
            ui.styled(" ", slt::Style::new().bg(bg));
            let name = text::fit(&session.project_name, name_width);
            let positions = app
                .match_positions
                .get(vi)
                .map(Vec::as_slice)
                .unwrap_or_default();
            render_chunks(ui, highlight_text(&name, positions, 0, bg, palette));
            ui.styled(" ", slt::Style::new().bg(bg));
            let time = text::truncate(time, time_width);
            ui.styled(
                format!(
                    "{}{}",
                    " ".repeat(time_width.saturating_sub(text::width(&time))),
                    time
                ),
                slt::Style::new().fg(palette.row_muted(selected)).bg(bg),
            );
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn build_session_row(
    session: &Session,
    palette: Palette,
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
        slt::Style::new().fg(palette.agent(session.agent)).bg(bg),
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
        chunks.extend(highlight_text(&proj_display, positions, 0, bg, palette));
    } else {
        chunks.push((
            proj_display,
            slt::Style::new()
                .fg(palette.row_text(bg == palette.selection_bg))
                .bg(bg),
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
                chunks.push((
                    "recap: ".to_string(),
                    slt::Style::new().fg(palette.secondary).bg(bg),
                ));
                chunks.push((
                    rest.to_string(),
                    slt::Style::new().fg(palette.secondary).bg(bg),
                ));
            } else {
                chunks.push((truncated, slt::Style::new().fg(palette.secondary).bg(bg)));
            }
        }
    }

    let left_width = indicator_width + chunk_width(&chunks);
    let padding = total_width.saturating_sub(left_width + git_info_width + right_display_width);
    if padding > 0 {
        chunks.push((" ".repeat(padding), slt::Style::new().bg(bg)));
    }

    if let Some(git_str) = git_info_str {
        chunks.push((git_str, slt::Style::new().fg(palette.secondary).bg(bg)));
    }
    chunks.push((
        format!("  {time_str}"),
        slt::Style::new()
            .fg(palette.row_muted(bg == palette.selection_bg))
            .bg(bg),
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
    palette: Palette,
) -> Vec<StyledChunk> {
    // `fuzzy::filter` hands back sorted, deduplicated positions, so probe them
    // with a binary search rather than the linear `contains` this used to do
    // once per character, per row, per frame.
    let is_match =
        |i: usize| u32::try_from(i + offset).is_ok_and(|pos| positions.binary_search(&pos).is_ok());

    let mut chunks: Vec<StyledChunk> = Vec::new();
    let mut offset_in_chars = 0;
    for grapheme in source.graphemes(true) {
        let end = offset_in_chars + grapheme.chars().count();
        let matched = (offset_in_chars..end).any(&is_match);
        let style = if matched {
            slt::Style::new()
                .fg(palette.accent)
                .bold()
                .underline()
                .bg(bg)
        } else {
            let style = slt::Style::new()
                .fg(palette.row_text(bg == palette.selection_bg))
                .bg(bg);
            if bg == palette.selection_bg {
                style.bold()
            } else {
                style
            }
        };
        if let Some((last, previous)) = chunks.last_mut()
            && *previous == style
        {
            last.push_str(grapheme);
        } else {
            chunks.push((grapheme.to_string(), style));
        }
        offset_in_chars = end;
    }

    chunks
}

#[cfg(test)]
mod slt_upgrade_tests {
    use super::*;
    use slt::{EventBuilder, KeyCode, KeyModifiers, TestBackend};

    fn app() -> App {
        let sessions = Agent::all()
            .iter()
            .enumerate()
            .map(|(i, &agent)| Session {
                agent,
                session_id: format!("fixture-{i}"),
                project_name: format!("needle-project-{i:02}"),
                project_path: format!("/synthetic/project-{i:02}"),
                summaries: vec!["needle".into()],
                timestamp: i as i64,
                git_branch: None,
                worktree: None,
                recap: None,
                interactive: true,
            })
            .collect();
        let mut app = App::new(
            sessions,
            None,
            5,
            false,
            None,
            Vec::new(),
            crate::settings::Settings::default(),
            None,
            HashSet::new(),
        );
        app.new_session_options = Agent::all()
            .iter()
            .map(|&agent| NewSessionOption {
                agent,
                label: agent.to_string(),
                command_suffix: "",
            })
            .collect();
        app
    }

    fn render(ui: &mut slt::Context, app: &mut App, result: &mut Option<String>) {
        render_frame(ui, app, result);
    }

    fn step(backend: &mut TestBackend, app: &mut App, events: EventBuilder) -> Option<String> {
        let mut result = None;
        let count = usize::from(app.mode == Mode::Browse);
        backend.render_with_events(events.build(), 0, count, |ui| render(ui, app, &mut result));
        for _ in 0..16 {
            if !backend.has_pending_input() {
                // Mode changes and filtering become visible on the next frame.
                backend.render(|ui| render(ui, app, &mut result));
                return result;
            }
            backend.render(|ui| render(ui, app, &mut result));
        }
        panic!("input queue failed to drain");
    }

    #[test]
    fn all_fifteen_provider_filters_cycle_in_both_directions() {
        let mut app = app();
        assert_eq!(Agent::all().len(), 15);
        let mut backend = TestBackend::new(40, 12);
        step(&mut backend, &mut app, EventBuilder::new());
        for &agent in Agent::all() {
            step(
                &mut backend,
                &mut app,
                EventBuilder::new().key_code(KeyCode::Tab),
            );
            assert_eq!(app.agent_filter, Some(agent));
            assert_eq!(app.filtered_indices.len(), 1);
            assert_eq!(app.selected_session().unwrap().agent, agent);
            backend.assert_contains(&format!("{agent} (1)"));
            backend.assert_contains("1/15");
        }
        step(
            &mut backend,
            &mut app,
            EventBuilder::new().key_code(KeyCode::Tab),
        );
        assert_eq!(app.agent_filter, None);
        for &agent in Agent::all().iter().rev() {
            step(
                &mut backend,
                &mut app,
                EventBuilder::new().key_code(KeyCode::BackTab),
            );
            assert_eq!(app.agent_filter, Some(agent));
            assert_eq!(app.selected_session().unwrap().agent, agent);
        }
        step(
            &mut backend,
            &mut app,
            EventBuilder::new().key_code(KeyCode::BackTab),
        );
        assert_eq!(app.agent_filter, None);
        assert!(app.query.is_empty());
    }

    #[test]
    fn small_terminal_reveals_every_provider_menu_selection_and_wraps() {
        for (width, height) in [(40, 12), (80, 10)] {
            let mut app = app();
            assert!(app.capture_active_session());
            app.mode = Mode::AgentSelect;
            let mut backend = TestBackend::new(width, height);
            for i in 0..15 {
                step(&mut backend, &mut app, EventBuilder::new());
                assert_eq!(app.agent_index, i);
                backend.assert_contains(&format!("{}) {}", i + 1, Agent::all()[i]));
                for key in ["Enter", "F1", "Esc"] {
                    backend.assert_line_contains(height - 1, key);
                }
                step(
                    &mut backend,
                    &mut app,
                    EventBuilder::new().key_code(KeyCode::Tab),
                );
            }
            assert_eq!(app.agent_index, 0);
            step(
                &mut backend,
                &mut app,
                EventBuilder::new().key_code(KeyCode::BackTab),
            );
            assert_eq!(app.agent_index, 14);
            backend.assert_contains("15) Antigravity");
            assert!(
                step(
                    &mut backend,
                    &mut app,
                    EventBuilder::new().key_code(KeyCode::Enter)
                )
                .is_none()
            );
            assert_eq!(app.mode, Mode::PermissionSelect);
            assert_eq!(app.mode_options, Agent::Antigravity.resume_mode_options());
        }
    }

    #[test]
    fn short_action_and_permission_menus_keep_selected_item_visible() {
        let mut app = app();
        app.selected = 14;
        assert!(app.capture_active_session());
        let mut backend = TestBackend::new(40, 9);
        app.mode = Mode::ActionSelect;
        app.action_index = available_actions(app.action_session().unwrap()).len() - 1;
        step(&mut backend, &mut app, EventBuilder::new());
        backend.assert_contains("Pin Session");
        app.mode = Mode::ResumeSelect;
        app.resume_mode_options = Agent::Antigravity.resume_mode_options().to_vec();
        app.resume_mode_index = app.resume_mode_options.len() - 1;
        step(&mut backend, &mut app, EventBuilder::new());
        backend.assert_contains("5) sandbox");
        app.mode = Mode::PermissionSelect;
        app.agent_index = 14;
        app.mode_options = app.resume_mode_options.clone();
        app.mode_index = 4;
        step(&mut backend, &mut app, EventBuilder::new());
        backend.assert_contains("5) sandbox");
    }

    #[test]
    fn tab_then_utf8_paste_drains_pending_input_without_losing_search_focus() {
        let mut app = app();
        let mut backend = TestBackend::new(80, 24);
        step(&mut backend, &mut app, EventBuilder::new());
        step(
            &mut backend,
            &mut app,
            EventBuilder::new()
                .key_code(KeyCode::Tab)
                .key('n')
                .paste("eedle"),
        );
        assert_eq!(app.agent_filter, Some(Agent::ClaudeCode));
        assert_eq!(app.query, "needle");
        assert_eq!(app.filtered_indices.len(), 1);
        step(
            &mut backend,
            &mut app,
            EventBuilder::new().key_with(KeyCode::Char('u'), KeyModifiers::CONTROL),
        );
        step(
            &mut backend,
            &mut app,
            EventBuilder::new().key('한').paste("e\u{301}👩‍💻"),
        );
        assert_eq!(app.query, "한e\u{301}👩‍💻");
        assert_eq!(app.search_textarea.cursor_col, 3);
        step(
            &mut backend,
            &mut app,
            EventBuilder::new().key_code(KeyCode::Backspace),
        );
        assert_eq!(app.query, "한e\u{301}");
        assert_eq!(app.search_textarea.cursor_col, 2);
        assert!(!backend.has_pending_input());
    }

    #[test]
    fn entering_action_menu_does_not_edit_the_now_inactive_search() {
        let mut app = app();
        let mut backend = TestBackend::new(80, 24);
        step(&mut backend, &mut app, EventBuilder::new());
        step(
            &mut backend,
            &mut app,
            EventBuilder::new()
                .key_code(KeyCode::Enter)
                .paste("inactive"),
        );
        assert_eq!(app.mode, Mode::ActionSelect);
        assert_eq!(app.action_session().unwrap().session_id, "fixture-0");
        assert!(app.query.is_empty());
        assert_eq!(app.search_textarea.lines, [""]);
    }

    #[test]
    fn tab_and_batched_enter_never_skip_action_or_resume_confirmation() {
        let mut app = app();
        let mut backend = TestBackend::new(80, 24);
        step(&mut backend, &mut app, EventBuilder::new());
        assert!(
            step(
                &mut backend,
                &mut app,
                EventBuilder::new()
                    .key_code(KeyCode::BackTab)
                    .key_code(KeyCode::Enter)
                    .key_code(KeyCode::Enter)
            )
            .is_none()
        );
        assert_eq!(app.mode, Mode::ActionSelect);
        assert_eq!(app.action_session().unwrap().agent, Agent::Antigravity);
        assert!(
            step(
                &mut backend,
                &mut app,
                EventBuilder::new()
                    .key_code(KeyCode::Enter)
                    .key_code(KeyCode::Enter)
            )
            .is_none()
        );
        assert_eq!(app.mode, Mode::ResumeSelect);
        assert_eq!(app.resume_mode_index, 0);
    }

    #[test]
    fn inactive_modes_preserve_search_and_escape_restores_typing() {
        for mode in [
            Mode::ActionSelect,
            Mode::AgentSelect,
            Mode::PermissionSelect,
            Mode::ResumeSelect,
            Mode::GroupedBrowse,
            Mode::BulkDelete,
            Mode::Preview,
            Mode::Help,
        ] {
            let mut app = app();
            let mut backend = TestBackend::new(80, 24);
            step(&mut backend, &mut app, EventBuilder::new().paste("needle"));
            assert!(app.capture_active_session());
            app.mode_options = Agent::ClaudeCode.resume_mode_options().to_vec();
            app.resume_mode_options = app.mode_options.clone();
            app.build_groups();
            app.mode = mode;
            step(
                &mut backend,
                &mut app,
                EventBuilder::new().paste("inactive").key('x'),
            );
            assert_eq!(app.query, "needle", "{mode:?}");
            assert_eq!(app.search_textarea.lines, ["needle"], "{mode:?}");
            for _ in 0..4 {
                if app.mode == Mode::Browse {
                    break;
                }
                step(
                    &mut backend,
                    &mut app,
                    EventBuilder::new().key_code(KeyCode::Esc),
                );
            }
            assert_eq!(app.mode, Mode::Browse, "{mode:?}");
            step(&mut backend, &mut app, EventBuilder::new().key('x'));
            assert_eq!(app.query, "needlex", "{mode:?}");
        }
    }

    #[test]
    fn mouse_scroll_and_click_resolve_visible_session_identity() {
        let mut app = app();
        let mut backend = TestBackend::new(40, 12);
        step(&mut backend, &mut app, EventBuilder::new());
        for _ in 0..14 {
            step(
                &mut backend,
                &mut app,
                EventBuilder::new().scroll_down(4, 4),
            );
        }
        assert_eq!(app.selected, 14);
        assert!(app.scroll_offset > 0);
        assert!(app.selected < app.scroll_offset + app.viewport_height);
        step(&mut backend, &mut app, EventBuilder::new().scroll_up(4, 4));
        assert_eq!(app.selected, 13);
        let target = app.sessions[app.filtered_indices[app.scroll_offset]].identity();
        step(&mut backend, &mut app, EventBuilder::new().click(4, 3));
        assert_eq!(app.mode, Mode::ActionSelect);
        assert_eq!(app.action_session().unwrap().identity(), target);
        assert!(app.query.is_empty());
    }

    #[test]
    fn action_mouse_uses_the_rows_painted_before_keyboard_navigation() {
        let mut app = app();
        assert!(app.capture_active_session());
        app.mode = Mode::ActionSelect;
        app.action_index = 1;
        let mut backend = TestBackend::new(80, 9);
        step(&mut backend, &mut app, EventBuilder::new());
        backend.assert_contains("1) Resume Session");
        let result = step(
            &mut backend,
            &mut app,
            EventBuilder::new().key_code(KeyCode::Down).click(4, 4),
        );
        assert!(result.is_none());
        assert_eq!(app.mode, Mode::ResumeSelect);
    }

    #[test]
    fn escape_wins_over_enter_in_every_launch_menu() {
        for (mode, expected) in [
            (Mode::ActionSelect, Mode::Browse),
            (Mode::AgentSelect, Mode::ActionSelect),
            (Mode::PermissionSelect, Mode::AgentSelect),
            (Mode::ResumeSelect, Mode::ActionSelect),
        ] {
            let mut app = app();
            assert!(app.capture_active_session());
            app.mode_options = Agent::ClaudeCode.resume_mode_options().to_vec();
            app.resume_mode_options = app.mode_options.clone();
            app.mode = mode;
            app.action_index = 2;
            let mut backend = TestBackend::new(80, 24);
            step(&mut backend, &mut app, EventBuilder::new());
            let result = step(
                &mut backend,
                &mut app,
                EventBuilder::new()
                    .key_code(KeyCode::Esc)
                    .key_code(KeyCode::Enter),
            );
            assert!(result.is_none(), "{mode:?} dispatched after cancellation");
            assert_eq!(app.mode, expected, "{mode:?}");
        }
    }

    #[test]
    fn escape_cancels_single_and_bulk_delete_before_any_confirmation() {
        for bulk in [false, true] {
            let mut app = app();
            let identity = app
                .sessions
                .iter()
                .find(|session| session.agent == Agent::Antigravity)
                .unwrap()
                .identity();
            app.active_session = Some(identity.clone());
            app.pending_delete = Some(identity.clone());
            if bulk {
                app.selected_set
                    .insert(identity.agent, HashSet::from([identity.session_id]));
            }
            app.mode = Mode::DeleteConfirm;
            app.delete_index = 0;
            let mut backend = TestBackend::new(80, 24);
            step(&mut backend, &mut app, EventBuilder::new());
            step(
                &mut backend,
                &mut app,
                EventBuilder::new()
                    .key_code(KeyCode::Esc)
                    .key_code(KeyCode::Enter),
            );
            assert!(app.pending_delete.is_none());
            assert_eq!(
                app.mode,
                if bulk {
                    Mode::BulkDelete
                } else {
                    Mode::ActionSelect
                }
            );
            assert_eq!(app.selected_set.len(), usize::from(bulk));
            assert!(app.deleted_tombstones.is_empty());
            assert_eq!(app.sessions.len(), 15);
        }
    }

    #[test]
    fn search_preserves_literal_punctuation_and_horizontal_cursor_editing() {
        let mut app = app();
        let mut backend = TestBackend::new(40, 12);
        step(&mut backend, &mut app, EventBuilder::new().paste("a?[b]"));
        step(
            &mut backend,
            &mut app,
            EventBuilder::new().key_code(KeyCode::Left),
        );
        step(
            &mut backend,
            &mut app,
            EventBuilder::new().key_code(KeyCode::Right).key('!'),
        );
        assert_eq!(app.mode, Mode::Browse);
        assert_eq!(app.query, "a?[b]!");
        assert_eq!(app.search_textarea.cursor_col, 6);
    }

    #[test]
    fn fresh_query_selects_the_best_match_before_a_batched_enter() {
        let mut app = app();
        app.selected = 14;
        let mut backend = TestBackend::new(80, 24);
        step(&mut backend, &mut app, EventBuilder::new());
        step(
            &mut backend,
            &mut app,
            EventBuilder::new()
                .paste("needle-project-00")
                .key_code(KeyCode::Enter),
        );
        assert_eq!(app.mode, Mode::ActionSelect);
        assert_eq!(app.action_session().unwrap().session_id, "fixture-0");
        assert_eq!(app.selected, 0);
        assert_eq!(app.scroll_offset, 0);
    }

    #[test]
    fn short_browse_keeps_essential_footer_keys_and_compact_columns() {
        for (width, height) in [(20, 8), (39, 12), (40, 12), (80, 24), (120, 28)] {
            let mut app = app();
            app.sessions[0].project_name =
                "\u{d55c}\u{ae00}e\u{301}\u{1f469}\u{200d}\u{1f4bb}project".into();
            app.update_filter();
            let mut backend = TestBackend::new(width, height);
            step(&mut backend, &mut app, EventBuilder::new());
            for key in ["Enter", "F1", "Esc"] {
                backend.assert_line_contains(height - 1, key);
            }
            backend.assert_line_contains(height - 2, "─");
            backend.assert_line_contains(height - 3, "15/15");
            assert!(text::width(&backend.line(3)) <= width as usize);
            assert!(text::width(&backend.line(height - 1)) <= width as usize);
        }
    }

    #[test]
    fn highlights_never_split_combining_or_emoji_graphemes() {
        let value = "\u{d55c}e\u{301}\u{1f469}\u{200d}\u{1f4bb}\u{1f1f0}\u{1f1f7}";
        for position in 0..value.chars().count() {
            let chunks = highlight_text(
                value,
                &[position as u32],
                0,
                Palette::dark().selection_bg,
                Palette::dark(),
            );
            assert_eq!(
                chunks
                    .iter()
                    .map(|(value, _)| value.as_str())
                    .collect::<String>(),
                value
            );
            assert_eq!(chunk_width(&chunks), text::width(value));
            let mut offset = 0;
            for (chunk, _) in chunks {
                offset += chunk.len();
                assert!(
                    offset == value.len()
                        || value
                            .grapheme_indices(true)
                            .any(|(index, _)| index == offset)
                );
            }
        }
    }

    #[test]
    fn scan_status_distinguishes_failures_from_empty_and_filtered_results() {
        let mut app = app();
        app.scanning_agents.insert(Agent::Antigravity);
        assert_eq!(browse_empty_state(&app).0, "Scanning sessions");
        app.scanning_agents.clear();
        app.failed_agents.insert(Agent::Antigravity);
        assert_eq!(browse_empty_state(&app).0, "Sessions unavailable");
        assert!(browse_status(&app).contains("Cached results"));
        app.sessions.clear();
        app.filtered_indices.clear();
        assert!(!browse_status(&app).contains("Cached results"));
        app.failed_agents.clear();
        assert_eq!(browse_empty_state(&app).0, "No saved sessions");
    }

    #[test]
    fn f1_returns_to_the_screen_that_opened_help() {
        let mut app = app();
        assert!(app.capture_active_session());
        app.mode = Mode::ActionSelect;
        let mut backend = TestBackend::new(40, 12);
        step(
            &mut backend,
            &mut app,
            EventBuilder::new().key_code(KeyCode::F(1)),
        );
        assert_eq!(app.mode, Mode::Help);
        step(
            &mut backend,
            &mut app,
            EventBuilder::new().key_code(KeyCode::Esc),
        );
        assert_eq!(app.mode, Mode::ActionSelect);
        assert!(app.active_session.is_some());
    }

    #[test]
    fn minimum_delete_confirmation_keeps_target_and_both_choices_visible() {
        let mut app = app();
        app.sessions[0].project_name = "TARGET".into();
        app.sessions[0].session_id = "c96a140c-d4c0-4996-9b9b-03a0468b1fcc".into();
        app.rebuild_session_index();
        app.pending_delete = Some(app.sessions[0].identity());
        let mut backend = TestBackend::new(20, 8);
        backend.render(|ui| render_single_delete_confirm(ui, &app));
        backend.assert_line_contains(3, "TARGET");
        backend.assert_line_contains(3, "c96a140");
        backend.assert_contains("Yes, delete");
        backend.assert_contains("Cancel");
        backend.assert_line_contains(7, "Esc");
    }

    #[test]
    fn terminal_theme_hint_is_bounded_and_unknown_defaults_to_dark() {
        for hint in [
            None,
            Some(""),
            Some("oops"),
            Some("0;256"),
            Some("0;-1"),
            Some("15;0"),
            Some("0;8"),
        ] {
            assert!(terminal_theme(hint).is_dark, "{hint:?}");
        }
        for hint in ["0;15", "0;7", "0;0;15", "0; 15 "] {
            assert!(!terminal_theme(Some(hint)).is_dark, "{hint}");
        }
    }

    #[test]
    fn appearance_switch_preserves_query_and_selection_and_restores_auto() {
        use crate::settings::Appearance;
        let mut app = app();
        let mut backend = TestBackend::new(40, 12);
        step(&mut backend, &mut app, EventBuilder::new().paste("needle"));
        app.selected = 2;
        let identity = app.selected_session().unwrap().identity();
        for (appearance, expected) in [
            (Appearance::Light, Palette::light()),
            (Appearance::Dark, Palette::dark()),
            (Appearance::Auto, Palette::dark()),
        ] {
            app.settings.appearance = appearance;
            step(&mut backend, &mut app, EventBuilder::new());
            assert_eq!(app.query, "needle");
            assert_eq!(app.selected_session().unwrap().identity(), identity);
            assert_eq!(
                backend.buffer().get(0, 0).style.bg,
                Some(expected.background)
            );
            assert_eq!(
                backend.buffer().get(0, 5).style.bg,
                Some(expected.selection_bg)
            );
            backend.assert_line_contains(11, "Enter");
        }
    }

    #[test]
    fn navigation_markers_stay_neutral_in_lists_and_menus() {
        for (appearance, palette) in [
            (crate::settings::Appearance::Dark, Palette::dark()),
            (crate::settings::Appearance::Light, Palette::light()),
        ] {
            for width in [20, 40, 80, 120] {
                let mut app = app();
                app.settings.appearance = appearance;
                app.agent_filter = Some(Agent::Codex);
                app.restart_search();
                let identity = app.selected_session().unwrap().identity();
                app.pinned_sessions.push(format!(
                    "{}:{}",
                    identity.agent.slug(),
                    identity.session_id
                ));
                let mut backend = TestBackend::new(width, 14);
                step(&mut backend, &mut app, EventBuilder::new());
                backend.assert_line_contains(3, ">*");
                for x in [0, 1] {
                    assert_eq!(
                        backend.buffer().get(x, 3).style.fg,
                        Some(palette.marker(true))
                    );
                }
                assert_eq!(
                    backend.buffer().get(2, 3).style.fg,
                    Some(palette.agent(Agent::Codex))
                );
                let badge = backend.line(1);
                let label_x = text::width(&badge[..badge.find("Codex").unwrap()]) as u32;
                assert_eq!(
                    backend.buffer().get(label_x, 1).style.fg,
                    Some(palette.agent(Agent::Codex))
                );
                assert_eq!(
                    backend.buffer().get(label_x + 7, 1).style.fg,
                    Some(palette.muted)
                );
            }
            for label in ["Resume", "Codex", "Dangerous mode"] {
                let mut backend = TestBackend::new(40, 8);
                backend.render(|ui| {
                    ui.set_theme(if appearance == crate::settings::Appearance::Light {
                        slt::Theme::light()
                    } else {
                        slt::Theme::dark()
                    });
                    render_menu_row(ui, 1, label, "command", true, palette.danger);
                });
                for x in 0..5 {
                    assert_eq!(
                        backend.buffer().get(x, 0).style.fg,
                        Some(palette.marker(true))
                    );
                }
                assert_eq!(backend.buffer().get(5, 0).style.fg, Some(palette.danger));
            }
        }
    }

    #[test]
    fn grouped_agent_color_does_not_leak_into_marker_or_summary() {
        for (theme, palette) in [
            (slt::Theme::dark(), Palette::dark()),
            (slt::Theme::light(), Palette::light()),
        ] {
            let app = app();
            let session = app
                .sessions
                .iter()
                .find(|session| session.agent == Agent::Codex)
                .unwrap();
            for width in [20, 39, 40, 80, 120] {
                let mut backend = TestBackend::new(width, 2);
                backend.render(|ui| {
                    ui.set_theme(theme);
                    render_grouped_session(ui, "> └─* ", session, "Summary 한글 e\u{301}", true);
                });
                let label_x = text::width("> └─* ") as u32;
                for x in 0..label_x {
                    assert_eq!(
                        backend.buffer().get(x, 0).style.fg,
                        Some(palette.marker(true))
                    );
                }
                for x in label_x..label_x + 5 {
                    assert_eq!(
                        backend.buffer().get(x, 0).style.fg,
                        Some(palette.agent(Agent::Codex))
                    );
                }
                assert_eq!(
                    backend.buffer().get(label_x + 7, 0).style.fg,
                    Some(palette.row_text(true))
                );
                assert!(text::width(&backend.line(0)) <= width as usize);
                backend.assert_empty_line(1);
            }
        }
    }

    #[test]
    fn destructive_choices_keep_neutral_pointers_and_red_labels() {
        for (appearance, palette) in [
            (crate::settings::Appearance::Dark, Palette::dark()),
            (crate::settings::Appearance::Light, Palette::light()),
        ] {
            let mut app = app();
            app.settings.appearance = appearance;
            app.mode = Mode::BulkDelete;
            let session = app.sessions[0].clone();
            app.toggle_checked(session.agent, &session.session_id);
            let mut backend = TestBackend::new(80, 12);
            step(&mut backend, &mut app, EventBuilder::new());
            let y = (0..12)
                .find(|&y| backend.line(y).starts_with(">[x]"))
                .unwrap();
            assert_eq!(
                backend.buffer().get(0, y).style.fg,
                Some(palette.marker(true))
            );
            assert_eq!(backend.buffer().get(2, y).style.fg, Some(palette.danger));
            app.delete_index = 0;
            backend
                .render(|ui| render_delete_dialog(ui, &app, "Delete?", &["Target".into()], false));
            let y = (0..12)
                .find(|&y| backend.line(y).starts_with("> Yes"))
                .unwrap();
            assert_eq!(
                backend.buffer().get(0, y).style.fg,
                Some(palette.marker(true))
            );
            assert_eq!(backend.buffer().get(2, y).style.fg, Some(palette.danger));
        }
    }

    #[test]
    fn narrow_error_notices_remain_visible_during_background_scans() {
        for (appearance, palette) in [
            (crate::settings::Appearance::Dark, Palette::dark()),
            (crate::settings::Appearance::Light, Palette::light()),
        ] {
            for kind in [NoticeKind::Warning, NoticeKind::Error] {
                let mut app = app();
                app.settings.appearance = appearance;
                app.notice = Some(Notice::new(kind, "Settings not saved: PermissionDenied"));
                app.scanning_agents.insert(Agent::ClaudeCode);
                app.failed_agents.insert(Agent::Codex);
                let mut backend = TestBackend::new(20, 8);
                step(&mut backend, &mut app, EventBuilder::new());
                backend.assert_line_contains(5, "! Settings not");
                assert_eq!(
                    backend.buffer().get(0, 5).style.fg,
                    Some(app.notice.as_ref().unwrap().color(palette))
                );
                assert!(text::width(&backend.line(5)) <= 20);
                let message = browse_status(&app);
                assert!(message.contains("! Settings not saved"));
                assert!(message.contains("Refresh failed: Codex"));
            }
        }
    }

    #[test]
    fn notice_color_does_not_recolor_result_count_or_scope() {
        for (appearance, palette) in [
            (crate::settings::Appearance::Dark, Palette::dark()),
            (crate::settings::Appearance::Light, Palette::light()),
        ] {
            for kind in [NoticeKind::Success, NoticeKind::Warning, NoticeKind::Error] {
                let mut app = app();
                app.settings.appearance = appearance;
                app.notice = Some(Notice::new(kind, "Notice"));
                let mut backend = TestBackend::new(80, 12);
                step(&mut backend, &mut app, EventBuilder::new());
                let row = backend.line(9);
                let start = row.find("Notice").unwrap() as u32;
                assert_eq!(backend.buffer().get(2, 9).style.fg, Some(palette.muted));
                assert_eq!(
                    backend.buffer().get(start, 9).style.fg,
                    Some(app.notice.as_ref().unwrap().color(palette))
                );
            }
        }
    }

    #[test]
    fn color_downsampling_keeps_row_text_and_metadata_readable() {
        let mut failures = Vec::new();
        for palette in [Palette::dark(), Palette::light()] {
            for depth in [slt::ColorDepth::TrueColor, slt::ColorDepth::EightBit] {
                for selected in [false, true] {
                    let background = if selected {
                        palette.selection_bg
                    } else {
                        palette.background
                    }
                    .downsampled(depth);
                    for color in [
                        palette.row_text(selected),
                        palette.row_muted(selected),
                        palette.accent,
                        palette.warning,
                        palette.danger,
                    ] {
                        let ratio =
                            slt::Color::contrast_ratio_f64(color.downsampled(depth), background);
                        if ratio < 4.5 {
                            failures
                                .push(format!("{color:?} on {background:?} at {depth:?}: {ratio}"));
                        }
                    }
                    for &agent in Agent::all() {
                        let color = palette.agent(agent).downsampled(depth);
                        let ratio = slt::Color::contrast_ratio_f64(color, background);
                        if ratio < 4.5 {
                            failures.push(format!(
                                "{agent}: {color:?} on {background:?} at {depth:?}: {ratio}"
                            ));
                        }
                    }
                }
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn selected_rows_and_search_matches_use_readable_semantic_tokens() {
        for palette in [Palette::dark(), Palette::light()] {
            let app = app();
            let chunks = build_session_row(
                &app.sessions[0],
                palette,
                palette.selection_bg,
                2,
                100,
                1,
                Some(&[0]),
                Some("Summary"),
                25,
            );
            for (value, style) in &chunks {
                if value.trim().is_empty() {
                    continue;
                }
                assert!(
                    slt::Color::contrast_ratio_f64(style.fg.unwrap(), palette.selection_bg) >= 4.5,
                    "{value}: {style:?}"
                );
            }
            assert!(
                chunks
                    .iter()
                    .any(|(_, style)| style.fg == Some(palette.accent)
                        && style.modifiers.contains(slt::Modifiers::UNDERLINE))
            );
            assert_eq!(chunks.last().unwrap().0, " ");
            let plain = highlight_text("project", &[], 0, palette.background, palette);
            assert!(!plain[0].1.modifiers.contains(slt::Modifiers::BOLD));
        }
    }

    #[test]
    fn grouped_selection_has_a_marker_without_relying_on_color() {
        let mut app = app();
        app.build_groups();
        app.mode = Mode::GroupedBrowse;
        let mut backend = TestBackend::new(80, 24);
        step(&mut backend, &mut app, EventBuilder::new());
        assert!(backend.line(4).starts_with('>'));
        step(
            &mut backend,
            &mut app,
            EventBuilder::new().key_code(KeyCode::Enter),
        );
        step(
            &mut backend,
            &mut app,
            EventBuilder::new().key_code(KeyCode::Down),
        );
        assert!(backend.line(5).starts_with('>'));
    }

    #[test]
    fn notices_have_semantic_color_and_a_plain_text_marker() {
        for palette in [Palette::dark(), Palette::light()] {
            for (kind, prefix, color) in [
                (NoticeKind::Info, "i ", palette.secondary),
                (NoticeKind::Success, "+ ", palette.success),
                (NoticeKind::Warning, "! ", palette.warning),
                (NoticeKind::Error, "! ", palette.danger),
            ] {
                let notice = Notice::new(kind, "Message");
                assert_eq!(notice.to_string(), format!("{prefix}Message"));
                assert_eq!(notice.color(palette), color);
            }
        }
    }

    #[test]
    fn sorting_dismisses_old_notice_and_exposes_the_new_sort_order() {
        let mut app = app();
        app.notice = Some(Notice::new(NoticeKind::Success, "Settings saved"));
        let mut backend = TestBackend::new(80, 24);
        step(&mut backend, &mut app, EventBuilder::new());
        backend.assert_contains("Settings saved");
        step(
            &mut backend,
            &mut app,
            EventBuilder::new().key_with(KeyCode::Char('s'), KeyModifiers::CONTROL),
        );
        assert!(app.notice.is_none());
        backend.assert_contains(&format!("Sort: {}", app.sort_mode.label()));
        backend.assert_not_contains("Settings saved");
    }

    #[test]
    fn saved_notice_never_hides_the_current_search_scope() {
        let mut app = app();
        app.notice = Some(Notice::new(NoticeKind::Success, "Settings saved"));
        assert!(browse_status(&app).contains("Name/path"));
        app.include_summaries = true;
        assert!(browse_status(&app).contains("All text"));
        assert!(browse_status(&app).contains("Settings saved"));
    }

    #[test]
    fn antigravity_delete_is_absent_and_bulk_selection_is_disabled() {
        let mut app = app();
        let mut backend = TestBackend::new(80, 24);
        step(
            &mut backend,
            &mut app,
            EventBuilder::new().key_code(KeyCode::BackTab),
        );
        step(
            &mut backend,
            &mut app,
            EventBuilder::new().key_code(KeyCode::Enter),
        );
        backend.assert_not_contains("Delete Session");
        assert!(!available_actions(app.action_session().unwrap()).contains(&Action::Delete));
        step(
            &mut backend,
            &mut app,
            EventBuilder::new().key_code(KeyCode::Esc),
        );
        step(
            &mut backend,
            &mut app,
            EventBuilder::new().key_with(KeyCode::Char('d'), KeyModifiers::CONTROL),
        );
        step(
            &mut backend,
            &mut app,
            EventBuilder::new().key(' ').key_code(KeyCode::Enter),
        );
        assert_eq!(app.mode, Mode::BulkDelete);
        assert_eq!(app.selection_count(), 0);
        assert_eq!(app.sessions.len(), 15);
        backend.assert_contains("0 selected");
    }
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
