use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
    home: PathBuf,
    project: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "agf-api-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let home = root.join("home");
        let project = root.join("project");
        fs::create_dir_all(home.join(".claude/projects/test")).unwrap();
        fs::create_dir(&project).unwrap();
        Self {
            root,
            home,
            project,
        }
    }

    fn seed(&self, id: &str) {
        fs::write(
            self.home.join(".claude/history.jsonl"),
            format!(
                "{}\n",
                json!({
                    "display": "private needle 한글", "timestamp": 1_788_566_400_000_u64,
                    "project": self.project, "sessionId": id,
                })
            ),
        )
        .unwrap();
        fs::write(self.home.join(".claude/projects/test").join(format!("{id}.jsonl")), format!("{}\n", json!({
            "type": "user", "cwd": self.project, "message": {"role": "user", "content": "private needle 한글"},
        }))).unwrap();
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_agf"));
        command
            .env_clear()
            .env("HOME", &self.home)
            .env("USERPROFILE", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join("config"))
            .env("XDG_DATA_HOME", self.home.join("data"))
            .env("XDG_CACHE_HOME", self.home.join("cache"))
            .env("APPDATA", self.home.join("appdata"))
            .env("LOCALAPPDATA", self.home.join("localappdata"))
            .env("PATH", self.root.join("absent-bin"))
            .env("CLAUDE_CONFIG_DIR", self.home.join(".claude"))
            .env("ANTHROPIC_API_KEY", "must-not-leak-secret")
            .current_dir(&self.project);
        #[cfg(windows)]
        if let Some(root) = std::env::var_os("SystemRoot") {
            command.env("SystemRoot", root);
        }
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command().args(args).output().unwrap()
    }
    fn json(&self, args: &[&str]) -> Value {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn search_show_and_plan_use_exact_identity_and_no_mutating_side_effects() {
    let fixture = Fixture::new();
    let id = "id with ' quote;$X";
    fixture.seed(id);
    let history = fixture.home.join(".claude/history.jsonl");
    let before = fs::read(&history).unwrap();
    let before_modified = fs::metadata(&history).unwrap().modified().unwrap();
    let result = fixture.json(&["search", "--agent", "claude"]);
    assert_eq!(result["schema_version"], 1);
    assert_eq!(result["agf_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(result["data"]["sessions"][0]["session_id"], id);
    assert!(result["data"]["sessions"][0].get("summaries").is_none());
    let hidden = fixture.json(&["search", "needle", "--agent", "claude"]);
    assert_eq!(hidden["data"]["total"], 0);
    let explicit = fixture.json(&[
        "search",
        "needle",
        "--agent",
        "claude",
        "--include-summaries",
    ]);
    assert_eq!(explicit["data"]["total"], 1);
    let shown = fixture.json(&["show", id, "--agent", "claude", "--include-summaries"]);
    assert_eq!(
        shown["data"]["session"]["summaries"][0],
        "private needle 한글"
    );
    let planned = fixture.json(&["resume-plan", id, "--agent", "claude"]);
    assert_eq!(planned["data"]["executed"], false);
    assert_eq!(planned["data"]["plan"]["program"], "claude");
    assert_eq!(planned["data"]["plan"]["args"], json!(["--resume", id]));
    assert_eq!(planned["data"]["plan"]["cwd"], json!(fixture.project));
    assert_eq!(
        planned["data"]["plan"]["env"]["CLAUDE_CONFIG_DIR"],
        json!(fixture.home.join(".claude"))
    );
    assert_eq!(planned["data"]["plan"]["executable_found"], false);
    assert!(!planned.to_string().contains("must-not-leak-secret"));
    assert_eq!(fs::read(&history).unwrap(), before);
    assert_eq!(
        fs::metadata(&history).unwrap().modified().unwrap(),
        before_modified
    );
    for path in [
        fixture.home.join("cache/agf"),
        fixture.home.join("Library/Caches/agf"),
    ] {
        assert!(!path.exists(), "read-only API created a persistent cache");
    }
}

#[test]
fn empty_search_and_legacy_json_list_are_successful_arrays() {
    let fixture = Fixture::new();
    let result = fixture.json(&["search", "--agent", "claude"]);
    assert_eq!(result["ok"], true);
    assert_eq!(result["data"]["sessions"], json!([]));
    assert_eq!(
        fixture.json(&["list", "--agent", "claude", "--format", "json"]),
        json!([])
    );
}

#[test]
fn semantic_errors_have_a_versioned_error_and_nonzero_status() {
    let fixture = Fixture::new();
    for (args, code) in [
        (vec!["search", "--agent", "not-an-agent"], "invalid_agent"),
        (vec!["search", "--limit", "0"], "invalid_request"),
        (vec!["search", "--limit", "201"], "invalid_request"),
        (vec!["show", "missing", "--agent", "claude"], "not_found"),
    ] {
        let output = fixture.run(&args);
        assert!(!output.status.success());
        let error: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(error["schema_version"], 1);
        assert_eq!(error["ok"], false);
        assert_eq!(error["error"]["code"], code);
    }
}

#[test]
fn known_project_scope_and_literal_version_query_are_preserved() {
    let fixture = Fixture::new();
    fixture.seed("one");
    let missing = fixture.root.join("other");
    let scoped = fixture.json(&[
        "search",
        "--agent",
        "claude",
        "--project",
        missing.to_str().unwrap(),
    ]);
    assert_eq!(scoped["data"]["total"], 0);
    let literal = fixture.json(&["search", "--agent", "claude", "--", "--version"]);
    assert_eq!(literal["data"]["total"], 0);
    let version = fixture.run(&["--version"]);
    assert!(version.status.success());
    assert_eq!(
        String::from_utf8(version.stdout).unwrap(),
        format!("agf {}\n", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn invalid_formats_and_option_like_source_ids_cannot_launch() {
    let fixture = Fixture::new();
    fixture.seed("--dangerously-skip-permissions");
    let listed = fixture.json(&["search", "--agent", "claude"]);
    assert_eq!(listed["data"]["total"], 0);
    let resume = fixture.run(&["resume", "--agent", "claude"]);
    assert!(!resume.status.success());
    assert!(!String::from_utf8_lossy(&resume.stdout).contains("claude --resume"));
    let format = fixture.run(&["list", "--format", "typo"]);
    assert_eq!(format.status.code(), Some(2));
    assert!(format.stdout.is_empty());
}

#[test]
fn malformed_provider_config_does_not_echo_secret_values() {
    let fixture = Fixture::new();
    let codex = fixture.home.join(".codex");
    fs::create_dir(&codex).unwrap();
    fs::write(
        codex.join("config.toml"),
        "unrelated_secret = \"CONFIG_SECRET_MUST_NOT_LEAK\" invalid\n",
    )
    .unwrap();
    let output = fixture
        .command()
        .env("CODEX_HOME", codex)
        .args(["search", "--agent", "codex"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["error"]["code"], "scan_failed");
    assert!(!String::from_utf8_lossy(&output.stdout).contains("CONFIG_SECRET_MUST_NOT_LEAK"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("CONFIG_SECRET_MUST_NOT_LEAK"));
}

// APFS requires valid Unicode filenames; this filesystem case targets Linux.
#[cfg(target_os = "linux")]
#[test]
fn non_unicode_canonical_scope_is_rejected_without_panicking() {
    use std::os::unix::ffi::OsStringExt;
    let fixture = Fixture::new();
    let target = fixture
        .root
        .join(std::ffi::OsString::from_vec(vec![b'p', 0xff]));
    fs::create_dir(&target).unwrap();
    let alias = fixture.root.join("alias");
    std::os::unix::fs::symlink(target, &alias).unwrap();
    let output = fixture.run(&["capabilities", "--project", alias.to_str().unwrap()]);
    assert!(!output.status.success());
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["error"]["code"], "invalid_request");
}
