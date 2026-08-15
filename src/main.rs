mod action;
mod cache;
mod config;
mod delete;
mod error;
mod fsx;
mod fuzzy;
mod list;
mod model;
mod scanner;
mod settings;
mod shell;
mod stats;
mod text;
mod tui;
mod watch;

use std::io::IsTerminal;
use std::io::Write;

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
        /// Shell type: zsh, bash, fish, or powershell (alias: pwsh)
        #[arg(value_parser = ["zsh", "bash", "fish", "powershell", "pwsh"])]
        shell: String,
    },
    /// Add agf to a shell config (auto-detected unless --shell is supplied)
    Setup {
        /// Shell type: zsh, bash, fish, or powershell
        #[arg(long)]
        shell: Option<String>,
    },
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
        /// Include Codex subagents and non-interactive exec sessions
        #[arg(long)]
        include_non_interactive: bool,
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
        /// Include Codex subagents and non-interactive exec sessions
        #[arg(long)]
        include_non_interactive: bool,
    },
    /// Show session statistics
    Stats {
        /// Output as JSON
        #[arg(long)]
        json: bool,
        /// Include Codex subagents and non-interactive exec sessions
        #[arg(long)]
        include_non_interactive: bool,
    },
    /// Live dashboard showing agent sessions with auto-refresh
    Watch {
        /// Refresh interval in seconds
        #[arg(long, default_value = "5", value_parser = clap::value_parser!(u64).range(1..))]
        interval: u64,
        /// Include Codex subagents and non-interactive exec sessions
        #[arg(long)]
        include_non_interactive: bool,
    },
}

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() -> anyhow::Result<()> {
    // Handle --version / -V manually (clap hides it due to args_conflicts_with_subcommands)
    if std::env::args().any(|a| a == "--version" || a == "-V") {
        write_stdout(format!("agf {VERSION}\n").as_bytes())?;
        return Ok(());
    }

    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Init { shell }) => {
            write_stdout(shell::shell_init(&shell).as_bytes())?;
            return Ok(());
        }
        Some(Commands::Setup { shell }) => {
            shell::setup(shell.as_deref())?;
            return Ok(());
        }
        Some(Commands::Resume {
            query,
            agent,
            list: list_count,
            mode,
            include_non_interactive,
        }) => {
            let query = query.join(" ");
            let sessions = scan_for_cli(agent.as_deref(), include_non_interactive)?;
            let mut fuzzy = fuzzy::FuzzyMatcher::new();
            let all_indices: Vec<usize> = (0..sessions.len()).collect();
            let results = fuzzy.filter(&sessions, &all_indices, &query, 5, true);

            if results.is_empty() {
                eprintln!("No session matching '{query}'");
                std::process::exit(1);
            }

            let chosen = if let Some(n) = list_count {
                if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
                    anyhow::bail!("resume --list requires an interactive terminal");
                }
                // Interactive: show top N and let user pick. `n.max(1)` guards
                // `--list 0`: an empty top_n would underflow `top_n.len() - 1`
                // below (results is already non-empty here).
                let top_n = results.iter().take(n.max(1)).collect::<Vec<_>>();
                for (i, r) in top_n.iter().enumerate() {
                    let s = &sessions[all_indices[r.index]];
                    eprintln!(
                        "  {}) {} | {} | {}",
                        i + 1,
                        s.agent,
                        text::sanitize_terminal(&s.project_name),
                        s.time_display()
                    );
                }
                eprint!("Select [1-{}]: ", top_n.len());
                let mut input = String::new();
                std::io::stdin().read_line(&mut input)?;
                let pick: usize = input
                    .trim()
                    .parse()
                    .map_err(|_| anyhow::anyhow!("invalid selection"))?;
                if !(1..=top_n.len()).contains(&pick) {
                    anyhow::bail!("selection must be between 1 and {}", top_n.len());
                }
                let idx = pick.saturating_sub(1).min(top_n.len() - 1);
                &sessions[all_indices[top_n[idx].index]]
            } else {
                &sessions[all_indices[results[0].index]]
            };

            // Build resume command with optional mode flags
            let flags = match mode.as_deref() {
                None => "",
                Some(requested) => chosen
                    .agent
                    .resume_mode_options()
                    .iter()
                    .find(|(label, _)| label.eq_ignore_ascii_case(requested))
                    .map(|(_, flags)| *flags)
                    .ok_or_else(|| {
                        let available = chosen
                            .agent
                            .resume_mode_options()
                            .iter()
                            .map(|(label, _)| *label)
                            .collect::<Vec<_>>()
                            .join(", ");
                        anyhow::anyhow!(
                            "unsupported mode {requested:?} for {}; expected one of: {available}",
                            chosen.agent
                        )
                    })?,
            };

            let cmd = action::resume_with_flags(chosen, flags);
            return deliver_command(&cmd);
        }
        Some(Commands::List {
            agent,
            limit,
            format,
            include_non_interactive,
        }) => {
            let mut sessions = scan_for_cli(agent.as_deref(), include_non_interactive)?;
            sessions.truncate(limit);
            if sessions.is_empty() {
                eprintln!("No sessions found.");
                std::process::exit(1);
            }
            list::list_sessions(&sessions, list::OutputFormat::parse(&format));
            return Ok(());
        }
        Some(Commands::Stats {
            json,
            include_non_interactive,
        }) => {
            let sessions = scan_for_cli(None, include_non_interactive)?;
            stats::print_stats(&sessions, json);
            return Ok(());
        }
        Some(Commands::Watch {
            interval,
            include_non_interactive,
        }) => {
            watch::run_watch(interval, include_non_interactive)?;
            return Ok(());
        }
        None => {}
    }

    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Err(anyhow::anyhow!(
            "interactive TUI requires terminal stdin and stdout; use `agf list` for pipelines"
        ));
    }

    let config = settings::Settings::load();

    let cmd_opt = {
        // Load whatever the cache has — even if stale, we open the TUI on it
        // immediately so cold starts feel instant. Stale agents refresh in
        // the background and stream their results into the running TUI.
        let (sessions, stale_agents) = cache::load_cache();

        // Cold cache + no installed agents: nothing we can ever scan.
        if sessions.is_empty() && stale_agents.is_empty() {
            eprintln!("No agent sessions found.");
            return Ok(());
        }

        let scan_rx = if stale_agents.is_empty() {
            None
        } else {
            Some(cache::start_stale_scan(&stale_agents))
        };
        let scanning_agents: std::collections::HashSet<model::Agent> =
            stale_agents.iter().copied().collect();

        let cwd = std::env::current_dir()
            .ok()
            .and_then(|p| p.to_str().map(str::to_owned));
        let include_summaries = config.search_scope == "all";
        let mut app = tui::App::new(
            sessions,
            cli.query,
            config.summary_search_count,
            include_summaries,
            cwd,
            config.pinned_sessions.clone(),
            config.clone(),
            scan_rx,
            scanning_agents,
        );

        // Apply sort from config (default: time). Always call apply_sort()
        // so the initial render is ordered — without this, agents loaded
        // from the cache appear in the order they were grouped on disk
        // (per-agent buckets) rather than by timestamp, so a session from
        // 11 minutes ago can land below sessions from a month ago on the
        // first frame.
        app.sort_mode = match config.sort_by.as_deref() {
            Some("name") => model::SortMode::Name,
            Some("agent") => model::SortMode::Agent,
            _ => model::SortMode::Time,
        };
        app.apply_sort();
        let result = app.run()?;
        // Persist whatever sessions accumulated during the TUI lifetime so
        // the next launch reflects all scans that completed before exit.
        // Agents still scanning at exit keep their prior cache entry so we
        // don't accidentally persist "empty + fresh-mtime" for them.
        let cache_skip_agents = app.cache_skip_agents();
        let cache_invalidate_agents = app.cache_invalidate_agents();
        cache::write_cache(
            &app.sessions,
            &cache_skip_agents,
            &cache_invalidate_agents,
            &app.scan_fingerprints,
        );
        result
    };

    if let Some(cmd) = cmd_opt {
        deliver_command(&cmd)?;
    }

    Ok(())
}

fn write_stdout(content: &[u8]) -> anyhow::Result<()> {
    match std::io::stdout().lock().write_all(content) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn scan_for_cli(
    agent_name: Option<&str>,
    include_non_interactive: bool,
) -> anyhow::Result<Vec<model::Session>> {
    let include_non_interactive =
        include_non_interactive || settings::Settings::load().include_non_interactive;
    let requested = agent_name
        .map(|name| {
            model::Agent::parse(name).ok_or_else(|| anyhow::anyhow!("unknown agent {name:?}"))
        })
        .transpose()?;
    let (mut sessions, stale) = cache::load_cache();
    let agents_to_scan = match requested {
        Some(agent) if stale.contains(&agent) || !config::is_agent_installed(agent) => vec![agent],
        Some(_) => Vec::new(),
        None => stale,
    };
    if !agents_to_scan.is_empty() {
        let mut succeeded = std::collections::HashSet::new();
        let mut observed_fingerprints = std::collections::HashMap::new();
        let mut failures = Vec::new();
        for (agent, result) in scanner::scan_agents_detailed(&agents_to_scan) {
            match result {
                Ok(scan) => {
                    sessions.retain(|session| session.agent != agent);
                    sessions.extend(scan.sessions);
                    if let Some(fingerprint) = scan.fingerprint {
                        observed_fingerprints.insert(agent, fingerprint);
                    }
                    succeeded.insert(agent);
                }
                Err(error) => failures.push((agent, error)),
            }
        }
        if let Some(agent) = requested
            && let Some((_, error)) = failures.iter().find(|(failed, _)| *failed == agent)
        {
            return Err(anyhow::anyhow!("{agent} scanner failed: {error}"));
        }
        for (agent, error) in &failures {
            eprintln!("[agf] {agent} scanner failed; keeping cached rows: {error}");
        }
        sessions.sort_by(|a, b| model::compare_sessions(a, b, model::SortMode::Time));

        let skip_agents: std::collections::HashSet<_> = config::installed_agents()
            .into_iter()
            .filter(|agent| !succeeded.contains(agent))
            .collect();
        cache::write_cache(
            &sessions,
            &skip_agents,
            &std::collections::HashSet::new(),
            &observed_fingerprints,
        );
    }
    if let Some(agent) = requested {
        sessions.retain(|session| session.agent == agent);
    }
    if !include_non_interactive {
        sessions.retain(|session| session.interactive);
    }
    Ok(sessions)
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

    let shell = shell::CommandShell::from_env();
    if shell.is_cd_only(cmd) {
        eprintln!("⚠  Shell integration not active — `cd` won't persist in your shell.");
        eprintln!("   Run `agf setup` to install the wrapper, then restart your shell.");
        println!("{cmd}");
        return Ok(());
    }

    if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
        return exec_via_shell(cmd, shell);
    }

    // Piped / redirected: preserve the printable contract so callers can capture output.
    println!("{cmd}");
    Ok(())
}

#[cfg(unix)]
fn exec_via_shell(cmd: &str, shell: shell::CommandShell) -> anyhow::Result<()> {
    use std::os::unix::process::CommandExt;
    let (exe, args) = shell.exec_parts();
    let err = std::process::Command::new(exe).args(args).arg(cmd).exec();
    // `exec` only returns on failure.
    Err(anyhow::anyhow!("failed to exec shell: {err}"))
}

#[cfg(not(unix))]
fn exec_via_shell(cmd: &str, shell: shell::CommandShell) -> anyhow::Result<()> {
    let (exe, args) = shell.exec_parts();
    let status = std::process::Command::new(exe)
        .args(args)
        .arg(cmd)
        .status()?;
    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}
