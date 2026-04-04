use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::model::{Agent, Session};
use crate::scanner;

struct WatchState {
    sessions: Vec<Session>,
    running_agents: Vec<Agent>,
    last_refresh: Instant,
    selected: usize,
    scroll_offset: usize,
}

pub fn run_watch(interval_secs: u64) -> anyhow::Result<()> {
    let sessions = scanner::scan_all();
    let running_agents = detect_running_agents();

    let mut state = WatchState {
        sessions,
        running_agents,
        last_refresh: Instant::now(),
        selected: 0,
        scroll_offset: 0,
    };

    let (tx, rx) = mpsc::channel::<(Vec<Session>, Vec<Agent>)>();

    slt::run_with(
        slt::RunConfig::default().title("agf watch").mouse(true),
        |ui: &mut slt::Context| {
            // Check for background refresh results
            if let Ok((new_sessions, new_running)) = rx.try_recv() {
                state.sessions = new_sessions;
                state.running_agents = new_running;
                state.last_refresh = Instant::now();
            }

            // Trigger background refresh
            if state.last_refresh.elapsed() >= Duration::from_secs(interval_secs) {
                state.last_refresh = Instant::now();
                let tx = tx.clone();
                std::thread::spawn(move || {
                    let sessions = scanner::scan_all();
                    let running = detect_running_agents();
                    let _ = tx.send((sessions, running));
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
            let viewport = (ui.height() as usize).saturating_sub(6).max(1);
            if state.selected < state.scroll_offset {
                state.scroll_offset = state.selected;
            } else if state.selected >= state.scroll_offset + viewport {
                state.scroll_offset = state.selected - viewport + 1;
            }

            // Render
            let running_names: Vec<String> =
                state.running_agents.iter().map(|a| a.to_string()).collect();
            let elapsed = state.last_refresh.elapsed().as_secs();

            let _ = ui.col(|ui| {
                ui.text("");
                let _ = ui.container().pl(2).pr(1).row(|ui| {
                    ui.text("agf watch")
                        .fg(slt::Color::Rgb(229, 229, 229))
                        .bold();
                    ui.spacer();
                    if running_names.is_empty() {
                        ui.text("no agents running")
                            .fg(slt::Color::Rgb(107, 114, 128));
                    } else {
                        ui.text(format!("running: {}", running_names.join(", ")))
                            .fg(slt::Color::Rgb(52, 211, 153));
                    }
                    ui.text(format!("  {elapsed}s ago"))
                        .fg(slt::Color::Rgb(107, 114, 128));
                });
                ui.separator_colored(slt::Color::Rgb(64, 64, 64));

                let _ = ui.container().grow(1).pr(1).col(|ui| {
                    if state.sessions.is_empty() {
                        let _ = ui.container().pl(2).col(|ui| {
                            let _ = ui.empty_state("No sessions", "Waiting for agent sessions...");
                        });
                        return;
                    }

                    let end = (state.scroll_offset + viewport).min(state.sessions.len());
                    for vi in state.scroll_offset..end {
                        let s = &state.sessions[vi];
                        let is_selected = vi == state.selected;
                        let bg = if is_selected {
                            slt::Color::Rgb(59, 59, 59)
                        } else {
                            slt::Color::Reset
                        };

                        let is_running = state.running_agents.contains(&s.agent);
                        let status = if is_running {
                            ("\u{25cf} ", slt::Color::Rgb(52, 211, 153))
                        } else {
                            ("\u{25cb} ", slt::Color::Rgb(107, 114, 128))
                        };

                        let (r, g, b) = s.agent.color();
                        let agent_color = slt::Color::Rgb(r, g, b);

                        let _ = ui.row(|ui| {
                            ui.styled(status.0.to_string(), slt::Style::new().fg(status.1).bg(bg));
                            ui.styled(
                                format!("{:<14}", s.agent.to_string()),
                                slt::Style::new().fg(agent_color).bold().bg(bg),
                            );
                            ui.styled(
                                format!("{:<20}", truncate(&s.project_name, 20)),
                                slt::Style::new().fg(slt::Color::Rgb(229, 229, 229)).bg(bg),
                            );
                            if let Some(branch) = &s.git_branch {
                                ui.styled(
                                    format!("  {branch}"),
                                    slt::Style::new().fg(slt::Color::Rgb(52, 211, 153)).bg(bg),
                                );
                            }
                            ui.styled(
                                format!("  {}", s.time_display()),
                                slt::Style::new().fg(slt::Color::Rgb(107, 114, 128)).bg(bg),
                            );
                        });
                    }
                });

                ui.separator_colored(slt::Color::Rgb(64, 64, 64));
                let _ = ui.container().pr(1).row(|ui| {
                    ui.spacer();
                    let _ = ui.help_colored(
                        &[("↑↓", "nav"), ("q/Esc", "quit")],
                        slt::Color::Rgb(107, 114, 128),
                        slt::Color::Rgb(64, 64, 64),
                    );
                });
            });
        },
    )?;
    Ok(())
}

fn detect_running_agents() -> Vec<Agent> {
    Agent::all()
        .iter()
        .copied()
        .filter(|agent| {
            std::process::Command::new("pgrep")
                .args(["-f", agent.cli_name()])
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        })
        .collect()
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max.saturating_sub(1)])
    }
}
