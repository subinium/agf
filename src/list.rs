use std::io::{self, Write};

use crate::model::Session;
use crate::text;

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum OutputFormat {
    Table,
    Json,
    Csv,
}

pub fn list_sessions(sessions: &[Session], format: OutputFormat) -> io::Result<()> {
    write_sessions(&mut io::stdout().lock(), sessions, format)
}

pub fn write_sessions(
    out: &mut impl Write,
    sessions: &[Session],
    format: OutputFormat,
) -> io::Result<()> {
    match format {
        OutputFormat::Table => print_table(out, sessions),
        OutputFormat::Json => print_json(out, sessions),
        OutputFormat::Csv => print_csv(out, sessions),
    }?;
    out.flush()
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

fn print_table(out: &mut impl Write, sessions: &[Session]) -> io::Result<()> {
    if sessions.is_empty() {
        return Ok(());
    }

    let a = Ansi::new();

    let max_project = sessions
        .iter()
        .map(|s| text::width(&text::sanitize_terminal(&s.project_name)))
        .max()
        .unwrap_or(7)
        .clamp(7, 25);
    let max_agent = 12;

    // Title
    writeln!(out)?;
    writeln!(
        out,
        "  {} {}",
        a.bold("agf"),
        a.dim(&format!("— {} sessions", sessions.len()))
    )?;
    writeln!(out)?;

    // Header
    writeln!(
        out,
        "  {}",
        a.dim(&format!(
            " {:<3}  {:<max_project$}  {:<max_agent$}  {:<14}  {:<10}  {}",
            "#", "PROJECT", "AGENT", "TIME", "BRANCH", "PATH"
        ))
    )?;
    writeln!(
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
    )?;

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

        writeln!(
            out,
            "   {}  {}  {}  {}  {}  {}",
            a.dim(&num),
            a.bold(&project),
            a.bold_rgb(r, g, b_val, &agent),
            a.dim(&time),
            a.rgb(52, 211, 153, &branch),
            a.dim(&path),
        )?;
    }
    writeln!(out)
}

fn print_json(out: &mut impl Write, sessions: &[Session]) -> io::Result<()> {
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
    let json = serde_json::to_string_pretty(&items).map_err(io::Error::other)?;
    writeln!(out, "{json}")
}

fn print_csv(out: &mut impl Write, sessions: &[Session]) -> io::Result<()> {
    writeln!(out, "project,agent,time,path,session_id,branch")?;
    for s in sessions {
        // Every field goes through `csv_escape`. Branch names in particular
        // may legally contain commas, which would silently shift every later
        // column in the consuming spreadsheet/script.
        writeln!(
            out,
            "{},{},{},{},{},{}",
            csv_escape(&s.project_name),
            csv_escape(&s.agent.to_string()),
            csv_escape(&s.time_display()),
            csv_escape(&s.project_path),
            csv_escape(&s.session_id),
            csv_escape(s.git_branch.as_deref().unwrap_or("")),
        )?;
    }
    Ok(())
}

// CSV is a raw, lossless export: quoting protects field boundaries, not
// spreadsheet formula evaluation. Consumers must import formula-like cells
// as text; prefixing them here would change session IDs and paths.
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

    fn session() -> Session {
        Session {
            agent: crate::model::Agent::Codex,
            session_id: "=session".into(),
            project_name: "=SUM(1,2)".into(),
            project_path: "/tmp/project".into(),
            summaries: vec!["summary".into()],
            timestamp: 1_800_000_000_000,
            git_branch: Some("feat/a,b".into()),
            worktree: None,
            recap: None,
            interactive: true,
        }
    }

    struct FailingWriter {
        remaining: usize,
        kind: io::ErrorKind,
        flush_only: bool,
    }

    impl Write for FailingWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.flush_only {
                return Ok(bytes.len());
            }
            if self.remaining == 0 {
                return Err(self.kind.into());
            }
            let written = self.remaining.min(bytes.len());
            self.remaining -= written;
            Ok(written)
        }

        fn flush(&mut self) -> io::Result<()> {
            if self.flush_only {
                Err(self.kind.into())
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn all_list_formats_propagate_partial_write_and_flush_errors() {
        for format in [OutputFormat::Table, OutputFormat::Json, OutputFormat::Csv] {
            for kind in [
                io::ErrorKind::BrokenPipe,
                io::ErrorKind::PermissionDenied,
                io::ErrorKind::WriteZero,
            ] {
                for flush_only in [false, true] {
                    let mut writer = FailingWriter {
                        remaining: 8,
                        kind,
                        flush_only,
                    };
                    assert_eq!(
                        write_sessions(&mut writer, &[session()], format)
                            .unwrap_err()
                            .kind(),
                        kind
                    );
                }
            }
        }
    }

    #[test]
    fn list_json_and_csv_keep_raw_session_values() {
        let mut out = Vec::new();
        write_sessions(&mut out, &[session()], OutputFormat::Json).unwrap();
        let json: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(json[0]["session_id"], "=session");
        assert_eq!(json[0]["project_name"], "=SUM(1,2)");
        out.clear();
        write_sessions(&mut out, &[session()], OutputFormat::Csv).unwrap();
        let csv = String::from_utf8(out).unwrap();
        assert!(csv.starts_with("project,agent,time,path,session_id,branch\n\"=SUM(1,2)\","));
        assert!(csv.ends_with(",=session,\"feat/a,b\"\n"));
    }

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
