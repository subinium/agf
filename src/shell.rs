use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;

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
    /// Resolve the active shell once per process. The wrapper sets `AGF_SHELL`
    /// before exec'ing `agf`; without it we fall back to `default_shell`.
    /// Cached so the TUI render path and `action.rs` don't re-read env.
    pub fn from_env() -> Self {
        static CACHE: OnceLock<CommandShell> = OnceLock::new();
        *CACHE.get_or_init(|| match std::env::var("AGF_SHELL").ok().as_deref() {
            Some(name) if !name.is_empty() => Self::from_name(Some(name)),
            _ => Self::default_shell(
                cfg!(windows),
                std::env::var_os("MSYSTEM").is_some_and(|v| !v.is_empty()),
                std::env::var("SHELL").ok().as_deref(),
            ),
        })
    }

    /// Pure helper behind `from_env` — classifies an explicit shell name string.
    /// Exposed so tests can drive it without mutating process env.
    fn from_name(name: Option<&str>) -> Self {
        match name {
            Some("powershell") | Some("pwsh") => Self::PowerShell,
            _ => Self::Posix,
        }
    }

    /// Default shell when `AGF_SHELL` is unset. Native Windows needs PowerShell
    /// (no `sh`, no `&&`); a POSIX layer signals itself via `MSYSTEM` or, for
    /// ones like Cygwin that don't, a `SHELL` that native shells never export.
    /// Args are passed in so tests stay pure.
    fn default_shell(is_windows: bool, msystem: bool, shell: Option<&str>) -> Self {
        if !is_windows || msystem {
            return Self::Posix;
        }
        // Classify SHELL's basename via `from_name` so shell names live in one place.
        match shell {
            Some(s) if !s.is_empty() => {
                let base = s
                    .rsplit(['/', '\\'])
                    .next()
                    .unwrap_or(s)
                    .to_ascii_lowercase();
                Self::from_name(Some(base.trim_end_matches(".exe")))
            }
            _ => Self::PowerShell,
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

    /// A quoted executable requires PowerShell's call operator. Keep static
    /// provider names unchanged for the existing wrapper/preview contract.
    pub fn program(&self, name: &str, default_name: &str) -> String {
        if name == default_name {
            return name.to_string();
        }
        let quoted = self.quote(name);
        match self {
            Self::Posix => quoted,
            Self::PowerShell => format!("& {quoted}"),
        }
    }

    /// Apply a static allowlist of provider storage variables to one command.
    /// PowerShell has no `env NAME=value command` syntax; isolate bookkeeping
    /// in a script scope and restore both values and absence in `finally`.
    pub fn with_environment(
        &self,
        cmd: &str,
        env: &std::collections::BTreeMap<String, String>,
    ) -> String {
        if env.is_empty() {
            return cmd.to_string();
        }
        match self {
            Self::Posix => {
                let assignments = env
                    .iter()
                    .map(|(key, value)| format!("{key}={}", self.quote(value)))
                    .collect::<Vec<_>>()
                    .join(" ");
                format!("{assignments} command {cmd}")
            }
            Self::PowerShell => {
                let mut before = String::new();
                let mut apply = String::new();
                let mut restore = String::new();
                for (index, (key, value)) in env.iter().enumerate() {
                    before.push_str(&format!("$__agfHad{index} = Test-Path -LiteralPath 'Env:{key}'; $__agfOld{index} = $env:{key}; "));
                    apply.push_str(&format!("$env:{key} = {}; ", self.quote(value)));
                    restore.push_str(&format!("if ($__agfHad{index}) {{ $env:{key} = $__agfOld{index} }} else {{ Remove-Item -LiteralPath 'Env:{key}' -ErrorAction SilentlyContinue }}; "));
                }
                format!("& {{ {before}try {{ {apply}{cmd} }} finally {{ {restore}}} }}")
            }
        }
    }

    pub fn error_command(&self, message: &str) -> String {
        let quoted = self.quote(message);
        match self {
            Self::Posix => format!("printf '%s\\n' {quoted} >&2; false"),
            Self::PowerShell => format!("Write-Error {quoted}"),
        }
    }

    /// Build "change directory to `path`, then run `cmd` only if the cd
    /// succeeded." The separator differs between shells.
    ///
    /// POSIX uses `&&`. PowerShell 5.1 has no `&&` (that lands in 7+), so
    /// we use `; if ($?) { ... }` which works in both 5.1 and 7+.
    ///
    /// When `quoted_path` is empty (or the empty quoted form `''` / `""`)
    /// the cd is skipped entirely — used by cwd-independent agents like
    /// Hermes that don't have a project root and shouldn't drag the user
    /// out of their current working directory on resume.
    pub fn cd_and(&self, quoted_path: &str, cmd: &str) -> String {
        if quoted_path.is_empty() || quoted_path == "''" || quoted_path == "\"\"" {
            return cmd.to_string();
        }
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
        let command = cmd.trim_start();
        match self {
            Self::Posix => command.starts_with("cd ") && !command.contains(" && "),
            Self::PowerShell => {
                command.starts_with("Set-Location ") && !command.contains("; if ($?) {")
            }
        }
    }

    /// Executable name and leading args used to evaluate a generated command
    /// string in this shell's syntax (e.g. `("sh", &["-c"])`).
    pub fn exec_parts(&self) -> (&'static str, &'static [&'static str]) {
        match self {
            Self::Posix => ("sh", &["-c"]),
            // On Unix, `pwsh` is the cross-platform binary; on Windows either
            // `pwsh` or the 5.1 `powershell.exe` work with the same flags.
            #[cfg(unix)]
            Self::PowerShell => ("pwsh", &["-NoProfile", "-Command"]),
            #[cfg(not(unix))]
            Self::PowerShell => ("powershell", &["-NoProfile", "-Command"]),
        }
    }

    /// Used only for a dedicated child shell, never for the parent-shell wrapper.
    pub fn exec_script(&self, cmd: &str) -> String {
        match self {
            Self::Posix => cmd.to_owned(),
            Self::PowerShell => format!(
                "$ErrorActionPreference = 'Stop'; $PSNativeCommandUseErrorActionPreference = $false; \
                 $global:LASTEXITCODE = 0; try {{ & {{ {cmd} }}; $__agfOk = $?; \
                 $__agfNative = $LASTEXITCODE; if ($__agfNative -ne 0) {{ exit $__agfNative }}; \
                 if (-not $__agfOk) {{ exit 1 }}; exit 0 }} \
                 catch {{ [Console]::Error.WriteLine('agf: PowerShell command failed'); exit 1 }}"
            ),
        }
    }
}

/// Detect user's shell and append the init line to the appropriate rc file.
pub fn setup(shell_override: Option<&str>) -> anyhow::Result<()> {
    let shell_path = std::env::var("SHELL").unwrap_or_default();
    let detected = shell_path.rsplit('/').next().unwrap_or("");
    let shell_name = shell_override.unwrap_or(detected).to_ascii_lowercase();

    let (rc_file, init_line, reload) = match shell_name.as_str() {
        "zsh" => (
            dirs::home_dir().unwrap_or_default().join(".zshrc"),
            r#"eval "$(agf init zsh)""#.to_string(),
            "source".to_string(),
        ),
        "bash" => {
            let home = dirs::home_dir().unwrap_or_default();
            // Login Bash reads .bash_profile on macOS. Linux interactive Bash
            // reads .bashrc; create it instead of overwriting an unrelated
            // .profile.
            let rc = if cfg!(target_os = "macos") {
                home.join(".bash_profile")
            } else {
                home.join(".bashrc")
            };
            (
                rc,
                r#"eval "$(agf init bash)""#.to_string(),
                "source".to_string(),
            )
        }
        "fish" => (
            dirs::config_dir()
                .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config"))
                .join("fish")
                .join("config.fish"),
            "agf init fish | source".to_string(),
            "source".to_string(),
        ),
        "powershell" => (
            powershell_profile_path(Some("powershell")),
            "agf init powershell | Out-String | Invoke-Expression".to_string(),
            ".".to_string(),
        ),
        "pwsh" => (
            powershell_profile_path(Some("pwsh")),
            "agf init powershell | Out-String | Invoke-Expression".to_string(),
            ".".to_string(),
        ),
        // No POSIX SHELL and we're on Windows — default to PowerShell.
        _ if shell_name.is_empty() && cfg!(windows) => (
            powershell_profile_path(None),
            "agf init powershell | Out-String | Invoke-Expression".to_string(),
            ".".to_string(),
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
            eprintln!(
                "Restart your shell or run: {reload} '{}'",
                rc_file.display()
            );
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
    // Atomic: this rewrites the user's shell rc in full, and a torn write here
    // would leave them with a broken (or empty) login shell.
    crate::fsx::write_atomic(&rc_file, content.as_bytes())?;

    eprintln!("Added to {}", rc_file.display());
    eprintln!(
        "Restart your shell or run: {reload} '{}'",
        rc_file.display()
    );
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
fn powershell_profile_path(preferred_shell: Option<&str>) -> PathBuf {
    if cfg!(windows) {
        let docs = dirs::document_dir()
            .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join("Documents"));
        let ps5 = docs.join("WindowsPowerShell");
        // Prefer PS 7 (`PowerShell`); fall back to PS 5.1 only when its dir
        // exists and PS 7's does not. Created on demand by setup().
        let ps7 = docs.join("PowerShell");
        let dir = match preferred_shell {
            Some("powershell") => ps5,
            Some("pwsh") => ps7,
            _ if ps5.exists() && !ps7.exists() => ps5,
            _ => ps7,
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
        _ => "echo 'Unsupported shell. Use zsh, bash, fish, or powershell.'".to_string(),
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
            ret=$?
        fi
    fi
    rm -f "$tmpfile"
    return $ret
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
            ret=$?
        fi
    fi
    rm -f "$tmpfile"
    return $ret
}"#;

const FISH_WRAPPER: &str = r#"function agf
    set -l tmpfile (mktemp); or return 1
    AGF_CMD_FILE=$tmpfile command agf $argv
    set -l ret $status
    if test $ret -eq 0; and test -f $tmpfile
        set -l result (cat $tmpfile)
        if test -n "$result"
            eval $result
            set ret $status
        end
    end
    rm -f $tmpfile
    return $ret
end"#;

// PowerShell wrapper. Compatible with Windows PowerShell 5.1 and PowerShell 7+.
//
// `AGF_SHELL=powershell` tells the agf binary to emit PowerShell-flavored
// commands (Set-Location + `; if ($?) { ... }` rather than `cd ... && ...`).
// Invoke-Expression runs in the caller's scope, so `Set-Location` persists
// after the wrapper returns — matching the POSIX `eval` semantics.
//
// `Get-Content -Encoding UTF8` is required: the agf binary writes the command
// file as raw UTF-8, and Windows PowerShell 5.1's default read encoding is
// the system ANSI code page (e.g. CP949 on Korean Windows, CP1252 on
// Western). Without the explicit encoding, non-ASCII project paths round-trip
// as mojibake and `Set-Location` fails. PS 7+ already defaults to UTF-8;
// specifying it is a no-op there.
const POWERSHELL_WRAPPER: &str = r#"function agf {
    $__agfExe = Get-Command -Name agf -CommandType Application -ErrorAction SilentlyContinue |
                Select-Object -First 1
    if (-not $__agfExe) {
        Write-Error 'agf: executable not found on PATH.'
        return
    }
    $__agfTmp = [System.IO.Path]::GetTempFileName()
    $__agfHadCmdFile = Test-Path Env:AGF_CMD_FILE
    $__agfOldCmdFile = $env:AGF_CMD_FILE
    $__agfHadShell = Test-Path Env:AGF_SHELL
    $__agfOldShell = $env:AGF_SHELL
    try {
        $env:AGF_CMD_FILE = $__agfTmp
        $env:AGF_SHELL = 'powershell'
        & $__agfExe.Source @args
        $__agfExit = $LASTEXITCODE
        if ($__agfExit -eq 0 -and (Test-Path -LiteralPath $__agfTmp)) {
            $__agfResult = Get-Content -Raw -LiteralPath $__agfTmp -Encoding UTF8
            if ($__agfResult) {
                Invoke-Expression $__agfResult
                $__agfInvokeOk = $?
                $__agfNativeExit = $LASTEXITCODE
                if ($__agfInvokeOk -or $__agfNativeExit -ne 0) {
                    $__agfExit = $__agfNativeExit
                } else {
                    $__agfExit = 1
                }
            }
        }
    }
    finally {
        Remove-Item -Force -LiteralPath $__agfTmp -ErrorAction SilentlyContinue
        if ($__agfHadCmdFile) { $env:AGF_CMD_FILE = $__agfOldCmdFile }
        else { Remove-Item -Path Env:AGF_CMD_FILE -ErrorAction SilentlyContinue }
        if ($__agfHadShell) { $env:AGF_SHELL = $__agfOldShell }
        else { Remove-Item -Path Env:AGF_SHELL -ErrorAction SilentlyContinue }
    }
    $global:LASTEXITCODE = $__agfExit
    if ($__agfExit -ne 0) {
        Write-Error "agf command failed with exit code $__agfExit"
    }
}"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn powershell_child_script_has_explicit_status_boundary() {
        let shell = CommandShell::PowerShell;
        let script = shell.exec_script("Write-Output 'ok'");
        assert!(script.contains("exit $__agfNative"));
        assert!(script.contains("$ErrorActionPreference = 'Stop'"));
        assert_eq!(CommandShell::Posix.exec_script("false"), "false");
        let program = std::env::var_os("AGF_TEST_PWSH")
            .or_else(|| cfg!(windows).then(|| "powershell.exe".into()));
        if let Some(program) = program {
            let native = if cfg!(windows) {
                "& cmd.exe /d /c 'exit 37'"
            } else {
                "& /bin/sh -c 'exit 37'"
            };
            let env = std::collections::BTreeMap::from([(
                "AGF_PS_TEST".into(),
                "temporary ' value".into(),
            )]);
            let wrapped = shell.with_environment(native, &env);
            for (script, code) in [
                (wrapped, 37),
                ("Write-Error 'probe'".into(), 1),
                ("Write-Output 'ok'".into(), 0),
            ] {
                let output = std::process::Command::new(&program)
                    .args([
                        "-NoLogo",
                        "-NoProfile",
                        "-NonInteractive",
                        "-Command",
                        &shell.exec_script(&script),
                    ])
                    .env("POWERSHELL_TELEMETRY_OPTOUT", "1")
                    .output()
                    .unwrap();
                assert_eq!(
                    output.status.code(),
                    Some(code),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn scoped_environment_does_not_require_a_path_helper() {
        let shell = CommandShell::Posix;
        let env = std::collections::BTreeMap::from([(
            "AGF_TEST_STORE".into(),
            "configured ' 한글".into(),
        )]);
        let command = format!(
            "/bin/sh -c {}",
            shell.quote("printf '%s' \"$AGF_TEST_STORE\"")
        );
        let wrapped = shell.with_environment(&command, &env);
        let script = format!("{wrapped} && printf '\\n%s' \"$AGF_TEST_STORE\"");
        let output = std::process::Command::new("/bin/sh")
            .args(["-c", &script])
            .env_clear()
            .env("PATH", "")
            .env("AGF_TEST_STORE", "original")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            "configured ' 한글\noriginal"
        );
    }

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
    fn executable_override_is_quoted_with_powershell_call_operator() {
        let name = "C:/Cursor's tools/agent.exe";
        assert_eq!(
            CommandShell::PowerShell.program(name, "cursor-agent"),
            "& 'C:/Cursor''s tools/agent.exe'"
        );
        assert_eq!(
            CommandShell::Posix.program("/Cursor's tools/agent", "cursor-agent"),
            "'/Cursor'\\''s tools/agent'"
        );
        assert_eq!(
            CommandShell::PowerShell.program("cursor-agent", "cursor-agent"),
            "cursor-agent"
        );
    }

    #[test]
    fn storage_environment_is_scoped_and_restored_in_powershell() {
        let env = std::collections::BTreeMap::from([(
            "CLAUDE_CONFIG_DIR".into(),
            "C:/user's \u{c800}\u{c7a5}".into(),
        )]);
        let script = CommandShell::PowerShell.with_environment("claude --resume 'id'", &env);
        assert!(script.starts_with("& { "));
        assert!(script.contains("$__agfHad0 = Test-Path -LiteralPath 'Env:CLAUDE_CONFIG_DIR'"));
        assert!(script.contains("$__agfOld0 = $env:CLAUDE_CONFIG_DIR"));
        assert!(script.contains("try { $env:CLAUDE_CONFIG_DIR = 'C:/user''s \u{c800}\u{c7a5}'; claude --resume 'id' } finally"));
        assert!(script.contains("if ($__agfHad0) { $env:CLAUDE_CONFIG_DIR = $__agfOld0 } else { Remove-Item -LiteralPath 'Env:CLAUDE_CONFIG_DIR'"));
        assert_eq!(
            CommandShell::Posix.with_environment("claude", &env),
            "CLAUDE_CONFIG_DIR='C:/user'\\''s \u{c800}\u{c7a5}' command claude"
        );
        assert_eq!(
            CommandShell::PowerShell.with_environment("claude", &std::collections::BTreeMap::new()),
            "claude"
        );
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
        assert!(!posix.is_cd_only("hermes --resume 'session-id'"));

        let pwsh = CommandShell::PowerShell;
        assert!(pwsh.is_cd_only("Set-Location '/tmp'"));
        assert!(!pwsh.is_cd_only("Set-Location '/tmp'; if ($?) { claude }"));
        assert!(!pwsh.is_cd_only("hermes --resume 'session-id'"));
    }

    #[test]
    fn default_shell_picks_powershell_on_native_windows() {
        assert_eq!(
            CommandShell::default_shell(true, false, None),
            CommandShell::PowerShell
        );
    }

    #[test]
    fn default_shell_honors_posix_layer_on_windows() {
        assert_eq!(
            CommandShell::default_shell(true, true, None),
            CommandShell::Posix
        );
        assert_eq!(
            CommandShell::default_shell(true, false, Some("/usr/bin/bash")),
            CommandShell::Posix
        );
        assert_eq!(
            CommandShell::default_shell(true, false, Some("/bin/sh")),
            CommandShell::Posix
        );
    }

    #[test]
    fn default_shell_classifies_shell_as_powershell_only_for_pwsh() {
        assert_eq!(
            CommandShell::default_shell(true, false, Some("pwsh")),
            CommandShell::PowerShell
        );
        assert_eq!(
            CommandShell::default_shell(true, false, Some("/bin/csh")),
            CommandShell::Posix
        );
    }

    #[test]
    fn default_shell_matches_basename_not_path_substring() {
        assert_eq!(
            CommandShell::default_shell(true, false, Some(r"C:\Git\usr\bin\bash.exe")),
            CommandShell::Posix
        );
        assert_eq!(
            CommandShell::default_shell(true, false, Some(r"C:\Users\bashfan\bin\pwsh.exe")),
            CommandShell::PowerShell
        );
    }

    #[test]
    fn default_shell_is_posix_off_windows() {
        assert_eq!(
            CommandShell::default_shell(false, false, None),
            CommandShell::Posix
        );
        // Even a Windows-looking SHELL must not flip a non-Windows host.
        assert_eq!(
            CommandShell::default_shell(false, true, Some("pwsh")),
            CommandShell::Posix
        );
    }

    #[test]
    fn default_shell_handles_uppercase_exe_and_empty_shell() {
        // Lowercasing must happen before `.exe` trimming.
        assert_eq!(
            CommandShell::default_shell(true, false, Some(r"C:\PowerShell\7\PWSH.EXE")),
            CommandShell::PowerShell
        );
        assert_eq!(
            CommandShell::default_shell(
                true,
                false,
                Some(r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe")
            ),
            CommandShell::PowerShell
        );
        // Empty SHELL means "unset", not a POSIX-layer signal.
        assert_eq!(
            CommandShell::default_shell(true, false, Some("")),
            CommandShell::PowerShell
        );
    }

    #[test]
    fn from_name_classifies_shells() {
        assert_eq!(
            CommandShell::from_name(Some("powershell")),
            CommandShell::PowerShell
        );
        assert_eq!(
            CommandShell::from_name(Some("pwsh")),
            CommandShell::PowerShell
        );
        assert_eq!(CommandShell::from_name(Some("bash")), CommandShell::Posix);
        assert_eq!(CommandShell::from_name(Some("")), CommandShell::Posix);
        assert_eq!(CommandShell::from_name(None), CommandShell::Posix);
    }
}
