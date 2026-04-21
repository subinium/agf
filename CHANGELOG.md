# Changelog

## [0.9.0] - 2026-04-21

### Fixed

- **UTF-8 panic on non-ASCII paths** — `gemini.rs` hashed-dir slicing, `codex.rs` title truncation, `list.rs`/`stats.rs`/`watch.rs` column truncation used byte-based slicing and crashed on Korean/emoji/CJK input. All paths now char-safe via shared `scanner::char_prefix` + local char-based `truncate` helpers.
- **Scanner panics silently dropped** — `scanner::scan_all` replaced `unwrap_or_default` on `JoinHandle::join` with `AGF_DEBUG=1`-gated stderr logging. Crashes are still non-fatal but now diagnosable.
- **Cache staleness broken for nested sources** — `cache::get_max_mtime` now recurses via `walkdir` (max depth 4). Previously, file writes inside `~/.codex/sessions/<date>/`, `~/.gemini/tmp/<dir>/chats/`, and `~/.cursor/{chats,projects}/*` did not bump the top-level dir mtime, leaving the cache permanently stale for Codex/Gemini/Cursor.
- **Cache/config write corruption** — atomic write-then-rename for `sessions.json` and `config.toml` prevents truncation on concurrent `agf` invocations or `^C`.
- **Gemini 64KB UTF-8 cut** — `String::from_utf8_lossy` over the hard-capped buffer could slice a multi-byte char. Buffer is now trimmed to the last valid UTF-8 boundary before decode.
- **Selection lost on sort** — `apply_sort` now snapshots the selected `session_id` and restores cursor position after reorder.
- **Resume bypassed PermissionSelect via number key** — `ActionSelect` number keys `1-9` (and mouse click) now route `Resume` through the `PermissionSelect`/`ResumeSelect` flow, matching the Enter path.
- **DeleteConfirm Up/Down toggled Yes/No** — only Left/Right, `h`/`l`, and Ctrl-h/l now toggle the horizontal choice.
- **Preview dismissed on any key** — only Esc and Left dismiss; Up/Down (and Ctrl-p/n/k/j) now cycle to prev/next session without leaving preview; other keys are no-ops.
- **`watch` thread leak** — refresh thread now gated by an `AtomicBool`; slow scans no longer accumulate threads every interval.
- **`agf setup` loose detection** — precise sentinel `# agf - AI Agent Session Finder` replaces `contains("agf init")`, avoiding false positives from user comments.
- **`agf setup <unknown-shell>` exit code** — now returns non-zero instead of silently `Ok(())`.
- **Silent config parse failure** — `Settings::load` now prints `[agf] config parse error at <path>: <err> — using defaults` instead of discarding pins silently.
- **Cache version/parse failures** — `AGF_DEBUG=1` now logs why cache was discarded (version mismatch or parse error).

### Changed

- **Startup flash eliminated** — `main.rs` enters the alt-screen (and hides the cursor) via a RAII guard **before** cache load and scan. Previously, cold-cache first-runs showed the shell prompt for 200ms–3s while scanning; now the terminal switches immediately and the scan runs under the TUI surface.
- **`which` fork storm removed** — `is_agent_installed` replaces 7 per-launch subprocess calls with a single cached `$PATH` directory walk via `OnceLock`. ~50ms saved on every startup.
- **No `Session` clone per keystroke** — `FuzzyMatcher::filter` signature is now `(&[Session], &[usize], query, ...)`; the TUI passes the agent-filtered indices directly instead of cloning `Session` values into a subset vec.
- **`name_col_width` cached** — computed once in `update_filter` and invalidated on sort/delete, not recomputed per-frame.
- **`agents_with_sessions` uses `agent_counts`** — avoids an O(N) walk through every cycle of the agent filter.
- **Cache + scan honor `installed_agents()`** — uninstalled agents no longer burn a thread, syscalls, or cache slot on every launch. Filter applied uniformly at cache, scanner, and TUI layers.
- **`scan_stale_agents` dispatch** — direct `match agent → scanner::*::scan()` instead of iterating `plugin::all_plugins()` inside each spawned thread.
- **Stats labels** — `Today / This week / This month` → `Last 24h / Last 7d / Last 30d` to match the actual rolling-window semantics.
- **Stats comment drift** — `"most common agent for color"` corrected to reflect the first-seen behavior it actually implements.
- **`watch` process detection** — `pgrep -f` → `pgrep -x` so running agents are matched by exact binary name, not by any cmdline containing the string (editors/greps no longer false-positive).
- **Redundant `Settings::load`** — `App::new` now accepts a `Settings` parameter instead of re-reading the config file.
- **`Settings::save_editable` + cache writes** — both use atomic tmpfile + rename.

### Added

- **Shared scanner helpers** in `src/scanner/mod.rs`: `char_prefix`, `read_first_line`, `first_line_truncated`. Removed duplicated file-reading and truncation logic from `codex.rs`/`pi.rs`/others.
- **`AltScreenGuard` (RAII) in `main.rs`** — ensures the alt-screen is left and cursor restored even on early-exit paths.
- **`decrement_agent_count()` helper in TUI** — keeps `agent_counts` consistent after single/bulk delete, which fixes the agent filter showing deleted agents.
- **`debug_assert!` in `delete_session`** — defense in depth against `session_id` values containing `/` or `..`.

### Removed

- **Per-project git-branch thread + 100ms timeout** — `claude::read_git_branch` now just `fs::read_to_string`s the ~30-byte `.git/HEAD`. The timeout-per-project was overhead, not safety.
- **`any_key_pressed` helper** — preview no longer dismisses on arbitrary keys; helper deleted.

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
