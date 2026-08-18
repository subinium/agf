use std::fmt;

use crate::shell::CommandShell;

// Serde derives are load-bearing for the session cache: unit variants
// serialize as their exact variant names ("ClaudeCode", "Codex", ...), which
// is the on-disk format of ~/.cache/agf/sessions.json.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[allow(clippy::enum_variant_names)]
pub enum Agent {
    ClaudeCode,
    Codex,
    Grok,
    Kimi,
    Qwen,
    OpenCode,
    Pi,
    OhMyPi,
    Kiro,
    CursorAgent,
    Gemini,
    Hermes,
    Yolop,
    PrimeAgent,
}

impl fmt::Display for Agent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Agent::ClaudeCode => write!(f, "Claude Code"),
            Agent::Codex => write!(f, "Codex"),
            Agent::Grok => write!(f, "Grok Build"),
            Agent::Kimi => write!(f, "Kimi Code"),
            Agent::Qwen => write!(f, "Qwen Code"),
            Agent::OpenCode => write!(f, "OpenCode"),
            Agent::Pi => write!(f, "pi"),
            Agent::OhMyPi => write!(f, "Oh My Pi"),
            Agent::Kiro => write!(f, "Kiro"),
            Agent::CursorAgent => write!(f, "Cursor CLI"),
            Agent::Gemini => write!(f, "Gemini"),
            Agent::Hermes => write!(f, "Hermes"),
            Agent::Yolop => write!(f, "Yolop"),
            Agent::PrimeAgent => write!(f, "Prime Agent"),
        }
    }
}

impl Agent {
    pub fn color(&self) -> (u8, u8, u8) {
        match self {
            Agent::ClaudeCode => (217, 119, 87), // #D97757 terra cotta (Anthropic)
            Agent::Codex => (0, 166, 126),       // #00A67E teal green (OpenAI)
            Agent::Grok => (229, 229, 229),      // xAI monochrome
            Agent::Kimi => (74, 120, 255),       // Kimi blue
            Agent::Qwen => (121, 93, 255),       // Qwen violet
            Agent::OpenCode => (59, 130, 246),   // #3B82F6 blue
            Agent::Pi => (236, 72, 153),         // #EC4899 pink
            Agent::OhMyPi => (249, 115, 22),     // #F97316 orange
            Agent::Kiro => (136, 69, 244),       // #8845F4 deep purple (AWS Kiro)
            Agent::CursorAgent => (245, 184, 65), // #F5B841 Cursor brand yellow
            Agent::Gemini => (66, 133, 244),     // #4285F4 Google blue
            Agent::Hermes => (168, 85, 247),     // #A855F7 purple (Nous Research)
            Agent::Yolop => (34, 197, 94),       // #22C55E green
            Agent::PrimeAgent => (99, 102, 241), // #6366F1 indigo
        }
    }

    pub fn all() -> &'static [Agent] {
        &[
            Agent::ClaudeCode,
            Agent::Codex,
            Agent::Grok,
            Agent::Kimi,
            Agent::Qwen,
            Agent::OpenCode,
            Agent::Pi,
            Agent::OhMyPi,
            Agent::Kiro,
            Agent::CursorAgent,
            Agent::Gemini,
            Agent::Hermes,
            Agent::Yolop,
            Agent::PrimeAgent,
        ]
    }

    /// CLI executable name used for launching and detection.
    pub fn cli_name(&self) -> &'static str {
        match self {
            Agent::ClaudeCode => "claude",
            Agent::Codex => "codex",
            Agent::Grok => "grok",
            Agent::Kimi => "kimi",
            Agent::Qwen => "qwen",
            Agent::OpenCode => "opencode",
            Agent::Pi => "pi",
            Agent::OhMyPi => "omp",
            Agent::Kiro => "kiro-cli",
            Agent::CursorAgent => "cursor-agent",
            Agent::Gemini => "gemini",
            Agent::Hermes => "hermes",
            Agent::Yolop => "yolop",
            Agent::PrimeAgent => "prime-agent",
        }
    }

    /// Shell command to resume the most recent session.
    ///
    /// `session_id` is escaped for `shell` rather than wrapped in raw single
    /// quotes: session ids come from parsed on-disk files and are not
    /// guaranteed to be quote-free, so an unescaped id could break the
    /// generated command — or inject shell — once the wrapper `eval`s it.
    pub fn resume_cmd(&self, session_id: &str, shell: &CommandShell) -> String {
        let id = shell.quote(session_id);
        match self {
            Agent::ClaudeCode => format!("claude --resume {id}"),
            Agent::Codex => format!("codex resume {id}"),
            Agent::Grok => format!("grok --resume {id}"),
            Agent::Kimi => format!("kimi --session {id}"),
            Agent::Qwen => format!("qwen --resume {id}"),
            Agent::OpenCode => format!("opencode -s {id}"),
            Agent::Pi => format!("pi --session {id}"),
            Agent::OhMyPi => format!("omp --resume {id}"),
            Agent::Kiro => format!("kiro-cli chat --resume-id {id}"),
            Agent::CursorAgent => format!("cursor-agent --resume {id}"),
            Agent::Gemini => format!("gemini --resume {id}"),
            Agent::Hermes => format!("hermes --resume {id}"),
            Agent::Yolop => format!("yolop --session {id}"),
            Agent::PrimeAgent => format!("prime-agent --resume {id}"),
        }
    }

    /// Permission/approval mode options for resuming a session with extra flags.
    pub fn resume_mode_options(&self) -> &'static [(&'static str, &'static str)] {
        match self {
            Agent::ClaudeCode => &[
                ("default", ""),
                ("acceptEdits", " --permission-mode acceptEdits"),
                ("plan (read-only)", " --permission-mode plan"),
                ("bypass permissions", " --dangerously-skip-permissions"),
            ],
            Agent::Codex => &[
                ("default", ""),
                ("workspace-write", " -a on-request -s workspace-write"),
                ("full-auto", " -a never -s workspace-write"),
                (
                    "bypass sandbox",
                    " --dangerously-bypass-approvals-and-sandbox",
                ),
            ],
            Agent::Gemini => &[
                ("default", ""),
                ("auto_edit", " --approval-mode auto_edit"),
                ("yolo (no approval)", " -y"),
                ("plan (read-only)", " --approval-mode plan"),
                ("sandbox", " -s"),
            ],
            Agent::Kimi => &[
                ("default", ""),
                ("auto", " --auto"),
                ("plan (read-only)", " --plan"),
                ("yolo (no approval)", " --yolo"),
            ],
            _ => &[("default", "")],
        }
    }

    /// Shell command to start a new session (base, without flags).
    pub fn new_session_cmd(&self) -> &'static str {
        match self {
            Agent::ClaudeCode => "claude",
            Agent::Codex => "codex",
            Agent::Grok => "grok",
            Agent::Kimi => "kimi",
            Agent::Qwen => "qwen",
            Agent::OpenCode => "opencode",
            Agent::Pi => "pi",
            Agent::OhMyPi => "omp",
            Agent::Kiro => "kiro-cli chat",
            Agent::CursorAgent => "cursor-agent",
            Agent::Gemini => "gemini",
            Agent::Hermes => "hermes",
            Agent::Yolop => "yolop",
            Agent::PrimeAgent => "prime-agent",
        }
    }

    /// Stable lowercase identifier used in settings and CLI filters.
    pub fn slug(self) -> &'static str {
        match self {
            Agent::ClaudeCode => "claude",
            Agent::Codex => "codex",
            Agent::Grok => "grok",
            Agent::Kimi => "kimi",
            Agent::Qwen => "qwen",
            Agent::OpenCode => "opencode",
            Agent::Pi => "pi",
            Agent::OhMyPi => "oh-my-pi",
            Agent::Kiro => "kiro",
            Agent::CursorAgent => "cursor",
            Agent::Gemini => "gemini",
            Agent::Hermes => "hermes",
            Agent::Yolop => "yolop",
            Agent::PrimeAgent => "prime-agent",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        let normalized = name.trim().to_ascii_lowercase().replace([' ', '_'], "-");
        match normalized.as_str() {
            "claude" | "claude-code" => Some(Agent::ClaudeCode),
            "codex" => Some(Agent::Codex),
            "grok" | "grok-build" | "xai" | "xai-grok" => Some(Agent::Grok),
            "kimi" | "kimi-code" | "kimi-cli" => Some(Agent::Kimi),
            "qwen" | "qwen-code" => Some(Agent::Qwen),
            "opencode" | "open-code" => Some(Agent::OpenCode),
            "pi" => Some(Agent::Pi),
            "omp" | "oh-my-pi" | "ohmypi" => Some(Agent::OhMyPi),
            "kiro" | "kiro-cli" => Some(Agent::Kiro),
            "cursor" | "cursor-cli" | "cursor-agent" => Some(Agent::CursorAgent),
            "gemini" => Some(Agent::Gemini),
            "hermes" => Some(Agent::Hermes),
            "yolop" => Some(Agent::Yolop),
            "prime" | "prime-agent" | "prime-intellect" => Some(Agent::PrimeAgent),
            _ => None,
        }
    }

    pub fn supports_delete(self) -> bool {
        !matches!(
            self,
            Agent::Grok | Agent::Kimi | Agent::Qwen | Agent::PrimeAgent
        )
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
    pub summaries: Vec<String>,
    pub timestamp: i64, // Unix ms
    pub git_branch: Option<String>,
    pub worktree: Option<String>,
    pub recap: Option<String>, // Claude Code away_summary, optionally prefixed with aiTitle
    /// Whether this is a top-level interactive session. Internal subagents and
    /// non-interactive exec runs remain discoverable but are hidden by default.
    pub interactive: bool,
}

impl Session {
    /// Short relative time without "ago": `now`, `3m`, `2h`, `5d`, `2w`, `1mo`
    pub fn relative_time_short(&self) -> String {
        if self.timestamp < 946_684_800_000 {
            return "unknown".to_string();
        }
        let now = chrono::Utc::now().timestamp_millis();
        let diff_secs = (now - self.timestamp) / 1000;
        if diff_secs < -300 {
            return "future".to_string();
        }
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
        use chrono::{Local, TimeZone};
        if self.timestamp < 946_684_800_000 {
            return "—".to_string();
        }
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
        // cwd-independent agents (Hermes) leave project_path empty so resume
        // doesn't drag the user out of their current directory; surface that
        // visually instead of rendering an empty cell.
        if self.project_path.is_empty() {
            return "—".to_string();
        }
        if let Some(home) = dirs::home_dir()
            && let Ok(rest) = std::path::Path::new(&self.project_path).strip_prefix(&home)
        {
            let display = if rest.as_os_str().is_empty() {
                "~".to_string()
            } else {
                format!("~/{}", rest.to_string_lossy())
            };
            return crate::text::sanitize_terminal(&display);
        }
        crate::text::sanitize_terminal(&self.project_path)
    }

    pub fn search_text(&self, max_summaries: usize, include_summaries: bool) -> String {
        let mut text = format!("{} {}", self.project_name, self.project_path);
        if include_summaries {
            for summary in self.summaries.iter().take(max_summaries) {
                text.push(' ');
                text.push_str(summary);
            }
        }
        if let Some(ref branch) = self.git_branch {
            text.push(' ');
            text.push_str(branch);
        }
        text
    }

    pub fn identity(&self) -> SessionIdentity {
        SessionIdentity {
            agent: self.agent,
            session_id: self.session_id.clone(),
        }
    }

    pub fn settings_key(&self) -> String {
        format!("{}:{}", self.agent.slug(), self.session_id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SessionIdentity {
    pub agent: Agent,
    pub session_id: String,
}

/// Deterministic total ordering for every non-fuzzy view.
pub fn compare_sessions(a: &Session, b: &Session, mode: SortMode) -> std::cmp::Ordering {
    let identity = || {
        a.agent
            .cmp(&b.agent)
            .then_with(|| a.project_path.cmp(&b.project_path))
            .then_with(|| a.session_id.cmp(&b.session_id))
    };
    match mode {
        SortMode::Time => b.timestamp.cmp(&a.timestamp).then_with(identity),
        SortMode::Name => compare_lowercase(&a.project_name, &b.project_name)
            .then_with(|| b.timestamp.cmp(&a.timestamp))
            .then_with(identity),
        SortMode::Agent => a
            .agent
            .cmp(&b.agent)
            .then_with(|| b.timestamp.cmp(&a.timestamp))
            .then_with(identity),
    }
}

fn compare_lowercase(a: &str, b: &str) -> std::cmp::Ordering {
    let mut a = a.chars().flat_map(char::to_lowercase);
    let mut b = b.chars().flat_map(char::to_lowercase);
    loop {
        match (a.next(), b.next()) {
            (Some(left), Some(right)) => match left.cmp(&right) {
                std::cmp::Ordering::Equal => {}
                ordering => return ordering,
            },
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (None, None) => return std::cmp::Ordering::Equal,
        }
    }
}

/// Clamp scanner timestamps to a plausible activity range. A corrupt future
/// timestamp must not pin a session to the top or be rendered deceptively as
/// `now`; callers provide their best durable fallback (often file mtime).
pub fn normalize_timestamp(candidate: i64, fallback: i64) -> i64 {
    const MIN_VALID_MS: i64 = 946_684_800_000; // 2000-01-01
    const FUTURE_SKEW_MS: i64 = 5 * 60 * 1000;
    let now = chrono::Utc::now().timestamp_millis();
    let valid =
        |timestamp: i64| (MIN_VALID_MS..=now.saturating_add(FUTURE_SKEW_MS)).contains(&timestamp);
    if valid(candidate) {
        candidate
    } else if valid(fallback) {
        fallback
    } else {
        0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Resume,
    NewSession,
    Open,
    Cd,
    Pin,
    Delete,
    Back,
}

impl Action {
    pub const MENU: [Action; 6] = [
        Action::Resume,
        Action::NewSession,
        Action::Open,
        Action::Cd,
        Action::Pin,
        Action::Delete,
    ];
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Action::Resume => write!(f, "Resume Session"),
            Action::NewSession => write!(f, "New Session"),
            Action::Open => write!(f, "Open in Editor"),
            Action::Cd => write!(f, "Go to Directory"),
            Action::Pin => write!(f, "Pin Session"),
            Action::Delete => write!(f, "Delete Session"),
            Action::Back => write!(f, "← Back"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pi_resume_command_uses_selected_session_id() {
        assert_eq!(
            Agent::Pi.resume_cmd(
                "019e14f4-c9a5-76dc-b7b6-0613e602a620",
                &crate::shell::CommandShell::Posix
            ),
            "pi --session '019e14f4-c9a5-76dc-b7b6-0613e602a620'"
        );
    }

    #[test]
    fn resume_cmd_escapes_session_id() {
        // A session id containing a single quote must not break out of the
        // quoted argument (broken command) or inject shell.
        assert_eq!(
            Agent::ClaudeCode.resume_cmd("a'b", &crate::shell::CommandShell::Posix),
            r#"claude --resume 'a'\''b'"#
        );
        // PowerShell doubles the embedded quote instead of `'\''`.
        assert_eq!(
            Agent::ClaudeCode.resume_cmd("a'b", &crate::shell::CommandShell::PowerShell),
            "claude --resume 'a''b'"
        );
    }

    #[test]
    fn yolop_resume_command_uses_selected_session_id() {
        assert_eq!(
            Agent::Yolop.resume_cmd(
                "session_019e3db018a17450aba5407af5777237",
                &crate::shell::CommandShell::Posix
            ),
            "yolop --session 'session_019e3db018a17450aba5407af5777237'"
        );
    }

    #[test]
    fn kiro_and_prime_resume_commands_keep_the_selected_id() {
        let shell = CommandShell::Posix;
        assert_eq!(
            Agent::Kiro.resume_cmd("older-session", &shell),
            "kiro-cli chat --resume-id 'older-session'"
        );
        assert_eq!(
            Agent::PrimeAgent.resume_cmd("prime-session", &shell),
            "prime-agent --resume 'prime-session'"
        );
    }

    #[test]
    fn grok_kimi_and_qwen_resume_commands_keep_the_selected_id() {
        let shell = CommandShell::Posix;
        assert_eq!(
            Agent::Grok.resume_cmd("grok-session", &shell),
            "grok --resume 'grok-session'"
        );
        assert_eq!(
            Agent::Kimi.resume_cmd("kimi-session", &shell),
            "kimi --session 'kimi-session'"
        );
        assert_eq!(
            Agent::Qwen.resume_cmd("qwen-session", &shell),
            "qwen --resume 'qwen-session'"
        );
        assert!(!Agent::Grok.supports_delete());
        assert!(!Agent::Kimi.supports_delete());
        assert!(!Agent::Qwen.supports_delete());
        assert!(!Agent::PrimeAgent.supports_delete());
        assert!(Agent::Codex.supports_delete());
    }

    #[test]
    fn oh_my_pi_resume_command_uses_selected_session_id() {
        assert_eq!(
            Agent::OhMyPi.resume_cmd(
                "019e14f4-c9a5-76dc-b7b6-0613e602a620",
                &crate::shell::CommandShell::Posix
            ),
            "omp --resume '019e14f4-c9a5-76dc-b7b6-0613e602a620'"
        );
    }

    #[test]
    fn time_sort_has_a_deterministic_identity_tie_breaker() {
        let make = |agent, id: &str| Session {
            agent,
            session_id: id.into(),
            project_name: "project".into(),
            project_path: "/tmp/project".into(),
            summaries: Vec::new(),
            timestamp: 1_800_000_000_000,
            git_branch: None,
            worktree: None,
            recap: None,
            interactive: true,
        };
        let mut sessions = [
            make(Agent::Codex, "b"),
            make(Agent::ClaudeCode, "z"),
            make(Agent::Codex, "a"),
        ];
        sessions.sort_by(|a, b| compare_sessions(a, b, SortMode::Time));
        let identities: Vec<_> = sessions
            .iter()
            .map(|session| (session.agent, session.session_id.as_str()))
            .collect();
        assert_eq!(
            identities,
            [
                (Agent::ClaudeCode, "z"),
                (Agent::Codex, "a"),
                (Agent::Codex, "b")
            ]
        );
    }

    #[test]
    fn display_path_does_not_tilde_a_home_name_sibling() {
        let Some(home) = dirs::home_dir() else {
            return;
        };
        let session = Session {
            agent: Agent::Codex,
            session_id: "id".into(),
            project_name: "project".into(),
            project_path: format!("{}2/repo", home.display()),
            summaries: Vec::new(),
            timestamp: 0,
            git_branch: None,
            worktree: None,
            recap: None,
            interactive: true,
        };
        assert_eq!(session.display_path(), format!("{}2/repo", home.display()));
    }

    #[test]
    fn invalid_and_future_times_are_visible_not_disguised_as_now() {
        let make = |timestamp| Session {
            agent: Agent::Codex,
            session_id: "id".into(),
            project_name: "p".into(),
            project_path: "/p".into(),
            summaries: Vec::new(),
            timestamp,
            git_branch: None,
            worktree: None,
            recap: None,
            interactive: true,
        };
        assert_eq!(make(0).relative_time_short(), "unknown");
        assert_eq!(
            make(chrono::Utc::now().timestamp_millis() + 10 * 60_000).relative_time_short(),
            "future"
        );
    }
}
