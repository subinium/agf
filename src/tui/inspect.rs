use super::palette::Palette;
use super::presentation::{footer, header, key_rows, wrap_text};
use super::*;

const CHROME_HEIGHT: usize = 5;

struct InspectRow {
    text: String,
    color: slt::Color,
    heading: bool,
    setting: Option<usize>,
    label_bytes: usize,
}

fn append_wrapped(rows: &mut Vec<InspectRow>, value: &str, width: usize, color: slt::Color) {
    if value.is_empty() && width > 0 {
        rows.push(InspectRow {
            text: String::new(),
            color,
            heading: false,
            setting: None,
            label_bytes: 0,
        });
        return;
    }
    rows.extend(wrap_text(value, width).into_iter().map(|text| InspectRow {
        text,
        color,
        heading: false,
        setting: None,
        label_bytes: 0,
    }));
}

fn append_heading(rows: &mut Vec<InspectRow>, title: &str, width: usize, palette: Palette) {
    let start = rows.len();
    append_wrapped(rows, title, width, palette.text);
    for row in &mut rows[start..] {
        row.heading = true;
    }
}

fn append_metadata(
    rows: &mut Vec<InspectRow>,
    label: &str,
    value: &str,
    width: usize,
    color: slt::Color,
) {
    let start = rows.len();
    append_wrapped(rows, &format!("{label}: {value}"), width, color);
    let label = format!("{label}:");
    let mut remaining = label.as_str();
    for row in &mut rows[start..] {
        if remaining.is_empty() {
            break;
        }
        if let Some(rest) = remaining.strip_prefix(&row.text) {
            row.label_bytes = row.text.len();
            remaining = rest;
        } else if row.text.starts_with(remaining) {
            row.label_bytes = remaining.len();
            break;
        } else {
            break;
        }
    }
}

fn preview_rows(session: &Session, width: usize, palette: Palette) -> Vec<InspectRow> {
    let mut rows = Vec::new();
    for (label, value, color) in [
        (
            "Agent",
            session.agent.to_string(),
            palette.agent(session.agent),
        ),
        ("Project", session.project_name.clone(), palette.text),
        ("Path", session.display_path(), palette.text),
        ("Session", session.session_id.clone(), palette.text),
        ("Time", session.time_display(), palette.text),
    ] {
        append_metadata(&mut rows, label, &value, width, color);
    }
    for (label, value, color) in [
        ("Branch", session.git_branch.as_deref(), palette.text),
        ("Worktree", session.worktree.as_deref(), palette.text),
        ("Recap", session.recap.as_deref(), palette.text),
    ] {
        if let Some(value) = value {
            append_metadata(&mut rows, label, value, width, color);
        }
    }
    if !session.summaries.is_empty() {
        append_heading(&mut rows, "History", width, palette);
        for (index, summary) in session.summaries.iter().enumerate() {
            append_wrapped(
                &mut rows,
                &format!("{}. {summary}", index + 1),
                width,
                palette.text,
            );
        }
    }
    rows
}

fn scroll_rows(ui: &slt::Context, offset: &mut usize, total: usize, viewport: usize) {
    let maximum = total.saturating_sub(viewport.max(1));
    *offset = (*offset).min(maximum);
    let page = viewport.saturating_sub(1).max(1);
    if ui.key_code(slt::KeyCode::Home) {
        *offset = 0;
    } else if ui.key_code(slt::KeyCode::End) {
        *offset = maximum;
    } else if ui.key_code(slt::KeyCode::PageUp) {
        *offset = offset.saturating_sub(page);
    } else if ui.key_code(slt::KeyCode::PageDown) {
        *offset = offset.saturating_add(page).min(maximum);
    } else if ui.scroll_up() {
        *offset = offset.saturating_sub(1);
    } else if ui.scroll_down() {
        *offset = offset.saturating_add(1).min(maximum);
    }
}

fn render_rows(
    ui: &mut slt::Context,
    rows: &[InspectRow],
    offset: usize,
    viewport: usize,
    selected: Option<usize>,
    palette: Palette,
) {
    let _ = ui.container().min_h(0).grow(1).col(|ui| {
        for row in rows.iter().skip(offset).take(viewport) {
            let highlighted = row.setting.is_some() && row.setting == selected;
            let background = if highlighted {
                palette.selection_bg
            } else {
                palette.background
            };
            let style = if highlighted {
                slt::Style::new().fg(palette.marker(true)).bold()
            } else if row.heading {
                slt::Style::new().fg(row.color).bold()
            } else {
                slt::Style::new().fg(row.color)
            }
            .bg(background);
            let _ = ui
                .container()
                .h(1)
                .min_h(1)
                .max_h(1)
                .px(1)
                .bg(background)
                .row(|ui| {
                    if row.label_bytes > 0 {
                        ui.styled(
                            &row.text[..row.label_bytes],
                            slt::Style::new().fg(palette.secondary).bg(background),
                        );
                        ui.styled(&row.text[row.label_bytes..], style);
                    } else {
                        ui.styled(&row.text, style);
                    }
                });
        }
    });
}

fn line_position(offset: usize, viewport: usize, total: usize) -> String {
    let start = if total == 0 || viewport == 0 {
        0
    } else {
        offset + 1
    };
    format!(
        "{start}-{}/{}",
        offset.saturating_add(viewport).min(total),
        total
    )
}

pub(super) fn preview(ui: &mut slt::Context, app: &mut App, _result: &mut Option<String>) {
    if ui.key_code(slt::KeyCode::Esc)
        || ui.key_code(slt::KeyCode::Left)
        || ui.key_mod('h', slt::KeyModifiers::CONTROL)
    {
        app.active_session = None;
        app.preview_scroll = 0;
        app.mode = Mode::Browse;
        return;
    }
    if app.action_session().is_none() {
        app.active_session = None;
        app.preview_scroll = 0;
        app.mode = Mode::Browse;
        return;
    }
    if ui.key_code(slt::KeyCode::Enter) {
        app.action_index = 0;
        app.mode = Mode::ActionSelect;
        return;
    }
    if ui.key_code(slt::KeyCode::F(1)) {
        app.help_return_mode = Mode::Preview;
        app.help_scroll = 0;
        app.help_settings = false;
        app.mode = Mode::Help;
        return;
    }

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
    } else if down && app.selected + 1 < app.filtered_indices.len() {
        app.selected += 1;
        app.adjust_scroll();
        app.capture_active_session();
    }

    let Some(session) = app.action_session() else {
        app.active_session = None;
        app.preview_scroll = 0;
        app.mode = Mode::Browse;
        return;
    };
    let identity = session.identity();
    let agent = session.agent;
    let height = ui.height();
    let viewport = (height as usize).saturating_sub(CHROME_HEIGHT);
    let palette = app.palette;
    let rows = preview_rows(session, (ui.width() as usize).saturating_sub(2), palette);
    // A named hook also catches an identity changed by the parent between frames.
    let previous = ui.use_state_named("agf.inspect.preview_identity", || None::<SessionIdentity>);
    if previous.get(ui).as_ref() != Some(&identity) {
        app.preview_scroll = 0;
        *previous.get_mut(ui) = Some(identity);
    }
    scroll_rows(ui, &mut app.preview_scroll, rows.len(), viewport);
    let detail = format!(
        "{} | session {}/{} | {}",
        agent,
        app.selected + 1,
        app.filtered_indices.len(),
        line_position(app.preview_scroll, viewport, rows.len()),
    );
    let _ = ui.container().h(height).col(|ui| {
        header(ui, "Session Detail", Some(&detail));
        render_rows(ui, &rows, app.preview_scroll, viewport, None, palette);
        let _ = ui.separator_colored(palette.border);
        footer(
            ui,
            &[
                ("Esc", "Back"),
                ("PgUp/PgDn", "Scroll"),
                ("Enter", "Actions"),
                ("Up/Down", "Session"),
            ],
        );
    });
}

fn keys_rows(width: usize, palette: Palette) -> Vec<InspectRow> {
    let sections: &[(&str, &[(&str, &str)])] = &[
        (
            "Browse",
            &[
                ("Type", "Search sessions"),
                ("Up/Down", "Navigate sessions"),
                ("Tab/Shift+Tab", "Next/previous agent"),
                ("Enter", "Open actions"),
                ("Ctrl+L", "Session details"),
                ("F1", "Help & Settings"),
                ("F2", "Toggle search scope"),
                ("F3/F4", "Previous/next summary"),
                ("Ctrl+S", "Cycle sort order"),
                ("Ctrl+G", "Project groups"),
                ("Ctrl+D", "Bulk delete selection"),
                ("Ctrl+U", "Clear search"),
                ("Esc", "Quit"),
                ("? [ ]", "Literal search characters"),
                ("Left/Right", "Move search caret"),
            ],
        ),
        (
            "Session details",
            &[
                ("Up/Down", "Previous/next session"),
                ("PgUp/PgDn", "Scroll details"),
                ("Home/End", "First/last detail page"),
                ("Wheel", "Scroll details"),
                ("Enter", "Open actions"),
                ("Esc/Left", "Back to browse"),
                ("F1", "Help & Settings"),
            ],
        ),
        (
            "Action menu",
            &[
                ("Up/Down", "Select option"),
                ("Tab/Shift+Tab", "Next/previous option"),
                ("1-9", "Choose numbered action"),
                ("Enter", "Choose selected action"),
                ("Esc", "Back"),
            ],
        ),
        (
            "Agent menu",
            &[
                ("Up/Down", "Select agent"),
                ("Tab/Shift+Tab", "Next/previous agent"),
                ("1-9", "Open numbered agent's modes"),
                ("Enter", "Open selected agent's modes"),
                ("Esc", "Back without launching"),
            ],
        ),
        (
            "Launch / resume modes",
            &[
                ("Up/Down", "Select launch mode"),
                ("Tab/Shift+Tab", "Next/previous launch mode"),
                ("1-9", "Launch with numbered mode"),
                ("Enter", "Launch with selected mode"),
                ("Esc", "Back without launching"),
            ],
        ),
        (
            "Project groups",
            &[
                ("Up/Down", "Navigate groups and sessions"),
                ("Enter/Space", "Expand/collapse group"),
                ("Enter", "Open actions on a session"),
                ("Ctrl+L", "Session details"),
                ("Esc/Ctrl+G", "Back to browse"),
            ],
        ),
        (
            "Bulk delete",
            &[
                ("Up/Down", "Navigate sessions"),
                ("Space", "Toggle selection and advance"),
                ("Enter", "Review deletion"),
                ("Esc", "Cancel selection"),
            ],
        ),
        (
            "Delete confirmation",
            &[
                ("Arrows", "Choose delete or cancel"),
                ("Enter", "Confirm selected choice"),
                ("Esc", "Back without deleting"),
            ],
        ),
        (
            "Help & Settings",
            &[
                ("Tab/Shift+Tab", "Switch Keys/Settings page"),
                ("PgUp/PgDn", "Scroll page"),
                ("Home/End", "First/last page"),
                ("Wheel", "Scroll page"),
                ("Up/Down", "Select a setting"),
                ("Enter/Space", "Change selected setting"),
                ("+/-", "Adjust history entries (1-50)"),
                ("Esc/F1", "Return to previous view"),
            ],
        ),
    ];
    let mut rows = Vec::new();
    for (index, (title, bindings)) in sections.iter().enumerate() {
        if index > 0 {
            append_wrapped(&mut rows, "", width, palette.muted);
        }
        append_heading(&mut rows, title, width, palette);
        rows.extend(
            key_rows(width, bindings)
                .into_iter()
                .map(|text| InspectRow {
                    text,
                    color: palette.text,
                    heading: false,
                    setting: None,
                    label_bytes: 0,
                }),
        );
    }
    rows
}

fn settings_rows(app: &App, width: usize, viewport: usize) -> Vec<InspectRow> {
    let palette = app.palette;
    let scope = if app.include_summaries {
        "Names, paths + history"
    } else {
        "Names and paths"
    };
    let scope = if text::width(scope) > width.saturating_sub(2) {
        if app.include_summaries {
            "All text"
        } else {
            "Names/path"
        }
    } else {
        scope
    };
    let values = [
        ("Search scope", scope.to_string()),
        ("History entries", app.summary_search_count.to_string()),
        (
            "Show recap",
            if app.show_recap { "On" } else { "Off" }.to_string(),
        ),
        ("Appearance", app.settings.appearance.label().to_string()),
    ];
    let mut rows = Vec::new();
    for (index, (label, value)) in values.iter().enumerate() {
        for (line, color) in [
            (label.to_string(), palette.secondary),
            (value.clone(), palette.text),
        ] {
            let label_line = line == *label;
            for (part, value) in wrap_text(&line, width.saturating_sub(2))
                .into_iter()
                .enumerate()
            {
                let prefix = if label_line && part == 0 && index == app.help_selected {
                    "> "
                } else {
                    "  "
                };
                rows.push(InspectRow {
                    text: text::truncate(&format!("{prefix}{value}"), width),
                    color,
                    heading: false,
                    setting: Some(index),
                    label_bytes: 0,
                });
            }
        }
    }
    let path = crate::settings::Settings::config_path();
    let config = wrap_text(&format!("Config: {}", path.to_string_lossy()), width);
    if rows.len().saturating_add(config.len()).saturating_add(1) <= viewport {
        append_wrapped(&mut rows, "", width, palette.muted);
        rows.extend(config.into_iter().map(|text| InspectRow {
            text,
            color: palette.muted,
            heading: false,
            setting: None,
            label_bytes: 0,
        }));
    }
    rows
}

fn setting_range(rows: &[InspectRow], selected: usize) -> std::ops::Range<usize> {
    let start = rows
        .iter()
        .position(|row| row.setting == Some(selected))
        .unwrap_or(0);
    let end = rows
        .iter()
        .rposition(|row| row.setting == Some(selected))
        .map_or(start, |index| index + 1);
    start..end
}

fn reveal_setting(offset: &mut usize, range: &std::ops::Range<usize>, viewport: usize) {
    if range.start < *offset || range.len() >= viewport {
        *offset = range.start;
    } else if range.end > offset.saturating_add(viewport) {
        *offset = range.end.saturating_sub(viewport);
    }
}

fn update_setting(ui: &slt::Context, app: &mut App) -> bool {
    let toggle = ui.key_code(slt::KeyCode::Enter) || ui.key(' ');
    match app.help_selected {
        0 if toggle => {
            app.include_summaries = !app.include_summaries;
            app.restart_search();
            true
        }
        1 => {
            let previous = app.summary_search_count;
            if ui.key('+') || ui.key('=') {
                app.summary_search_count = previous.saturating_add(1).min(50);
            } else if ui.key('-') {
                app.summary_search_count = previous.saturating_sub(1).max(1);
            }
            if previous != app.summary_search_count {
                app.restart_search();
                true
            } else {
                false
            }
        }
        2 if toggle => {
            app.show_recap = !app.show_recap;
            true
        }
        3 if toggle => {
            app.settings.appearance = app.settings.appearance.next();
            true
        }
        _ => false,
    }
}

pub(super) fn help(ui: &mut slt::Context, app: &mut App) {
    if ui.key_code(slt::KeyCode::Esc) || ui.key_code(slt::KeyCode::F(1)) {
        app.mode = app.help_return_mode;
        return;
    }
    let switched =
        ui.consume_key_code(slt::KeyCode::Tab) || ui.consume_key_code(slt::KeyCode::BackTab);
    if switched {
        app.help_settings = !app.help_settings;
        app.help_scroll = 0;
    }
    let height = ui.height();
    let palette = app.palette;
    let viewport = (height as usize).saturating_sub(CHROME_HEIGHT + 1);
    let width = (ui.width() as usize).saturating_sub(2);
    let geometry = (width, viewport);
    let previous_geometry =
        ui.use_state_named("agf.inspect.help_geometry", || None::<(usize, usize)>);
    let geometry_changed = *previous_geometry.get(ui) != Some(geometry);
    *previous_geometry.get_mut(ui) = Some(geometry);
    let rows = if app.help_settings {
        app.help_selected = app.help_selected.min(3);
        let previous = app.help_selected;
        let up = ui.key_code(slt::KeyCode::Up)
            || ui.key_mod('p', slt::KeyModifiers::CONTROL)
            || ui.key_mod('k', slt::KeyModifiers::CONTROL);
        let down = ui.key_code(slt::KeyCode::Down)
            || ui.key_mod('n', slt::KeyModifiers::CONTROL)
            || ui.key_mod('j', slt::KeyModifiers::CONTROL);
        if up {
            app.help_selected = app.help_selected.saturating_sub(1);
        } else if down {
            app.help_selected = app.help_selected.saturating_add(1).min(3);
        }
        let rows = settings_rows(app, width, viewport);
        scroll_rows(ui, &mut app.help_scroll, rows.len(), viewport);
        let range = setting_range(&rows, app.help_selected);
        let editing = ui.key_code(slt::KeyCode::Enter)
            || ui.key(' ')
            || ui.key('+')
            || ui.key('=')
            || ui.key('-');
        let visible =
            range.start >= app.help_scroll && range.end <= app.help_scroll.saturating_add(viewport);
        if geometry_changed || switched || up || down || editing {
            reveal_setting(&mut app.help_scroll, &range, viewport);
        }
        // An edit must target a visible setting on the already-open Settings page.
        if editing
            && !switched
            && previous == app.help_selected
            && visible
            && update_setting(ui, app)
        {
            app.save_settings();
        }
        settings_rows(app, width, viewport)
    } else {
        let rows = keys_rows(width, palette);
        scroll_rows(ui, &mut app.help_scroll, rows.len(), viewport);
        rows
    };
    let detail = format!(
        "{} | {}",
        if app.help_settings {
            "Keys / [Settings]"
        } else {
            "[Keys] / Settings"
        },
        line_position(app.help_scroll, viewport, rows.len()),
    );
    let _ = ui.container().h(height).col(|ui| {
        header(ui, "Help & Settings", Some(&detail));
        render_rows(
            ui,
            &rows,
            app.help_scroll,
            viewport,
            app.help_settings.then_some(app.help_selected),
            palette,
        );
        let _ = ui.container().px(1).h(1).min_h(1).col(|ui| {
            let (message, color) = app
                .notice
                .as_ref()
                .map_or((String::new(), palette.secondary), |notice| {
                    (notice.to_string(), notice.color(palette))
                });
            ui.text(truncate_str(&message, width)).fg(color);
        });
        let _ = ui.separator_colored(palette.border);
        if app.help_settings {
            let edit = if app.help_selected == 1 {
                ("+/-", "Adjust")
            } else if app.help_selected == 3 {
                ("Enter", "Change")
            } else {
                ("Enter", "Toggle")
            };
            footer(
                ui,
                &[
                    ("Tab", "Keys"),
                    edit,
                    ("Esc", "Back"),
                    ("Up/Down", "Select"),
                ],
            );
        } else {
            footer(
                ui,
                &[
                    ("Tab", "Settings"),
                    ("PgUp/PgDn", "Scroll"),
                    ("Esc", "Back"),
                ],
            );
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use slt::{EventBuilder, KeyCode, TestBackend};

    fn fixture() -> App {
        let sessions = (0..3)
            .map(|index| Session {
                agent: Agent::ClaudeCode,
                session_id: format!("inspect-fixture-{index}"),
                project_name: format!("\u{d504}\u{b85c}\u{c81d}\u{d2b8}-{index}"),
                project_path: format!(
                    "/synthetic/{}/project-{index}",
                    "\u{acbd}\u{b85c}".repeat(18)
                ),
                summaries: (1..=12)
                    .map(|number| {
                        format!(
                            "Summary {number}: {} summary-tail-{number:02}",
                            "\u{c138}\u{c158}\u{c694}\u{c57d} ".repeat(12)
                        )
                    })
                    .collect(),
                timestamp: 0,
                git_branch: Some("feature/\u{d55c}\u{ae00}-branch".into()),
                worktree: Some("/synthetic/\u{c791}\u{c5c5}-tree".into()),
                recap: Some("Existing recap only".into()),
                interactive: true,
            })
            .collect::<Vec<_>>();
        let session_index = sessions
            .iter()
            .enumerate()
            .map(|(index, session)| (session.identity(), index))
            .collect();
        App {
            filtered_indices: (0..sessions.len()).collect(),
            match_positions: vec![Vec::new(); sessions.len()],
            sessions,
            session_index,
            selected: 0,
            query: String::new(),
            mode: Mode::Help,
            agent_filter: None,
            action_index: 0,
            agent_index: 0,
            delete_index: 1,
            pending_delete: None,
            active_session: None,
            new_session_options: Vec::new(),
            mode_index: 0,
            mode_options: Vec::new(),
            resume_mode_index: 0,
            resume_mode_options: Vec::new(),
            scroll_offset: 0,
            viewport_height: 7,
            sort_mode: SortMode::Time,
            selected_set: HashMap::new(),
            summary_offsets: HashMap::new(),
            summary_search_count: 5,
            include_summaries: false,
            show_recap: false,
            help_selected: 0,
            preview_scroll: 0,
            help_scroll: 0,
            help_settings: false,
            help_return_mode: Mode::Browse,
            notice: None,
            palette: Palette::dark(),
            system_theme: None,
            search_textarea: slt::TextareaState::new(),
            cwd: None,
            agent_counts: HashMap::from([(Agent::ClaudeCode, 3)]),
            pinned_sessions: Vec::new(),
            settings: crate::settings::Settings::default(),
            groups: Vec::new(),
            group_expanded: HashSet::new(),
            grouped_selected: 0,
            grouped_scroll: 0,
            name_col_width_cache: None,
            scan_rx: None,
            scanning_agents: HashSet::new(),
            failed_agents: HashSet::new(),
            scan_fingerprints: HashMap::new(),
            deleted_tombstones: HashMap::new(),
            fuzzy: FuzzyMatcher::new(),
        }
    }

    fn help_step(backend: &mut TestBackend, app: &mut App, events: EventBuilder) {
        backend.render_with_events(events.build(), 0, 0, |ui| help(ui, app));
    }

    fn preview_step(backend: &mut TestBackend, app: &mut App, events: EventBuilder) {
        let mut result = None;
        backend.render_with_events(events.build(), 0, 0, |ui| preview(ui, app, &mut result));
        assert!(result.is_none());
    }

    fn themed_step(backend: &mut TestBackend, app: &mut App, theme: slt::Theme) {
        backend.render(|ui| {
            ui.set_theme(theme);
            app.palette = Palette::from_ui(ui);
            let height = ui.height();
            let _ = ui
                .container()
                .h(height)
                .bg(app.palette.background)
                .col(|ui| {
                    if app.mode == Mode::Preview {
                        preview(ui, app, &mut None);
                    } else {
                        help(ui, app);
                    }
                });
        });
    }

    struct ResizableBackend {
        size: (u32, u32),
        buffer: slt::Buffer,
    }

    impl slt::Backend for ResizableBackend {
        fn size(&self) -> (u32, u32) {
            self.size
        }

        fn buffer_mut(&mut self) -> &mut slt::Buffer {
            &mut self.buffer
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl ResizableBackend {
        fn step(
            &mut self,
            state: &mut slt::AppState,
            app: &mut App,
            size: (u32, u32),
            theme: slt::Theme,
            events: EventBuilder,
        ) {
            self.size = size;
            self.buffer = slt::Buffer::empty(slt::Rect::new(0, 0, size.0, size.1));
            slt::frame(
                self,
                state,
                &slt::RunConfig::default(),
                &events.build(),
                &mut |ui| {
                    ui.set_theme(theme);
                    app.palette = Palette::from_ui(ui);
                    let _ = ui
                        .container()
                        .h(size.1)
                        .bg(app.palette.background)
                        .col(|ui| help(ui, app));
                },
            )
            .unwrap();
        }

        fn line(&self, y: u32) -> String {
            (0..self.size.0)
                .map(|x| self.buffer.get(x, y).symbol.as_str())
                .collect::<String>()
                .trim_end()
                .to_string()
        }
    }

    #[test]
    fn settings_resize_reveals_appearance_without_resetting_manual_scroll() {
        let mut app = fixture();
        app.help_settings = true;
        let mut state = slt::AppState::new();
        let mut backend = ResizableBackend {
            size: (40, 12),
            buffer: slt::Buffer::empty(slt::Rect::new(0, 0, 40, 12)),
        };
        backend.step(
            &mut state,
            &mut app,
            (40, 12),
            slt::Theme::dark(),
            EventBuilder::new(),
        );
        for _ in 0..3 {
            backend.step(
                &mut state,
                &mut app,
                (40, 12),
                slt::Theme::dark(),
                EventBuilder::new().key_code(KeyCode::Down),
            );
        }
        assert_eq!(app.help_selected, 3);
        assert_eq!(app.help_scroll, 2);
        for (appearance, theme) in [
            (crate::settings::Appearance::Auto, slt::Theme::dark()),
            (crate::settings::Appearance::Dark, slt::Theme::dark()),
            (crate::settings::Appearance::Light, slt::Theme::light()),
        ] {
            app.settings.appearance = appearance;
            backend.step(&mut state, &mut app, (40, 12), theme, EventBuilder::new());
            assert!(backend.line(7).contains("> Appearance"));
            assert!(backend.line(8).contains(appearance.label()));
        }
        backend.step(
            &mut state,
            &mut app,
            (20, 8),
            slt::Theme::light(),
            EventBuilder::new(),
        );
        assert!(
            backend.line(3).contains("> Appearance"),
            "{}",
            backend.line(3)
        );
        assert!(backend.line(4).contains("Light"));
        assert_eq!(app.help_scroll, 6);
        for size in [(40, 12), (20, 12), (20, 8)] {
            backend.step(
                &mut state,
                &mut app,
                size,
                slt::Theme::dark(),
                EventBuilder::new(),
            );
            assert!((3..size.1 - 3).any(|y| backend.line(y).contains("> Appearance")));
            assert_eq!(app.help_selected, 3);
        }

        backend.step(
            &mut state,
            &mut app,
            (20, 8),
            slt::Theme::dark(),
            EventBuilder::new().key_code(KeyCode::Home),
        );
        assert_eq!(app.help_scroll, 0);
        for theme in [slt::Theme::light(), slt::Theme::dark()] {
            backend.step(&mut state, &mut app, (20, 8), theme, EventBuilder::new());
            assert_eq!(
                app.help_scroll, 0,
                "theme changes must not undo manual scrolling"
            );
            assert!(!backend.line(3).contains("Appearance"));
        }
        let appearance = app.settings.appearance;
        backend.step(
            &mut state,
            &mut app,
            (20, 8),
            slt::Theme::dark(),
            EventBuilder::new().key_code(KeyCode::Enter),
        );
        assert_eq!(
            app.settings.appearance, appearance,
            "a hidden edit may only reveal the setting"
        );
        assert_eq!(app.help_scroll, 6);
        assert!(backend.line(3).contains("> Appearance"));
        assert!(app.notice.is_none());
    }

    #[test]
    fn selected_settings_fill_the_row_in_both_themes_without_losing_values() {
        for (width, height) in [(20, 8), (40, 12)] {
            let mut app = fixture();
            app.help_settings = true;
            let mut backend = TestBackend::new(width, height);
            for theme in [slt::Theme::dark(), slt::Theme::light(), slt::Theme::dark()] {
                for include_summaries in [false, true] {
                    app.include_summaries = include_summaries;
                    themed_step(&mut backend, &mut app, theme);
                    let palette = app.palette;
                    backend.assert_line_contains(3, "> Search scope");
                    backend.assert_line_contains(
                        4,
                        match (width, include_summaries) {
                            (_, false) => "Names and paths",
                            (20, true) => "All text",
                            (_, true) => "Names, paths + history",
                        },
                    );
                    for y in [3, 4] {
                        for x in 0..width {
                            assert_eq!(
                                backend.buffer().get(x, y).style.bg,
                                Some(palette.selection_bg),
                                "selected row must fill {width} columns: ({x}, {y})"
                            );
                        }
                    }
                    assert_eq!(
                        backend.buffer().get(1, 3).style,
                        slt::Style::new()
                            .fg(palette.marker(true))
                            .bg(palette.selection_bg)
                            .bold()
                    );
                    assert_ne!(palette.marker(true), palette.accent);
                    let selected_value = backend.buffer().get(3, 4).style;
                    assert_eq!(selected_value.fg, Some(palette.selection_text));
                    assert!(
                        slt::Color::contrast_ratio_f64(
                            palette.selection_text,
                            palette.selection_bg
                        ) >= 4.5
                    );
                    assert_eq!(
                        backend.buffer().get(0, height - 2).style.fg,
                        Some(palette.border)
                    );
                    backend.assert_line_contains(height - 1, "Enter");
                    backend.assert_line_contains(height - 1, "Esc");
                    if height == 12 {
                        assert_eq!(
                            backend.buffer().get(0, 5).style.bg,
                            Some(palette.background)
                        );
                        assert_eq!(backend.buffer().get(3, 5).style.fg, Some(palette.secondary));
                        assert_eq!(backend.buffer().get(3, 6).style.fg, Some(palette.text));
                    }
                }
            }
        }
    }

    #[test]
    fn help_section_headings_are_neutral_and_bold_even_when_wrapped() {
        let titles = [
            "Browse",
            "Session details",
            "Action menu",
            "Agent menu",
            "Launch / resume modes",
            "Project groups",
            "Bulk delete",
            "Delete confirmation",
            "Help & Settings",
        ];
        for width in [6, 18, 38, 78] {
            let height = keys_rows(width, Palette::dark()).len() as u32 + 2;
            let mut backend = TestBackend::new(width as u32 + 2, height);
            for theme in [slt::Theme::dark(), slt::Theme::light(), slt::Theme::dark()] {
                let mut palette = Palette::dark();
                let mut rows = Vec::new();
                backend.render(|ui| {
                    ui.set_theme(theme);
                    palette = Palette::from_ui(ui);
                    rows = keys_rows(width, palette);
                    let _ = ui.container().h(height).bg(palette.background).col(|ui| {
                        render_rows(ui, &rows, 0, rows.len(), None, palette);
                    });
                });
                assert_eq!(
                    rows.iter()
                        .filter(|row| row.heading)
                        .map(|row| row.text.clone())
                        .collect::<Vec<_>>(),
                    titles
                        .iter()
                        .flat_map(|title| wrap_text(title, width))
                        .collect::<Vec<_>>()
                );
                for (y, row) in rows.iter().enumerate() {
                    backend.assert_line(y as u32, format!(" {}", row.text).trim_end());
                    let style = slt::Style::new().fg(palette.text).bg(palette.background);
                    let expected = if row.heading { style.bold() } else { style };
                    for x in 1..=row.text.len() {
                        assert_eq!(backend.buffer().get(x as u32, y as u32).style, expected);
                    }
                }
            }
        }
    }

    #[test]
    fn preview_history_is_neutral_and_bold_and_only_agent_values_use_brand_color() {
        let mut session = fixture().sessions.remove(0);
        session.project_path = "/project".into();
        session.git_branch = Some("main".into());
        session.worktree = None;
        session.recap = Some("Existing recap".into());
        for width in [1, 4, 5, 6, 18, 38] {
            for &agent in Agent::all() {
                session.agent = agent;
                session.project_name = agent.to_string();
                session.summaries = vec![format!("History from {agent}")];
                let height = preview_rows(&session, width, Palette::dark()).len() as u32 + 2;
                let mut backend = TestBackend::new(width as u32 + 2, height);
                for theme in [slt::Theme::dark(), slt::Theme::light(), slt::Theme::dark()] {
                    let mut palette = Palette::dark();
                    let mut rows = Vec::new();
                    backend.render(|ui| {
                        ui.set_theme(theme);
                        palette = Palette::from_ui(ui);
                        rows = preview_rows(&session, width, palette);
                        let _ = ui.container().h(height).bg(palette.background).col(|ui| {
                            render_rows(ui, &rows, 0, rows.len(), None, palette);
                        });
                    });
                    let agent_rows = wrap_text(&format!("Agent: {agent}"), width).len();
                    assert_eq!(
                        rows.iter()
                            .take(agent_rows)
                            .map(|row| &row.text[..row.label_bytes])
                            .collect::<String>(),
                        "Agent:"
                    );
                    assert_eq!(
                        rows.iter()
                            .filter(|row| row.heading)
                            .map(|row| row.text.clone())
                            .collect::<Vec<_>>(),
                        wrap_text("History", width)
                    );
                    for (y, row) in rows.iter().enumerate() {
                        backend.assert_line(y as u32, format!(" {}", row.text).trim_end());
                        for (byte, _) in row.text.char_indices() {
                            let color = if byte < row.label_bytes {
                                palette.secondary
                            } else if y < agent_rows {
                                palette.agent(agent)
                            } else {
                                palette.text
                            };
                            let style = slt::Style::new().fg(color).bg(palette.background);
                            let expected = if row.heading { style.bold() } else { style };
                            let x = 1 + text::width(&row.text[..byte]);
                            assert_eq!(
                                backend.buffer().get(x as u32, y as u32).style,
                                expected,
                                "width {width}, agent {agent}, row {y}, column {x}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn inspect_pages_keep_explicit_backgrounds_across_theme_changes() {
        for (width, height) in [(20, 8), (40, 12), (80, 24)] {
            for (mode, settings) in [
                (Mode::Preview, false),
                (Mode::Help, false),
                (Mode::Help, true),
            ] {
                let mut app = fixture();
                app.mode = mode;
                app.help_settings = settings;
                assert!(app.capture_active_session());
                let mut backend = TestBackend::new(width, height);
                for theme in [slt::Theme::dark(), slt::Theme::light(), slt::Theme::dark()] {
                    themed_step(&mut backend, &mut app, theme);
                    for y in 0..height {
                        let background = if settings && (3..5).contains(&y) {
                            app.palette.selection_bg
                        } else {
                            app.palette.background
                        };
                        for x in 0..width {
                            assert_eq!(
                                backend.buffer().get(x, y).style.bg,
                                Some(background),
                                "{mode:?}, settings {settings}, {width}x{height}, ({x}, {y})"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn preview_separates_metadata_labels_from_values_in_both_themes() {
        let mut app = fixture();
        app.mode = Mode::Preview;
        app.sessions[0].project_name = "plain-project".into();
        assert!(app.capture_active_session());
        let mut backend = TestBackend::new(40, 12);
        for theme in [slt::Theme::dark(), slt::Theme::light()] {
            themed_step(&mut backend, &mut app, theme);
            let palette = app.palette;
            backend.assert_line(3, " Agent: Claude Code");
            backend.assert_line(4, " Project: plain-project");
            assert_eq!(backend.buffer().get(1, 3).style.fg, Some(palette.secondary));
            assert_eq!(
                backend.buffer().get(8, 3).style.fg,
                Some(palette.agent(Agent::ClaudeCode))
            );
            assert_eq!(backend.buffer().get(1, 4).style.fg, Some(palette.secondary));
            assert_eq!(backend.buffer().get(10, 4).style.fg, Some(palette.text));
            for x in 0..40 {
                assert_eq!(
                    backend.buffer().get(x, 4).style.bg,
                    Some(palette.background)
                );
            }
        }
    }

    #[test]
    fn notice_severity_keeps_a_text_cue_and_semantic_color_in_both_themes() {
        let mut app = fixture();
        app.help_settings = true;
        let mut backend = TestBackend::new(40, 12);
        for theme in [slt::Theme::dark(), slt::Theme::light()] {
            for (kind, cue) in [
                (NoticeKind::Info, "i"),
                (NoticeKind::Success, "+"),
                (NoticeKind::Warning, "!"),
                (NoticeKind::Error, "!"),
            ] {
                app.notice = Some(Notice::new(kind, "Status message"));
                themed_step(&mut backend, &mut app, theme);
                backend.assert_line(9, &format!(" {cue} Status message"));
                assert_eq!(
                    backend.buffer().get(1, 9).style.fg,
                    Some(app.notice.as_ref().unwrap().color(app.palette))
                );
                backend.assert_line_contains(3, "Search scope");
                backend.assert_line_contains(11, "Enter");
                backend.assert_line_contains(11, "Esc");
            }
        }
    }

    #[test]
    fn preview_wraps_cjk_metadata_without_truncating_existing_history() {
        let app = fixture();
        let rows = preview_rows(&app.sessions[0], 38, app.palette);
        assert!(rows.iter().all(|row| text::width(&row.text) <= 38));
        let contents = rows.iter().map(|row| row.text.as_str()).collect::<String>();
        assert!(contents.contains(&app.sessions[0].project_path));
        assert!(contents.contains("summary-tail-12"));
        assert!(contents.contains("Existing recap only"));
        assert!(!contents.contains("..."));
    }

    #[test]
    fn preview_scrolls_at_40_by_12_with_fixed_header_and_footer() {
        let mut app = fixture();
        app.mode = Mode::Preview;
        assert!(app.capture_active_session());
        let mut backend = TestBackend::new(40, 12);
        preview_step(&mut backend, &mut app, EventBuilder::new());
        backend.assert_contains("Session Detail");
        backend.assert_line_contains(3, "Agent:");
        backend.assert_line_contains(11, "Esc");
        let identity = app.active_session.clone();
        preview_step(
            &mut backend,
            &mut app,
            EventBuilder::new().key_code(KeyCode::End),
        );
        assert!(app.preview_scroll > 0);
        backend.assert_contains("summary-tail-12");
        backend.assert_line_contains(11, "Esc");
        let end = app.preview_scroll;
        preview_step(
            &mut backend,
            &mut app,
            EventBuilder::new().key_code(KeyCode::PageUp),
        );
        assert!(app.preview_scroll < end);
        preview_step(
            &mut backend,
            &mut app,
            EventBuilder::new().key_code(KeyCode::Home),
        );
        assert_eq!(app.preview_scroll, 0);
        preview_step(
            &mut backend,
            &mut app,
            EventBuilder::new().scroll_down(10, 5),
        );
        assert_eq!(app.preview_scroll, 1);
        assert_eq!(app.active_session, identity);
        assert_eq!(app.selected, 0);
        preview_step(&mut backend, &mut app, EventBuilder::new().scroll_up(10, 5));
        assert_eq!(app.preview_scroll, 0);
    }

    #[test]
    fn preview_cycles_sessions_and_resets_scroll_only_on_identity_change() {
        let mut app = fixture();
        app.mode = Mode::Preview;
        assert!(app.capture_active_session());
        let mut backend = TestBackend::new(40, 12);
        preview_step(&mut backend, &mut app, EventBuilder::new());
        preview_step(
            &mut backend,
            &mut app,
            EventBuilder::new().key_code(KeyCode::PageDown),
        );
        assert!(app.preview_scroll > 0);
        preview_step(
            &mut backend,
            &mut app,
            EventBuilder::new().key_code(KeyCode::Up),
        );
        assert!(app.preview_scroll > 0);
        preview_step(
            &mut backend,
            &mut app,
            EventBuilder::new().key_code(KeyCode::Down),
        );
        assert_eq!(app.selected, 1);
        assert_eq!(app.preview_scroll, 0);
        preview_step(
            &mut backend,
            &mut app,
            EventBuilder::new().key_code(KeyCode::End),
        );
        app.selected = 2;
        assert!(app.capture_active_session());
        preview_step(&mut backend, &mut app, EventBuilder::new());
        assert_eq!(app.preview_scroll, 0);
        assert_eq!(app.mode, Mode::Preview);
    }

    #[test]
    fn preview_enter_opens_actions_and_left_returns_without_launching() {
        for (key, mode) in [
            (KeyCode::Enter, Mode::ActionSelect),
            (KeyCode::Left, Mode::Browse),
            (KeyCode::Esc, Mode::Browse),
        ] {
            let mut app = fixture();
            app.mode = Mode::Preview;
            assert!(app.capture_active_session());
            let mut backend = TestBackend::new(40, 12);
            preview_step(&mut backend, &mut app, EventBuilder::new().key_code(key));
            assert_eq!(app.mode, mode);
            assert_eq!(app.active_session.is_some(), mode == Mode::ActionSelect);
        }
    }

    #[test]
    fn preview_help_round_trip_retains_scroll_and_session_identity() {
        let mut app = fixture();
        app.mode = Mode::Preview;
        assert!(app.capture_active_session());
        let mut backend = TestBackend::new(40, 12);
        preview_step(&mut backend, &mut app, EventBuilder::new());
        preview_step(
            &mut backend,
            &mut app,
            EventBuilder::new().key_code(KeyCode::End),
        );
        let offset = app.preview_scroll;
        let identity = app.active_session.clone();
        preview_step(
            &mut backend,
            &mut app,
            EventBuilder::new().key_code(KeyCode::F(1)),
        );
        assert_eq!(app.mode, Mode::Help);
        assert_eq!(app.help_return_mode, Mode::Preview);
        help_step(&mut backend, &mut app, EventBuilder::new());
        help_step(
            &mut backend,
            &mut app,
            EventBuilder::new().key_code(KeyCode::F(1)),
        );
        assert_eq!(app.mode, Mode::Preview);
        preview_step(&mut backend, &mut app, EventBuilder::new());
        assert_eq!(app.preview_scroll, offset);
        assert_eq!(app.active_session, identity);
        backend.assert_contains("summary-tail-12");
    }

    #[test]
    fn preview_does_not_open_actions_for_a_missing_active_session() {
        let mut app = fixture();
        app.mode = Mode::Preview;
        app.active_session = Some(SessionIdentity {
            agent: Agent::ClaudeCode,
            session_id: "removed-session".into(),
        });
        let mut backend = TestBackend::new(40, 12);
        preview_step(
            &mut backend,
            &mut app,
            EventBuilder::new().key_code(KeyCode::Enter),
        );
        assert_eq!(app.mode, Mode::Browse);
        assert!(app.active_session.is_none());
    }

    #[test]
    fn help_keys_are_scrollable_without_mutating_hidden_settings() {
        let mut app = fixture();
        let mut backend = TestBackend::new(40, 12);
        help_step(&mut backend, &mut app, EventBuilder::new());
        backend.assert_contains("Help & Settings");
        backend.assert_contains("Browse");
        backend.assert_line_contains(11, "Tab");
        for selected in 0..4 {
            app.help_selected = selected;
            for key in [
                KeyCode::Enter,
                KeyCode::Char(' '),
                KeyCode::Char('+'),
                KeyCode::Char('-'),
                KeyCode::Right,
            ] {
                help_step(&mut backend, &mut app, EventBuilder::new().key_code(key));
            }
        }
        assert!(!app.include_summaries);
        assert!(!app.show_recap);
        assert_eq!(app.summary_search_count, 5);
        assert_eq!(app.settings.appearance.label(), "Auto");
        help_step(
            &mut backend,
            &mut app,
            EventBuilder::new().key_code(KeyCode::PageDown),
        );
        assert!(app.help_scroll > 0);
        help_step(
            &mut backend,
            &mut app,
            EventBuilder::new().key_code(KeyCode::End),
        );
        backend.assert_contains("Esc/F1");
        backend.assert_line_contains(11, "Tab");
        let end = app.help_scroll;
        help_step(
            &mut backend,
            &mut app,
            EventBuilder::new().key_code(KeyCode::PageUp),
        );
        assert!(app.help_scroll < end);
        help_step(
            &mut backend,
            &mut app,
            EventBuilder::new().key_code(KeyCode::Home),
        );
        assert_eq!(app.help_scroll, 0);
        help_step(
            &mut backend,
            &mut app,
            EventBuilder::new().scroll_down(5, 5),
        );
        assert_eq!(app.help_scroll, 1);
        help_step(&mut backend, &mut app, EventBuilder::new().scroll_up(5, 5));
        assert_eq!(app.help_scroll, 0);
    }

    #[test]
    fn help_binding_reference_covers_modes_and_literal_search_keys() {
        let rows = keys_rows(38, Palette::dark());
        assert!(rows.iter().all(|row| text::width(&row.text) <= 38));
        let contents = rows
            .iter()
            .map(|row| row.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        for binding in [
            "Ctrl+L",
            "Ctrl+S",
            "Ctrl+G",
            "Ctrl+D",
            "Ctrl+U",
            "F1",
            "F2",
            "F3/F4",
            "? [ ]",
            "Left/Right",
            "1-9",
            "Delete confirmation",
            "Bulk delete",
        ] {
            assert!(contents.contains(binding), "missing {binding}: {contents}");
        }
    }

    #[test]
    fn all_four_settings_are_reachable_at_40_by_12() {
        let mut app = fixture();
        let mut backend = TestBackend::new(40, 12);
        help_step(
            &mut backend,
            &mut app,
            EventBuilder::new().key_code(KeyCode::Tab),
        );
        assert!(app.help_settings);
        backend.assert_contains("> Search scope");
        backend.assert_contains("Names and paths");
        backend.assert_contains("History entries");
        backend.assert_contains("Show recap");
        backend.assert_not_contains("name_path");
        backend.assert_not_contains("summary_search_count");
        for label in ["> History entries", "> Show recap", "> Appearance"] {
            help_step(
                &mut backend,
                &mut app,
                EventBuilder::new().key_code(KeyCode::Down),
            );
            backend.assert_contains(label);
            backend.assert_line_contains(11, "Tab");
        }
        assert_eq!(app.help_selected, 3);
        help_step(
            &mut backend,
            &mut app,
            EventBuilder::new().key_code(KeyCode::Up),
        );
        assert_eq!(app.help_selected, 2);
        backend.assert_contains("> Show recap");
        assert!(!app.include_summaries);
        assert!(!app.show_recap);
        assert_eq!(app.summary_search_count, 5);
        help_step(
            &mut backend,
            &mut app,
            EventBuilder::new().key_code(KeyCode::BackTab),
        );
        assert!(!app.help_settings);
        assert_eq!(app.help_scroll, 0);
    }

    #[test]
    fn settings_navigation_reveals_selection_in_short_viewports() {
        let mut app = fixture();
        app.help_settings = true;
        let mut backend = TestBackend::new(40, 8);
        help_step(&mut backend, &mut app, EventBuilder::new());
        for label in ["> History entries", "> Show recap", "> Appearance"] {
            help_step(
                &mut backend,
                &mut app,
                EventBuilder::new().key_code(KeyCode::Down),
            );
            backend.assert_contains(label);
        }
        assert!(app.help_scroll > 0);
        backend.assert_contains("Auto");
    }

    #[test]
    fn settings_at_twenty_columns_keep_labels_and_values_within_two_rows() {
        for scope in [false, true] {
            for recap in [false, true] {
                let mut app = fixture();
                app.help_settings = true;
                app.include_summaries = scope;
                app.show_recap = recap;
                app.summary_search_count = 50;
                let rows = settings_rows(&app, 18, 2);
                assert_eq!(rows.len(), 8);
                assert!(rows.iter().all(|row| text::width(&row.text) <= 18));
                let values = [
                    (
                        "Search scope",
                        if scope { "All text" } else { "Names and paths" },
                    ),
                    ("History entries", "50"),
                    ("Show recap", if recap { "On" } else { "Off" }),
                    ("Appearance", "Auto"),
                ];
                let mut backend = TestBackend::new(20, 8);
                help_step(&mut backend, &mut app, EventBuilder::new());
                for (selected, (label, value)) in values.iter().enumerate() {
                    assert_eq!(setting_range(&rows, selected).len(), 2);
                    if selected > 0 {
                        help_step(
                            &mut backend,
                            &mut app,
                            EventBuilder::new().key_code(KeyCode::Down),
                        );
                    }
                    backend.assert_line_contains(3, label);
                    backend.assert_line_contains(4, value);
                }
                let normal_rows = settings_rows(&app, 38, 6);
                assert_eq!(normal_rows.len(), 8);
                assert_eq!(
                    normal_rows[1].text.trim(),
                    if scope {
                        "Names, paths + history"
                    } else {
                        "Names and paths"
                    },
                );
            }
        }
    }

    #[test]
    fn search_scope_can_toggle_on_and_back_off_at_twenty_by_eight() {
        let mut app = fixture();
        app.help_settings = true;
        let mut backend = TestBackend::new(20, 8);
        for (key, enabled, value) in [
            (KeyCode::Enter, true, "All text"),
            (KeyCode::Char(' '), false, "Names and paths"),
        ] {
            help_step(&mut backend, &mut app, EventBuilder::new());
            let rows = settings_rows(&app, 18, 2);
            let range = setting_range(&rows, app.help_selected);
            assert!(range.start >= app.help_scroll);
            assert!(range.end <= app.help_scroll + 2);
            backend.render_with_events(EventBuilder::new().key_code(key).build(), 0, 0, |ui| {
                // Check the edit reducer without persisting the user's settings.
                assert!(update_setting(ui, &mut app));
            });
            assert_eq!(app.include_summaries, enabled);
            help_step(&mut backend, &mut app, EventBuilder::new());
            backend.assert_line_contains(3, "Search scope");
            backend.assert_line_contains(4, value);
        }
        assert!(app.notice.is_none());
    }

    #[test]
    fn editing_an_offscreen_setting_only_reveals_it() {
        let mut app = fixture();
        app.help_settings = true;
        let mut backend = TestBackend::new(40, 8);
        help_step(&mut backend, &mut app, EventBuilder::new());
        help_step(
            &mut backend,
            &mut app,
            EventBuilder::new().key_code(KeyCode::End),
        );
        assert!(app.help_scroll > 0);
        help_step(
            &mut backend,
            &mut app,
            EventBuilder::new().key_code(KeyCode::Enter),
        );
        assert!(!app.include_summaries);
        backend.assert_contains("> Search scope");
    }

    #[test]
    fn appearance_cycles_and_remains_reachable_at_twenty_by_eight() {
        let mut app = fixture();
        app.help_settings = true;
        let mut backend = TestBackend::new(20, 8);
        help_step(&mut backend, &mut app, EventBuilder::new());
        for _ in 0..3 {
            help_step(
                &mut backend,
                &mut app,
                EventBuilder::new().key_code(KeyCode::Down),
            );
        }
        assert_eq!(app.help_selected, 3);
        backend.assert_line_contains(3, "Appearance");
        backend.assert_line_contains(4, "Auto");
        backend.assert_line_contains(7, "Enter");
        backend.assert_line_contains(7, "Esc");
        for (key, label) in [
            (KeyCode::Enter, "Dark"),
            (KeyCode::Char(' '), "Light"),
            (KeyCode::Enter, "Auto"),
        ] {
            backend.render_with_events(EventBuilder::new().key_code(key).build(), 0, 0, |ui| {
                // Exercise the reducer without saving the user's configuration.
                assert!(update_setting(ui, &mut app));
            });
            help_step(&mut backend, &mut app, EventBuilder::new());
            backend.assert_line_contains(3, "Appearance");
            backend.assert_line_contains(4, label);
        }
        assert!(app.notice.is_none());
        assert!(!app.include_summaries);
        assert!(!app.show_recap);
        assert_eq!(app.summary_search_count, 5);
    }

    #[test]
    fn setting_edits_keep_existing_toggle_and_count_semantics() {
        let mut app = fixture();
        let mut backend = TestBackend::new(40, 12);
        for (selected, key) in [
            (0, KeyCode::Enter),
            (2, KeyCode::Char(' ')),
            (1, KeyCode::Char('+')),
            (1, KeyCode::Char('-')),
        ] {
            app.help_selected = selected;
            backend.render_with_events(EventBuilder::new().key_code(key).build(), 0, 0, |ui| {
                // Exercise the same reducer without writing the user's configuration.
                assert!(update_setting(ui, &mut app));
            });
        }
        assert!(app.include_summaries);
        assert!(app.show_recap);
        assert_eq!(app.summary_search_count, 5);
        for (count, key) in [(1, '-'), (50, '+')] {
            app.summary_search_count = count;
            backend.render_with_events(EventBuilder::new().key(key).build(), 0, 0, |ui| {
                assert!(!update_setting(ui, &mut app));
            });
            assert_eq!(app.summary_search_count, count);
        }
    }

    #[test]
    fn help_returns_to_the_opening_mode_without_losing_preview_scroll() {
        for mode in [
            Mode::Browse,
            Mode::GroupedBrowse,
            Mode::Preview,
            Mode::ActionSelect,
            Mode::BulkDelete,
        ] {
            for key in [KeyCode::Esc, KeyCode::F(1)] {
                let mut app = fixture();
                app.help_return_mode = mode;
                app.preview_scroll = 17;
                let mut backend = TestBackend::new(40, 12);
                help_step(&mut backend, &mut app, EventBuilder::new().key_code(key));
                assert_eq!(app.mode, mode);
                assert_eq!(app.preview_scroll, 17);
            }
        }
    }

    #[test]
    fn untrusted_detail_rows_are_sanitized_and_fit_narrow_widths() {
        let mut app = fixture();
        app.sessions[0]
            .project_name
            .push_str("\u{1b}[31m\n\u{202e}tail");
        for width in [0, 1, 2, 12, 38, 78] {
            let rows = preview_rows(&app.sessions[0], width, app.palette);
            assert!(rows.iter().all(|row| text::width(&row.text) <= width));
            assert!(rows.iter().all(|row| !row.text.contains('\u{1b}')
                && !row.text.contains('\u{202e}')
                && !row.text.contains('\n')));
        }
    }

    #[test]
    fn settings_notice_keeps_all_controls_and_essential_keys_visible() {
        let mut app = fixture();
        app.help_settings = true;
        app.notice = Some(Notice::new(
            NoticeKind::Success,
            format!(
                "settings saved: {}\u{1b}\u{202e}",
                "\u{c124}\u{c815}".repeat(30)
            ),
        ));
        let mut backend = TestBackend::new(40, 12);
        help_step(&mut backend, &mut app, EventBuilder::new());
        for label in ["Search scope", "History entries", "Show recap"] {
            backend.assert_contains(label);
        }
        backend.assert_line_contains(9, "settings saved");
        for key in ["Tab Keys", "Enter Toggle", "Esc Back"] {
            backend.assert_line_contains(11, key);
        }
        assert!(text::width(&backend.line(9)) <= 40);
        assert!(!backend.line(9).contains('\u{1b}'));
        assert!(!backend.line(9).contains('\u{202e}'));
        help_step(
            &mut backend,
            &mut app,
            EventBuilder::new().key_code(KeyCode::Down),
        );
        for key in ["Tab Keys", "+/- Adjust", "Esc Back"] {
            backend.assert_line_contains(11, key);
        }
        backend.assert_line_contains(9, "settings saved");
    }

    #[test]
    fn settings_config_path_only_uses_room_left_after_all_controls() {
        let mut app = fixture();
        app.help_settings = true;
        let small = settings_rows(&app, 38, 6);
        assert_eq!(small.len(), 8);
        assert!(small.iter().all(|row| row.setting.is_some()));
        let mut backend = TestBackend::new(80, 24);
        help_step(&mut backend, &mut app, EventBuilder::new());
        backend.assert_contains("Config:");
        backend.assert_contains("config.toml");
        backend.assert_line_contains(23, "Esc");
    }
}
