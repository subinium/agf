use crate::model::{Action, Agent, Session};
use crate::shell::CommandShell;

/// A read-only launch description. Availability is a snapshot, not a promise
/// that the provider will accept the session or allow the requested mode.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ResumePlan {
    pub agent: String,
    pub session_id: String,
    pub program: String,
    pub args: Vec<String>,
    pub env: std::collections::BTreeMap<String, String>,
    pub cwd: Option<String>,
    pub executable_found: bool,
    pub working_directory_exists: bool,
}

/// Construct data only: never execute a provider or modify its configuration.
/// `cwd: None` inherits the caller's directory, including Prime's recovery
/// behavior when its recorded directory no longer exists.
pub fn resume_plan(session: &Session, mode: Option<&str>) -> Result<ResumePlan, String> {
    if !crate::model::valid_resume_id(&session.session_id) {
        return Err("invalid resume session ID".to_string());
    }
    // Reject caller-supplied flags before any storage configuration is read.
    session.agent.resume_mode_args(mode)?;
    resume_plan_with_environment(
        session,
        mode,
        crate::config::resume_environment(session.agent)?,
    )
}

fn resume_plan_with_environment(
    session: &Session,
    mode: Option<&str>,
    env: std::collections::BTreeMap<String, String>,
) -> Result<ResumePlan, String> {
    let mut args = session.agent.resume_args(&session.session_id);
    args.extend(session.agent.resume_mode_args(mode)?);
    let path = resume_launch_path(session);
    let program = crate::config::agent_program(session.agent);
    let executable_found = crate::config::is_program_installed(&program);
    Ok(ResumePlan {
        agent: session.agent.slug().to_string(),
        session_id: session.session_id.clone(),
        program,
        args,
        env,
        cwd: (!path.is_empty()).then(|| path.to_string()),
        executable_found,
        working_directory_exists: session.project_path.is_empty()
            || std::path::Path::new(&session.project_path).is_dir(),
    })
}

pub fn generate_command(
    session: &Session,
    action: Action,
    new_agent: Option<Agent>,
) -> Option<String> {
    let shell = CommandShell::from_env();
    let quoted_path = shell.quote(&session.project_path);

    match action {
        Action::Resume => {
            let cmd = session.agent.resume_cmd(&session.session_id, &shell);
            let env = crate::config::resume_environment(session.agent).ok()?;
            let cmd = shell.with_environment(&cmd, &env);
            let resume_path = resume_launch_path(session);
            Some(shell.cd_and(&shell.quote(resume_path), &cmd))
        }
        Action::NewSession => {
            let agent = new_agent.unwrap_or(session.agent);
            let cmd = agent.new_session_command(&shell);
            let env = crate::config::resume_environment(agent).ok()?;
            let cmd = shell.with_environment(&cmd, &env);
            Some(shell.cd_and(&quoted_path, &cmd))
        }
        Action::Open => {
            let editor = detect_editor();
            Some(shell.cd_and(&quoted_path, &format!("{editor} .")))
        }
        Action::Cd if session.project_path.is_empty() => None,
        Action::Cd => Some(shell.cd_only(&quoted_path)),
        Action::Delete | Action::Back | Action::Pin => None,
    }
}

pub fn action_preview(session: &Session, action: Action) -> String {
    let shell = CommandShell::from_env();
    match action {
        Action::Resume => session.agent.resume_cmd(&session.session_id, &shell),
        Action::NewSession => "choose agent CLI...".to_string(),
        Action::Open => format!("{} .", detect_editor()),
        Action::Cd if session.project_path.is_empty() => "no project directory".to_string(),
        Action::Cd => shell.cd_only(&shell.quote(&session.display_path())),
        Action::Pin => "toggle pin".to_string(),
        Action::Delete => "remove session data".to_string(),
        Action::Back => "return to session list".to_string(),
    }
}

/// `cd`-prefixed preview of `cmd`, matching what the generated command will
/// actually do.
///
/// Previews use the `~`-shortened `display_path()` for readability but must
/// still *quote* it the way the executed command does, and must drop the `cd`
/// entirely for cwd-independent agents — `display_path()` renders their empty
/// `project_path` as `—`, which `cd_and`'s empty-path check cannot recognise,
/// so the preview used to read `cd — && hermes`.
pub fn preview_cd_and(shell: &CommandShell, session: &Session, cmd: &str) -> String {
    if session.project_path.is_empty() {
        return cmd.to_string();
    }
    shell.cd_and(&shell.quote(&session.display_path()), cmd)
}

/// Detect editor from config, then $EDITOR, then $VISUAL, fallback to "vim".
///
/// Resolved once per process and cached: `save_editable()` never writes the
/// `editor` key, so the value cannot change observably mid-run, and this is
/// called from the action-menu render path every frame (same rationale as
/// `CommandShell::from_env` in shell.rs).
pub fn detect_editor() -> String {
    use std::sync::OnceLock;
    static EDITOR: OnceLock<String> = OnceLock::new();
    EDITOR
        .get_or_init(|| {
            let config = crate::settings::Settings::load();
            if let Some(editor) = config.editor.filter(|e| !e.is_empty()) {
                return editor;
            }
            std::env::var("EDITOR")
                .ok()
                .filter(|e| !e.is_empty())
                .or_else(|| std::env::var("VISUAL").ok().filter(|e| !e.is_empty()))
                .unwrap_or_else(|| "vim".to_string())
        })
        .clone()
}

pub fn resume_with_flags(session: &Session, flags: &str) -> String {
    let shell = CommandShell::from_env();
    let quoted_path = shell.quote(resume_launch_path(session));
    let base_cmd = session.agent.resume_cmd(&session.session_id, &shell);
    let cmd = scoped_launch(&shell, session.agent, &format!("{base_cmd}{flags}"));
    shell.cd_and(&quoted_path, &cmd)
}

fn scoped_launch(shell: &CommandShell, agent: Agent, command: &str) -> String {
    match crate::config::resume_environment(agent) {
        Ok(env) => shell.with_environment(command, &env),
        Err(error) => shell.error_command(&format!("agf: invalid resume plan: {error}")),
    }
}

fn resume_launch_path(session: &Session) -> &str {
    if session.agent == Agent::PrimeAgent
        && !session.project_path.is_empty()
        && !std::path::Path::new(&session.project_path).is_dir()
    {
        // Prime Agent can resolve/fork an ID from another cwd. A guaranteed
        // failing `cd` prevents its own recovery prompt from running at all.
        ""
    } else {
        &session.project_path
    }
}

pub fn new_session_with_flags(session: &Session, agent: Agent, flags: &str) -> String {
    let shell = CommandShell::from_env();
    let quoted_path = shell.quote(&session.project_path);
    let base = agent.new_session_command(&shell);
    let cmd = scoped_launch(&shell, agent, &format!("{base}{flags}"));
    shell.cd_and(&quoted_path, &cmd)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Agent;

    fn session(project_path: &str) -> Session {
        Session {
            agent: Agent::Hermes,
            session_id: "sid".to_string(),
            project_name: "p".to_string(),
            project_path: project_path.to_string(),
            summaries: Vec::new(),
            timestamp: 0,
            git_branch: None,
            worktree: None,
            recap: None,
            interactive: true,
        }
    }

    fn test_plan(session: &Session, mode: Option<&str>) -> Result<ResumePlan, String> {
        resume_plan_with_environment(session, mode, std::collections::BTreeMap::new())
    }

    /// Regression: the preview passed the raw `~`-shortened path straight into
    /// `cd_and`, so a path with a space rendered a command that would not run.
    #[test]
    fn preview_cd_and_quotes_the_path_like_the_real_command() {
        let shell = CommandShell::Posix;
        let s = session("/tmp/my project");
        assert_eq!(
            preview_cd_and(&shell, &s, "hermes"),
            "cd '/tmp/my project' && hermes"
        );
    }

    /// Regression: cwd-independent agents leave `project_path` empty, but
    /// `display_path()` renders that as `—`, which `cd_and`'s empty check
    /// cannot recognise — the preview used to read `cd — && hermes`.
    #[test]
    fn preview_cd_and_drops_the_cd_for_cwd_independent_agents() {
        let shell = CommandShell::Posix;
        assert_eq!(preview_cd_and(&shell, &session(""), "hermes"), "hermes");
    }

    #[test]
    fn cd_preview_reports_missing_directory_instead_of_a_broken_command() {
        assert!(generate_command(&session(""), Action::Cd, None).is_none());
        assert_eq!(
            action_preview(&session(""), Action::Cd),
            "no project directory"
        );
    }

    #[test]
    fn prime_resume_skips_a_deleted_stored_cwd() {
        let mut s = session("/definitely/missing/agf-prime-project");
        s.agent = Agent::PrimeAgent;
        assert_eq!(resume_launch_path(&s), "");
        assert_eq!(s.agent.resume_args(&s.session_id), ["--resume", "sid"]);
    }

    #[test]
    fn resume_plan_preserves_data_and_rejects_arbitrary_flags() {
        let mut s = session("/definitely/missing/agf-project");
        s.agent = Agent::Codex;
        s.session_id = "a'b; $(echo nope)".into();
        let plan = test_plan(&s, Some("workspace-write")).unwrap();
        assert_eq!(plan.agent, "codex");
        assert_eq!(
            plan.args,
            [
                "resume",
                "a'b; $(echo nope)",
                "-a",
                "on-request",
                "-s",
                "workspace-write"
            ]
        );
        assert_eq!(plan.cwd.as_deref(), Some(s.project_path.as_str()));
        assert!(!plan.working_directory_exists);
        assert!(resume_plan(&s, Some("--dangerously-bypass-approvals-and-sandbox")).is_err());
        assert!(resume_plan(&s, Some("default; echo nope")).is_err());
        assert!(
            serde_json::to_value(plan)
                .unwrap()
                .get("executable_found")
                .unwrap()
                .is_boolean()
        );
    }

    #[test]
    fn resume_plan_rejects_invalid_ids_before_modes_or_storage_resolution() {
        let mut s = session("");
        s.agent = Agent::Codex;
        for id in [
            "",
            "--dangerously-skip-permissions",
            "id\0tail",
            "id\ntail",
            "id\u{85}tail",
        ] {
            s.session_id = id.to_string();
            assert_eq!(
                resume_plan(&s, Some("invalid-mode")).unwrap_err(),
                "invalid resume session ID"
            );
        }
        s.session_id = "a".repeat(1025);
        assert_eq!(
            resume_plan(&s, None).unwrap_err(),
            "invalid resume session ID"
        );
    }

    #[test]
    fn resume_plan_retains_inherited_directory_semantics() {
        let s = session("");
        let plan = test_plan(&s, None).unwrap();
        assert!(plan.cwd.is_none());
        assert!(plan.working_directory_exists);
        assert_eq!(plan.args, ["--resume", "sid"]);
        let mut prime = session("/definitely/missing/agf-prime-project");
        prime.agent = Agent::PrimeAgent;
        let plan = test_plan(&prime, Some("default")).unwrap();
        assert!(plan.cwd.is_none());
        assert!(!plan.working_directory_exists);
    }

    #[test]
    fn resume_plan_default_is_opt_in_and_covers_every_provider() {
        for agent in Agent::all() {
            let mut s = session("");
            s.agent = *agent;
            let plan = test_plan(&s, None).unwrap();
            assert_eq!(plan.args, agent.resume_args("sid"));
            assert_eq!(plan.args, test_plan(&s, Some("default")).unwrap().args);
            for (mode, flags) in agent.resume_mode_options() {
                let plan = test_plan(&s, Some(mode)).unwrap();
                let mut expected = agent.resume_args("sid");
                expected.extend(flags.split_ascii_whitespace().map(str::to_string));
                assert_eq!(plan.args, expected);
            }
        }
    }
}
