#![cfg(unix)]

use std::path::PathBuf;
use std::process::Command;

fn python() -> PathBuf {
    if let Some(path) = std::env::var_os("AGF_TEST_PYTHON") {
        let path = PathBuf::from(path);
        assert!(
            path.is_absolute() && path.is_file(),
            "AGF_TEST_PYTHON must name an absolute Python 3 executable"
        );
        return path;
    }
    PathBuf::from("python3")
}

fn python_command() -> Command {
    let mut command = Command::new(python());
    command
        .args(["-I", "-X", "utf8"])
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("LC_ALL", "C")
        .env("TZ", "UTC");
    command
}

fn run_case(case: &str) {
    let helper = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/support/runtime_pty.py");
    let output = python_command()
        .arg(helper)
        .arg(case)
        .arg(env!("CARGO_BIN_EXE_agf"))
        .output()
        .expect("start isolated Python PTY controller");
    assert!(
        output.status.success(),
        "PTY case {case} failed ({:?})\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)
        .expect("PTY controller must return its verification matrix");
    assert_eq!(report["case"], case);
    assert_eq!(report["passed"], true);
    println!("{}", String::from_utf8_lossy(&output.stdout).trim());
}

#[test]
fn real_tui_resize_utf8_paste_and_escape_restore_terminal() {
    run_case("render_resize_input_quit");
}

#[test]
fn real_tui_resume_handoff_preserves_literal_arguments_and_cooked_terminal() {
    run_case("resume_handoff");
}

#[test]
fn terminal_screen_replays_diff_and_partial_frames() {
    let helper =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/support/runtime_screen_test.py");
    let output = python_command()
        .arg(helper)
        .output()
        .expect("start deterministic terminal screen replay tests");
    assert!(
        output.status.success(),
        "screen replay tests failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    println!("{}", String::from_utf8_lossy(&output.stderr).trim());
}
