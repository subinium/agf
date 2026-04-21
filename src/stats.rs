use std::collections::HashMap;
use std::io::{self, IsTerminal, Write};

use crate::model::{Agent, Session};

pub fn print_stats(sessions: &[Session], json: bool) {
    if json {
        print_json(sessions);
    } else {
        print_text(sessions);
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

fn print_text(sessions: &[Session]) {
    if sessions.is_empty() {
        eprintln!("No sessions found.");
        return;
    }

    let a = Ansi::new();
    let mut out = io::stdout().lock();
    let total = sessions.len();
    let bar_width = 25;

    // Title
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "  {} {}",
        a.bold("agf stats"),
        a.dim(&format!("— {total} sessions total"))
    );

    // Sessions per agent
    let _ = writeln!(out);
    let _ = writeln!(out, "  {}", a.bold("Sessions by Agent"));
    let _ = writeln!(out);

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
        let name = agent.to_string();
        let pad = col_width.saturating_sub(name.len());
        let _ = writeln!(
            out,
            "   {}{} {} {:>3} {:>3}%",
            a.rgb(r, g, b, &name),
            " ".repeat(pad),
            a.bar_rgb(r, g, b, filled, empty),
            a.bold(&count.to_string()),
            pct,
        );
    }

    // Top projects
    let _ = writeln!(out);
    let _ = writeln!(out, "  {}", a.bold("Top Projects"));
    let _ = writeln!(out);

    let mut by_project: HashMap<String, (usize, Option<Agent>)> = HashMap::new();
    for s in sessions {
        let entry = by_project
            .entry(s.project_name.clone())
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

    let max_name_len = project_list
        .iter()
        .map(|(n, _, _)| n.len())
        .max()
        .unwrap_or(10)
        .min(22);

    for (name, count, agent) in &project_list {
        let (r, g, b) = agent.color();
        let display = truncate(name, max_name_len);
        let filled = (count * bar_width) / max_proj_count;
        let filled = filled.max(if *count > 0 { 1 } else { 0 });
        let empty = bar_width.saturating_sub(filled);
        let pad = max_name_len.saturating_sub(display.len());
        let _ = writeln!(
            out,
            "   {}{} {} {:>3}",
            a.bold(&display),
            " ".repeat(pad),
            a.bar_rgb(r, g, b, filled, empty),
            count,
        );
    }

    // Activity timeline
    let now = chrono::Utc::now().timestamp_millis();
    let day_ms: i64 = 86_400_000;
    let week_ms: i64 = 7 * day_ms;
    let month_ms: i64 = 30 * day_ms;

    let mut today = 0usize;
    let mut this_week = 0usize;
    let mut this_month = 0usize;
    let mut older = 0usize;

    for s in sessions {
        let age = now - s.timestamp;
        if age < day_ms {
            today += 1;
        } else if age < week_ms {
            this_week += 1;
        } else if age < month_ms {
            this_month += 1;
        } else {
            older += 1;
        }
    }

    let _ = writeln!(out);
    let _ = writeln!(out, "  {}", a.bold("Activity"));
    let _ = writeln!(out);

    let max_time = [today, this_week, this_month, older]
        .into_iter()
        .max()
        .unwrap_or(1);

    let time_items = [
        ("Last 24h", today, (52, 211, 153)),        // green
        ("Last 7d", this_week, (139, 92, 246)),     // violet
        ("Last 30d", this_month, (59, 130, 246)),   // blue
        ("Older", older, (107, 114, 128)),          // gray
    ];
    for (label, count, (r, g, b)) in &time_items {
        let filled = (count * bar_width).checked_div(max_time).unwrap_or(0);
        let filled = filled.max(if *count > 0 { 1 } else { 0 });
        let empty = bar_width.saturating_sub(filled);
        let pad = 12usize.saturating_sub(label.len());
        let _ = writeln!(
            out,
            "   {}{} {} {:>3}",
            a.dim(label),
            " ".repeat(pad),
            a.bar_rgb(*r, *g, *b, filled, empty),
            count,
        );
    }
    let _ = writeln!(out);
}

fn print_json(sessions: &[Session]) {
    let mut by_agent: HashMap<String, usize> = HashMap::new();
    for s in sessions {
        *by_agent.entry(s.agent.to_string()).or_insert(0) += 1;
    }

    let mut by_project: HashMap<String, usize> = HashMap::new();
    for s in sessions {
        *by_project.entry(s.project_name.clone()).or_insert(0) += 1;
    }

    let now = chrono::Utc::now().timestamp_millis();
    let day_ms: i64 = 86_400_000;
    let week_ms: i64 = 7 * day_ms;
    let month_ms: i64 = 30 * day_ms;

    let mut today = 0usize;
    let mut this_week = 0usize;
    let mut this_month = 0usize;
    let mut older = 0usize;

    for s in sessions {
        let age = now - s.timestamp;
        if age < day_ms {
            today += 1;
        } else if age < week_ms {
            this_week += 1;
        } else if age < month_ms {
            this_month += 1;
        } else {
            older += 1;
        }
    }

    let json = serde_json::json!({
        "total": sessions.len(),
        "by_agent": by_agent,
        "by_project": by_project,
        "activity": {
            "today": today,
            "this_week": this_week,
            "this_month": this_month,
            "older": older,
        }
    });
    if let Ok(s) = serde_json::to_string_pretty(&json) {
        println!("{s}");
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
