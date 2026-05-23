# Changelog

## [0.11.2] - 2026-05-23

### Fixed

- **pi: resume the selected session by id** (#43, by @shellus) — pi sessions were resumed with a bare `pi --resume`, which opens an interactive picker / the latest session for the cwd and ignored the session the user actually selected in agf. The command is now `pi --session '<id>'`. Verified against the pi-mono source (`resolveSessionPath` in `packages/coding-agent/src/main.ts`): an argument with no `/`, `\`, or `.jsonl` is treated as a session-id prefix and matched against the session store; because agf wraps resume as `cd '<project_path>' && pi --session '<id>'`, pi resolves it in the session's own project directory and resumes directly without a fork prompt. (Confirmed on pi 0.53.0.)
- **pi: keep every session from a project selectable** (#43, by @shellus) — the scanner deduped to the most recent session per project directory, a workaround for the old "only resumes latest" assumption. Now that resume-by-id works, all sessions are listed and individually resumable.
- **pi: show prompt summaries and full history** (#43, by @shellus) — the scanner now extracts user-message text from each session's JSONL (collapsing whitespace, capped at 120 chars per line) so the listing shows what a session was about, and the Preview `History:` pane lists every prompt — parity with the other agents instead of bare `project (model)` rows.

### Changed

- **cache: `CACHE_VERSION` bumped to 5** (#43, by @shellus) — the pi payload now carries prompt summaries, so 0.11.x cache entries written before this release would otherwise keep rendering only the project name until each session's source mtime happened to change. The bump forces a one-time rescan on upgrade.

### Internal

- **pi scanner: bound the per-file read with a 512 KiB budget** — collecting every prompt requires reading the whole JSONL, but pi transcripts can grow to several MB and the `CACHE_VERSION` bump forces a cold rescan for everyone on upgrade. The read loop now stops after 512 KiB (the header is the first line, so it is always captured), mirroring the `read_head_tail` cap added in v0.10.1 after large Claude logs stalled the TUI.
- **model: document Kiro's resume-latest behavior** — restored the note (dropped in #43) that `kiro-cli chat --resume` has no per-session flag and ignores `session_id`, now placed on the `Agent::Kiro` arm of `resume_cmd` where the command is defined.

### Reverted

- **TUI default-to-cwd search query** (from #43) — the merged PR also pre-filled the search box with `$PWD` when `agf` was launched without a query. That changed the no-arg behavior for *every* agent (empty list in non-project directories) and re-introduced the cwd special-casing deliberately removed in v0.11.0 (the cwd-match sort boost made time-sort look broken). Reverted to keep this release scoped to pi; a cwd default can land later as its own opt-in setting.

## [0.11.1] - 2026-05-04

### Changed

- **`superlighttui` bumped 0.17 → 0.20.1** — picks up 8 patch/minor releases of the underlying TUI library since v0.10.0 was tagged. Drop-in: every public API agf uses (`Context`, `Color::Rgb`, `KeyCode`, `KeyModifiers`, `RunConfig`, `TextareaState`, container/col/row builders, `help_colored`, `separator_colored`, `consume_key`, `mouse_down`, `quit`) is unchanged; agf does not touch any of the v0.20 breaking APIs (`gauge`/`line_gauge`/`breadcrumb` chainable builders, `scrollable_with_gutter` `GutterOpts`, `Constraints` `WidthSpec`/`HeightSpec` redesign, `f32 → f64` ratio unification on `SplitPane`). Notable improvements inherited for free:
  - **3–5× flush-path speedup on redraw-heavy frames** (SLT 0.18.2 #62) — `flush_buffer_diff` now coalesces consecutive same-style cells in a row into a single `Print(run)` instead of per-cell, dropping `queue!` calls roughly 12000 → 2000 on a 200×60 redraw.
  - **~1000× speedup on static frames** (SLT 0.20.0 #171) — per-row hash skip in `flush_buffer_diff` short-circuits cell iteration on rows whose contents and style didn't change. The agf list view, which is mostly static while the user reads it, gets the full benefit.
  - **`rgb_to_ansi256` u8 overflow fix at `r=g=b=248`** (SLT 0.19.1 #104) — agf's color palette (`Rgb(229, 229, 229)`, `Rgb(245, 158, 11)`, etc.) sits below the threshold so the user-visible bug never fired in agf, but downstream installs on color-limited terminals now route grayscale tones to the correct ANSI 256 cell instead of silently mapping to `Indexed(0)` (Black).
  - **`stdout` BufWriter** (SLT 0.19.1 #172) — every frame now batches dozens of `write_all` ANSI commands behind a 64 KiB `BufWriter`, ending with a single `flush()`. Reduces syscalls per frame on every TUI mode without any agf-side change.
  - **Bordered title CJK truncation + overdraw fixes, image command count drop, treemap/textarea panic fixes** (SLT 0.19.x), **textarea undo/redo, modal `tab_trap` opt-in, `Anchor` enum + `modal_at`** (SLT 0.20.0) — not used by agf today; available for future TUI work.

### Fixed

- **DeleteConfirm: arrow Up/Down (and `j`/`k`, `Ctrl-j`/`Ctrl-k`) now toggle Yes/No** ([#38](https://github.com/subinium/agf/issues/38)) — previously only Left/Right worked, which was surprising on the v0.11.0 release because every other modal-style picker in agf accepts both axes. The toggle is the same direction-agnostic flip (`Yes ↔ No`) regardless of which arrow key fires.
- **ActionSelect / AgentSelect / PermissionSelect / ResumeSelect: arrow keys cycle through the menu instead of stopping at edges** ([#39](https://github.com/subinium/agf/issues/39)) — `Up` on the first item now jumps to the last; `Down` on the last wraps to the first. Tab/BackTab already wrapped — Up/Down/`Ctrl-p`/`Ctrl-n`/`Ctrl-j`/`Ctrl-k` now match.
- **Footer shows `agf v<version>` on the leftmost cell of every mode's status bar** ([#40](https://github.com/subinium/agf/issues/40)) — version is read from `CARGO_PKG_VERSION` at compile time so it stays in sync with the published crate. The 10 per-mode status bars are now routed through a single `render_footer` helper to keep layout uniform.

## [0.11.0] - 2026-05-04

### Added

- **Hermes Agent support** (#34, by @SHL0MS) — adds [Hermes Agent](https://github.com/NousResearch/hermes-agent) (Nous Research's self-improving agent) as a first-class scanner. Reads top-level sessions from `~/.hermes/state.db` (SQLite); aggregates child-session titles as additional summaries (same pattern as OpenCode subagents); cascade-deletes messages → child sessions → parent → on-disk JSON dumps under `~/.hermes/sessions/session_<id>.json`. Resume via `hermes --resume <session_id>`.

### Fixed

- **scanner/hermes: pull first user message as preview when title is NULL** — Hermes lazily auto-generates titles after the first exchange, so short sessions stay un-titled and the listing fell back to bare `api_server session (model) — N msgs`. The scan query now selects the earliest `messages.content WHERE role='user'`, collapses whitespace, strips `<user_query>` wrapper tags, and caps to 160 chars so the detail pane shows what the conversation was actually about.
- **scanner/hermes: empty project_path so resume stays in the user's cwd** — Hermes is cwd-independent; the original PR set `project_path = ~/.hermes`, which made `agf` emit `cd ~/.hermes && hermes --resume <id>` and yanked the shell out of whatever project the user was working in. `shell::cd_and` now skips the `cd` when the quoted path is empty (`""` / `''` / `\"\"`), and the Hermes scanner leaves `project_path` empty. `display_path()` renders empty paths as `—` so the TUI doesn't show a blank cell.
- **delete/hermes: wrap cascade in a single SQLite transaction** — the original four DELETEs ran independently; a mid-cascade failure (disk full, DB locked, etc.) could leave orphan messages whose `sessions` row was already gone, surfacing as ghost rows on the next scan. The four DELETEs are now `BEGIN`/`COMMIT`-wrapped via `Connection::transaction`.
- **TUI: time sort not applied on first frame when `config.sort_by` is unset** (regression visible since the cache was grouped per-agent) — `main.rs` only called `app.apply_sort()` inside an `if let Some(sort_by) = config.sort_by`, so the default-sort path silently rendered the cache's per-agent grouping order. A session from 11 minutes ago could land below sessions from a month ago on the first paint. `apply_sort()` is now always called; `sort_mode` falls through to `SortMode::Time` when `config.sort_by` is `None`.
- **TUI: cwd-match boost removed** — the secondary sort that pushed sessions whose `project_path == $PWD` above everything else made the time-sorted listing look broken when `agf` was launched from inside a project: 11 sessions from "this project" surfaced first, then suddenly a 2-minute-old session from another project, then a 31-minute-old Hermes session, and so on. The boost was implicit (no on-screen indicator), so users read it as "time sort is wrong." Pinning is still honored — it's an explicit user action — but cwd is no longer special-cased.
- **cache: bump `CACHE_VERSION` to 3** — Hermes Agent gained a new entry in the per-agent cache map, and the Hermes session payload now carries first-user-message previews instead of bare source/model fallbacks. Without bumping, 0.10.x cache files would surface as stale "cli session (...)" summaries on first 0.11.0 launch until the underlying DB mtime happened to change. The bump forces a one-time rescan on upgrade.

### Notes

- Cursor CLI scanner regression tracked separately in [#35](https://github.com/subinium/agf/issues/35) — recent cursor-agent installs write transcripts as `<id>/<id>.jsonl` (depth 4) instead of the legacy `<id>.txt` (depth 3) the scanner expects, so `agf list --agent cursor-agent` returns 0 sessions on those installs. Out of scope for this release.

## [0.10.3] - 2026-05-01

### Fixed

- **scanner/claude: skip sessions whose per-session JSONL is missing** (#27, #29, by @Mert-coderoid) — `~/.claude/history.jsonl` accumulates session IDs forever; Claude Code never trims it when the per-session JSONL under `~/.claude/projects/*/<id>.jsonl` is deleted. Those orphan IDs surfaced in the listing and resume failed with `No conversation found`. `scanner::claude::scan` now filters its output through the set of session IDs that have a JSONL on disk; the directory walk is shared with `scan_session_metadata` so the tree is still read once per scan.
- **delete/codex: also remove the SQLite `threads` row** (#28, #30, by @Mert-coderoid) — since the Codex scanner moved to `state_*.sqlite` as primary source, deletes that only removed the rollout JSONL and `history.jsonl` entry came back on the next scan. `delete_codex_session` now walks every `state_*.sqlite` in `~/.codex/` and runs `DELETE FROM threads WHERE id = ?1`; older-schema dbs without `threads` are skipped silently.
- **scanner/codex: prune orphan `threads` rows on scan** (#31, #32, by @Mert-coderoid) — `state_*.sqlite` rows whose rollout JSONL no longer exists kept dominating `agf list` / `agf stats` (e.g. 324 ghost rows for an empty `~/.codex/sessions/`) and could not be resumed. `scan_sqlite` now builds the live session-id set from `~/.codex/sessions/`, excludes orphans from the listing, and `DELETE`s them from every `state_*.sqlite`. A walker error with no live IDs collected falls back to the legacy "surface every row" behavior so a transient I/O failure cannot wipe the table. Note: Codex scans are no longer strictly read-only — they will hard-delete unrecoverable orphan rows.

## [0.10.2] - 2026-04-25

### Fixed

- **Cache write race on early TUI exit (regression in 0.10.1)** — v0.10.1 introduced streaming background scans; if the user exited the TUI before a worker finished, `cache::write_cache` persisted that agent as `(empty session list, fresh mtime)`, hiding its sessions on the next launch until file mtime changed again. `write_cache` now takes the still-scanning agent set and carries the prior cache entry verbatim for those agents — only completed scans replace cache state.

## [0.10.1] - 2026-04-25

### Fixed

- **TUI hangs / never opens on heavy Claude logs** — `scanner::claude::scan_session_metadata` line-iterated every per-session JSONL to EOF to find the latest `away_summary` recap. On a directory with multi-MB session files (10+ MB jsonl is common with long Claude Code sessions), rayon would parse hundreds of MB in parallel and the TUI never showed up. Fixed by capping per-file I/O to 16 KB head + 256 KB tail via a new `scanner::read_head_tail` helper: `cwd` and `aiTitle` are extracted from the head, the latest `away_summary` from the tail, and small files (≤ 272 KB) still read in full. Cold scan on a ~1.6 GB Claude log directory dropped from 48 s to 0.7 s in local testing.

### Changed

- **Background scan + streaming TUI ingest** — `cache::start_stale_scan` returns an `mpsc::Receiver<ScanResult>` and the TUI now drains it on every render frame. With a warm cache, the TUI opens instantly on cached sessions and the scanning agents stream results in as they finish (footer shows `• scanning N…` until every worker reports). With a cold cache, the TUI still opens immediately on the first agent that completes instead of blocking on the slowest one. Final cache write happens at TUI exit.
- **Per-agent scan timing** under `AGF_DEBUG=1` so users can locate the slow agent on their machine.

## [0.10.0] - 2026-04-25

### Added

- **Windows / PowerShell support** (#22, by @MilkClouds) — `agf init powershell` (alias `pwsh`) emits a wrapper compatible with Windows PowerShell 5.1 and PowerShell 7+; `agf setup` auto-detects PowerShell on Windows and writes to `$PROFILE.CurrentUserAllHosts`. A new `CommandShell` (Posix / PowerShell), selected via the `AGF_SHELL` env var the wrapper sets, routes `action::*` and the TUI new-session preview through shell-specific quoting (`''` vs `'\''`) and `cd_and` (`Set-Location ...; if ($?) { ... }` vs `cd ... && ...`). POSIX behavior is unchanged when `AGF_SHELL` is unset.
- **Windows agent detection** — `is_agent_installed` now matches `%PATHEXT%`-aware stems on Windows, so `claude.exe` / `claude.cmd` / `claude.ps1` resolve correctly. Previously every agent read as "not installed" on Windows and the TUI showed "No agent sessions found" even when sessions existed on disk.
- **UTF-8 round-trip in PowerShell wrapper** — wrapper reads `AGF_CMD_FILE` with `-Encoding UTF8`, fixing CP949/CP1252 mojibake on Windows PowerShell 5.1 (Korean Windows etc.) so non-ASCII project paths `Set-Location` correctly.
- **CI runs on Windows** — `windows-latest` job added so PATHEXT stemming, PowerShell command synthesis, and the wrapper UTF-8 contract cannot silently regress.
- **Release ships Windows x86_64 binary** — `release.yml` matrix gains `x86_64-pc-windows-msvc`; tagged releases now include `agf-x86_64-pc-windows-msvc.zip` alongside the existing macOS/Linux tarballs.

### Changed

- **`CommandShell::from_env` cached via `OnceLock`** — the wrapper sets `AGF_SHELL` once before exec'ing `agf`, so the value is immutable for the process lifetime; caching collapses repeated env lookups on the TUI render path and at every `action::*` call.

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
