mod action;
mod cache;
mod config;
mod delete;
mod error;
mod fuzzy;
mod list;
mod model;
mod plugin;
mod scanner;
mod settings;
mod shell;
mod stats;
mod tui;
mod watch;

use std::io::IsTerminal;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "agf",
    about = "AI Agent Session Finder TUI",
    args_conflicts_with_subcommands = true
)]
struct Cli {
    /// Optional query to pre-filter sessions
    query: Option<String>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Output shell wrapper function for the given shell
    Init {
        /// Shell type: zsh, bash, or fish
        shell: String,
    },
    /// Auto-detect shell and add agf to your shell config
    Setup,
    /// Fuzzy-match a session and resume it directly (no TUI)
    Resume {
        /// Fuzzy query to match a session (project name, path, summary)
        query: Vec<String>,
        /// Filter by agent name (e.g. claude, codex, gemini)
        #[arg(long)]
        agent: Option<String>,
        /// Show top N matches interactively instead of picking the best
        #[arg(long)]
        list: Option<usize>,
        /// Permission/approval mode (e.g. acceptEdits, yolo, full-auto)
        #[arg(long)]
        mode: Option<String>,
    },
    /// List sessions as plain text (for scripting)
    List {
        /// Filter by agent name (e.g. claude, codex, gemini)
        #[arg(long)]
        agent: Option<String>,
        /// Maximum number of sessions to show
        #[arg(long, default_value = "20")]
        limit: usize,
        /// Output format: table, json, csv
        #[arg(long, default_value = "table")]
        format: String,
    },
    /// Show session statistics
    Stats {
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Live dashboard showing agent sessions with auto-refresh
    Watch {
        /// Refresh interval in seconds
        #[arg(long, default_value = "5")]
        interval: u64,
    },
}

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() -> anyhow::Result<()> {
    // Handle --version / -V manually (clap hides it due to args_conflicts_with_subcommands)
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("agf {VERSION}");
        return Ok(());
    }

    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Init { shell }) => {
            print!("{}", shell::shell_init(&shell));
            return Ok(());
        }
        Some(Commands::Setup) => {
            shell::setup()?;
            return Ok(());
        }
        Some(Commands::Resume {
            query,
            agent,
            list: list_count,
            mode,
        }) => {
            let query = query.join(" ");
            let mut sessions = scanner::scan_all();
            if let Some(ref agent_name) = agent {
                sessions = list::filter_by_agent(sessions, agent_name);
            }
            let mut fuzzy = fuzzy::FuzzyMatcher::new();
            let all_indices: Vec<usize> = (0..sessions.len()).collect();
            let results = fuzzy.filter(&sessions, &all_indices, &query, 5, false);

            if results.is_empty() {
                eprintln!("No session matching '{query}'");
                std::process::exit(1);
            }

            let chosen = if let Some(n) = list_count {
                // Interactive: show top N and let user pick
                let top_n = results.iter().take(n).collect::<Vec<_>>();
                for (i, r) in top_n.iter().enumerate() {
                    let s = &sessions[all_indices[r.index]];
                    eprintln!(
                        "  {}) {} | {} | {}",
                        i + 1,
                        s.agent,
                        s.project_name,
                        s.time_display()
                    );
                }
                eprint!("Select [1-{}]: ", top_n.len());
                let mut input = String::new();
                std::io::stdin().read_line(&mut input)?;
                let pick: usize = input.trim().parse().unwrap_or(1);
                let idx = pick.saturating_sub(1).min(top_n.len() - 1);
                &sessions[all_indices[top_n[idx].index]]
            } else {
                &sessions[all_indices[results[0].index]]
            };

            // Build resume command with optional mode flags
            let flags = mode
                .as_deref()
                .and_then(|m| {
                    chosen
                        .agent
                        .resume_mode_options()
                        .iter()
                        .find(|(label, _)| label.to_lowercase().contains(&m.to_lowercase()))
                        .map(|(_, f)| *f)
                })
                .unwrap_or("");

            let cmd = action::resume_with_flags(chosen, flags);
            return deliver_command(&cmd);
        }
        Some(Commands::List {
            agent,
            limit,
            format,
        }) => {
            let mut sessions = scanner::scan_all();
            if let Some(ref agent_name) = agent {
                sessions = list::filter_by_agent(sessions, agent_name);
            }
            sessions.truncate(limit);
            if sessions.is_empty() {
                eprintln!("No sessions found.");
                std::process::exit(1);
            }
            list::list_sessions(&sessions, list::OutputFormat::parse(&format));
            return Ok(());
        }
        Some(Commands::Stats { json }) => {
            let sessions = scanner::scan_all();
            stats::print_stats(&sessions, json);
            return Ok(());
        }
        Some(Commands::Watch { interval }) => {
            watch::run_watch(interval)?;
            return Ok(());
        }
        None => {}
    }

    let config = settings::Settings::load();

    // Enter alt-screen BEFORE scanning so the user doesn't see their shell prompt
    // during the first-run scan (which can take 200ms-3s). The guard is scoped so
    // it drops before `deliver_command()` runs — that path may `exec sh -c` and
    // inherits terminal state.
    let cmd_opt = {
        let guard = AltScreenGuard::new();

        // Use cache for faster TUI startup
        let (mut sessions, stale_agents) = cache::load_cache();
        if !stale_agents.is_empty() {
            cache::scan_stale_agents(&stale_agents, &mut sessions);
            cache::write_cache(&sessions);
        }

        // Apply max_sessions limit from config
        if let Some(max) = config.max_sessions {
            sessions.truncate(max);
        }

        if sessions.is_empty() {
            // Drop guard early so the message lands on the real terminal.
            drop(guard);
            eprintln!("No agent sessions found.");
            return Ok(());
        }

        // Keep the guard alive for the full TUI session.
        let _guard = guard;

        let cwd = std::env::current_dir()
            .ok()
            .and_then(|p| p.to_str().map(|s| s.to_string()));
        let include_summaries = config.search_scope == "all";
        let mut app = tui::App::new(
            sessions,
            cli.query,
            config.summary_search_count,
            include_summaries,
            cwd,
            config.pinned_sessions.clone(),
            config.clone(),
        );

        // Apply sort_by from config
        if let Some(ref sort_by) = config.sort_by {
            app.sort_mode = match sort_by.as_str() {
                "name" => model::SortMode::Name,
                "agent" => model::SortMode::Agent,
                _ => model::SortMode::Time,
            };
            app.apply_sort();
        }
        app.run()?
    };

    if let Some(cmd) = cmd_opt {
        deliver_command(&cmd)?;
    }

    Ok(())
}

/// RAII guard that enters the alternate screen and hides the cursor on
/// construction, and restores both on drop. Uses raw ANSI escape codes to
/// avoid taking a direct dependency on crossterm.
///
/// Safe to nest: SLT's `run_with` also enters the alt-screen via the same
/// VT control sequence; entering twice is idempotent at the terminal level.
struct AltScreenGuard;

impl AltScreenGuard {
    fn new() -> Self {
        use std::io::Write;
        let mut out = std::io::stdout();
        // ESC[?1049h  enter alt-screen buffer
        // ESC[?25l    hide cursor
        let _ = out.write_all(b"\x1b[?1049h\x1b[?25l");
        let _ = out.flush();
        Self
    }
}

impl Drop for AltScreenGuard {
    fn drop(&mut self) {
        use std::io::Write;
        let mut out = std::io::stdout();
        // ESC[?1049l  leave alt-screen buffer
        // ESC[?25h    show cursor
        let _ = out.write_all(b"\x1b[?1049l\x1b[?25h");
        let _ = out.flush();
    }
}

/// Deliver a generated shell command to the parent context.
///
/// Priority:
/// 1. `AGF_CMD_FILE` set  → write to file (shell wrapper eval path; normal install).
/// 2. Interactive TTY     → exec the command via `sh -c` so Resume / New Session /
///    Open runs immediately in the current terminal without requiring the user
///    to copy-paste a printed command.
/// 3. Non-interactive     → print to stdout (scripting-friendly fallback).
///
/// A command whose only effect is `cd` (no ` && `) needs shell integration to
/// persist in the parent shell. We warn and still print the command so the
/// user sees something actionable.
fn deliver_command(cmd: &str) -> anyhow::Result<()> {
    if let Ok(file) = std::env::var("AGF_CMD_FILE") {
        std::fs::write(&file, cmd)?;
        return Ok(());
    }

    let is_cd_only = !cmd.contains(" && ");

    if is_cd_only {
        eprintln!("⚠  Shell integration not active — `cd` won't persist in your shell.");
        eprintln!("   Run `agf setup` to install the wrapper, then restart your shell.");
        println!("{cmd}");
        return Ok(());
    }

    if std::io::stdout().is_terminal() {
        return exec_via_shell(cmd);
    }

    // Piped / redirected: preserve the printable contract so callers can capture output.
    println!("{cmd}");
    Ok(())
}

#[cfg(unix)]
fn exec_via_shell(cmd: &str) -> anyhow::Result<()> {
    use std::os::unix::process::CommandExt;
    let err = std::process::Command::new("sh").arg("-c").arg(cmd).exec();
    // `exec` only returns on failure.
    Err(anyhow::anyhow!("failed to exec shell: {err}"))
}

#[cfg(not(unix))]
fn exec_via_shell(cmd: &str) -> anyhow::Result<()> {
    let status = std::process::Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .status()?;
    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}
