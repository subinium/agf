use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Value, json};

const ID: &str = "0198f0aa-1111-7000-8000-0000000000aa";
const LOG: &str = include_str!("fixtures/muse/session.jsonl");

struct Fixture {
    root: PathBuf,
    project: PathBuf,
    log: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "agf-muse-cli-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let project = root.join("project with spaces");
        fs::create_dir_all(&project).unwrap();
        let log = root
            .join("data/muse/sessions/2026/09/01")
            .join(ID)
            .join("session.jsonl");
        fs::create_dir_all(log.parent().unwrap()).unwrap();
        // Preserve JSON escaping for native Windows paths.
        let contents = LOG
            .lines()
            .map(|line| {
                let mut record: Value = serde_json::from_str(line).unwrap();
                if record["payload_type"] == "runtime.session.metadata" {
                    record["payload"]["record"]["workspace_root"] = json!(project);
                }
                record.to_string() + "\n"
            })
            .collect::<String>();
        fs::write(&log, contents).unwrap();
        Self { root, project, log }
    }
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_agf"));
        command
            .env_clear()
            .env("HOME", self.root.join("home"))
            .env("USERPROFILE", self.root.join("home"))
            .env("XDG_DATA_HOME", "data")
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_CACHE_HOME", self.root.join("cache"))
            .env("APPDATA", self.root.join("appdata"))
            .env("LOCALAPPDATA", self.root.join("localappdata"))
            .env("PATH", self.root.join("absent-bin"))
            .env("META_API_KEY", "must-not-leak")
            .current_dir(&self.root);
        #[cfg(windows)]
        if let Some(root) = std::env::var_os("SystemRoot") {
            command.env("SystemRoot", root);
        }
        command
    }
    fn json(&self, args: &[&str]) -> Value {
        let output = self.command().args(args).output().unwrap();
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
fn search_show_and_resume_use_native_identity_and_preserve_storage() {
    let fixture = Fixture::new();
    let before = fs::read(&fixture.log).unwrap();
    let mtime = fixture.log.metadata().unwrap().modified().unwrap();
    let index = fixture.root.join("data/muse/session-index.db");
    fs::write(&index, b"index must not be opened or modified").unwrap();
    let result = fixture.json(&["search", "--agent", "muse-code"]);
    assert_eq!(result["data"]["sessions"][0]["agent"], "muse");
    assert_eq!(result["data"]["sessions"][0]["session_id"], ID);
    assert!(result["data"]["sessions"][0].get("summaries").is_none());
    let hidden = fixture.json(&["search", "한국어", "--agent", "muse"]);
    assert_eq!(hidden["data"]["total"], 0);
    let found = fixture.json(&["search", "한국어", "--agent", "muse", "--include-summaries"]);
    assert_eq!(found["data"]["total"], 1);
    let shown = fixture.json(&["show", ID, "--agent", "muse", "--include-summaries"]);
    assert_eq!(
        shown["data"]["session"]["summaries"][0],
        "AGF fixture prompt 한국어"
    );
    for (mode, extra) in [
        ("default", None),
        ("no approval (sandbox on)", Some("--disable-approval")),
        ("yolo (no sandbox)", Some("--yolo")),
    ] {
        let result = fixture.json(&["resume-plan", ID, "--agent", "muse", "--mode", mode]);
        let plan = &result["data"]["plan"];
        let mut args = vec!["resume", ID];
        args.extend(extra);
        assert_eq!(plan["program"], "muse");
        assert_eq!(plan["args"], json!(args));
        assert_eq!(plan["cwd"], json!(fixture.project));
        assert_eq!(plan["env"].as_object().unwrap().len(), 1);
        let storage = PathBuf::from(plan["env"]["XDG_DATA_HOME"].as_str().unwrap());
        assert!(storage.is_absolute());
        // Windows current_dir may omit canonicalize's verbatim path prefix.
        assert_eq!(storage.canonicalize().unwrap(), fixture.root.join("data"));
        assert_eq!(plan["executable_found"], false);
        assert_eq!(result["data"]["executed"], false);
        assert!(!result.to_string().contains("must-not-leak"));
    }
    assert_eq!(fs::read(&fixture.log).unwrap(), before);
    assert_eq!(fixture.log.metadata().unwrap().modified().unwrap(), mtime);
    assert_eq!(
        fs::read(index).unwrap(),
        b"index must not be opened or modified"
    );
    assert!(!fixture.root.join("cache/agf").exists());
}

#[test]
fn capabilities_and_legacy_list_register_muse_and_report_scan_failures() {
    let fixture = Fixture::new();
    let capabilities = fixture.json(&["capabilities", "--agent", "muse"]);
    assert_eq!(capabilities["data"]["providers"][0]["id"], "muse");
    assert_eq!(capabilities["data"]["providers"][0]["name"], "Muse Code");
    let listed = fixture.json(&["list", "--agent", "muse", "--format", "json"]);
    assert_eq!(listed.as_array().unwrap().len(), 1);
    fs::remove_dir_all(fixture.root.join("data/muse/sessions")).unwrap();
    assert_eq!(
        fixture.json(&["search", "--agent", "muse"])["data"]["total"],
        0
    );
    fs::write(fixture.root.join("data/muse/sessions"), "not a directory").unwrap();
    let output = fixture
        .command()
        .args(["search", "--agent", "muse"])
        .output()
        .unwrap();
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(!output.status.success());
    assert!(body.to_string().contains("scan_failed"));
}
