mod action;
mod config;
mod delete;
mod error;
mod fuzzy;
mod git;
mod model;
mod scanner;
mod settings;
mod shell;
mod tui;

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
    },
}

fn main() -> anyhow::Result<()> {
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
        Some(Commands::Resume { query }) => {
            let query = query.join(" ");
            let sessions = scanner::scan_all();
            let mut fuzzy = fuzzy::FuzzyMatcher::new();
            let results = fuzzy.filter(&sessions, &query, 5, false);

            if results.is_empty() {
                eprintln!("No session matching '{query}'");
                std::process::exit(1);
            }

            let best = &sessions[results[0].index];
            if let Some(cmd) = action::generate_command(best, model::Action::Resume, None) {
                println!("{cmd}");
            }
            return Ok(());
        }
        None => {}
    }

    let config = settings::Settings::load();
    let mut sessions = scanner::scan_all();

    // Apply max_sessions limit from config
    if let Some(max) = config.max_sessions {
        sessions.truncate(max);
    }

    if sessions.is_empty() {
        eprintln!("No agent sessions found.");
        return Ok(());
    }

    let include_summaries = config.search_scope == "all";
    let mut app = tui::App::new(
        sessions,
        cli.query,
        config.summary_search_count,
        include_summaries,
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
        // Print command to stdout — the shell wrapper evals it in the real terminal
        println!("{cmd}");
    }

    Ok(())
}
