# Changelog

## [0.6.4] - 2025-03-20

### Changed

- **SLT upgrade: v0.6 → v0.15** — major TUI library upgrade bringing 9 minor versions of improvements.
- **Rounded borders** — filter bar and bulk-delete header now use `Border::Rounded` with colored borders for a modern look.
- **Native separators** — replaced manual `"─".repeat()` with SLT's `separator_colored()`.
- **Native help bars** — all footer keybinding hints now use SLT's `help()` widget for consistent styling.
- **Responsive breakpoints** — compact layout now uses `ui.breakpoint()` instead of manual width checks.
- **Inline text with `line()`** — preview details, headers, and info rows use `line()` for proper inline text rendering.

### Added

- **Agent filter badges** — agent filter indicator now uses `badge_colored()` / `badge()` widgets.
- **Empty state** — shows a friendly "No sessions found" message when search/filter returns no results.
- **Section dividers in Help** — help screen uses `divider_text()` for section headers.
- **Key hints in Help** — keybindings displayed with `key_hint()` widget for visual distinction.
- **Terminal title** — window title set to "agf" via `RunConfig::default().title()`.

### Removed

- **Dead legacy files** — deleted unused `tui/input.rs` and `tui/render.rs` (ratatui/crossterm remnants).

### Fixed

- **49 `#[must_use]` warnings** — all unused `Response` returns from SLT 0.11+ properly handled.
- **Clippy clean** — resolved 17 clippy suggestions (collapsible if-statements, redundant imports).

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
