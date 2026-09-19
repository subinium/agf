use std::io::IsTerminal;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use crate::model::{Agent, Session};
use crate::scanner;
use crate::text;
use crate::tui::palette::Palette;

struct WatchState {
    sessions: Vec<Session>,
    running_agents: RunningAgents,
    last_refresh: Instant,
    selected: usize,
    scroll_offset: usize,
}

type ScanBatch = Vec<(Agent, Result<crate::scanner::CompletedScan, String>)>;

#[derive(Debug, PartialEq, Eq)]
struct RunningAgents(Option<Vec<Agent>>);

impl RunningAgents {
    fn label(&self) -> String {
        match &self.0 {
            None => "running status unknown".to_string(),
            Some(agents) if agents.is_empty() => "no agents running".to_string(),
            Some(agents) => format!(
                "running: {}",
                agents
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }

    fn is_running(&self, agent: Agent) -> Option<bool> {
        self.0.as_ref().map(|agents| agents.contains(&agent))
    }
}

fn cacheable_scan_batch(
    batch: &ScanBatch,
) -> (
    Vec<Session>,
    std::collections::HashMap<Agent, crate::cache::SourceFingerprint>,
) {
    let mut sessions = Vec::new();
    let mut fingerprints = std::collections::HashMap::new();
    for (agent, result) in batch {
        if let Ok(scan) = result
            && let Some(fingerprint) = &scan.fingerprint
            && fingerprint.is_complete()
        {
            // Persist the full worker result before applying display filters,
            // including hidden Codex subagent/exec rows.
            sessions.extend(scan.sessions.iter().cloned());
            fingerprints.insert(*agent, fingerprint.clone());
        }
    }
    (sessions, fingerprints)
}

fn refresh() -> (ScanBatch, RunningAgents) {
    let batch = scanner::scan_agents_detailed(&crate::config::installed_agents());
    let (sessions, fingerprints) = cacheable_scan_batch(&batch);
    if !fingerprints.is_empty() {
        crate::cache::write_cache(
            &sessions,
            &std::collections::HashSet::new(),
            &std::collections::HashSet::new(),
            &fingerprints,
        );
    }
    (batch, detect_running_agents())
}

/// Replace only agents whose scanner completed successfully. A transient
/// permission/SQLite error must not erase stale-but-useful rows in watch mode.
fn merge_scan_batch(sessions: &mut Vec<Session>, batch: ScanBatch, include_non_interactive: bool) {
    for (agent, result) in batch {
        match result {
            Ok(scan) => {
                let mut new_sessions = scan.sessions;
                if !include_non_interactive {
                    new_sessions.retain(|session| session.interactive);
                }
                sessions.retain(|session| session.agent != agent);
                sessions.extend(new_sessions);
            }
            Err(error) => {
                if std::env::var("AGF_DEBUG").is_ok() {
                    eprintln!("[agf] {agent} watch refresh failed: {error}");
                }
            }
        }
    }
    sessions.sort_by(|a, b| crate::model::compare_sessions(a, b, crate::model::SortMode::Time));
}

pub fn run_watch(interval_secs: u64, include_non_interactive: bool) -> anyhow::Result<()> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Err(anyhow::anyhow!(
            "watch requires terminal stdin and stdout; use `agf list` for pipelines"
        ));
    }
    // Paint stale cache immediately; filesystem/SQLite scans and process
    // probes run off the render thread even on the first frame.
    let settings = crate::settings::Settings::load();
    let include_non_interactive = include_non_interactive || settings.include_non_interactive;
    let (mut sessions, _) = crate::cache::load_cache();
    if !include_non_interactive {
        sessions.retain(|session| session.interactive);
    }

    let mut state = WatchState {
        sessions,
        running_agents: RunningAgents(None),
        last_refresh: Instant::now(),
        selected: 0,
        scroll_offset: 0,
    };

    let (tx, rx) = mpsc::channel::<(ScanBatch, RunningAgents)>();
    let refreshing = Arc::new(AtomicBool::new(true));
    {
        let tx = tx.clone();
        let refreshing = Arc::clone(&refreshing);
        std::thread::spawn(move || {
            let _ = tx.send(refresh());
            refreshing.store(false, Ordering::SeqCst);
        });
    }

    let depth = slt::ColorDepth::detect();
    let theme = watch_theme(
        settings.appearance,
        std::env::var("COLORFGBG").ok().as_deref(),
    );
    slt::run_with(
        slt::RunConfig::default()
            .title("agf watch")
            .mouse(true)
            .color_depth(depth)
            .theme(theme),
        |ui: &mut slt::Context| {
            // Check for background refresh results
            if let Ok((new_sessions, new_running)) = rx.try_recv() {
                let selected_identity = state.sessions.get(state.selected).map(Session::identity);
                merge_scan_batch(&mut state.sessions, new_sessions, include_non_interactive);
                state.running_agents = new_running;
                // Clamp the cursor: a refresh can shrink the list (sessions
                // deleted elsewhere), and a stale `selected` past the end
                // would push the scroll offset past the list and blank the
                // viewport until the user pressed Up repeatedly.
                state.selected = selected_identity
                    .and_then(|identity| {
                        state
                            .sessions
                            .iter()
                            .position(|session| session.identity() == identity)
                    })
                    .unwrap_or_else(|| state.selected.min(state.sessions.len().saturating_sub(1)));
                state.last_refresh = Instant::now();
            }

            // Trigger background refresh (guard against overlapping scans)
            if state.last_refresh.elapsed() >= Duration::from_secs(interval_secs)
                && !refreshing.swap(true, Ordering::SeqCst)
            {
                state.last_refresh = Instant::now();
                let tx = tx.clone();
                let r = Arc::clone(&refreshing);
                std::thread::spawn(move || {
                    let _ = tx.send(refresh());
                    r.store(false, Ordering::SeqCst);
                });
            }

            // Input
            if ui.key_code(slt::KeyCode::Esc) || ui.key('q') {
                ui.quit();
            }
            if (ui.key_code(slt::KeyCode::Up) || ui.key_mod('k', slt::KeyModifiers::CONTROL))
                && state.selected > 0
            {
                state.selected -= 1;
            }
            if (ui.key_code(slt::KeyCode::Down) || ui.key_mod('j', slt::KeyModifiers::CONTROL))
                && state.selected + 1 < state.sessions.len()
            {
                state.selected += 1;
            }

            // Scroll
            let viewport = watch_viewport(ui.height()).max(1) as usize;
            let margin = 3usize.min(viewport.saturating_sub(1));
            if state.selected < state.scroll_offset {
                state.scroll_offset = state.selected;
            } else if state.selected >= state.scroll_offset + viewport.saturating_sub(margin) {
                state.scroll_offset = (state.selected + margin + 1).saturating_sub(viewport);
            }
            let max_offset = state.sessions.len().saturating_sub(viewport);
            if state.scroll_offset > max_offset {
                state.scroll_offset = max_offset;
            }

            // Render
            let elapsed = state.last_refresh.elapsed().as_secs();
            if depth == slt::ColorDepth::Basic {
                ui.provide(depth, |ui| render_watch(ui, &state, elapsed));
            } else {
                render_watch(ui, &state, elapsed);
            }
        },
    )?;
    Ok(())
}

fn watch_theme(appearance: crate::settings::Appearance, colorfgbg: Option<&str>) -> slt::Theme {
    match appearance {
        crate::settings::Appearance::Auto => crate::tui::terminal_theme(colorfgbg),
        crate::settings::Appearance::Dark => slt::Theme::dark(),
        crate::settings::Appearance::Light => slt::Theme::light(),
    }
}

fn watch_viewport(height: u32) -> u32 {
    height.saturating_sub(if height >= 4 { 4 } else { 2 })
}

fn render_watch(ui: &mut slt::Context, state: &WatchState, elapsed: u64) {
    let palette = Palette::from_ui(ui);
    let (width, height) = (ui.width(), ui.height());
    if width == 0 || height == 0 {
        return;
    }
    let viewport = watch_viewport(height);
    let _ = ui
        .container()
        .w(width)
        .h(height)
        .text_color(palette.text)
        .bg(palette.background)
        .col(|ui| {
            if height >= 2 {
                render_watch_header(ui, state.running_agents.label(), elapsed, palette);
            }
            if height >= 4 {
                let _ = ui.separator_colored(palette.border);
            }
            if viewport > 0 {
                let _ = ui.container().h(viewport).min_h(viewport).col(|ui| {
                    if state.sessions.is_empty() {
                        ui.text(text::truncate("No sessions", width as usize))
                            .fg(palette.text);
                        if viewport > 1 {
                            ui.text(text::truncate(
                                "Waiting for agent sessions...",
                                width as usize,
                            ))
                            .fg(palette.muted);
                        }
                        return;
                    }
                    let end = (state.scroll_offset + viewport as usize).min(state.sessions.len());
                    for vi in state.scroll_offset..end {
                        let session = &state.sessions[vi];
                        render_watch_row(
                            ui,
                            session,
                            state.running_agents.is_running(session.agent),
                            vi == state.selected,
                            palette,
                        );
                    }
                });
            }
            if height >= 4 {
                let _ = ui.separator_colored(palette.border);
            }
            crate::tui::render_footer(ui, &[("Up/Down", "Move"), ("q", "Quit"), ("Esc", "Quit")]);
        });
}

fn render_watch_header(ui: &mut slt::Context, running: String, elapsed: u64, palette: Palette) {
    ui.container().h(1).min_h(1).draw(move |buffer, rect| {
        if rect.height == 0 {
            return;
        }
        let width = rect.width as usize;
        let title = text::truncate("agf watch", width);
        let style = slt::Style::new().bg(palette.background);
        buffer.set_string(rect.x, rect.y, &title, style.fg(palette.text).bold());
        let title_width = text::width(&title);
        let remaining = width.saturating_sub(title_width + 2);
        let time = format!("  {elapsed}s ago");
        let time_width = if text::width(&time) <= remaining {
            text::width(&time)
        } else {
            0
        };
        let status = text::truncate(&running, remaining.saturating_sub(time_width));
        if !status.is_empty() {
            buffer.set_string(
                rect.x + title_width as u32 + 2,
                rect.y,
                &status,
                style.fg(palette.secondary),
            );
        }
        if time_width > 0 {
            buffer.set_string(
                rect.x + (width - time_width) as u32,
                rect.y,
                &time,
                style.fg(palette.muted),
            );
        }
    });
}

fn render_watch_row(
    ui: &mut slt::Context,
    session: &Session,
    running: Option<bool>,
    selected: bool,
    palette: Palette,
) {
    let agent = session.agent;
    let label = agent.to_string();
    let project = text::sanitize_terminal(&session.project_name);
    let branch = session
        .git_branch
        .as_deref()
        .map(text::sanitize_terminal)
        .unwrap_or_default();
    let time = format!("  {}", session.time_display());
    ui.container().h(1).min_h(1).draw(move |buffer, rect| {
        if rect.height == 0 {
            return;
        }
        let width = rect.width as usize;
        let background = if selected {
            palette.selection_bg
        } else {
            palette.background
        };
        let style = slt::Style::new()
            .fg(palette.row_text(selected))
            .bg(background);
        let metadata = style.fg(palette.row_muted(selected));
        buffer.set_string(rect.x, rect.y, &" ".repeat(width), style);
        let marker_width = width.min(2);
        buffer.set_string(
            rect.x,
            rect.y,
            &text::fit(if selected { ">" } else { "" }, marker_width),
            style.fg(palette.marker(selected)).bold(),
        );
        let status_width = width.saturating_sub(marker_width).min(2);
        let (status, color) = match running {
            Some(true) => ("\u{25cf}", palette.success),
            Some(false) => ("\u{25cb}", palette.row_muted(selected)),
            None => ("?", palette.row_muted(selected)),
        };
        buffer.set_string(
            rect.x + marker_width as u32,
            rect.y,
            &text::truncate(status, status_width),
            style.fg(color),
        );
        let agent_x = marker_width + status_width;
        let agent_width = ((width - agent_x) / 2).min(14);
        // Only the brand name carries its hue, never padding or metadata.
        buffer.set_string(
            rect.x + agent_x as u32,
            rect.y,
            &text::truncate(&label, agent_width.saturating_sub(1)),
            style.fg(palette.agent(agent)),
        );
        let project_x = agent_x + agent_width;
        let remaining = width - project_x;
        let time_width = if remaining >= text::width(&time) + 8 {
            text::width(&time)
        } else {
            0
        };
        let project_width = (remaining - time_width).min(20);
        buffer.set_string(
            rect.x + project_x as u32,
            rect.y,
            &text::fit(&project, project_width),
            style,
        );
        let branch_x = project_x + project_width;
        let branch_width = width - branch_x - time_width;
        if !branch.is_empty() && branch_width > 2 {
            buffer.set_string(
                rect.x + branch_x as u32 + 2,
                rect.y,
                &text::truncate(&branch, branch_width - 2),
                metadata,
            );
        }
        if time_width > 0 {
            buffer.set_string(
                rect.x + (width - time_width) as u32,
                rect.y,
                &time,
                metadata,
            );
        }
    });
}

/// Which agent CLIs currently have a running process.
///
/// One `pgrep` spawn per candidate, so the candidate list is the *installed*
/// agents rather than every agent `agf` knows about — probing for a CLI that
/// isn't on this machine can never succeed. `output()` (not `status()`) is
/// required: the child's stdout must be captured, or it would paint over the
/// TUI. Note this runs on the refresh worker thread, not the render path.
fn detect_running_agents() -> RunningAgents {
    #[cfg(windows)]
    return RunningAgents(None);

    #[cfg(not(windows))]
    {
        static PGREP_AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if !*PGREP_AVAILABLE.get_or_init(|| {
            std::process::Command::new("pgrep")
                .arg("--version")
                .output()
                .is_ok()
        }) {
            return RunningAgents(None);
        }
        probe_running_agents(&crate::config::installed_agents(), |agent| {
            std::process::Command::new("pgrep")
                .args(["-x", agent.cli_name()])
                .output()
                .map(|output| output.status.code())
        })
    }
}

#[cfg(any(test, not(windows)))]
fn probe_running_agents(
    agents: &[Agent],
    mut probe: impl FnMut(Agent) -> std::io::Result<Option<i32>>,
) -> RunningAgents {
    let mut running = Vec::new();
    for &agent in agents {
        match probe(agent) {
            Ok(Some(0)) => running.push(agent),
            Ok(Some(1)) => {}
            // Exit 1 means no matches; other exit codes, signals and spawn
            // failures do not establish that no agents are running.
            _ => return RunningAgents(None),
        }
    }
    RunningAgents(Some(running))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render_fixture(
        width: u32,
        height: u32,
        theme: slt::Theme,
        depth: slt::ColorDepth,
        state: &WatchState,
    ) -> (slt::TestBackend, Palette) {
        let mut backend = slt::TestBackend::new(width, height);
        let mut palette = Palette::dark();
        backend.render(|ui| {
            ui.set_theme(theme);
            let mut render = |ui: &mut slt::Context| {
                palette = Palette::from_ui(ui);
                render_watch(ui, state, 7);
            };
            if depth == slt::ColorDepth::Basic {
                ui.provide(depth, render);
            } else {
                render(ui);
            }
        });
        (backend, palette)
    }

    fn render_state() -> WatchState {
        let mut first = session(Agent::Codex, "first", 1);
        first.git_branch = Some("main".into());
        let mut second = session(Agent::ClaudeCode, "second", 1);
        second.git_branch = Some("feature".into());
        WatchState {
            sessions: vec![first, second],
            running_agents: RunningAgents(Some(vec![Agent::Codex])),
            last_refresh: Instant::now(),
            selected: 0,
            scroll_offset: 0,
        }
    }

    #[test]
    fn watch_appearance_honors_settings_and_terminal_hint() {
        use crate::settings::Appearance;
        for hint in [None, Some("invalid"), Some("15;0")] {
            assert!(watch_theme(Appearance::Auto, hint).is_dark);
        }
        assert!(!watch_theme(Appearance::Auto, Some("0;15")).is_dark);
        assert!(watch_theme(Appearance::Dark, Some("0;15")).is_dark);
        assert!(!watch_theme(Appearance::Light, Some("15;0")).is_dark);
    }

    #[test]
    fn watch_body_uses_readable_palette_roles_at_every_color_depth() {
        let state = render_state();
        for theme in [slt::Theme::dark(), slt::Theme::light()] {
            for depth in [
                slt::ColorDepth::TrueColor,
                slt::ColorDepth::EightBit,
                slt::ColorDepth::Basic,
                slt::ColorDepth::NoColor,
            ] {
                let (backend, palette) = render_fixture(80, 10, theme, depth, &state);
                backend.assert_line_contains(2, "> \u{25cf} Codex");
                backend.assert_line_contains(3, "  \u{25cb} Claude Code");
                for (index, session) in state.sessions.iter().enumerate() {
                    let y = index as u32 + 2;
                    let selected = index == state.selected;
                    let style_at = |x| backend.buffer().get(x, y).style;
                    assert_eq!(style_at(0).fg, Some(palette.marker(selected)));
                    assert!(style_at(0).modifiers.contains(slt::Modifiers::BOLD));
                    assert_eq!(style_at(4).fg, Some(palette.agent(session.agent)));
                    assert!(!style_at(4).modifiers.contains(slt::Modifiers::BOLD));
                    assert_eq!(style_at(17).fg, Some(palette.row_text(selected)));
                    assert_eq!(style_at(18).fg, Some(palette.row_text(selected)));
                    assert_eq!(style_at(40).fg, Some(palette.row_muted(selected)));
                    assert_eq!(style_at(79).fg, Some(palette.row_muted(selected)));
                    assert_eq!(
                        style_at(2).fg,
                        Some(if selected {
                            palette.success
                        } else {
                            palette.row_muted(false)
                        })
                    );
                    for x in [0, 2, 4, 18, 40, 79] {
                        let style = style_at(x);
                        let foreground = style.fg.unwrap().downsampled(depth);
                        let background = style.bg.unwrap().downsampled(depth);
                        if depth == slt::ColorDepth::NoColor {
                            assert_eq!(foreground, slt::Color::Reset);
                            assert_eq!(background, slt::Color::Reset);
                        } else {
                            assert!(
                                slt::Color::contrast_ratio_f64(foreground, background) >= 4.5,
                                "{depth:?}: {foreground:?} on {background:?} at ({x}, {y})"
                            );
                        }
                    }
                }
                assert_eq!(backend.buffer().get(0, 0).style.fg, Some(palette.text));
                assert_eq!(
                    backend.buffer().get(11, 0).style.fg,
                    Some(palette.secondary)
                );
                assert_eq!(backend.buffer().get(79, 0).style.fg, Some(palette.muted));
                for y in 0..10 {
                    for x in 0..80 {
                        assert_eq!(
                            backend.buffer().get(x, y).style.bg,
                            Some(if y == 2 {
                                palette.selection_bg
                            } else {
                                palette.background
                            }),
                            "background at ({x}, {y})"
                        );
                    }
                }
                if depth == slt::ColorDepth::Basic {
                    let (foreground, background) = if theme.is_dark {
                        (slt::Color::White, slt::Color::Black)
                    } else {
                        (slt::Color::Black, slt::Color::White)
                    };
                    assert_eq!(palette.text, foreground);
                    assert_eq!(palette.background, background);
                    for y in 0..10 {
                        for x in 0..80 {
                            if let Some(color) = backend.buffer().get(x, y).style.fg {
                                assert_eq!(color, foreground);
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn watch_selection_is_independent_of_running_status_and_color() {
        for depth in [slt::ColorDepth::Basic, slt::ColorDepth::NoColor] {
            for running in [
                None,
                Some(vec![]),
                Some(vec![Agent::Codex, Agent::ClaudeCode]),
            ] {
                let mut state = render_state();
                state.running_agents = RunningAgents(running);
                for selected in 0..2 {
                    state.selected = selected;
                    let (backend, _) = render_fixture(20, 8, slt::Theme::dark(), depth, &state);
                    assert_eq!(backend.line(2).starts_with('>'), selected == 0);
                    assert_eq!(backend.line(3).starts_with('>'), selected == 1);
                    let symbol = match state.running_agents.is_running(Agent::Codex) {
                        Some(true) => '\u{25cf}',
                        Some(false) => '\u{25cb}',
                        None => '?',
                    };
                    assert_eq!(backend.line(2).chars().nth(2), Some(symbol));
                    assert_eq!(backend.line(3).chars().nth(2), Some(symbol));
                    backend.assert_line_contains(7, "Esc");
                    backend.assert_line_contains(2, "project");
                }
            }
        }
    }

    #[test]
    fn watch_frame_stays_bounded_with_long_metadata_and_tiny_viewports() {
        let mut state = render_state();
        state.sessions[0].project_name = "\u{d504}\u{b85c}\u{c81d}\u{d2b8}\n\tlong".repeat(20);
        state.sessions[0].git_branch = Some("feature/very-long-branch".repeat(20));
        for width in [1, 3, 5, 10, 20, 40, 80, 120] {
            for height in 1..=8 {
                for theme in [slt::Theme::dark(), slt::Theme::light()] {
                    let (backend, _) =
                        render_fixture(width, height, theme, slt::ColorDepth::Basic, &state);
                    for y in 0..height {
                        assert!(text::width(&backend.line(y)) <= width as usize);
                    }
                    if width >= 5 {
                        backend.assert_line_contains(height - 1, "Esc");
                    }
                    if width >= 20 && height >= 5 {
                        backend.assert_line_contains(2, "> \u{25cf} Codex");
                        backend.assert_line_not_contains(height - 1, "feature/");
                    }
                }
            }
        }
    }

    #[test]
    fn watch_empty_state_uses_body_roles_and_keeps_footer_visible() {
        let mut state = render_state();
        state.sessions.clear();
        for theme in [slt::Theme::dark(), slt::Theme::light()] {
            for depth in [slt::ColorDepth::TrueColor, slt::ColorDepth::Basic] {
                let (backend, palette) = render_fixture(20, 8, theme, depth, &state);
                backend.assert_line_contains(2, "No sessions");
                backend.assert_line_contains(3, "Waiting for");
                backend.assert_line_contains(7, "Esc");
                assert_eq!(backend.buffer().get(0, 2).style.fg, Some(palette.text));
                assert_eq!(backend.buffer().get(0, 3).style.fg, Some(palette.muted));
            }
        }
    }

    fn complete_fingerprint() -> crate::cache::SourceFingerprint {
        let mut value = serde_json::to_value(crate::cache::SourceFingerprint::default()).unwrap();
        value["complete"] = true.into();
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn process_probe_distinguishes_unknown_from_confirmed_empty() {
        let agents = [Agent::Codex, Agent::ClaudeCode];
        let empty = probe_running_agents(&agents, |_| Ok(Some(1)));
        assert_eq!(empty.label(), "no agents running");
        assert_eq!(empty.is_running(Agent::Codex), Some(false));
        for status in [None, Some(2), Some(3)] {
            assert_eq!(
                probe_running_agents(&agents, |_| Ok(status)),
                RunningAgents(None)
            );
        }
        let mut probes = 0;
        let missing = probe_running_agents(&agents, |_| {
            probes += 1;
            Err(std::io::ErrorKind::NotFound.into())
        });
        assert_eq!(probes, 1);
        assert_eq!(missing.label(), "running status unknown");
        assert_eq!(missing.is_running(Agent::Codex), None);
        assert_eq!(
            probe_running_agents(&agents, |_| Ok(Some(0))),
            RunningAgents(Some(agents.to_vec()))
        );
    }

    #[test]
    fn watch_cache_snapshot_preserves_hidden_rows_and_requires_complete_fingerprints() {
        let mut hidden = session(Agent::Codex, "hidden", 2);
        hidden.interactive = false;
        let batch = vec![
            (
                Agent::Codex,
                Ok(crate::scanner::CompletedScan {
                    sessions: vec![session(Agent::Codex, "visible", 1), hidden],
                    fingerprint: Some(complete_fingerprint()),
                }),
            ),
            (Agent::ClaudeCode, Err("scan failed".into())),
            (
                Agent::Pi,
                Ok(crate::scanner::CompletedScan {
                    sessions: vec![session(Agent::Pi, "changing", 3)],
                    fingerprint: None,
                }),
            ),
            (
                Agent::OhMyPi,
                Ok(crate::scanner::CompletedScan {
                    sessions: vec![session(Agent::OhMyPi, "unreadable", 4)],
                    fingerprint: Some(crate::cache::SourceFingerprint::default()),
                }),
            ),
        ];
        let (cached, fingerprints) = cacheable_scan_batch(&batch);
        assert_eq!(cached.len(), 2);
        assert!(cached.iter().any(|session| !session.interactive));
        assert_eq!(fingerprints.len(), 1);
        assert!(fingerprints.contains_key(&Agent::Codex));
        let mut display = Vec::new();
        merge_scan_batch(&mut display, batch, false);
        assert!(!display.iter().any(|session| session.session_id == "hidden"));
        assert!(cached.iter().any(|session| session.session_id == "hidden"));
    }

    fn session(agent: Agent, id: &str, timestamp: i64) -> Session {
        Session {
            agent,
            session_id: id.to_string(),
            project_name: "project".to_string(),
            project_path: "/tmp/project".to_string(),
            summaries: Vec::new(),
            timestamp,
            git_branch: None,
            worktree: None,
            recap: None,
            interactive: true,
        }
    }

    #[test]
    fn failed_watch_scan_preserves_stale_agent_rows() {
        let mut sessions = vec![
            session(Agent::ClaudeCode, "stale", 1),
            session(Agent::Codex, "old", 2),
        ];
        merge_scan_batch(
            &mut sessions,
            vec![
                (Agent::ClaudeCode, Err("locked".to_string())),
                (
                    Agent::Codex,
                    Ok(crate::scanner::CompletedScan {
                        sessions: vec![session(Agent::Codex, "fresh", 3)],
                        fingerprint: Some(crate::cache::SourceFingerprint::default()),
                    }),
                ),
            ],
            false,
        );

        assert!(sessions.iter().any(|session| session.session_id == "stale"));
        assert!(!sessions.iter().any(|session| session.session_id == "old"));
        assert!(sessions.iter().any(|session| session.session_id == "fresh"));
    }
}
