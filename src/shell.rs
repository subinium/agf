use std::fs;
use std::path::PathBuf;

/// Which shell's command syntax is in effect for the current invocation.
///
/// Selected by the `AGF_SHELL` environment variable, which the installed
/// shell wrapper sets before invoking the real `agf` binary. Falls back to
/// POSIX when the variable is absent, matching the pre-existing behavior
/// for bash/zsh/fish users.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandShell {
    Posix,
    PowerShell,
}

impl CommandShell {
    pub fn from_env() -> Self {
        match std::env::var("AGF_SHELL").as_deref() {
            Ok("powershell") | Ok("pwsh") => Self::PowerShell,
            _ => Self::Posix,
        }
    }

    /// Escape a string so it can be interpolated into a single-quoted
    /// literal for this shell.
    ///
    /// POSIX: `'...'` with embedded `'` written as `'\''`.
    /// PowerShell: `'...'` with embedded `'` written as `''`.
    pub fn quote(&self, s: &str) -> String {
        match self {
            Self::Posix => format!("'{}'", s.replace('\'', "'\\''")),
            Self::PowerShell => format!("'{}'", s.replace('\'', "''")),
        }
    }

    /// Build "change directory to `path`, then run `cmd` only if the cd
    /// succeeded." The separator differs between shells.
    ///
    /// POSIX uses `&&`. PowerShell 5.1 has no `&&` (that lands in 7+), so
    /// we use `; if ($?) { ... }` which works in both 5.1 and 7+.
    pub fn cd_and(&self, quoted_path: &str, cmd: &str) -> String {
        match self {
            Self::Posix => format!("cd {quoted_path} && {cmd}"),
            Self::PowerShell => format!("Set-Location {quoted_path}; if ($?) {{ {cmd} }}"),
        }
    }

    /// Build a "cd only, no follow-up" command.
    pub fn cd_only(&self, quoted_path: &str) -> String {
        match self {
            Self::Posix => format!("cd {quoted_path}"),
            Self::PowerShell => format!("Set-Location {quoted_path}"),
        }
    }

    /// True if `cmd` only changes directory (no chained follow-up).
    /// Used by the delivery path to warn when shell integration is missing
    /// (a bare `cd` printed to stdout doesn't persist in the parent shell).
    pub fn is_cd_only(&self, cmd: &str) -> bool {
        match self {
            Self::Posix => !cmd.contains(" && "),
            Self::PowerShell => !cmd.contains("; if ($?) {"),
        }
    }
}

/// Detect user's shell and append the init line to the appropriate rc file.
pub fn setup() -> anyhow::Result<()> {
    let shell_path = std::env::var("SHELL").unwrap_or_default();
    let shell_name = shell_path.rsplit('/').next().unwrap_or("");

    let (rc_file, init_line) = match shell_name {
        "zsh" => (
            dirs::home_dir().unwrap_or_default().join(".zshrc"),
            r#"eval "$(agf init zsh)""#.to_string(),
        ),
        "bash" => {
            let home = dirs::home_dir().unwrap_or_default();
            // Prefer .bashrc, fall back to .bash_profile on macOS
            let rc = if home.join(".bashrc").exists() {
                home.join(".bashrc")
            } else {
                home.join(".bash_profile")
            };
            (rc, r#"eval "$(agf init bash)""#.to_string())
        }
        "fish" => (
            dirs::config_dir()
                .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config"))
                .join("fish")
                .join("config.fish"),
            "agf init fish | source".to_string(),
        ),
        // No POSIX SHELL and we're on Windows — default to PowerShell.
        _ if shell_name.is_empty() && cfg!(windows) => (
            powershell_profile_path(),
            "agf init powershell | Out-String | Invoke-Expression".to_string(),
        ),
        _ => {
            eprintln!("Unsupported shell: {shell_name}");
            eprintln!("Manually add to your shell config:");
            eprintln!("  eval \"$(agf init zsh)\"                            # for zsh");
            eprintln!("  eval \"$(agf init bash)\"                           # for bash");
            eprintln!("  agf init fish | source                             # for fish");
            eprintln!("  agf init powershell | Out-String | Invoke-Expression  # for PowerShell");
            return Err(anyhow::anyhow!("unsupported shell: {shell_name}"));
        }
    };

    // Check if already configured (match the marker we write below, not a loose substring)
    if rc_file.exists() {
        let content = fs::read_to_string(&rc_file)?;
        if content.contains("# agf - AI Agent Session Finder") {
            eprintln!("Already configured in {}", rc_file.display());
            eprintln!("Restart your shell or run: source {}", rc_file.display());
            return Ok(());
        }
    }

    // Ensure parent directory exists (for fish / PowerShell)
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

    eprintln!("Added to {}", rc_file.display());
    eprintln!("Restart your shell or run: source {}", rc_file.display());
    Ok(())
}

/// Resolve the PowerShell `$PROFILE` (CurrentUserAllHosts) path.
///
/// On Windows, PowerShell 7 (`pwsh`) and Windows PowerShell 5.1 use distinct
/// profile directories. Prefer an existing `PowerShell` dir (PS 7); fall back
/// to `WindowsPowerShell` (PS 5.1) if only that one exists; otherwise default
/// to the PS 7 path (modern default, created on demand).
///
/// On non-Windows, PowerShell 7 uses `~/.config/powershell/profile.ps1`.
fn powershell_profile_path() -> PathBuf {
    if cfg!(windows) {
        let docs = dirs::document_dir()
            .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join("Documents"));
        let ps7 = docs.join("PowerShell");
        let ps5 = docs.join("WindowsPowerShell");
        let dir = if ps7.exists() {
            ps7
        } else if ps5.exists() {
            ps5
        } else {
            ps7
        };
        dir.join("profile.ps1")
    } else {
        dirs::config_dir()
            .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config"))
            .join("powershell")
            .join("profile.ps1")
    }
}

pub fn shell_init(shell: &str) -> String {
    match shell {
        "zsh" => ZSH_WRAPPER.to_string(),
        "bash" => BASH_WRAPPER.to_string(),
        "fish" => FISH_WRAPPER.to_string(),
        "powershell" | "pwsh" => POWERSHELL_WRAPPER.to_string(),
        other => {
            format!("echo \"Unsupported shell: {other}. Use zsh, bash, fish, or powershell.\"")
        }
    }
}

const ZSH_WRAPPER: &str = r#"function agf() {
    local tmpfile
    tmpfile="$(mktemp)" || return 1
    AGF_CMD_FILE="$tmpfile" command agf "$@"
    local ret=$?
    if [ $ret -eq 0 ] && [ -f "$tmpfile" ]; then
        local result
        result="$(cat "$tmpfile")"
        if [ -n "$result" ]; then
            eval "$result"
        fi
    fi
    rm -f "$tmpfile"
}"#;

const BASH_WRAPPER: &str = r#"function agf() {
    local tmpfile
    tmpfile="$(mktemp)" || return 1
    AGF_CMD_FILE="$tmpfile" command agf "$@"
    local ret=$?
    if [ $ret -eq 0 ] && [ -f "$tmpfile" ]; then
        local result
        result="$(cat "$tmpfile")"
        if [ -n "$result" ]; then
            eval "$result"
        fi
    fi
    rm -f "$tmpfile"
}"#;

const FISH_WRAPPER: &str = r#"function agf
    set -l tmpfile (mktemp); or return 1
    AGF_CMD_FILE=$tmpfile command agf $argv
    set -l ret $status
    if test $ret -eq 0; and test -f $tmpfile
        set -l result (cat $tmpfile)
        if test -n "$result"
            eval $result
        end
    end
    rm -f $tmpfile
end"#;

// PowerShell wrapper. Compatible with Windows PowerShell 5.1 and PowerShell 7+.
//
// `AGF_SHELL=powershell` tells the agf binary to emit PowerShell-flavored
// commands (Set-Location + `; if ($?) { ... }` rather than `cd ... && ...`).
// Invoke-Expression runs in the caller's scope, so `Set-Location` persists
// after the wrapper returns — matching the POSIX `eval` semantics.
const POWERSHELL_WRAPPER: &str = r#"function agf {
    $__agfExe = Get-Command -Name agf -CommandType Application -ErrorAction SilentlyContinue |
                Select-Object -First 1
    if (-not $__agfExe) {
        Write-Error 'agf: executable not found on PATH.'
        return
    }
    $__agfTmp = [System.IO.Path]::GetTempFileName()
    try {
        $env:AGF_CMD_FILE = $__agfTmp
        $env:AGF_SHELL = 'powershell'
        & $__agfExe.Source @args
        $__agfExit = $LASTEXITCODE
        if ($__agfExit -eq 0 -and (Test-Path -LiteralPath $__agfTmp)) {
            $__agfResult = Get-Content -Raw -LiteralPath $__agfTmp
            if ($__agfResult) {
                Invoke-Expression $__agfResult
            }
        }
    }
    finally {
        if (Test-Path -LiteralPath $__agfTmp) {
            Remove-Item -Force -LiteralPath $__agfTmp -ErrorAction SilentlyContinue
        }
        Remove-Item -Path Env:AGF_CMD_FILE -ErrorAction SilentlyContinue
        Remove-Item -Path Env:AGF_SHELL -ErrorAction SilentlyContinue
    }
}"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn posix_quote_escapes_single_quote() {
        let s = CommandShell::Posix.quote("Jon's files");
        assert_eq!(s, r#"'Jon'\''s files'"#);
    }

    #[test]
    fn powershell_quote_doubles_single_quote() {
        let s = CommandShell::PowerShell.quote("Jon's files");
        assert_eq!(s, "'Jon''s files'");
    }

    #[test]
    fn cd_and_posix_uses_amp_amp() {
        let cmd = CommandShell::Posix.cd_and("'/tmp'", "claude");
        assert_eq!(cmd, "cd '/tmp' && claude");
    }

    #[test]
    fn cd_and_powershell_uses_if_dollar_question() {
        let cmd = CommandShell::PowerShell.cd_and("'C:\\tmp'", "claude");
        assert_eq!(cmd, "Set-Location 'C:\\tmp'; if ($?) { claude }");
    }

    #[test]
    fn is_cd_only_detects_chained_commands() {
        let posix = CommandShell::Posix;
        assert!(posix.is_cd_only("cd '/tmp'"));
        assert!(!posix.is_cd_only("cd '/tmp' && claude"));

        let pwsh = CommandShell::PowerShell;
        assert!(pwsh.is_cd_only("Set-Location '/tmp'"));
        assert!(!pwsh.is_cd_only("Set-Location '/tmp'; if ($?) { claude }"));
    }

    #[test]
    fn from_env_reads_agf_shell() {
        let prev = std::env::var("AGF_SHELL").ok();

        std::env::set_var("AGF_SHELL", "powershell");
        assert_eq!(CommandShell::from_env(), CommandShell::PowerShell);

        std::env::set_var("AGF_SHELL", "pwsh");
        assert_eq!(CommandShell::from_env(), CommandShell::PowerShell);

        std::env::remove_var("AGF_SHELL");
        assert_eq!(CommandShell::from_env(), CommandShell::Posix);

        match prev {
            Some(v) => std::env::set_var("AGF_SHELL", v),
            None => std::env::remove_var("AGF_SHELL"),
        }
    }
}
