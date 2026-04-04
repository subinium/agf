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
            let results = fuzzy.filter(&sessions, &query, 5, false);

            if results.is_empty() {
                eprintln!("No session matching '{query}'");
                std::process::exit(1);
            }

            let chosen = if let Some(n) = list_count {
                // Interactive: show top N and let user pick
                let top_n = results.iter().take(n).collect::<Vec<_>>();
                for (i, r) in top_n.iter().enumerate() {
                    let s = &sessions[r.index];
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
                &sessions[top_n[idx].index]
            } else {
                &sessions[results[0].index]
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
            if let Ok(file) = std::env::var("AGF_CMD_FILE") {
                std::fs::write(&file, &cmd)?;
            } else {
                println!("{cmd}");
            }
            return Ok(());
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
        eprintln!("No agent sessions found.");
        return Ok(());
    }

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
    if let Some(cmd) = app.run()? {
        // Write command to AGF_CMD_FILE (temp file) — the shell wrapper evals it
        if let Ok(file) = std::env::var("AGF_CMD_FILE") {
            std::fs::write(&file, &cmd)?;
        } else {
            // Fallback for direct invocation (e.g., `agf resume`)
            println!("{cmd}");
        }
    }

    Ok(())
}
