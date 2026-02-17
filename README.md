<div align="center">

# agf

**One TUI to find, resume, and manage all your AI coding agent sessions.**

Stop remembering session IDs. Stop `cd`-ing to project directories.\
Just type `agf`.

<!-- Uncomment after adding a demo recording:
<img src="./assets/demo.gif" alt="agf demo" width="740">
-->

<!--
[![CI](https://github.com/subinium/agf/actions/workflows/ci.yml/badge.svg)](https://github.com/subinium/agf/actions)
[![Crates.io](https://img.shields.io/crates/v/agf)](https://crates.io/crates/agf)
[![Release](https://img.shields.io/github/v/release/subinium/agent-tui-finder)](https://github.com/subinium/agf/releases)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
-->

**Supports:** Claude Code · Codex · Cursor

</div>

---

## Install

### Homebrew

```bash
brew install subinium/tap/agf
```

### Cargo

```bash
cargo install agf
```

### From source

```bash
git clone https://github.com/subinium/agf.git
cd agent-tui-finder
cargo install --path .
```

### Pre-built binaries

Grab the latest from [Releases](https://github.com/subinium/agf/releases).

## Setup

Add the shell wrapper so `cd` and `exec` work correctly:

```bash
# Zsh (~/.zshrc)
eval "$(agf init zsh)"

# Bash (~/.bashrc)
eval "$(agf init bash)"

# Fish (~/.config/fish/config.fish)
agf init fish | source
```

## Usage

```bash
agf                # open session finder
agf my-project     # open with pre-applied filter
```

### Keybindings

| Key | Action |
|:---|:---|
| Type anything | Fuzzy search |
| `↑` `↓` | Navigate |
| `Enter` | Open action menu |
| `→` | Preview session details |
| `Tab` | Cycle agent filter |
| `Ctrl+S` | Cycle sort (time / name / agent) |
| `Esc` | Quit |

### Actions

| Action | Description |
|:---|:---|
| **Resume** | Launch agent with saved session ID |
| **New session** | Start fresh with any installed agent |
| **cd** | Navigate to project directory |
| **Delete** | Remove session data (with confirmation) |

## Config

`~/.config/agf/config.toml`

```toml
sort_by = "time"       # "time", "name", "agent"
max_sessions = 200     # limit loaded sessions
```

## Supported Agents

| Agent | Resume Command | Data Source |
|:---|:---|:---|
| Claude Code | `claude --resume <id>` | `~/.claude/` |
| Codex | `codex resume <id>` | `~/.codex/` |
| Cursor | `cursor .` | `~/.cursor/` |

Only agents installed on your system are shown. `agf` auto-detects via `which`.

## Requirements

- macOS or Linux
- One or more of: `claude`, `codex`, `cursor`

## Contributing

PRs and issues welcome. See [CONTRIBUTING](#) or just:

```bash
git clone https://github.com/subinium/agf.git
cd agent-tui-finder
cargo run
```

## License

[MIT](LICENSE)
