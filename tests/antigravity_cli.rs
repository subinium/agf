use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, params};
use serde_json::{Value, json};

const ID: &str = "c96a140c-d4c0-4996-9b9b-03a0468b1fcc";
const CHILD: &str = "24f72224-8c55-49dc-95c2-ac6191a0e852";

struct Fixture {
    root: PathBuf,
    home: PathBuf,
    store: PathBuf,
    project: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "agf-antigravity-{}-{}-{}",
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
        let store = home.join(".gemini/antigravity-cli");
        let project = root.join("project's \u{d55c}\u{ae00}");
        fs::create_dir_all(&store).unwrap();
        fs::create_dir(&project).unwrap();
        Self {
            root,
            home,
            store,
            project,
        }
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
            .env("AGF_SHELL", "posix")
            .current_dir(&self.project);
        #[cfg(windows)]
        if let Some(system_root) = std::env::var_os("SystemRoot") {
            command.env("SystemRoot", system_root);
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

    fn database(&self) -> Connection {
        let connection = Connection::open(self.store.join("conversation_summaries.db")).unwrap();
        connection
            .execute_batch(
                "PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;
            CREATE TABLE conversation_summaries (
                conversation_id TEXT PRIMARY KEY, title TEXT, preview TEXT,
                workspace_uris TEXT, last_modified_time TEXT,
                parent_conversation_id TEXT, nesting_depth INTEGER
            );",
            )
            .unwrap();
        let workspace = json!([url::Url::from_directory_path(&self.project)
            .unwrap()
            .as_str()])
        .to_string();
        for (id, parent, depth) in [(ID, "", 0), (CHILD, ID, 1)] {
            connection
                .execute(
                    "INSERT INTO conversation_summaries VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        id,
                        "private synthetic title",
                        "private preview",
                        workspace,
                        "2026-09-18T05:31:00Z",
                        parent,
                        depth
                    ],
                )
                .unwrap();
        }
        let transcript = self
            .store
            .join("brain")
            .join(ID)
            .join(".system_generated/logs/transcript.jsonl");
        fs::create_dir_all(transcript.parent().unwrap()).unwrap();
        fs::write(transcript, format!("{}\n", json!({
            "type": "USER_INPUT", "created_at": "2026-09-18T05:30:00Z",
            "content": "<USER_REQUEST>private synthetic prompt</USER_REQUEST><ADDITIONAL_METADATA>do not show</ADDITIONAL_METADATA>"
        }))).unwrap();
        connection
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn content_snapshot(root: &Path) -> Vec<(PathBuf, Vec<u8>, SystemTime)> {
    let mut files = walkdir::WalkDir::new(root)
        .into_iter()
        .map(Result::unwrap)
        .filter(|entry| {
            entry.file_type().is_file() && !entry.path().to_string_lossy().ends_with("-shm")
        })
        .map(|entry| {
            let path = entry.into_path();
            let content = fs::read(&path).unwrap();
            let modified = fs::metadata(&path).unwrap().modified().unwrap();
            (path, content, modified)
        })
        .collect::<Vec<_>>();
    files.sort_by(|a, b| a.0.cmp(&b.0));
    files
}

#[test]
fn antigravity_read_only_api_preserves_wal_and_exact_resume_contract() {
    let fixture = Fixture::new();
    let _writer = fixture.database();
    let before = content_snapshot(&fixture.store);
    let result = fixture.json(&["search", "--agent", "agy"]);
    assert_eq!(result["data"]["total"], 1);
    let session = &result["data"]["sessions"][0];
    assert_eq!(session["agent"], "antigravity");
    assert_eq!(session["session_id"], ID);
    assert!(session.get("summaries").is_none());
    assert_eq!(
        Path::new(session["project_path"].as_str().unwrap())
            .canonicalize()
            .unwrap(),
        fixture.project
    );
    let all = fixture.json(&[
        "search",
        "--agent",
        "antigravity",
        "--include-non-interactive",
    ]);
    assert_eq!(all["data"]["total"], 2);
    let shown = fixture.json(&["show", ID, "--agent", "agy", "--include-summaries"]);
    assert!(shown.to_string().contains("private synthetic prompt"));
    assert!(!shown.to_string().contains("do not show"));
    for (mode, suffix) in [
        (None, vec![]),
        (Some("accept-edits"), vec!["--mode", "accept-edits"]),
        (Some("plan (read-only)"), vec!["--mode", "plan"]),
        (
            Some("bypass permissions"),
            vec!["--dangerously-skip-permissions"],
        ),
        (Some("sandbox"), vec!["--sandbox"]),
    ] {
        let mut args = vec!["resume-plan", ID, "--agent", "antigravity"];
        if let Some(mode) = mode {
            args.extend(["--mode", mode]);
        }
        let plan = fixture.json(&args);
        let mut expected = vec!["--conversation", ID];
        expected.extend(suffix);
        assert_eq!(plan["data"]["plan"]["args"], json!(expected));
        assert_eq!(plan["data"]["plan"]["program"], "agy");
        assert_eq!(plan["data"]["executed"], false);
        assert_eq!(plan["data"]["plan"]["executable_found"], false);
        assert_eq!(plan["data"]["plan"]["working_directory_exists"], true);
    }
    assert!(
        !fixture
            .run(&["resume-plan", ID, "--agent", "agy", "--mode", "--untrusted"])
            .status
            .success()
    );
    assert_eq!(content_snapshot(&fixture.store), before);
    assert!(!fixture.home.join("cache/agf").exists());
}

#[test]
fn antigravity_scan_errors_are_not_empty_success() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.store.join("conversation_summaries.db")).unwrap();
    let output = fixture.run(&["search", "--agent", "antigravity"]);
    assert!(!output.status.success());
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["error"]["code"], "scan_failed");
    let mixed = fixture.json(&["search"]);
    assert!(
        mixed["data"]["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| {
                warning["agent"] == "antigravity" && warning["code"] == "scan_failed"
            })
    );
}

#[test]
fn antigravity_is_registered_in_capabilities_and_empty_stores_are_valid() {
    let fixture = Fixture::new();
    let capabilities = fixture.json(&["capabilities", "--agent", "agy"]);
    let providers = capabilities["data"]["providers"].as_array().unwrap();
    assert_eq!(providers.len(), 1);
    assert!(providers[0].to_string().contains("antigravity"));
    let empty = fixture.json(&["search", "--agent", "agy"]);
    assert_eq!(empty["data"]["total"], 0);
    assert_eq!(empty["data"]["warnings"], json!([]));
}

#[test]
fn unverified_antigravity_home_does_not_rebase_scanning_or_resume() {
    let fixture = Fixture::new();
    let _writer = fixture.database();
    let output = fixture
        .command()
        .env("ANTIGRAVITY_CLI_HOME", "../unsupported root")
        .args(["resume-plan", ID, "--agent", "antigravity"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["data"]["plan"]["session_id"], ID);
    assert!(
        result["data"]["plan"]["env"]
            .get("ANTIGRAVITY_CLI_HOME")
            .is_none()
    );
    assert!(!fixture.root.join("unsupported root").exists());
}
