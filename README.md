# agf

[![CI](https://github.com/subinium/agf/actions/workflows/ci.yml/badge.svg)](https://github.com/subinium/agf/actions)
[![Release](https://img.shields.io/github/v/release/subinium/agf?include_prereleases&sort=semver)](https://github.com/subinium/agf/releases)
[![crates.io](https://img.shields.io/crates/v/agf.svg)](https://crates.io/crates/agf)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

> A fast TUI to find, resume, and manage your AI coding agent sessions.
> Supports **Claude Code**, **Codex**, **OpenCode**, **pi**, **Kiro**, **Cursor CLI**, and **Gemini** — all in one place.

Built with [SuperLightTUI (SLT)](https://github.com/subinium/SuperLightTUI) — an immediate-mode terminal UI library for Rust.

![agf demo](./assets/demo.gif)

## Quick Start

```bash
cargo install agf
```

Requires a Rust toolchain (`rustup` recommended). Prebuilt binaries for macOS, Linux, and Windows are also available on the [Releases page](https://github.com/subinium/agf/releases).

Then run `agf setup` and restart your shell. Type `agf` to launch.

### Quick Resume (no TUI)

```bash
agf resume project-name   # fuzzy-matches and resumes the best match directly
```

## Why agf?

If you use AI coding agents, you've probably done this:

1. Forget which project you were working on
2. `cd` into the wrong directory
3. Try to remember the session ID
4. Give up and start a new session

`agf` fixes that. It scans all your agent sessions, shows them in a searchable list, and lets you resume with one keystroke.

## Features

- **Unified view** — Claude Code, Codex, OpenCode, pi, Kiro, Cursor CLI, and Gemini sessions in one list
- **Fuzzy search** — find any session by project name or summary
- **One-key resume** — select a session and hit Enter
- **Agent filter** — Tab to cycle through agents
- **Smart cd** — jump to any project directory
- **Bulk delete** — `Ctrl+D` to multi-select and batch-delete sessions
- **New session** — launch a new agent session with optional permission mode
- **Quick resume** — `agf resume <query>` to skip the TUI and resume directly
- **Unicode search** — Korean (한글) and CJK input supported
- **Mouse support** — click to select sessions, scroll wheel navigation
- **Resume mode picker** — Tab on Resume to choose permission/approval mode (yolo, plan, etc.)
- **Auto-detection** — only shows agents installed on your system
- **Git branch** — shows the current branch of each project's working directory
- **Worktree support** — detects Claude Code `--worktree` sessions; shows worktree name in the list and parent branch in the detail view

## Keybindings

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

## Config

Optional. Create `~/.config/agf/config.toml`:

```toml
sort_by = "time"            # "time" | "name" | "agent"
max_sessions = 200
search_scope = "name_path"  # "name_path" (default) | "all" (include summaries)
summary_search_count = 5    # number of summaries included when search_scope = "all"
```

You can also edit `search_scope` and `summary_search_count` interactively by pressing `?` in the TUI.

## Supported Agents

| Agent | Resume Command | Data Source |
|:---|:---|:---|
| [Claude Code](https://github.com/anthropics/claude-code) | `claude --resume <id>` | `~/.claude/history.jsonl` + `~/.claude/projects/` |
| [Codex](https://github.com/openai/codex) | `codex resume <id>` | `~/.codex/sessions/**/*.jsonl` |
| [OpenCode](https://github.com/opencode-ai/opencode) | `opencode -s <id>` | `~/.local/share/opencode/opencode.db` |
| [pi](https://github.com/badlogic/pi-mono) | `pi --resume` | `~/.pi/agent/sessions/<cwd>/*.jsonl` |
| [Kiro](https://kiro.dev) | `kiro-cli chat --resume` | `~/Library/Application Support/kiro-cli/data.sqlite3` |
| [Cursor CLI](https://docs.cursor.com/agent) | `cursor-agent --resume <id>` | `~/.cursor/projects/*/agent-transcripts/*.txt` |
| [Gemini](https://github.com/google-gemini/gemini-cli) | `gemini --resume <id>` | `~/.gemini/tmp/<project>/chats/session-*.json` |

### Session Storage Paths

| Agent | Format | Default Path |
|:---|:---|:---|
| Claude Code | JSONL | `~/.claude/history.jsonl` (sessions)<br>`~/.claude/projects/*/` (worktree detection) |
| Codex | JSONL | `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl` |
| OpenCode | SQLite | `~/.local/share/opencode/opencode.db` |
| pi | JSONL | `~/.pi/agent/sessions/--<encoded-cwd>--/<ts>_<id>.jsonl` |
| Kiro | SQLite | macOS: `~/Library/Application Support/kiro-cli/data.sqlite3`<br>Linux: `~/.local/share/kiro-cli/data.sqlite3` |
| Cursor CLI | SQLite + TXT | `~/.cursor/chats/*/<id>/store.db`<br>`~/.cursor/projects/*/agent-transcripts/<id>.txt` |
| Gemini | JSON | `~/.gemini/tmp/<project>/chats/session-<date>-<id>.json`<br>`<project>` is a named dir or SHA-256 hash of the project path<br>Project paths resolved via `~/.gemini/projects.json` |

## Install (other methods)

<details>
<summary>From source</summary>

```bash
git clone https://github.com/subinium/agf.git
cd agf
cargo install --path .
agf setup
```

</details>

## Upgrading

After upgrading, run `agf setup` again (or restart your shell) to apply the latest shell wrapper.

See [CHANGELOG.md](CHANGELOG.md) for full details.

## Requirements

- macOS, Linux, or Windows (PowerShell 5.1+ / PowerShell 7+)
- One or more of: `claude`, `codex`, `opencode`, `pi`, `kiro-cli`, `cursor-agent`, `gemini`

### Shells

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

## Contributing

Issues and PRs are welcome.

### Contributors

[![Contributors](https://contrib.rocks/image?repo=subinium/agf)](https://github.com/subinium/agf/graphs/contributors)

## License

[MIT](LICENSE)

---

### Agent Support Roadmap

**Amp** is not yet supported. Amp stores sessions on a remote server, making it difficult to reliably resolve project paths from session metadata. We are monitoring upstream changes and will add support when feasible.
