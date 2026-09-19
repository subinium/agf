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
    if case.starts_with("theme_") && case != "theme_settings" {
        for check in [
            "neutral_markers_pins_numbers",
            "grouped_only_agent_is_branded",
            "danger_label_and_checkbox",
            "footer_accent",
            "search_underline",
        ] {
            assert_eq!(report["checks"][check], true, "missing theme check {check}");
        }
        for agent in ["claude", "codex"] {
            assert!(report["checks"]["agent_brand_colors"][agent].is_string());
        }
        if case.ends_with("_basic") {
            assert_eq!(report["checks"]["basic_monochrome_fallback"], true);
            assert_eq!(report["checks"]["basic_status_markers"], true);
            assert!(report["checks"]["selected_rgb_contrast_minimum"].is_null());
        }
    }
    if case.starts_with("watch_") {
        for check in [
            "watch_color_roles",
            "watch_unknown_process_status",
            "watch_compact_resize",
            "no_real_process_probe",
        ] {
            assert_eq!(report["checks"][check], true, "missing watch check {check}");
        }
    }
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
fn real_tui_antigravity_conversation_mode_handoff_and_disabled_delete() {
    run_case("antigravity_handoff");
}

#[test]
fn real_tui_antigravity_plan_handoff_preserves_default_storage_env() {
    run_case("antigravity_plan_handoff");
}

#[test]
fn real_tui_mouse_resize_long_provider_menu_and_focus_return() {
    run_case("mouse_and_long_menu");
}

#[test]
fn real_tui_ctrl_c_restores_terminal_from_help() {
    run_case("ctrl_c_quit");
}

#[test]
fn real_tui_literal_search_and_left_right_caret_editing() {
    run_case("literal_query_caret");
}

#[test]
fn real_tui_help_keys_scroll_and_settings_are_reachable_at_40x12() {
    run_case("help_compact");
}

#[test]
fn real_tui_long_details_scroll_without_changing_session() {
    run_case("details_scroll");
}

#[test]
fn real_tui_footer_keys_remain_whole_at_mobile_and_wide_sizes() {
    run_case("footer_widths");
}

#[test]
fn real_tui_user_query_and_provider_changes_select_first_match() {
    run_case("query_selection_reset");
}

#[test]
fn real_tui_failed_provider_is_visible_without_hiding_healthy_sessions() {
    run_case("scan_failure_status");
}

#[test]
fn real_tui_empty_failed_provider_is_not_reported_as_no_saved_sessions() {
    run_case("scan_failure_empty");
}

#[test]
fn real_tui_dark_palette_preserves_selection_caret_and_actions() {
    run_case("theme_dark");
}

#[test]
fn real_tui_light_palette_preserves_selection_caret_and_actions() {
    run_case("theme_light");
}

#[test]
fn real_tui_dark_256_color_selection_keeps_readable_contrast() {
    run_case("theme_dark_256");
}

#[test]
fn real_tui_light_256_color_selection_keeps_readable_contrast() {
    run_case("theme_light_256");
}

#[test]
fn real_tui_dark_basic_fallback_preserves_monochrome_markers_and_modifiers() {
    run_case("theme_dark_basic");
}

#[test]
fn real_tui_light_basic_fallback_preserves_monochrome_markers_and_modifiers() {
    run_case("theme_light_basic");
}

#[test]
fn real_tui_auto_palette_uses_terminal_background_hint() {
    run_case("theme_auto_light");
}

#[test]
fn real_tui_no_color_preserves_selection_caret_and_actions() {
    run_case("theme_no_color");
}

#[test]
fn real_watch_dark_palette_preserves_selection_and_restores_terminal() {
    run_case("watch_dark");
}

#[test]
fn real_watch_light_palette_preserves_selection_and_restores_terminal() {
    run_case("watch_light");
}

#[test]
fn real_tui_appearance_setting_cycles_and_persists_at_compact_sizes() {
    run_case("theme_settings");
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
