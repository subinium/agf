use std::collections::HashMap;
use std::io::{self, Write};

use crate::model::{Agent, Session};
use crate::text;

pub fn print_stats(sessions: &[Session], json: bool) -> io::Result<()> {
    write_stats(&mut io::stdout().lock(), sessions, json)
}

pub fn write_stats(out: &mut impl Write, sessions: &[Session], json: bool) -> io::Result<()> {
    if json {
        print_json(out, sessions)?;
    } else {
        print_text(out, sessions)?;
    }
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
    fn bar_rgb(&self, r: u8, g: u8, b: u8, filled: usize, empty: usize) -> String {
        let bar = "\u{2588}".repeat(filled);
        let space = "\u{2591}".repeat(empty);
        if self.enabled {
            format!("\x1b[38;2;{r};{g};{b}m{bar}\x1b[38;2;60;60;60m{space}\x1b[0m")
        } else {
            format!("{bar}{space}")
        }
    }
}

fn print_text(out: &mut impl Write, sessions: &[Session]) -> io::Result<()> {
    if sessions.is_empty() {
        return writeln!(out, "No sessions found.");
    }

    let a = Ansi::new();
    let total = sessions.len();
    let bar_width = 25;

    // Title
    writeln!(out)?;
    writeln!(
        out,
        "  {} {}",
        a.bold("agf stats"),
        a.dim(&format!("— {total} sessions total"))
    )?;

    // Sessions per agent
    writeln!(out)?;
    writeln!(out, "  {}", a.bold("Sessions by Agent"))?;
    writeln!(out)?;

    let mut by_agent: Vec<(Agent, usize)> = Vec::new();
    let mut agent_map: HashMap<Agent, usize> = HashMap::new();
    for s in sessions {
        *agent_map.entry(s.agent).or_insert(0) += 1;
    }
    for a_type in Agent::all() {
        if let Some(&count) = agent_map.get(a_type) {
            by_agent.push((*a_type, count));
        }
    }
    by_agent.sort_by_key(|y| std::cmp::Reverse(y.1));
    let max_agent_count = by_agent.first().map(|(_, c)| *c).unwrap_or(1);

    let col_width: usize = 14; // fixed column for agent names
    for (agent, count) in &by_agent {
        let (r, g, b) = agent.color();
        let filled = (count * bar_width) / max_agent_count;
        let filled = filled.max(if *count > 0 { 1 } else { 0 });
        let empty = bar_width.saturating_sub(filled);
        let pct = (*count as f64 / total as f64 * 100.0) as u32;
        writeln!(
            out,
            "   {} {} {:>3} {:>3}%",
            a.rgb(r, g, b, &text::fit(&agent.to_string(), col_width)),
            a.bar_rgb(r, g, b, filled, empty),
            a.bold(&count.to_string()),
            pct,
        )?;
    }

    // Top projects
    writeln!(out)?;
    writeln!(out, "  {}", a.bold("Top Projects"))?;
    writeln!(out)?;

    let mut by_project: HashMap<String, (usize, Option<Agent>)> = HashMap::new();
    for s in sessions {
        let entry = by_project
            .entry(text::sanitize_terminal(&s.project_name))
            .or_insert((0, None));
        entry.0 += 1;
        // Keep first-seen agent (project color)
        if entry.1.is_none() {
            entry.1 = Some(s.agent);
        }
    }
    let mut project_list: Vec<(String, usize, Agent)> = by_project
        .into_iter()
        .map(|(name, (count, agent))| (name, count, agent.unwrap_or(Agent::ClaudeCode)))
        .collect();
    project_list.sort_by_key(|y| std::cmp::Reverse(y.1));
    project_list.truncate(10);
    let max_proj_count = project_list.first().map(|(_, c, _)| *c).unwrap_or(1);

    // Terminal columns, not bytes: `"프로젝트".len()` is 12 while the name
    // occupies 8 columns, which used to over-measure the column and then
    // saturate the per-row padding to zero, so every bar started at a
    // different offset.
    let max_name_width = project_list
        .iter()
        .map(|(n, _, _)| text::width(n))
        .max()
        .unwrap_or(10)
        .min(22);

    for (name, count, agent) in &project_list {
        let (r, g, b) = agent.color();
        let filled = (count * bar_width) / max_proj_count;
        let filled = filled.max(if *count > 0 { 1 } else { 0 });
        let empty = bar_width.saturating_sub(filled);
        writeln!(
            out,
            "   {} {} {:>3}",
            a.bold(&text::fit(name, max_name_width)),
            a.bar_rgb(r, g, b, filled, empty),
            count,
        )?;
    }

    // Activity timeline
    let now = chrono::Utc::now().timestamp_millis();
    let activity = activity_counts(sessions, now);

    writeln!(out)?;
    writeln!(out, "  {}", a.bold("Activity"))?;
    writeln!(out)?;

    let max_time = [
        activity.today,
        activity.week,
        activity.month,
        activity.older,
        activity.future,
    ]
    .into_iter()
    .max()
    .unwrap_or(1);

    let time_items = [
        ("Last 24h", activity.today, (52, 211, 153)),
        ("Last 7d", activity.week, (139, 92, 246)),
        ("Last 30d", activity.month, (59, 130, 246)),
        ("Older", activity.older, (107, 114, 128)),
        ("Future/invalid", activity.future, (239, 68, 68)),
    ];
    for (label, count, (r, g, b)) in &time_items {
        let filled = (count * bar_width).checked_div(max_time).unwrap_or(0);
        let filled = filled.max(if *count > 0 { 1 } else { 0 });
        let empty = bar_width.saturating_sub(filled);
        writeln!(
            out,
            "   {} {} {:>3}",
            a.dim(&text::fit(label, 12)),
            a.bar_rgb(*r, *g, *b, filled, empty),
            count,
        )?;
    }
    writeln!(out)
}

fn print_json(out: &mut impl Write, sessions: &[Session]) -> io::Result<()> {
    let mut by_agent: HashMap<String, usize> = HashMap::new();
    for s in sessions {
        *by_agent.entry(s.agent.to_string()).or_insert(0) += 1;
    }

    let mut by_project: HashMap<String, usize> = HashMap::new();
    for s in sessions {
        *by_project.entry(s.project_name.clone()).or_insert(0) += 1;
    }

    let now = chrono::Utc::now().timestamp_millis();
    let activity = activity_counts(sessions, now);

    let json = serde_json::json!({
        "total": sessions.len(),
        "by_agent": by_agent,
        "by_project": by_project,
        "activity": {
            "today": activity.today,
            "this_week": activity.week,
            "this_month": activity.month,
            "older": activity.older,
            "future_or_invalid": activity.future,
        }
    });
    let json = serde_json::to_string_pretty(&json).map_err(io::Error::other)?;
    writeln!(out, "{json}")
}

#[derive(Debug, Default, PartialEq, Eq)]
struct ActivityCounts {
    today: usize,
    week: usize,
    month: usize,
    older: usize,
    future: usize,
}

fn activity_counts(sessions: &[Session], now: i64) -> ActivityCounts {
    const DAY: i64 = 86_400_000;
    const MIN_VALID: i64 = 946_684_800_000;
    let mut counts = ActivityCounts::default();
    for session in sessions {
        if session.timestamp < MIN_VALID || session.timestamp > now.saturating_add(5 * 60_000) {
            counts.future += 1;
            continue;
        }
        let age = now.saturating_sub(session.timestamp);
        if age <= DAY {
            counts.today += 1;
        }
        if age <= 7 * DAY {
            counts.week += 1;
        }
        if age <= 30 * DAY {
            counts.month += 1;
        } else {
            counts.older += 1;
        }
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FailingWriter {
        kind: io::ErrorKind,
        flush_only: bool,
    }

    impl Write for FailingWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.flush_only {
                Ok(bytes.len())
            } else {
                Err(self.kind.into())
            }
        }

        fn flush(&mut self) -> io::Result<()> {
            Err(self.kind.into())
        }
    }

    #[test]
    fn both_stats_formats_propagate_write_and_flush_errors() {
        let session = Session {
            agent: Agent::Codex,
            session_id: "sid".into(),
            project_name: "project".into(),
            project_path: "/tmp/project".into(),
            summaries: Vec::new(),
            timestamp: 1_800_000_000_000,
            git_branch: None,
            worktree: None,
            recap: None,
            interactive: true,
        };
        for json in [false, true] {
            for sessions in [std::slice::from_ref(&session), &[]] {
                for kind in [
                    io::ErrorKind::BrokenPipe,
                    io::ErrorKind::PermissionDenied,
                    io::ErrorKind::WriteZero,
                ] {
                    for flush_only in [false, true] {
                        let mut writer = FailingWriter { kind, flush_only };
                        assert_eq!(
                            write_stats(&mut writer, sessions, json).unwrap_err().kind(),
                            kind
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn empty_stats_are_written_to_the_injected_writer() {
        let mut out = Vec::new();
        write_stats(&mut out, &[], false).unwrap();
        assert_eq!(out, b"No sessions found.\n");
        out.clear();
        write_stats(&mut out, &[], true).unwrap();
        let json: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(json["total"], 0);
    }

    #[test]
    fn activity_windows_are_cumulative_and_future_is_separate() {
        let now = 1_800_000_000_000i64;
        let make = |timestamp: i64| Session {
            agent: Agent::Codex,
            session_id: timestamp.to_string(),
            project_name: "project".into(),
            project_path: "/tmp/project".into(),
            summaries: Vec::new(),
            timestamp,
            git_branch: None,
            worktree: None,
            recap: None,
            interactive: true,
        };
        let sessions = [
            make(now),
            make(now - 2 * 86_400_000),
            make(now - 10 * 86_400_000),
            make(now + 10 * 60_000),
        ];
        assert_eq!(
            activity_counts(&sessions, now),
            ActivityCounts {
                today: 1,
                week: 2,
                month: 3,
                older: 0,
                future: 1
            }
        );
    }
}
