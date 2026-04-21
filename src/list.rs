use std::io::{self, IsTerminal, Write};

use unicode_width::UnicodeWidthStr;

use crate::model::Session;

pub enum OutputFormat {
    Table,
    Json,
    Csv,
}

impl OutputFormat {
    pub fn parse(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "json" => Self::Json,
            "csv" => Self::Csv,
            _ => Self::Table,
        }
    }
}

pub fn list_sessions(sessions: &[Session], format: OutputFormat) {
    match format {
        OutputFormat::Table => print_table(sessions),
        OutputFormat::Json => print_json(sessions),
        OutputFormat::Csv => print_csv(sessions),
    }
}

struct Ansi {
    enabled: bool,
}

impl Ansi {
    fn new() -> Self {
        Self {
            enabled: io::stdout().is_terminal(),
        }
    }
    fn rgb(&self, r: u8, g: u8, b: u8, text: &str) -> String {
        if self.enabled {
            format!("\x1b[38;2;{r};{g};{b}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }
    fn bold(&self, text: &str) -> String {
        if self.enabled {
            format!("\x1b[1m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }
    fn dim(&self, text: &str) -> String {
        if self.enabled {
            format!("\x1b[2m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }
    fn bold_rgb(&self, r: u8, g: u8, b: u8, text: &str) -> String {
        if self.enabled {
            format!("\x1b[1;38;2;{r};{g};{b}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }
}

fn print_table(sessions: &[Session]) {
    if sessions.is_empty() {
        return;
    }

    let a = Ansi::new();
    let mut out = io::stdout().lock();

    let max_project = sessions
        .iter()
        .map(|s| UnicodeWidthStr::width(s.project_name.as_str()))
        .max()
        .unwrap_or(7)
        .clamp(7, 25);
    let max_agent = 12;

    // Title
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "  {} {}",
        a.bold("agf"),
        a.dim(&format!("— {} sessions", sessions.len()))
    );
    let _ = writeln!(out);

    // Header
    let _ = writeln!(
        out,
        "  {}",
        a.dim(&format!(
            " {:<3}  {:<max_project$}  {:<max_agent$}  {:<14}  {:<10}  {}",
            "#", "PROJECT", "AGENT", "TIME", "BRANCH", "PATH"
        ))
    );
    let _ = writeln!(
        out,
        "  {}",
        a.dim(&format!(
            " {}  {}  {}  {}  {}  {}",
            "─".repeat(3),
            "─".repeat(max_project),
            "─".repeat(max_agent),
            "─".repeat(14),
            "─".repeat(10),
            "─".repeat(20)
        ))
    );

    for (i, s) in sessions.iter().enumerate() {
        let path = s.display_path();
        let (r, g, b_val) = s.agent.color();
        let project = format!("{:<max_project$}", truncate(&s.project_name, max_project));
        let agent = format!("{:<max_agent$}", truncate(&s.agent.to_string(), max_agent));
        let time = format!("{:<14}", s.time_display());
        let branch = format!(
            "{:<10}",
            truncate(s.git_branch.as_deref().unwrap_or("—"), 10)
        );
        let num = format!("{:>3}", i + 1);

        let _ = writeln!(
            out,
            "   {}  {}  {}  {}  {}  {}",
            a.dim(&num),
            a.bold(&project),
            a.bold_rgb(r, g, b_val, &agent),
            a.dim(&time),
            a.rgb(52, 211, 153, &branch),
            a.dim(&path),
        );
    }
    let _ = writeln!(out);
}

fn print_json(sessions: &[Session]) {
    let items: Vec<serde_json::Value> = sessions
        .iter()
        .map(|s| {
            serde_json::json!({
                "agent": s.agent.to_string(),
                "session_id": s.session_id,
                "project_name": s.project_name,
                "project_path": s.project_path,
                "timestamp": s.timestamp,
                "time": s.time_display(),
                "git_branch": s.git_branch,
                "worktree": s.worktree,
                "summaries": s.summaries,
            })
        })
        .collect();
    if let Ok(json) = serde_json::to_string_pretty(&items) {
        println!("{json}");
    }
}

fn print_csv(sessions: &[Session]) {
    println!("project,agent,time,path,session_id,branch");
    for s in sessions {
        println!(
            "{},{},{},{},{},{}",
            csv_escape(&s.project_name),
            s.agent,
            s.time_display(),
            csv_escape(&s.project_path),
            s.session_id,
            s.git_branch.as_deref().unwrap_or(""),
        );
    }
}

fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

fn truncate(s: &str, max: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max {
        s.to_string()
    } else {
        let prefix: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{prefix}…")
    }
}

pub fn filter_by_agent(sessions: Vec<Session>, agent_name: &str) -> Vec<Session> {
    let agent_lower = agent_name.to_lowercase();
    sessions
        .into_iter()
        .filter(|s| {
            s.agent.cli_name().to_lowercase() == agent_lower
                || s.agent.to_string().to_lowercase() == agent_lower
                || s.agent
                    .to_string()
                    .to_lowercase()
                    .replace(' ', "")
                    .contains(&agent_lower)
        })
        .collect()
}
