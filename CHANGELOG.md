# Changelog

## [0.6.0] - 2025-03-14

### Breaking Changes

- **TUI engine: ratatui → [SuperLightTUI (SLT)](https://github.com/subinium/SuperLightTUI)** — complete rewrite of the rendering layer from ratatui's retained-mode to SLT's immediate-mode architecture. Same look, fewer dependencies, simpler code.
- **Shell wrapper updated** — the wrapper now uses a temp file instead of stdout capture. Run `agf setup` again after upgrading, or restart your shell.
- **Keybinding change**: summary cycling changed from `Shift+↑`/`Shift+↓` to `[`/`]`.

### Fixed

- **Scanner hang on unreadable `.git/HEAD`** — `read_git_branch()` now has a 100ms timeout per path, preventing infinite blocking when a git directory is on an unresponsive filesystem.

### Changed

- `ratatui` and `crossterm` dependencies removed; replaced with `superlighttui` v0.6.
- TUI source consolidated from 3 files (~2,350 lines) into a single `tui/mod.rs` (~1,850 lines).
- Shell wrappers (zsh, bash, fish) use `AGF_CMD_FILE` temp file for command passing instead of stdout capture.

## [0.5.5] - 2025-03-10

- Resume mode picker with `Tab` on the action menu.
- Parallelize worktree scanning for faster startup.
