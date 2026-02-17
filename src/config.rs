use std::path::PathBuf;
use std::process::Command;

use crate::error::AgfError;
use crate::model::Agent;

pub fn home_dir() -> Result<PathBuf, AgfError> {
    dirs::home_dir().ok_or(AgfError::NoHomeDir)
}

pub fn claude_dir() -> Result<PathBuf, AgfError> {
    Ok(home_dir()?.join(".claude"))
}

pub fn codex_dir() -> Result<PathBuf, AgfError> {
    Ok(home_dir()?.join(".codex"))
}

pub fn cursor_dir() -> Result<PathBuf, AgfError> {
    Ok(home_dir()?.join(".cursor"))
}

pub fn is_agent_installed(agent: Agent) -> bool {
    let cmd = match agent {
        Agent::ClaudeCode => "claude",
        Agent::Codex => "codex",
        Agent::Cursor => "cursor",
    };
    Command::new("which")
        .arg(cmd)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

pub fn installed_agents() -> Vec<Agent> {
    Agent::all()
        .iter()
        .copied()
        .filter(|a| is_agent_installed(*a))
        .collect()
}
