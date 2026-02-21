use ratatui::prelude::*;
use ratatui::widgets::{
    Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
};

use crate::action;
use crate::model::{Action, Agent};

use super::App;

// Color constants
const HIGHLIGHT_BG: Color = Color::Rgb(59, 59, 59);
const BRIGHT_WHITE: Color = Color::Rgb(229, 229, 229);
const GRAY_500: Color = Color::Rgb(107, 114, 128);
const GRAY_400: Color = Color::Rgb(163, 163, 163);
const VIOLET: Color = Color::Rgb(139, 92, 246);
const YELLOW: Color = Color::Rgb(245, 158, 11);
const SEPARATOR: Color = Color::Rgb(64, 64, 64);
const RED: Color = Color::Rgb(239, 68, 68);
const GREEN_400: Color = Color::Rgb(52, 211, 153);
const CYAN: Color = Color::Rgb(34, 211, 238);

fn agent_color(agent: Agent) -> Color {
    let (r, g, b) = agent.color();
    Color::Rgb(r, g, b)
}

pub fn render_browse(f: &mut Frame, app: &App) {
    let area = f.area();
    let is_compact = area.width < 60;

    let chunks = Layout::vertical([
        Constraint::Length(3), // header / filter bar
        Constraint::Min(1),    // session list
        Constraint::Length(1), // footer
    ])
    .split(area);

    // Header: filter bar
    render_filter_bar(f, chunks[0], app);

    // Session list
    if is_compact {
        render_session_list_compact(f, chunks[1], app);
    } else {
        render_session_list(f, chunks[1], app);
    }

    // Footer
    render_footer(f, chunks[2], app);
}

fn render_filter_bar(f: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(SEPARATOR));

    let inner = block.inner(area);
    f.render_widget(block, area);

    // Left side: prompt + query
    let prompt = Span::styled("> ", Style::new().fg(YELLOW).bold());
    let query = Span::styled(&app.query, Style::new().fg(Color::White));
    let cursor = Span::styled("_", Style::new().fg(YELLOW));

    // Right side: agent filter indicator (only shown when filtering)
    let filter_label = match app.agent_filter {
        Some(agent) => {
            let label = format!("[{}]", agent);
            Span::styled(label, Style::new().fg(agent_color(agent)).bold())
        }
        None => Span::styled("[All]", Style::new().fg(GRAY_500)),
    };

    let right_width = filter_label.width();
    let left_width = (inner.width as usize).saturating_sub(right_width + 1);

    let left_area = Rect::new(inner.x, inner.y, left_width as u16, inner.height);
    let left_line = Line::from(vec![prompt, query, cursor]);
    f.render_widget(Paragraph::new(left_line), left_area);

    let right_area = Rect::new(
        inner.x + left_width as u16,
        inner.y,
        (right_width + 1) as u16,
        inner.height,
    );
    f.render_widget(
        Paragraph::new(Line::from(vec![Span::raw(" "), filter_label])).alignment(Alignment::Right),
        right_area,
    );
}

type ProjSpansFn<'a> = dyn Fn(&str, Color) -> Vec<Span<'a>>;

/// Shared row-builder for session list views. The caller provides the leading
/// indicator spans (cursor / checkbox) and their total display width, plus an
/// optional project-name renderer (for fuzzy-match highlighting in browse mode).
/// Everything else — agent label, project truncation, summary, padding, branch,
/// time, git-dirty — is handled here.
fn build_session_row<'a>(
    session: &crate::model::Session,
    bg: Color,
    indicator_width: usize,
    total_width: usize,
    right_margin: usize,
    proj_spans_fn: Option<&ProjSpansFn<'a>>,
    summary_text: Option<&str>,
) -> Vec<Span<'a>> {
    use unicode_width::UnicodeWidthStr;

    let mut spans: Vec<Span<'a>> = Vec::new();

    // Agent label
    let agent_label = format!("{:<12}", session.agent.to_string());
    spans.push(Span::styled(
        agent_label,
        Style::new().fg(agent_color(session.agent)).bold().bg(bg),
    ));

    // Right side layout: [padding][git_info][  time  ][margin]
    //
    // time is anchored to the right edge (fixed width per row, varies only by relative age).
    // git_info (worktree OR branch) floats between project/summary and time.
    // This prevents time from jumping between rows when git_info is absent on some sessions.
    let time_str = session.time_display();
    let time_width = UnicodeWidthStr::width(time_str.as_str()) + 2; // "  " prefix
    let right_display_width = time_width + right_margin;

    let git_info_str: Option<String> = if let Some(ref wt) = session.worktree {
        Some(format!("  {wt}"))
    } else {
        session.git_branch.as_ref().map(|b| format!("  {b}"))
    };
    let git_info_width = git_info_str
        .as_deref()
        .map(UnicodeWidthStr::width)
        .unwrap_or(0);

    // Truncate project name to fit, always — no max_proj > 3 guard that can cause overflow
    let fixed_left = indicator_width + 12; // indicator + agent
    let max_proj =
        total_width.saturating_sub(fixed_left + right_display_width + git_info_width + 4);
    let proj_display = if max_proj == 0 {
        String::new()
    } else if session.project_name.width() > max_proj {
        truncate_str(&session.project_name, max_proj)
    } else {
        session.project_name.clone()
    };

    // Project name (highlighted or plain)
    if let Some(render_fn) = proj_spans_fn {
        spans.extend(render_fn(&proj_display, bg));
    } else {
        spans.push(Span::styled(
            proj_display,
            Style::new().fg(BRIGHT_WHITE).bold().bg(bg),
        ));
    }

    // Git dirty indicator
    if session.git_dirty == Some(true) {
        spans.push(Span::styled("*", Style::new().fg(YELLOW).bold().bg(bg)));
    }

    let left_used: usize = indicator_width + spans.iter().map(|s| s.width()).sum::<usize>();
    let available = total_width.saturating_sub(left_used + git_info_width + right_display_width);

    // Summary
    if available > 7 {
        if let Some(summary) = summary_text {
            let sep = "  ";
            let max_summary = available.saturating_sub(sep.len());
            if max_summary > 5 {
                let truncated = truncate_str(summary, max_summary);
                spans.push(Span::styled(sep, Style::new().bg(bg)));
                spans.push(Span::styled(truncated, Style::new().fg(GRAY_400).bg(bg)));
            }
        }
    }

    // Padding: fills space so that git_info + time flush to the right edge
    let left_width: usize = indicator_width + spans.iter().map(|s| s.width()).sum::<usize>();
    let padding = total_width.saturating_sub(left_width + git_info_width + right_display_width);
    if padding > 0 {
        spans.push(Span::styled(" ".repeat(padding), Style::new().bg(bg)));
    }

    // Right parts: [git_info][  time  ][margin]
    if let Some(git_str) = git_info_str {
        let color = if session.worktree.is_some() {
            CYAN
        } else {
            GREEN_400
        };
        spans.push(Span::styled(git_str, Style::new().fg(color).bg(bg)));
    }
    spans.push(Span::styled(
        format!("  {time_str}"),
        Style::new().fg(VIOLET).bg(bg),
    ));
    if right_margin > 0 {
        spans.push(Span::styled(" ".repeat(right_margin), Style::new().bg(bg)));
    }

    spans
}

/// Render a session list with scrollbar. Shared between browse and bulk-delete views.
fn render_session_list_with_scrollbar(
    f: &mut Frame,
    area: Rect,
    lines: Vec<Line>,
    total_items: usize,
    selected: usize,
) {
    let visible_count = area.height as usize;
    let max_lines = visible_count;

    let mut padded = lines;
    while padded.len() < max_lines {
        padded.push(Line::from(""));
    }
    padded.truncate(max_lines);

    f.render_widget(Paragraph::new(padded), area);

    if total_items > visible_count {
        let mut scrollbar_state = ScrollbarState::new(total_items).position(selected);
        let scrollbar =
            Scrollbar::new(ScrollbarOrientation::VerticalRight).style(Style::new().fg(GRAY_500));
        f.render_stateful_widget(scrollbar, area, &mut scrollbar_state);
    }
}

fn render_session_list(f: &mut Frame, area: Rect, app: &App) {
    let visible_count = area.height as usize;
    let scroll_offset = app.scroll_offset;
    let total_width = area.width as usize;
    let right_margin = 1usize;

    let mut lines: Vec<Line> = Vec::new();

    let end = (scroll_offset + visible_count).min(app.filtered_indices.len());
    for vi in scroll_offset..end {
        let session_idx = app.filtered_indices[vi];
        let session = &app.sessions[session_idx];
        let is_selected = vi == app.selected;
        let match_positions = app.match_positions.get(vi).cloned().unwrap_or_default();

        let bg = if is_selected {
            HIGHLIGHT_BG
        } else {
            Color::Reset
        };
        let indicator = if is_selected { "> " } else { "  " };

        let mp = match_positions;
        let proj_fn =
            move |text: &str, bg: Color| -> Vec<Span<'_>> { highlight_text(text, &mp, 0, bg) };

        let summary_offset = app
            .summary_offsets
            .get(&session.session_id)
            .copied()
            .unwrap_or(0);
        let summary_text = session.summaries.get(summary_offset).map(|s| s.as_str());

        let mut spans = vec![Span::styled(
            indicator,
            Style::new().fg(Color::White).bg(bg),
        )];
        spans.extend(build_session_row(
            session,
            bg,
            2,
            total_width,
            right_margin,
            Some(&proj_fn),
            summary_text,
        ));

        lines.push(Line::from(spans));
    }

    render_session_list_with_scrollbar(f, area, lines, app.filtered_indices.len(), app.selected);
}

fn render_session_list_compact(f: &mut Frame, area: Rect, app: &App) {
    let visible_count = area.height as usize;
    let scroll_offset = app.scroll_offset;

    let mut lines: Vec<Line> = Vec::new();

    let end = (scroll_offset + visible_count).min(app.filtered_indices.len());
    for vi in scroll_offset..end {
        let session_idx = app.filtered_indices[vi];
        let session = &app.sessions[session_idx];
        let is_selected = vi == app.selected;

        let bg = if is_selected {
            HIGHLIGHT_BG
        } else {
            Color::Reset
        };
        let indicator = if is_selected { "> " } else { "  " };

        let mut spans: Vec<Span> = Vec::new();
        spans.push(Span::styled(
            indicator,
            Style::new().fg(Color::White).bg(bg),
        ));
        spans.push(Span::styled(
            format!("{:<12}", session.agent.to_string()),
            Style::new().fg(agent_color(session.agent)).bold().bg(bg),
        ));
        spans.push(Span::styled(
            format!("{:<20}", truncate_str(&session.project_name, 20)),
            Style::new().fg(BRIGHT_WHITE).bold().bg(bg),
        ));
        if let Some(ref wt) = session.worktree {
            spans.push(Span::styled(
                format!("{:<8}", truncate_str(wt, 8)),
                Style::new().fg(CYAN).bg(bg),
            ));
        } else if let Some(ref branch) = session.git_branch {
            spans.push(Span::styled(
                format!("{:<8}", truncate_str(branch, 8)),
                Style::new().fg(GREEN_400).bg(bg),
            ));
        } else {
            spans.push(Span::styled("        ", Style::new().bg(bg)));
        }

        spans.push(Span::styled(
            format!("{:>12}", session.time_display()),
            Style::new().fg(VIOLET).bg(bg),
        ));

        lines.push(Line::from(spans));
    }

    let paragraph = Paragraph::new(lines);
    f.render_widget(paragraph, area);
}

fn render_footer(f: &mut Frame, area: Rect, app: &App) {
    let total = app.sessions.len();
    let filtered = app.filtered_indices.len();

    let mut parts = vec![Span::styled(
        format!(" {filtered} of {total} sessions"),
        Style::new().fg(GRAY_500),
    )];

    if let Some(agent) = app.agent_filter {
        parts.push(Span::styled(
            format!(" ({agent})"),
            Style::new().fg(agent_color(agent)),
        ));
    }

    parts.push(Span::styled(
        format!(
            " │ sort:{} │ tab agent │ ↑↓ nav │ shift ↑↓ summary │ → detail │ enter select │ ^s sort │ ? help │ esc quit",
            app.sort_mode.label()
        ),
        Style::new().fg(GRAY_500),
    ));

    let line = Line::from(parts);
    f.render_widget(Paragraph::new(line), area);
}

pub fn render_action_select(f: &mut Frame, app: &App) {
    let area = f.area();
    let session = match app.selected_session() {
        Some(s) => s,
        None => return,
    };

    let chunks = Layout::vertical([
        Constraint::Length(1), // separator
        Constraint::Length(1), // session info
        Constraint::Length(1), // separator
        Constraint::Length(1), // blank
        Constraint::Min(4),    // actions
        Constraint::Length(1), // blank
        Constraint::Length(1), // separator
        Constraint::Length(1), // footer
    ])
    .split(area);

    // Separator
    let sep = "─".repeat(area.width as usize);
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(&sep, Style::new().fg(SEPARATOR)))),
        chunks[0],
    );

    // Session info line
    let info_spans = vec![
        Span::styled(
            format!(" {} ", session.agent),
            Style::new().fg(agent_color(session.agent)).bold(),
        ),
        Span::styled("│ ", Style::new().fg(SEPARATOR)),
        Span::styled(&session.project_name, Style::new().fg(BRIGHT_WHITE).bold()),
        Span::styled(" │ ", Style::new().fg(SEPARATOR)),
        Span::styled(session.display_path(), Style::new().fg(GRAY_500)),
    ];
    let mut info = info_spans;
    if let Some(ref branch) = session.git_branch {
        info.push(Span::styled(" │ ", Style::new().fg(SEPARATOR)));
        info.push(Span::styled(branch.as_str(), Style::new().fg(GREEN_400)));
    }
    info.push(Span::styled(" │ ", Style::new().fg(SEPARATOR)));
    info.push(Span::styled(
        session.time_display(),
        Style::new().fg(VIOLET),
    ));
    f.render_widget(Paragraph::new(Line::from(info)), chunks[1]);

    f.render_widget(
        Paragraph::new(Line::from(Span::styled(&sep, Style::new().fg(SEPARATOR)))),
        chunks[2],
    );

    // Actions
    let actions = Action::MENU;
    let mut action_lines: Vec<Line> = Vec::new();

    for (i, &act) in actions.iter().enumerate() {
        let is_selected = i == app.action_index;
        let bg = if is_selected {
            HIGHLIGHT_BG
        } else {
            Color::Reset
        };
        let indicator = format!(" {}) ", i + 1);

        let label = act.to_string();

        let base_style = if act == Action::Delete {
            Style::new().fg(RED).bg(bg)
        } else if act == Action::Back {
            Style::new().fg(GRAY_500).bg(bg)
        } else {
            Style::new().fg(BRIGHT_WHITE).bg(bg)
        };
        let label_style = if is_selected {
            base_style.bold()
        } else {
            base_style
        };

        let preview = action::action_preview(session, act);
        let used = 3 + label.len() + 4;
        let preview_width = (area.width as usize).saturating_sub(used);
        let padding = " ".repeat(preview_width.saturating_sub(preview.len()));

        action_lines.push(Line::from(vec![
            Span::styled(indicator, Style::new().fg(Color::White).bg(bg)),
            Span::styled(label, label_style),
            Span::styled(format!("    {preview}"), Style::new().fg(GRAY_500).bg(bg)),
            Span::styled(padding, Style::new().bg(bg)),
        ]));
    }

    let actions_widget = Paragraph::new(action_lines);
    f.render_widget(actions_widget, chunks[4]);

    // Bottom separator
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(&sep, Style::new().fg(SEPARATOR)))),
        chunks[6],
    );

    // Footer
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            " enter confirm │ esc back",
            Style::new().fg(GRAY_500),
        ))),
        chunks[7],
    );
}

pub fn render_agent_select(f: &mut Frame, app: &App) {
    let area = f.area();
    let session = match app.selected_session() {
        Some(s) => s,
        None => return,
    };

    let chunks = Layout::vertical([
        Constraint::Length(1), // separator
        Constraint::Length(1), // header
        Constraint::Length(1), // separator
        Constraint::Length(1), // blank
        Constraint::Min(3),    // agents
        Constraint::Length(1), // blank
        Constraint::Length(1), // separator
        Constraint::Length(1), // footer
    ])
    .split(area);

    let sep = "─".repeat(area.width as usize);
    f.render_widget(
        Paragraph::new(Span::styled(&sep, Style::new().fg(SEPARATOR))),
        chunks[0],
    );

    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" New session in ", Style::new().fg(BRIGHT_WHITE)),
            Span::styled(session.display_path(), Style::new().fg(GRAY_500)),
            Span::styled("  (tab → permission mode)", Style::new().fg(GRAY_500)),
        ])),
        chunks[1],
    );

    f.render_widget(
        Paragraph::new(Span::styled(&sep, Style::new().fg(SEPARATOR))),
        chunks[2],
    );

    // Option list
    let mut agent_lines: Vec<Line> = Vec::new();
    for (i, opt) in app.new_session_options.iter().enumerate() {
        let is_selected = i == app.agent_index;
        let bg = if is_selected {
            HIGHLIGHT_BG
        } else {
            Color::Reset
        };
        let indicator = format!(" {}) ", i + 1);
        let label = &opt.label;

        // Command preview
        let preview = if let Some(s) = app.selected_session() {
            let base = opt.agent.new_session_cmd();
            format!("cd {} && {base}", s.display_path())
        } else {
            String::new()
        };

        let used = indicator.len() + label.len() + 4;
        let preview_width = (area.width as usize).saturating_sub(used);
        let padding = " ".repeat(preview_width.saturating_sub(preview.len()));

        agent_lines.push(Line::from(vec![
            Span::styled(indicator, Style::new().fg(GRAY_400).bg(bg)),
            Span::styled(label.clone(), {
                let s = Style::new().fg(agent_color(opt.agent)).bg(bg);
                if is_selected {
                    s.bold()
                } else {
                    s
                }
            }),
            Span::styled(format!("    {preview}"), Style::new().fg(GRAY_500).bg(bg)),
            Span::styled(padding, Style::new().bg(bg)),
        ]));
    }

    f.render_widget(Paragraph::new(agent_lines), chunks[4]);

    f.render_widget(
        Paragraph::new(Span::styled(&sep, Style::new().fg(SEPARATOR))),
        chunks[6],
    );

    f.render_widget(
        Paragraph::new(Span::styled(
            " 1-9 select │ tab mode │ enter confirm │ esc back",
            Style::new().fg(GRAY_500),
        )),
        chunks[7],
    );
}

pub fn render_mode_select(f: &mut Frame, app: &App) {
    let area = f.area();
    if app.selected_session().is_none() {
        return;
    }

    let agent_label = app
        .new_session_options
        .get(app.agent_index)
        .map(|o| o.label.as_str())
        .unwrap_or("agent");

    let chunks = Layout::vertical([
        Constraint::Length(1), // separator
        Constraint::Length(1), // header
        Constraint::Length(1), // separator
        Constraint::Length(1), // blank
        Constraint::Min(3),    // mode options
        Constraint::Length(1), // blank
        Constraint::Length(1), // separator
        Constraint::Length(1), // footer
    ])
    .split(area);

    let sep = "─".repeat(area.width as usize);
    f.render_widget(
        Paragraph::new(Span::styled(&sep, Style::new().fg(SEPARATOR))),
        chunks[0],
    );

    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" Select mode for ", Style::new().fg(BRIGHT_WHITE)),
            Span::styled(agent_label, Style::new().fg(YELLOW).bold()),
        ])),
        chunks[1],
    );

    f.render_widget(
        Paragraph::new(Span::styled(&sep, Style::new().fg(SEPARATOR))),
        chunks[2],
    );

    let mut mode_lines: Vec<Line> = Vec::new();
    for (i, &(label, flags)) in app.mode_options.iter().enumerate() {
        let is_selected = i == app.mode_index;
        let bg = if is_selected {
            HIGHLIGHT_BG
        } else {
            Color::Reset
        };
        let indicator = format!(" {}) ", i + 1);
        let flag_preview = if flags.is_empty() {
            String::new()
        } else {
            format!("  {}", flags.trim())
        };
        let padding_len = (area.width as usize)
            .saturating_sub(indicator.len() + label.len() + flag_preview.len());

        mode_lines.push(Line::from(vec![
            Span::styled(indicator, Style::new().fg(GRAY_400).bg(bg)),
            Span::styled(label, {
                let s = Style::new().fg(BRIGHT_WHITE).bg(bg);
                if is_selected {
                    s.bold()
                } else {
                    s
                }
            }),
            Span::styled(flag_preview, Style::new().fg(GRAY_500).bg(bg)),
            Span::styled(" ".repeat(padding_len), Style::new().bg(bg)),
        ]));
    }

    f.render_widget(Paragraph::new(mode_lines), chunks[4]);

    f.render_widget(
        Paragraph::new(Span::styled(&sep, Style::new().fg(SEPARATOR))),
        chunks[6],
    );

    f.render_widget(
        Paragraph::new(Span::styled(
            " 1-9 select │ enter confirm │ esc back",
            Style::new().fg(GRAY_500),
        )),
        chunks[7],
    );
}

pub fn render_bulk_delete(f: &mut Frame, app: &App) {
    let area = f.area();

    let chunks = Layout::vertical([
        Constraint::Length(3), // header
        Constraint::Min(1),    // session list
        Constraint::Length(1), // footer
    ])
    .split(area);

    // Header: DELETE MODE
    render_bulk_delete_header(f, chunks[0], app);

    // Session list with checkboxes
    render_bulk_delete_list(f, chunks[1], app);

    // Footer
    render_bulk_delete_footer(f, chunks[2], app);
}

fn render_bulk_delete_header(f: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(RED));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let count = app.selected_set.len();
    let mut spans = vec![Span::styled(" DELETE MODE", Style::new().fg(RED).bold())];
    if count > 0 {
        spans.push(Span::styled(
            format!("  ({count} selected)"),
            Style::new().fg(RED),
        ));
    }

    f.render_widget(Paragraph::new(Line::from(spans)), inner);
}

fn render_bulk_delete_list(f: &mut Frame, area: Rect, app: &App) {
    let visible_count = area.height as usize;
    let scroll_offset = app.scroll_offset;
    let total_width = area.width as usize;
    let right_margin = 1usize;

    let mut lines: Vec<Line> = Vec::new();

    let end = (scroll_offset + visible_count).min(app.filtered_indices.len());
    for vi in scroll_offset..end {
        let session_idx = app.filtered_indices[vi];
        let session = &app.sessions[session_idx];
        let is_cursor = vi == app.selected;
        let is_checked = app.selected_set.contains(&session_idx);

        let bg = if is_cursor {
            HIGHLIGHT_BG
        } else {
            Color::Reset
        };

        let indicator = match (is_cursor, is_checked) {
            (true, true) => ">[x] ",
            (true, false) => ">[ ] ",
            (false, true) => " [x] ",
            (false, false) => " [ ] ",
        };

        let indicator_style = if is_checked {
            Style::new().fg(RED).bold().bg(bg)
        } else {
            Style::new().fg(Color::White).bg(bg)
        };

        let mut spans = vec![Span::styled(indicator, indicator_style)];
        spans.extend(build_session_row(
            session,
            bg,
            5, // indicator width for checkbox
            total_width,
            right_margin,
            None, // no highlight in bulk-delete mode
            session.summaries.first().map(|s| s.as_str()),
        ));

        lines.push(Line::from(spans));
    }

    render_session_list_with_scrollbar(f, area, lines, app.filtered_indices.len(), app.selected);
}

fn render_bulk_delete_footer(f: &mut Frame, area: Rect, app: &App) {
    let count = app.selected_set.len();
    let parts = vec![
        Span::styled(format!(" {count} selected"), Style::new().fg(RED).bold()),
        Span::styled(
            " │ space toggle │ enter delete │ esc cancel",
            Style::new().fg(GRAY_500),
        ),
    ];

    f.render_widget(Paragraph::new(Line::from(parts)), area);
}

pub fn render_delete_confirm(f: &mut Frame, app: &App) {
    if !app.selected_set.is_empty() {
        render_bulk_delete_confirm(f, app);
        return;
    }

    let area = f.area();
    let session = match app.selected_session() {
        Some(s) => s,
        None => return,
    };

    let chunks = Layout::vertical([
        Constraint::Length(1), // separator
        Constraint::Length(1), // header
        Constraint::Length(1), // separator
        Constraint::Length(1), // blank
        Constraint::Length(1), // session info
        Constraint::Length(1), // path
        Constraint::Length(1), // summary
        Constraint::Length(1), // blank
        Constraint::Length(2), // options
        Constraint::Length(1), // blank
        Constraint::Length(1), // separator
    ])
    .split(area);

    let sep = "─".repeat(area.width as usize);
    f.render_widget(
        Paragraph::new(Span::styled(&sep, Style::new().fg(SEPARATOR))),
        chunks[0],
    );

    f.render_widget(
        Paragraph::new(Span::styled(
            " Delete session?",
            Style::new().fg(RED).bold(),
        )),
        chunks[1],
    );

    f.render_widget(
        Paragraph::new(Span::styled(&sep, Style::new().fg(SEPARATOR))),
        chunks[2],
    );

    // Session info
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                format!("  {} ", session.agent),
                Style::new().fg(agent_color(session.agent)).bold(),
            ),
            Span::styled("│ ", Style::new().fg(SEPARATOR)),
            Span::styled(&session.project_name, Style::new().fg(BRIGHT_WHITE)),
            Span::styled(" │ ", Style::new().fg(SEPARATOR)),
            Span::styled(&session.session_id, Style::new().fg(GRAY_500)),
        ])),
        chunks[4],
    );

    f.render_widget(
        Paragraph::new(Span::styled(
            format!("  {}", session.display_path()),
            Style::new().fg(GRAY_500),
        )),
        chunks[5],
    );

    if let Some(summary) = session.summaries.first() {
        let truncated = truncate_str(summary, area.width as usize - 6);
        f.render_widget(
            Paragraph::new(Span::styled(
                format!("  \"{truncated}\""),
                Style::new().fg(GRAY_400),
            )),
            chunks[6],
        );
    }

    // Options
    let options = ["Yes, delete", "Cancel"];
    let mut opt_lines: Vec<Line> = Vec::new();
    for (i, &opt) in options.iter().enumerate() {
        let is_selected = i == app.delete_index;
        let bg = if is_selected {
            HIGHLIGHT_BG
        } else {
            Color::Reset
        };
        let indicator = if is_selected { " > " } else { "   " };

        let label_style = if i == 0 {
            Style::new().fg(RED).bold().bg(bg)
        } else {
            Style::new().fg(BRIGHT_WHITE).bg(bg)
        };

        let desc = if i == 0 {
            "removes session data only"
        } else {
            "go back"
        };

        opt_lines.push(Line::from(vec![
            Span::styled(indicator, Style::new().fg(Color::White).bg(bg)),
            Span::styled(opt, label_style),
            Span::styled(format!("    {desc}"), Style::new().fg(GRAY_500).bg(bg)),
        ]));
    }

    f.render_widget(Paragraph::new(opt_lines), chunks[8]);

    f.render_widget(
        Paragraph::new(Span::styled(&sep, Style::new().fg(SEPARATOR))),
        chunks[10],
    );
}

fn render_bulk_delete_confirm(f: &mut Frame, app: &App) {
    let area = f.area();
    let count = app.selected_set.len();

    // Collect selected session names
    let mut names: Vec<String> = app
        .selected_set
        .iter()
        .filter_map(|&idx| app.sessions.get(idx))
        .map(|s| s.project_name.clone())
        .collect();
    names.sort();

    let show_count = names.len().min(5);
    let list_height = if names.len() > 5 {
        show_count + 1 // +1 for "… and N more"
    } else {
        show_count
    };

    let chunks = Layout::vertical([
        Constraint::Length(1),                  // separator
        Constraint::Length(1),                  // header
        Constraint::Length(1),                  // separator
        Constraint::Length(1),                  // blank
        Constraint::Length(list_height as u16), // session list
        Constraint::Length(1),                  // blank
        Constraint::Length(2),                  // options
        Constraint::Min(0),                     // spacer
        Constraint::Length(1),                  // separator
    ])
    .split(area);

    let sep = "─".repeat(area.width as usize);
    f.render_widget(
        Paragraph::new(Span::styled(&sep, Style::new().fg(SEPARATOR))),
        chunks[0],
    );

    f.render_widget(
        Paragraph::new(Span::styled(
            format!(" Delete {count} sessions?"),
            Style::new().fg(RED).bold(),
        )),
        chunks[1],
    );

    f.render_widget(
        Paragraph::new(Span::styled(&sep, Style::new().fg(SEPARATOR))),
        chunks[2],
    );

    // Session name list (max 5)
    let mut session_lines: Vec<Line> = Vec::new();
    for (i, name) in names.iter().enumerate() {
        if i >= 5 {
            session_lines.push(Line::from(Span::styled(
                format!("  … and {} more", count - 5),
                Style::new().fg(GRAY_500),
            )));
            break;
        }
        session_lines.push(Line::from(Span::styled(
            format!("  • {name}"),
            Style::new().fg(BRIGHT_WHITE),
        )));
    }
    f.render_widget(Paragraph::new(session_lines), chunks[4]);

    // Options
    let options = ["Yes, delete all", "Cancel"];
    let mut opt_lines: Vec<Line> = Vec::new();
    for (i, &opt) in options.iter().enumerate() {
        let is_selected = i == app.delete_index;
        let bg = if is_selected {
            HIGHLIGHT_BG
        } else {
            Color::Reset
        };
        let indicator = if is_selected { " > " } else { "   " };

        let label_style = if i == 0 {
            Style::new().fg(RED).bold().bg(bg)
        } else {
            Style::new().fg(BRIGHT_WHITE).bg(bg)
        };

        let desc = if i == 0 {
            "removes session data only"
        } else {
            "go back"
        };

        opt_lines.push(Line::from(vec![
            Span::styled(indicator, Style::new().fg(Color::White).bg(bg)),
            Span::styled(opt, label_style),
            Span::styled(format!("    {desc}"), Style::new().fg(GRAY_500).bg(bg)),
        ]));
    }
    f.render_widget(Paragraph::new(opt_lines), chunks[6]);

    f.render_widget(
        Paragraph::new(Span::styled(&sep, Style::new().fg(SEPARATOR))),
        chunks[8],
    );
}

fn highlight_text(text: &str, positions: &[u32], offset: usize, bg: Color) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let chars: Vec<char> = text.chars().collect();

    let mut i = 0;
    while i < chars.len() {
        let global_pos = (i + offset) as u32;
        if positions.contains(&global_pos) {
            // Highlighted char
            spans.push(Span::styled(
                chars[i].to_string(),
                Style::new().fg(YELLOW).bold().underlined().bg(bg),
            ));
            i += 1;
        } else {
            // Collect normal chars
            let start = i;
            while i < chars.len() && !positions.contains(&((i + offset) as u32)) {
                i += 1;
            }
            let normal: String = chars[start..i].iter().collect();
            spans.push(Span::styled(
                normal,
                Style::new().fg(BRIGHT_WHITE).bold().bg(bg),
            ));
        }
    }
    spans
}

pub fn render_preview(f: &mut Frame, app: &App) {
    let area = f.area();
    let session = match app.selected_session() {
        Some(s) => s,
        None => return,
    };

    let chunks = Layout::vertical([
        Constraint::Length(1), // separator
        Constraint::Length(1), // header
        Constraint::Length(1), // separator
        Constraint::Length(1), // blank
        Constraint::Min(6),    // detail lines
        Constraint::Length(1), // blank
        Constraint::Length(1), // separator
        Constraint::Length(1), // footer
    ])
    .split(area);

    let sep = "─".repeat(area.width as usize);
    f.render_widget(
        Paragraph::new(Span::styled(&sep, Style::new().fg(SEPARATOR))),
        chunks[0],
    );

    f.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(
            " Session Detail",
            Style::new().fg(BRIGHT_WHITE).bold(),
        )])),
        chunks[1],
    );

    f.render_widget(
        Paragraph::new(Span::styled(&sep, Style::new().fg(SEPARATOR))),
        chunks[2],
    );

    // Detail lines
    let mut lines: Vec<Line> = vec![
        Line::from(vec![
            Span::styled("  Agent:    ", Style::new().fg(GRAY_500)),
            Span::styled(
                session.agent.to_string(),
                Style::new().fg(agent_color(session.agent)).bold(),
            ),
        ]),
        Line::from(vec![
            Span::styled("  Project:  ", Style::new().fg(GRAY_500)),
            Span::styled(&session.project_name, Style::new().fg(BRIGHT_WHITE).bold()),
        ]),
        Line::from(vec![
            Span::styled("  Path:     ", Style::new().fg(GRAY_500)),
            Span::styled(session.display_path(), Style::new().fg(GRAY_400)),
        ]),
        Line::from(vec![
            Span::styled("  Session:  ", Style::new().fg(GRAY_500)),
            Span::styled(&session.session_id, Style::new().fg(GRAY_400)),
        ]),
        Line::from(vec![
            Span::styled("  Time:     ", Style::new().fg(GRAY_500)),
            Span::styled(session.time_display(), Style::new().fg(VIOLET)),
        ]),
    ];

    if let Some(ref branch) = session.git_branch {
        let dirty_marker = if session.git_dirty == Some(true) {
            " *"
        } else {
            ""
        };
        lines.push(Line::from(vec![
            Span::styled("  Branch:   ", Style::new().fg(GRAY_500)),
            Span::styled(
                format!("{branch}{dirty_marker}"),
                Style::new().fg(GREEN_400),
            ),
        ]));
    }
    if let Some(ref wt) = session.worktree {
        lines.push(Line::from(vec![
            Span::styled("  Worktree: ", Style::new().fg(GRAY_500)),
            Span::styled(wt.clone(), Style::new().fg(CYAN)),
        ]));
    }

    if !session.summaries.is_empty() {
        lines.push(Line::from(vec![Span::styled(
            "  History:  ",
            Style::new().fg(GRAY_500),
        )]));
        let max_width = area.width.saturating_sub(14) as usize;
        for (i, summary) in session.summaries.iter().enumerate() {
            let truncated = truncate_str(summary, max_width);
            lines.push(Line::from(vec![
                Span::styled(format!("    {:>2}. ", i + 1), Style::new().fg(GRAY_500)),
                Span::styled(truncated, Style::new().fg(GRAY_400)),
            ]));
        }
    }

    f.render_widget(Paragraph::new(lines), chunks[4]);

    f.render_widget(
        Paragraph::new(Span::styled(&sep, Style::new().fg(SEPARATOR))),
        chunks[6],
    );

    f.render_widget(
        Paragraph::new(Span::styled(
            " enter actions │ esc/← back │ any key back",
            Style::new().fg(GRAY_500),
        )),
        chunks[7],
    );
}

pub fn render_help(f: &mut Frame, app: &App) {
    let area = f.area();
    let sep = "─".repeat(area.width as usize);

    let chunks = Layout::vertical([
        Constraint::Length(1), // separator
        Constraint::Length(1), // header
        Constraint::Length(1), // separator
        Constraint::Length(1), // blank
        Constraint::Min(4),    // content
        Constraint::Length(1), // blank
        Constraint::Length(1), // separator
        Constraint::Length(1), // footer
    ])
    .split(area);

    f.render_widget(
        Paragraph::new(Span::styled(&sep, Style::new().fg(SEPARATOR))),
        chunks[0],
    );
    f.render_widget(
        Paragraph::new(Span::styled(
            " Help & Settings",
            Style::new().fg(BRIGHT_WHITE).bold(),
        )),
        chunks[1],
    );
    f.render_widget(
        Paragraph::new(Span::styled(&sep, Style::new().fg(SEPARATOR))),
        chunks[2],
    );

    let key = |k: &'static str| Span::styled(format!("  {k:<18}"), Style::new().fg(BRIGHT_WHITE));
    let desc = |d: String| Span::styled(d, Style::new().fg(GRAY_400));
    let section = |s: &'static str| {
        Line::from(vec![Span::styled(
            format!("  {s}"),
            Style::new().fg(GRAY_500),
        )])
    };

    let search_scope_label = if app.include_summaries {
        "all (name + path + summaries)"
    } else {
        "name_path (default)"
    };

    let config_path = crate::settings::Settings::config_path();
    let config_path_str = config_path.to_string_lossy().to_string();

    let lines: Vec<Line> = vec![
        section("── Keybindings ─────────────────────"),
        Line::from(vec![key("↑ / ↓"), desc("Navigate sessions".to_string())]),
        Line::from(vec![
            key("Shift+↑ / Shift+↓"),
            desc("Cycle session summary".to_string()),
        ]),
        Line::from(vec![key("→"), desc("Session detail / history".to_string())]),
        Line::from(vec![key("Enter"), desc("Open action menu".to_string())]),
        Line::from(vec![
            key("Tab / Shift+Tab"),
            desc("Cycle agent filter".to_string()),
        ]),
        Line::from(vec![key("Ctrl+S"), desc("Cycle sort mode".to_string())]),
        Line::from(vec![
            key("Ctrl+D"),
            desc("Enter bulk delete mode".to_string()),
        ]),
        Line::from(vec![key("?"), desc("This help panel".to_string())]),
        Line::from(vec![key("Esc"), desc("Quit".to_string())]),
        Line::from(vec![]),
        section("── Settings (↑↓ navigate, enter/space toggle, +/- adjust) ──"),
        Line::from(vec![
            key("sort_by"),
            desc(app.sort_mode.label().to_string()),
        ]),
        {
            let selected = app.help_selected == 0;
            let bg = if selected {
                Color::DarkGray
            } else {
                Color::Reset
            };
            let indicator = if selected { "> " } else { "  " };
            Line::from(vec![
                Span::styled(
                    format!("{indicator}{:<18}", "search_scope"),
                    Style::new().fg(BRIGHT_WHITE).bg(bg),
                ),
                Span::styled(
                    format!("  {search_scope_label}"),
                    Style::new()
                        .fg(if selected { BRIGHT_WHITE } else { GRAY_400 })
                        .bg(bg),
                ),
            ])
        },
        {
            let selected = app.help_selected == 1;
            let bg = if selected {
                Color::DarkGray
            } else {
                Color::Reset
            };
            let indicator = if selected { "> " } else { "  " };
            Line::from(vec![
                Span::styled(
                    format!("{indicator}{:<18}", "summary_search_count"),
                    Style::new().fg(BRIGHT_WHITE).bg(bg),
                ),
                Span::styled(
                    format!("  {}", app.summary_search_count),
                    Style::new()
                        .fg(if selected { BRIGHT_WHITE } else { GRAY_400 })
                        .bg(bg),
                ),
            ])
        },
        Line::from(vec![]),
        section("── Config File ──────────────────────"),
        Line::from(vec![
            Span::styled("  ", Style::new()),
            Span::styled(config_path_str, Style::new().fg(GRAY_400)),
        ]),
    ];

    f.render_widget(Paragraph::new(lines), chunks[4]);

    f.render_widget(
        Paragraph::new(Span::styled(&sep, Style::new().fg(SEPARATOR))),
        chunks[6],
    );
    f.render_widget(
        Paragraph::new(Span::styled(
            " ↑↓ navigate │ enter/space toggle │ +/- adjust │ esc/q close",
            Style::new().fg(GRAY_500),
        )),
        chunks[7],
    );
}

fn truncate_str(s: &str, max_width: usize) -> String {
    use unicode_width::UnicodeWidthChar;

    // Collapse newlines/tabs into spaces so multi-line content never breaks a row.
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
        // Re-truncate leaving room for "..."
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
