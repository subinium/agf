use std::io::{self, Write};

use crate::model::Session;
use crate::text;

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
            enabled: text::color_enabled(),
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
        .map(|s| text::width(&text::sanitize_terminal(&s.project_name)))
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
        let path = text::sanitize_terminal(&s.display_path());
        let (r, g, b_val) = s.agent.color();
        // `text::fit`, not `format!("{:<n$}", …)`: the column budget is in
        // terminal columns but `{:<n$}` pads by char count, so a CJK project
        // name would push every column after it out of alignment.
        let project = text::fit(&text::sanitize_terminal(&s.project_name), max_project);
        let agent = text::fit(&s.agent.to_string(), max_agent);
        let time = text::fit(&s.time_display(), 14);
        let branch = text::fit(
            &text::sanitize_terminal(s.git_branch.as_deref().unwrap_or("—")),
            10,
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
                "interactive": s.interactive,
            })
        })
        .collect();
    if let Ok(json) = serde_json::to_string_pretty(&items) {
        let mut out = io::stdout().lock();
        let _ = writeln!(out, "{json}");
    }
}

fn print_csv(sessions: &[Session]) {
    let mut out = io::stdout().lock();
    let _ = writeln!(out, "project,agent,time,path,session_id,branch");
    for s in sessions {
        // Every field goes through `csv_escape`. Branch names in particular
        // may legally contain commas, which would silently shift every later
        // column in the consuming spreadsheet/script.
        let _ = writeln!(
            out,
            "{},{},{},{},{},{}",
            csv_escape(&s.project_name),
            csv_escape(&s.agent.to_string()),
            csv_escape(&s.time_display()),
            csv_escape(&s.project_path),
            csv_escape(&s.session_id),
            csv_escape(s.git_branch.as_deref().unwrap_or("")),
        );
    }
}

fn csv_escape(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_escape_quotes_every_separator_bearing_field() {
        assert_eq!(csv_escape("plain"), "plain");
        // Git allows commas in branch names; unquoted they shift the row.
        assert_eq!(csv_escape("feat/a,b"), "\"feat/a,b\"");
        assert_eq!(csv_escape("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(csv_escape("two\nlines"), "\"two\nlines\"");
        assert_eq!(csv_escape("cr\rlf"), "\"cr\rlf\"");
    }
}
