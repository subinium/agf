# agf

[![CI](https://github.com/subinium/agf/actions/workflows/ci.yml/badge.svg)](https://github.com/subinium/agf/actions)
[![Release](https://img.shields.io/github/v/release/subinium/agf?include_prereleases&sort=semver)](https://github.com/subinium/agf/releases)
[![crates.io](https://img.shields.io/crates/v/agf.svg)](https://crates.io/crates/agf)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

> Find the AI coding session you meant to resume.

`agf` is a local-first fuzzy finder for AI coding-agent sessions.
Search local sessions across **Claude Code**, **Codex**, **Gemini CLI**, **Cursor CLI**, **OpenCode**, **Kiro**, and **pi** — then resume the right one in a keystroke.

![agf demo](./assets/demo.gif)

## Install

```bash
cargo install agf
agf setup
agf
```

Requires a Rust toolchain (`rustup` recommended). Prebuilt binaries for macOS, Linux, and Windows are also available on the [Releases page](https://github.com/subinium/agf/releases).

### Quick Resume (no TUI)

```bash
agf resume project-name   # fuzzy-matches and resumes the best match directly
```

## Why agf?

AI coding agents are great at keeping context — until you lose the terminal.

You switch projects, close a tab, forget the session ID, or resume the wrong agent.
Then you either dig through history files or start over.

`agf` gives you one searchable list of local agent sessions and resumes the right one.

## Supported agents

`agf` reads the session files each agent already stores locally. No account, no cloud sync, no extra agent process.

| Agent | Resume command | Local session source |
|:---|:---|:---|
| [Claude Code](https://github.com/anthropics/claude-code) | `claude --resume <id>` | `~/.claude/history.jsonl` + `~/.claude/projects/` |
| [Codex](https://github.com/openai/codex) | `codex resume <id>` | `~/.codex/sessions/**/*.jsonl` |
| [Gemini CLI](https://github.com/google-gemini/gemini-cli) | `gemini --resume <id>` | `~/.gemini/tmp/<project>/chats/session-*.json` |
| [Cursor CLI](https://docs.cursor.com/agent) | `cursor-agent --resume <id>` | `~/.cursor/projects/*/agent-transcripts/*.txt` |
| [OpenCode](https://github.com/opencode-ai/opencode) | `opencode -s <id>` | `~/.local/share/opencode/opencode.db` |
| [Kiro](https://kiro.dev) | `kiro-cli chat --resume` | `~/Library/Application Support/kiro-cli/data.sqlite3` |
| [pi](https://github.com/badlogic/pi-mono) | `pi --resume` | `~/.pi/agent/sessions/<cwd>/*.jsonl` |

<details>
<summary>Full session storage paths</summary>

| Agent | Format | Default Path |
|:---|:---|:---|
| Claude Code | JSONL | `~/.claude/history.jsonl` (sessions)<br>`~/.claude/projects/*/` (worktree detection) |
| Codex | JSONL | `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl` |
| OpenCode | SQLite | `~/.local/share/opencode/opencode.db` |
| pi | JSONL | `~/.pi/agent/sessions/--<encoded-cwd>--/<ts>_<id>.jsonl` |
| Kiro | SQLite | macOS: `~/Library/Application Support/kiro-cli/data.sqlite3`<br>Linux: `~/.local/share/kiro-cli/data.sqlite3` |
| Cursor CLI | SQLite + TXT | `~/.cursor/chats/*/<id>/store.db`<br>`~/.cursor/projects/*/agent-transcripts/<id>.txt` |
| Gemini | JSON | `~/.gemini/tmp/<project>/chats/session-<date>-<id>.json`<br>`<project>` is a named dir or SHA-256 hash of the project path<br>Project paths resolved via `~/.gemini/projects.json` |

</details>

## Features

- **Cross-agent search** — see all supported agents in one list
- **Fuzzy search** — find sessions by project name, path, branch, or summary
- **One-key resume** — resume the selected session with the right agent command
- **Quick resume** — `agf resume <query>` skips the TUI entirely
- **Bulk delete** — `Ctrl+D` to multi-select and clean up stale sessions
- **Project awareness** — git branches and Claude Code `--worktree` sessions surface in the UI

Also supports Unicode/CJK search, mouse navigation, agent filters, permission/approval-mode picker, agent auto-detection, and shell wrappers for zsh, bash, fish, and PowerShell.

## Basic controls

| Key | Action |
|:---|:---|
| Type anything | Fuzzy search |
| `↑` `↓` / `Ctrl+K` `Ctrl+J` | Navigate |
| `Enter` | Open action menu |
| `Tab` / `Shift+Tab` | Cycle agent filter |
| `→` / `Ctrl+L` | Preview session |
| `Ctrl+D` | Bulk delete |
| `?` | Help / settings |
| `Esc` | Quit |

<details>
<summary>Full keybindings</summary>

### Browse

| Key | Action |
|:---|:---|
| Type anything | Fuzzy search |
| `↑` `↓` / `Ctrl+K` `Ctrl+J` | Navigate |
| `[` `]` | Cycle session summary |
| `Enter` | Open action menu |
| `→` / `Ctrl+L` | Preview session details |
| `Tab` / `Shift+Tab` | Cycle agent filter |
| `Ctrl+S` | Cycle sort (time / name / agent) |
| `Ctrl+D` | Enter bulk delete mode |
| `?` | Help / settings |
| `Esc` | Quit |

### Bulk Delete (`Ctrl+D`)

| Key | Action |
|:---|:---|
| `Space` | Toggle selection + move down |
| `↑` `↓` / `Ctrl+K` `Ctrl+J` | Navigate |
| `Enter` | Confirm deletion (when items selected) |
| `Esc` | Cancel and return to browse |

### New Session (Agent Select)

| Key | Action |
|:---|:---|
| `1`-`9` | Quick select agent |
| `Tab` | Open permission/approval mode picker |
| `Enter` | Launch with default mode |
| `Esc` | Back |

</details>

## Configuration

Optional. Create `~/.config/agf/config.toml`:

```toml
sort_by = "time"            # "time" | "name" | "agent"
max_sessions = 200
search_scope = "name_path"  # "name_path" (default) | "all" (include summaries)
summary_search_count = 5    # number of summaries included when search_scope = "all"
```

You can also edit `search_scope` and `summary_search_count` interactively by pressing `?` in the TUI.

## Shell integration

`agf setup` auto-detects your shell and installs the wrapper. Supported shells:

- **zsh / bash** — appends to `~/.zshrc` or `~/.bashrc`
- **fish** — writes to `~/.config/fish/config.fish`
- **PowerShell** (Windows or cross-platform `pwsh`) — writes to `$PROFILE.CurrentUserAllHosts` (`Documents\PowerShell\profile.ps1` on Windows, `~/.config/powershell/profile.ps1` elsewhere)

If auto-detection misses your shell, run the matching `agf init` form manually:

```bash
eval "$(agf init zsh)"                               # zsh
eval "$(agf init bash)"                              # bash
agf init fish | source                               # fish
agf init powershell | Out-String | Invoke-Expression # PowerShell
```

After upgrading, run `agf setup` again (or restart your shell) to apply the latest wrapper.
See [CHANGELOG.md](CHANGELOG.md) for release notes.

## Requirements

- macOS, Linux, or Windows (PowerShell 5.1+ / PowerShell 7+)
- One or more of: `claude`, `codex`, `opencode`, `pi`, `kiro-cli`, `cursor-agent`, `gemini`

## Install from source

```bash
git clone https://github.com/subinium/agf.git
cd agf
cargo install --path .
agf setup
```

## Limitations

`agf` works best with agents that store resumable sessions locally.

**Amp** is not supported yet because its sessions are stored remotely, which makes it hard to reliably resolve local project paths from session metadata. We are monitoring upstream changes and will add support when feasible.

## Built with

`agf` is written in Rust and built with [SuperLightTUI (SLT)](https://github.com/subinium/SuperLightTUI) — an immediate-mode terminal UI library for Rust.

## Contributing

Issues and PRs are welcome.

[![Contributors](https://contrib.rocks/image?repo=subinium/agf)](https://github.com/subinium/agf/graphs/contributors)

## License

[MIT](LICENSE)
