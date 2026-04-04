use std::io;
use std::path::PathBuf;

use crate::model::{Agent, Session};

/// Trait for agent plugins. Each AI agent scanner implements this.
/// Some methods are reserved for future use (e.g., direct dispatch instead of match on Agent).
#[allow(dead_code)]
pub trait AgentPlugin: Send + Sync {
    fn agent(&self) -> Agent;
    fn name(&self) -> &str;
    fn cli_name(&self) -> &str;
    fn color(&self) -> (u8, u8, u8);
    fn scan(&self) -> Vec<Session>;
    fn delete(&self, session: &Session) -> Result<(), io::Error>;
    fn resume_cmd(&self, session_id: &str) -> String;
    fn new_session_cmd(&self) -> &str;
    fn resume_mode_options(&self) -> &[(&str, &str)] {
        &[("default", "")]
    }
    /// Paths to check for mtime-based cache invalidation.
    fn data_sources(&self) -> Vec<PathBuf>;
}

/// Return all registered agent plugins.
pub fn all_plugins() -> Vec<Box<dyn AgentPlugin>> {
    vec![
        Box::new(PluginAdapter(Agent::ClaudeCode)),
        Box::new(PluginAdapter(Agent::Codex)),
        Box::new(PluginAdapter(Agent::OpenCode)),
        Box::new(PluginAdapter(Agent::Pi)),
        Box::new(PluginAdapter(Agent::Kiro)),
        Box::new(PluginAdapter(Agent::CursorAgent)),
        Box::new(PluginAdapter(Agent::Gemini)),
    ]
}

/// Adapter that bridges the existing Agent enum methods to the AgentPlugin trait.
/// This allows incremental migration — scanners can be moved to direct trait impls later.
struct PluginAdapter(Agent);

impl AgentPlugin for PluginAdapter {
    fn agent(&self) -> Agent {
        self.0
    }

    fn name(&self) -> &str {
        match self.0 {
            Agent::ClaudeCode => "Claude Code",
            Agent::Codex => "Codex",
            Agent::OpenCode => "OpenCode",
            Agent::Pi => "pi",
            Agent::Kiro => "Kiro",
            Agent::CursorAgent => "Cursor CLI",
            Agent::Gemini => "Gemini",
        }
    }

    fn cli_name(&self) -> &str {
        self.0.cli_name()
    }

    fn color(&self) -> (u8, u8, u8) {
        self.0.color()
    }

    fn scan(&self) -> Vec<Session> {
        use crate::scanner;
        match self.0 {
            Agent::ClaudeCode => scanner::claude::scan().unwrap_or_default(),
            Agent::Codex => scanner::codex::scan().unwrap_or_default(),
            Agent::OpenCode => scanner::opencode::scan().unwrap_or_default(),
            Agent::Pi => scanner::pi::scan().unwrap_or_default(),
            Agent::Kiro => scanner::kiro::scan().unwrap_or_default(),
            Agent::CursorAgent => scanner::cursor_agent::scan().unwrap_or_default(),
            Agent::Gemini => scanner::gemini::scan().unwrap_or_default(),
        }
    }

    fn delete(&self, session: &Session) -> Result<(), io::Error> {
        crate::delete::delete_session(session)
    }

    fn resume_cmd(&self, session_id: &str) -> String {
        self.0.resume_cmd(session_id)
    }

    fn new_session_cmd(&self) -> &str {
        self.0.new_session_cmd()
    }

    fn resume_mode_options(&self) -> &[(&str, &str)] {
        self.0.resume_mode_options()
    }

    fn data_sources(&self) -> Vec<PathBuf> {
        use crate::config;
        match self.0 {
            Agent::ClaudeCode => config::claude_dir()
                .map(|d| vec![d.join("history.jsonl")])
                .unwrap_or_default(),
            Agent::Codex => config::codex_dir().map(|d| vec![d]).unwrap_or_default(),
            Agent::OpenCode => config::opencode_data_dir()
                .map(|d| vec![d.join("opencode.db")])
                .unwrap_or_default(),
            Agent::Pi => config::pi_sessions_dir()
                .map(|d| vec![d])
                .unwrap_or_default(),
            Agent::Kiro => config::kiro_data_dir()
                .map(|d| vec![d.join("data.sqlite3")])
                .unwrap_or_default(),
            Agent::CursorAgent => config::cursor_dir()
                .map(|d| vec![d.join("chats"), d.join("projects")])
                .unwrap_or_default(),
            Agent::Gemini => config::gemini_dir()
                .map(|d| vec![d.join("tmp")])
                .unwrap_or_default(),
        }
    }
}
