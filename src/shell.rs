use std::fs;

/// Detect user's shell and append the init line to the appropriate rc file.
pub fn setup() -> anyhow::Result<()> {
    let shell_path = std::env::var("SHELL").unwrap_or_default();
    let shell_name = shell_path.rsplit('/').next().unwrap_or("");

    let (rc_file, init_line) = match shell_name {
        "zsh" => (
            dirs::home_dir().unwrap_or_default().join(".zshrc"),
            r#"eval "$(agf init zsh)""#,
        ),
        "bash" => {
            let home = dirs::home_dir().unwrap_or_default();
            // Prefer .bashrc, fall back to .bash_profile on macOS
            let rc = if home.join(".bashrc").exists() {
                home.join(".bashrc")
            } else {
                home.join(".bash_profile")
            };
            (rc, r#"eval "$(agf init bash)""#)
        }
        "fish" => (
            dirs::config_dir()
                .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config"))
                .join("fish")
                .join("config.fish"),
            "agf init fish | source",
        ),
        _ => {
            eprintln!("Unsupported shell: {shell_name}");
            eprintln!("Manually add to your shell config:");
            eprintln!("  eval \"$(agf init zsh)\"   # for zsh");
            eprintln!("  eval \"$(agf init bash)\"  # for bash");
            eprintln!("  agf init fish | source    # for fish");
            return Ok(());
        }
    };

    // Check if already configured
    if rc_file.exists() {
        let content = fs::read_to_string(&rc_file)?;
        if content.contains("agf init") {
            println!("Already configured in {}", rc_file.display());
            println!("Restart your shell or run: source {}", rc_file.display());
            return Ok(());
        }
    }

    // Ensure parent directory exists (for fish)
    if let Some(parent) = rc_file.parent() {
        fs::create_dir_all(parent)?;
    }

    // Append the init line
    let mut content = if rc_file.exists() {
        fs::read_to_string(&rc_file)?
    } else {
        String::new()
    };

    if !content.ends_with('\n') && !content.is_empty() {
        content.push('\n');
    }
    content.push_str(&format!("\n# agf - AI Agent Session Finder\n{init_line}\n"));
    fs::write(&rc_file, content)?;

    println!("Added to {}", rc_file.display());
    println!("Restart your shell or run: source {}", rc_file.display());
    Ok(())
}

pub fn shell_init(shell: &str) -> String {
    match shell {
        "zsh" => ZSH_WRAPPER.to_string(),
        "bash" => BASH_WRAPPER.to_string(),
        "fish" => FISH_WRAPPER.to_string(),
        other => format!("echo \"Unsupported shell: {other}. Use zsh, bash, or fish.\""),
    }
}

const ZSH_WRAPPER: &str = r#"function agf() {
    local result
    result="$(command agf "$@")"
    if [ $? -eq 0 ] && [ -n "$result" ]; then
        eval "$result"
    fi
}"#;

const BASH_WRAPPER: &str = r#"function agf() {
    local result
    result="$(command agf "$@")"
    if [ $? -eq 0 ] && [ -n "$result" ]; then
        eval "$result"
    fi
}"#;

const FISH_WRAPPER: &str = r#"function agf
    set -l result (command agf $argv)
    if test $status -eq 0; and test -n "$result"
        eval $result
    end
end"#;
