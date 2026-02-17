use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Agent {
    ClaudeCode,
    Codex,
    Cursor,
}

impl fmt::Display for Agent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Agent::ClaudeCode => write!(f, "Claude Code"),
            Agent::Codex => write!(f, "Codex"),
            Agent::Cursor => write!(f, "Cursor"),
        }
    }
}

impl Agent {
    pub fn color(&self) -> (u8, u8, u8) {
        match self {
            Agent::ClaudeCode => (217, 119, 6),   // #D97706 amber
            Agent::Codex => (16, 185, 129),        // #10B981 emerald
            Agent::Cursor => (99, 102, 241),       // #6366F1 indigo
        }
    }

    pub fn all() -> &'static [Agent] {
        &[Agent::ClaudeCode, Agent::Codex, Agent::Cursor]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortMode {
    Time,
    Name,
    Agent,
}

impl SortMode {
    pub fn next(self) -> Self {
        match self {
            SortMode::Time => SortMode::Name,
            SortMode::Name => SortMode::Agent,
            SortMode::Agent => SortMode::Time,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SortMode::Time => "time",
            SortMode::Name => "name",
            SortMode::Agent => "agent",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Session {
    pub agent: Agent,
    pub session_id: String,
    pub project_name: String,
    pub project_path: String,
    pub summary: Option<String>,
    pub timestamp: i64, // Unix ms
    pub git_branch: Option<String>,
    pub git_dirty: Option<bool>,
}

impl Session {
    /// Short relative time without "ago": `now`, `3m`, `2h`, `5d`, `2w`, `1mo`
    pub fn relative_time_short(&self) -> String {
        let now = chrono::Utc::now().timestamp_millis();
        let diff_secs = (now - self.timestamp) / 1000;
        if diff_secs < 0 {
            return "now".to_string();
        }
        let diff_secs = diff_secs as u64;
        match diff_secs {
            0..=59 => "now".to_string(),
            60..=3599 => format!("{}m", diff_secs / 60),
            3600..=86399 => format!("{}h", diff_secs / 3600),
            86400..=604799 => format!("{}d", diff_secs / 86400),
            604800..=2_629_799 => format!("{}w", diff_secs / 604800),
            _ => format!("{}mo", diff_secs / 2_629_800),
        }
    }

    /// Absolute date: `MM/DD` or `MM/DD/YY` if different year
    pub fn date_str(&self) -> String {
        use chrono::{TimeZone, Local};
        let dt = match Local.timestamp_millis_opt(self.timestamp) {
            chrono::LocalResult::Single(dt) => dt,
            _ => return String::new(),
        };
        let now = Local::now();
        if dt.format("%Y").to_string() == now.format("%Y").to_string() {
            dt.format("%m/%d").to_string()
        } else {
            dt.format("%m/%d/%y").to_string()
        }
    }

    /// Combined: `2h · 02/17`
    pub fn time_display(&self) -> String {
        format!("{} · {}", self.relative_time_short(), self.date_str())
    }

    pub fn display_path(&self) -> String {
        if let Some(home) = dirs::home_dir() {
            if let Some(rest) = self.project_path.strip_prefix(home.to_str().unwrap_or("")) {
                return format!("~{rest}");
            }
        }
        self.project_path.clone()
    }

    pub fn search_text(&self) -> String {
        let mut text = format!("{} {}", self.project_name, self.project_path);
        if let Some(ref summary) = self.summary {
            text.push(' ');
            text.push_str(summary);
        }
        if let Some(ref branch) = self.git_branch {
            text.push(' ');
            text.push_str(branch);
        }
        text
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Resume,
    NewSession,
    Cd,
    Delete,
    Back,
}

impl Action {
    pub const MENU: [Action; 5] = [
        Action::Resume,
        Action::NewSession,
        Action::Cd,
        Action::Delete,
        Action::Back,
    ];
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Action::Resume => write!(f, "Resume session"),
            Action::NewSession => write!(f, "New session"),
            Action::Cd => write!(f, "cd to directory"),
            Action::Delete => write!(f, "Delete session"),
            Action::Back => write!(f, "← Back"),
        }
    }
}
