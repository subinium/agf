use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

fn capabilities(with_test_environment: bool) -> Output {
    let absent_bin = std::env::temp_dir().join(format!(
        "agf-antigravity-capabilities-{}-{}-absent-bin",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    assert!(!absent_bin.exists());
    let mut command = Command::new(env!("CARGO_BIN_EXE_agf"));
    command
        .env_clear()
        .env("PATH", absent_bin)
        .args(["capabilities", "--agent", "agy"]);
    #[cfg(windows)]
    if let Some(system_root) = std::env::var_os("SystemRoot") {
        command.env("SystemRoot", system_root);
    }
    if with_test_environment {
        // These variables belong to the unit-test helper, never the production binary.
        command
            .env("AGF_ANTIGRAVITY_CLI_TEST_CHILD", "1")
            .env("AGF_ANTIGRAVITY_CLI_TEST_HOME", "")
            .env("AGF_ANTIGRAVITY_CLI_TEST_ARGS", "not a JSON argument array");
    }
    command.output().unwrap()
}

#[test]
fn production_capabilities_register_antigravity_without_test_runtime_overrides() {
    let baseline = capabilities(false);
    assert!(
        baseline.status.success(),
        "{}",
        String::from_utf8_lossy(&baseline.stderr)
    );
    let result: Value = serde_json::from_slice(&baseline.stdout).unwrap();
    assert_eq!(result["schema_version"], 1);
    assert_eq!(result["agf_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(result["ok"], true);
    let providers = result["data"]["providers"].as_array().unwrap();
    assert_eq!(providers.len(), 1);
    assert_eq!(providers[0]["id"], "antigravity");
    assert_eq!(providers[0]["command"], "agy");
    assert_eq!(providers[0]["program"], "agy");
    assert_eq!(providers[0]["installed"], false);
    assert_eq!(providers[0]["version_probe"], "not_run");
    assert_eq!(result["data"]["read_only"], true);
    assert_eq!(result["data"]["launches_agents"], false);
    assert_eq!(result["data"]["writes_agent_stores"], false);

    let with_test_environment = capabilities(true);
    assert!(
        with_test_environment.status.success(),
        "{}",
        String::from_utf8_lossy(&with_test_environment.stderr)
    );
    assert_eq!(with_test_environment.status.code(), baseline.status.code());
    assert_eq!(with_test_environment.stdout, baseline.stdout);
    assert_eq!(with_test_environment.stderr, baseline.stderr);
}
