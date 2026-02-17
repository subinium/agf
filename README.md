# agf
[![CI](https://github.com/subinium/agf/actions/workflows/ci.yml/badge.svg)](https://github.com/subinium/agf/actions)
[![Release](https://img.shields.io/github/v/release/subinium/agf)](https://github.com/subinium/agf/releases)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

> A fast TUI to find, resume, and manage your AI coding agent sessions.
> Supports **Claude Code**, **Codex**, and **Cursor** — all in one place.

<!-- Uncomment after adding a demo recording:
![agf demo](./assets/demo.gif)
-->

## Quick Start

```bash
brew install subinium/tap/agf
agf setup
```

Restart your shell. Then just type `agf`.

## Why agf?

If you use AI coding agents, you've probably done this:

1. Forget which project you were working on
2. `cd` into the wrong directory
3. Try to remember the session ID
4. Give up and start a new session

`agf` fixes that. It scans all your agent sessions, shows them in a searchable list, and lets you resume with one keystroke.

## Features

- **Unified view** — Claude Code, Codex, and Cursor sessions in one list
- **Fuzzy search** — find any session by project name or summary
- **One-key resume** — select a session and hit Enter
- **Agent filter** — Tab to cycle through agents
- **Smart cd** — jump to any project directory
- **Session cleanup** — delete old sessions with confirmation
- **Auto-detection** — only shows agents installed on your system

## Keybindings

| Key | Action |
|:---|:---|
| Type anything | Fuzzy search |
| `↑` `↓` | Navigate |
| `Enter` | Open action menu |
| `→` | Preview session details |
| `Tab` / `Shift+Tab` | Cycle agent filter |
| `Ctrl+S` | Cycle sort (time / name / agent) |
| `Esc` | Quit |

## Config

Optional. Create `~/.config/agf/config.toml`:

```toml
sort_by = "time"       # "time" | "name" | "agent"
max_sessions = 200
```

## Supported Agents

| Agent | Resume | Data Source |
|:---|:---|:---|
| Claude Code | `claude --resume <id>` | `~/.claude/` |
| Codex | `codex resume <id>` | `~/.codex/` |
| Cursor | `cursor .` | `~/.cursor/` |

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

## Requirements

- macOS or Linux
- One or more of: `claude`, `codex`, `cursor`

## Contributing

Issues and PRs are welcome.

## License

[MIT](LICENSE)
