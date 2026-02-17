# agf

**One TUI to find, resume, and manage all your AI coding agent sessions.**

[![CI](https://github.com/subinium/agf/actions/workflows/ci.yml/badge.svg)](https://github.com/subinium/agf/actions)
[![Release](https://img.shields.io/github/v/release/subinium/agf)](https://github.com/subinium/agf/releases)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

<!-- Uncomment after adding a demo recording:
![agf demo](./assets/demo.gif)
-->

## Quick Start

```bash
brew install subinium/tap/agf
agf setup
```

That's it. Restart your shell and run `agf`.

## What it does

`agf` scans sessions from **Claude Code**, **Codex**, and **Cursor**, then lets you:

- **Resume** any session with one keystroke
- **Fuzzy search** across all projects and summaries
- **Filter by agent** with Tab
- **cd** into project directories
- **Delete** old sessions

No more remembering session IDs or `cd`-ing to project directories.

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

Optional. `~/.config/agf/config.toml`

```toml
sort_by = "time"       # "time" | "name" | "agent"
max_sessions = 200
```

## Alternative Install

<details>
<summary>From source</summary>

```bash
git clone https://github.com/subinium/agf.git
cd agf
cargo install --path .
agf setup
```

</details>

<details>
<summary>Pre-built binaries</summary>

Download from [Releases](https://github.com/subinium/agf/releases) and place in your `$PATH`.

</details>

<details>
<summary>Manual shell setup</summary>

If `agf setup` doesn't work for your environment:

```bash
# Zsh — add to ~/.zshrc
eval "$(agf init zsh)"

# Bash — add to ~/.bashrc
eval "$(agf init bash)"

# Fish — add to ~/.config/fish/config.fish
agf init fish | source
```

</details>

## Supported Agents

| Agent | Data Source | Detected via |
|:---|:---|:---|
| Claude Code | `~/.claude/` | `which claude` |
| Codex | `~/.codex/` | `which codex` |
| Cursor | `~/.cursor/` | `which cursor` |

Only installed agents are shown.

## Requirements

- macOS or Linux
- One or more of: `claude`, `codex`, `cursor`

## License

[MIT](LICENSE)
