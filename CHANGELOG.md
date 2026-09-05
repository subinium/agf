# Changelog

## [Unreleased]

## [0.15.0] - 2026-09-06

### Added

- Versioned, read-only JSON `search`, `show`, `resume-plan` and `capabilities`
  commands. Results are bounded and summaries opt-in; exact identities, scope,
  error codes and per-request pagination are explicit (#85).
- Local stdio MCP tools over the same core, using official `rmcp` 3.2, with
  fixed scopes, bounded incoming messages/concurrency and no model calls or
  agent execution. A portable Skill and client integration guide are included.
- Structured resume plans contain literal argv, cwd, storage-root environment
  and executable/directory availability. Plans never execute commands.
- Current Gemini JSONL support alongside legacy JSON, migration deduplication,
  bounded record recovery and explicit Cursor executable selection (#84).

### Changed

- SuperLightTUI 0.22.3 to 0.24.0; Rust 2024/MSRV 1.88 retained (#86).
- Default builds include the optional `mcp` feature; no-default builds retain
  the native TUI and JSON API without the MCP dependency.
- Provider root overrides are shared across scans, fingerprints and resume.
  Codex supports separate SQLite storage and user-config precedence; supported
  Claude, Gemini, pi, Hermes and OpenCode root settings are honored (#83).
- Gemini direct deletion is disabled to preserve native sidecar/subagent
  consistency. Other native-managed deletion restrictions remain intact.
- Empty JSON lists succeed; invalid output formats are rejected. Stdout write
  failures propagate, with BrokenPipe handled at the CLI boundary (#79).

### Fixed

- Preserve malformed/unreadable settings instead of replacing unrelated values.
  Parser diagnostics and automation errors do not echo configuration snippets.
- Freeze resolved executable paths and storage roots before changing cwd;
  temporary shell environments do not overwrite the parent environment.
- Preserve native failure codes through a dedicated PowerShell child even
  after environment restoration.
- Reject option-like/control-character session IDs before discovery/launch;
  preserve literal metacharacters as arguments and keep plans non-executing.
- Preserve large Unicode record metadata/activity at bounded UTF-8 prefixes
  for Qwen and Prime Agent, while rejecting malformed interior bytes (#87).
- Retain failed source scans for retry and fingerprint followed file-symlink
  target metadata; incomplete fingerprints are not stored as fresh (#88).
- Bound OpenCode/Hermes title aggregation before materialization (#89).
- Keep footer/separator clicks outside session rows, match visible-row budgets,
  hide unavailable Cd actions, and use grapheme-based search cursors (#90).
- Distinguish unknown process state from confirmed idle state in Watch and
  preserve unfiltered successful scan data in its refreshed cache (#75).
- Preserve file permissions and report Unix parent-directory sync failures
  after atomic writes; no Windows crash-durability guarantee is implied.

### Scope

- Existing 14 scanner providers remain; no Copilot scanner or browser port.
- Codex user configuration is supported, not its full project/profile/managed
  stack. Oh My Pi profile/XDG extensions remain outside the validated scope.
- Metadata and summaries remain untrusted data. MCP annotations are not user
  permission to execute a returned plan or select a bypass mode.
- CSV retains original values; use text import for untrusted spreadsheet data.
- Protocol/PTY fixtures do not establish physical OS IME or actual provider
  execution. Validation and supported client setup are documented separately.

## [0.14.1] - 2026-08-18

### Added

- **Grok Build support** — discovers official xAI Grok sessions under `$GROK_HOME/sessions` (default `~/.grok/sessions`), reads durable activity/title/recap/git/worktree metadata from `summary.json`, filters hidden subagents by default, and resumes the exact selected ID with `grok --resume`.
- **Kimi Code support** — discovers v1 ISO-timestamp and v2 epoch-ms `state.json` sessions under `$KIMI_CODE_HOME/sessions` (default `~/.kimi-code/sessions`), uses `session_index.jsonl` to recover migrated legacy working directories, and resumes with `kimi --session`.
- **Qwen Code support** — discovers current `projects/*/chats` and legacy `tmp/*/chats` JSONL sessions, honors `QWEN_RUNTIME_DIR`, `QWEN_HOME`, and `advanced.runtimeOutputDir`, surfaces custom titles/first prompts/branches, and resumes with `qwen --resume`.

### Fixed

- The New Session permission picker now uses the same current flag definitions as Resume, removing a second stale Codex flag table and automatically keeping Kimi permission modes consistent.

### Performance and safety

- Qwen scanning reads at most a bounded head window plus 64 KiB title windows, including a safe oversized-first-prompt fallback; it never materializes whole multi-megabyte transcripts merely to render the list.
- Grok and Kimi metadata documents have explicit size bounds, and all three scanners propagate directory/read failures so a partial scan cannot replace valid stale cache rows.
- Direct AGF deletion is fail-closed for Grok, Kimi, and Qwen because their native tools coordinate active sessions and secondary indexes that a file-only delete would bypass; the single-delete action is hidden and bulk mode marks these rows as unavailable.

## [0.14.0] - 2026-08-15

### Added

- **Prime Agent support** — discovers flat Prime Agent v3 JSONL sessions, resumes exact IDs with `prime-agent --resume`, honors environment/global session-root settings, and sorts by durable user/assistant activity. Named sessions, git state, and current recap metadata are surfaced. Deletion is fail-closed because upstream exposes no public safe delete command for active daemon sessions.
- **Kiro v3 support** — discovers metadata/event pairs under `$KIRO_HOME/sessions/cli` in addition to the v2 SQLite store. Kiro resume now uses the selected conversation ID via `--resume-id`.
- **Codex non-interactive filtering** — subagent and exec threads are hidden by default and can be included with `--include-non-interactive` or the matching setting.
- Linux ARM64 release artifacts, an MSRV 1.88 CI lane, and a RustSec audit gate.

### Fixed

- Codex scans are now strictly read-only. Partial rollout walks can no longer delete valid rows from Codex-owned SQLite databases.
- Cache invalidation uses nanosecond/size/path fingerprints, follows deeper Codex stores, and watches SQLite WAL files plus every secondary scanner input. Stale rows remain visible when a refresh fails and failed scans are never cached as successful emptiness.
- Project View stores stable `(agent, session ID)` identities and rebuilds groups after streamed results, preventing wrong-session actions and out-of-bounds panics. `max_sessions` is now display-only and never truncates persistent cache data.
- Time, name, and agent sorts use deterministic total ordering; timestamps are normalized globally and legacy Codex activity uses rollout mtime.
- Exact quick-resume behavior: summary queries work, agent filters push down to one scanner, invalid permission modes fail clearly, and current Codex approval/sandbox flags replace removed aliases.
- Terminal behavior: non-TTY TUI/watch calls fail cleanly, the session-row mouse offset is correct, shell wrappers preserve agent exit status, cwd-independent Hermes resumes execute, `NO_COLOR`/`TERM=dumb` are honored, and untrusted display metadata cannot emit terminal controls.
- Deletes use collision-safe atomic writes and exact session matching; Claude append-only history is never rewritten while an agent may be writing it, Gemini duplicates are removed across stores, and Hermes prefix siblings survive.
- Stats windows are cumulative and future/invalid timestamps are separate. Watch opens from cache immediately, preserves selection by identity, rejects zero intervals, and avoids repeated missing-`pgrep` probes.

### Performance

- Cursor store databases are indexed once instead of scanned once per transcript.
- Pi, Cursor, Kiro, and Prime Agent JSONL parsing has total and per-line allocation bounds; Kiro v2 previews no longer materialize unlimited conversation blobs.
- CLI commands reuse fresh per-agent cache entries and scan only stale/requested agents.
- Text truncation is grapheme-aware, preserving emoji/ZWJ clusters while maintaining terminal-column alignment.

### Changed

- SuperLightTUI 0.20.1 → 0.22.3, including upstream terminal lifecycle, suspend/resume, bidi, and Unicode fixes.
- Rusqlite is pinned to 0.39.0 so the declared Rust 1.88 MSRV remains buildable; the 0.40 transitive build script currently requires Rust 1.95 without declaring it.
- Cache schema 8 → 9. Pin and summary state now use composite agent/session identities while legacy bare-ID pins remain readable.

## [0.13.1] - 2026-07-31

Post-release audit of 0.13.0. No new features and no CLI changes; everything below is a correctness, durability, or performance fix found by reading the 0.13.0 diff and then the rest of the tree.

### Fixed

- **Delete had no path-traversal guard in release builds** — the check was a `debug_assert!`, which the release profile compiles out. `Session`s are rebuilt from `~/.cache/agf/sessions.json`, an ordinary user-writable file that re-runs no scanner validation, and Yolop's delete joins the id straight onto its sessions directory before calling `remove_dir_all`. The guard is now a runtime check on every agent, plus a Yolop-specific `session_<32 hex>` shape check.
- **Rewriting `history.jsonl` was not crash-safe** — deleting a Claude Code or Codex session read the whole history file, filtered it in memory, and wrote it back over the original. An interruption between the truncate and the last byte left a half-written file with no original. All full-file rewrites (including `agf setup`'s shell rc edit) now write a fsync'd temp and `rename` over the destination.
- **Claude Code recaps were always one idle cycle stale** — cache freshness keyed on `history.jsonl` alone, but `away_summary` recaps, `aiTitle`, and the worktree label all come from `~/.claude/projects/*/<id>.jsonl`, which changes without touching history. `projects/` is now part of the freshness check (measured: ~5 ms added per launch on a 1,478-file tree).
- **CJK project names broke column alignment** in `agf list`, `agf stats`, and the compact TUI layout — column budgets were measured in terminal columns but padded with `{:<n$}`, which pads by `char` count. A 5-character Hangul name rendered 15 columns wide and pushed every column after it out of line; `agf stats` measured names in *bytes*, which saturated the padding to zero. All display-width work now goes through one width-aware helper.
- **`agf list --format csv` did not escape every field** — `session_id`, `agent`, `time`, and `branch` were written raw. Git permits commas in branch names, which silently shifted every later column in the consuming tool.
- **Yolop: an unreadable `events.jsonl` dropped the whole session** — `workspace.json` already supplies the project, title, and worktree, and the session is still resumable, so a failed log read now degrades the entry instead of removing it from the listing.
- **Yolop: an implausible future timestamp pinned a session to the top forever** — `updated_at`, event `ts`, and the log mtime are all self-reported or clock-dependent. Candidates more than a day ahead of now are ignored, falling back to "now" so such a session ages normally.
- **Agent counts drifted from the list they label** — with `max_sessions` set, an incoming scan recorded its batch size *before* truncation, so the agent badge advertised sessions that had just been dropped off the end. Counts are now recomputed from the list after every merge and delete.
- **Codex would read a superseded database at `state_10.sqlite`** — `state_*.sqlite` files were ranked lexicographically, which sorts `state_10` below `state_9`. They are now ranked by their numeric suffix.
- **Action previews showed commands that would not run** — the `cd` preview interpolated an unquoted path (breaking on any path with a space), and cwd-independent agents rendered as `cd — && hermes` because `display_path()` renders their empty path as an em dash. Previews now quote exactly like the executed command and drop the `cd` where the real command does.

### Performance

- **Bulk delete is one filesystem pass per agent, not per session** — deleting N sessions previously meant N full walks of `~/.claude/projects` (or of the pi/Oh My Pi tree, reading every transcript in full on each walk) plus N read-modify-write cycles over `history.jsonl`. Deletes are now batched by agent.
- **pi / Oh My Pi delete reads a bounded header** instead of every byte of every transcript; the session record is on line 1 (pi) or line 2 (Oh My Pi). Codex's rollout delete likewise reads only the first line.
- **Search-match highlighting uses a binary search** over the sorted match positions rather than a linear scan per character, per row, per frame.
- **`agf watch` probes only installed agents** and stops probing entirely when `pgrep` is absent (Windows, minimal containers), instead of spawning one failing process per known agent every refresh tick.

### Changed

- **Removed the unused `AgentPlugin` trait** (`src/plugin.rs`) — only 2 of its 8 methods were ever called through the trait, and `name()` duplicated `Display for Agent` string-for-string, yet `docs/adding-an-agent.md` required filling it in for every new agent. Cache freshness paths moved to `config::data_sources()`; adding an agent now touches 8 sites instead of 12.
- **`CACHE_VERSION` 7 → 8** — forces one rescan so upgraded entries pick up the new Claude freshness inputs and drop any persisted far-future Yolop timestamps.
- **`AgfError::NoDataDir`** distinguishes a missing platform data directory from a missing home directory (Kiro and Yolop previously reported "No home directory found" when the home directory was present).

## [0.13.0] - 2026-07-31

### Added

- **Oh My Pi (`omp`) session support** ([#56](https://github.com/subinium/agf/pull/56), by @Michelh91) — discover, preview, resume (`omp --resume <id>`), and delete Oh My Pi sessions under `~/.omp/agent/sessions/`, reusing the pi JSONL parser and excluding nested subagent transcripts.
- **Yolop session support** ([#55](https://github.com/subinium/agf/pull/55), by @chaliy) — discover, preview, resume, and delete sessions from Yolop's platform-native session store. Current metadata supplies session titles, canonical repository names, timestamps, and concise worktree labels; older sessions fall back to first prompts and repository-name recovery.
- **Contributor guide for adding an agent** — [`docs/adding-an-agent.md`](docs/adding-an-agent.md) documents the full wiring checklist, including the three `Vec` registrations the compiler can't enforce.

### Fixed

- **Bulk delete could delete the wrong sessions** ([#58](https://github.com/subinium/agf/pull/58)) — multi-select stored `sessions` Vec indices captured at toggle time, but a background scan landing mid-selection reorders that Vec, so confirming a bulk delete removed different sessions than the ones checked. Selections are now keyed by `(agent, session_id)` and resolved at delete time.
- **`session_id` is shell-escaped in resume commands** ([#58](https://github.com/subinium/agf/pull/58)) — the id was wrapped in raw single quotes, so an id containing `'` broke the eval'd command and a crafted transcript filename could inject shell. It now goes through the shell-aware quoter (POSIX and PowerShell).
- **Cursor and pi sorted by creation time, not last activity** ([#58](https://github.com/subinium/agf/pull/58)) — a session created long ago but used today sorted as old and sank below stale ones. Both now use the transcript's last-modified time (pi: the max of its header timestamp and mtime), matching the other agents. Oh My Pi inherits the pi fix.
- **Grouped view ordered projects by the first session, not the newest** ([#58](https://github.com/subinium/agf/pull/58)) — correct only under Time sort; grouped view now keys on each group's max session timestamp.
- **Codex timestamp overflow on corrupt data** ([#58](https://github.com/subinium/agf/pull/58)) — `updated_at * 1000` now saturates instead of wrapping to a garbage sort key.
- **Streaming startup keeps the initial cursor at the top** ([#57](https://github.com/subinium/agf/pull/57), by @MilkClouds) — when a fast scanner (commonly OpenCode) returned before a slower one (commonly Claude Code), the later merge pushed the cursor down the newly sorted list. The top row now follows incoming results until the user moves away from it, while explicit selections remain anchored.

### Performance

- **Faster cold scan on large `~/.claude/projects` trees** ([#58](https://github.com/subinium/agf/pull/58)) — the Claude metadata scan read whole transcript bodies (up to 272 KB each) just to reach the tail for the off-by-default recap. The tail window is now 32 KB; measured read I/O dropped 62 MB → 29 MB (−53%) on a 714-file tree, with the recap unchanged.

### Changed

- **Dropped `panic = "abort"`** ([#58](https://github.com/subinium/agf/pull/58)) — restores `scan_all`'s per-thread panic isolation so one malformed session file can't abort the whole listing.

## [0.12.0] - 2026-06-11

### Added

- **Grouped view: `Ctrl+L` previews the selected session** ([#50](https://github.com/subinium/agf/pull/50), by @soomtong) — the first grouped→preview path; child rows jump straight to the session detail pane.
- **`j` / `k` navigation in the action menu, `Ctrl+P` / `Ctrl+N` in the help screen** ([#50](https://github.com/subinium/agf/pull/50), by @soomtong) — consistent with the other modes; the footer advertises `Tab/jk`.

### Changed

- **List scrolling keeps a 3-row margin below the cursor** ([#50](https://github.com/subinium/agf/pull/50), by @soomtong) — browse, grouped browse, and `agf watch` all scroll before the cursor hits the bottom edge. Review fixups hardened the arithmetic (see Fixed) and added the missing end-of-list clamps so the margin can't overscroll into blank rows.
- **Summary cycling `[` / `]` now wraps around** ([#50](https://github.com/subinium/agf/pull/50), by @soomtong) — cycling past the last summary returns to the first; the help screen reads `[ or ]` to avoid `/`-key confusion.
- **Deletes only disappear from the UI when the delete actually succeeded** — both delete paths previously did `let _ = delete_session(...)` and removed the row unconditionally, so a permissions failure looked like success until the session reappeared on next launch. Failed deletes now stay visible.
- **Rust edition 2024, `rust-version = "1.88"` declared** ([#52](https://github.com/subinium/agf/pull/52)) — the floor is set by let-chains; the code already used 1.87 APIs (`usize::is_multiple_of`), which `clippy::incompatible_msrv` surfaced the moment an MSRV was declared.
- **rusqlite 0.32 → 0.40, toml 0.8 → 1** ([#52](https://github.com/subinium/agf/pull/52)) — both verified zero-code-change bumps; the bundled SQLite moves to libsqlite3-sys 0.38. sha2 0.11 (breaks `{:x}` digest formatting) and superlighttui 0.21 (adds 29 `#[must_use]` call sites) were evaluated and deliberately deferred.

### Fixed

- **Windows without the shell wrapper: bare `agf` no longer crashes with `program not found`** ([#49](https://github.com/subinium/agf/pull/49), by @MilkClouds) — with `AGF_SHELL` unset the shell defaulted to POSIX on every OS, so native Windows exec'd `cd '…' && agent` via a nonexistent `sh`. The default is now OS-aware via a pure, unit-tested `default_shell` (native Windows → PowerShell; `MSYSTEM`/unix-style `SHELL` → POSIX). POSIX hosts are bit-for-bit unaffected.
- **Upgrades no longer serve stale per-session cache data** ([#51](https://github.com/subinium/agf/pull/51), closes [#37](https://github.com/subinium/agf/issues/37)) — mtime freshness can't see "the data didn't change but its interpretation did" when scanner logic changes within one `CACHE_VERSION`. The cache now stamps the writing binary's version and any mismatch forces a one-time rescan; `AGF_DEBUG=1` logs `cache built by X, current Y → rescanning`. The write-side carry-over path is gated the same way, and per-release `CACHE_VERSION` bumps are no longer part of the release checklist.
- **Gemini scanner: UTF-8 char-boundary panic in `extract_summary_partial`** ([#52](https://github.com/subinium/agf/pull/52)) — a multi-byte (e.g. CJK) character straddling the 1 KiB partial-read window made the byte slice panic and silently killed the entire Gemini scan. The window now backs up to a char boundary; CJK regression test added.
- **`agf resume --list 0 <query>` no longer panics** ([#52](https://github.com/subinium/agf/pull/52)) — `--list 0` underflowed `top_n.len() - 1`; it now shows the top result instead.
- **Cursor scanner: `hex_decode` panic on non-ASCII store.db metadata** ([#52](https://github.com/subinium/agf/pull/52)) — a corrupt `meta` blob could index mid-char; an `is_ascii` guard rejects it and the scan continues.
- **Scroll-margin arithmetic underflow** (review fixups on [#50](https://github.com/subinium/agf/pull/50)) — the margin branch can fire while `selected < visible`, where `selected - visible + 1 + margin` underflows usize: debug builds panicked on routine down-navigation (release builds wrapped to the right value by accident). All three sites now use saturating arithmetic; regression tests pin no-underflow, margin, and end-clamp behavior.
- **`agf watch`: selection clamped when a refresh shrinks the list** ([#52](https://github.com/subinium/agf/pull/52)) — a background refresh that removed sessions could leave the cursor past the end, blanking the viewport.
- **Ctrl+H no longer leaks into the fuzzy-search textarea** ([#50](https://github.com/subinium/agf/pull/50), by @soomtong).

### Internal

- **Audit-driven modernization** ([#52](https://github.com/subinium/agf/pull/52)) — 31 verified findings applied: three shared scanner helpers (`collapse_whitespace`, `project_name_from_path`, `push_concat_titles`) replacing 5–6 duplicated sites each; `Agent` derives serde and the hand-rolled string maps are deleted (on-disk cache format pinned byte-identical by a round-trip test); let-else/let-chain flattening across scanners, delete, and TUI dispatch; per-frame allocation cuts (`render_chunks` takes its chunks by value, `update_filter`/`apply_sort` stop cloning, `detect_editor` caches config for the process lifetime); claude `history.jsonl` pre-filters orphaned sessions the way codex has since v0.11.4; `read_first_line` gets the same 512 KiB byte budget as the other bounded readers. Tests grew 53 → 75.
- **CI hardening** ([#52](https://github.com/subinium/agf/pull/52)) — clippy runs `--all-targets` (tests were unlinted in CI) including a new Windows lint job that caught a real platform gap on its first run; `Swatinem/rust-cache` (Test job 35s → 17s); per-ref concurrency cancellation; `--locked` everywhere cargo resolves. The release workflow gains a tag↔`Cargo.toml` version guard, loud checksum failures, and a graceful publish skip when `CARGO_REGISTRY_TOKEN` is unset instead of a red run on every release.

## [0.11.4] - 2026-05-29

### Fixed

- **`scanner/pi`: a single invalid-UTF-8 line no longer drops the rest of the file (or the whole session)** — `parse_session` used `reader.lines().map_while(Result::ok)`, which stops at the first `Err` and silently truncates every prompt summary that follows. Worse, when the bad line lands before the session header (a real failure mode for crash-truncated or rotation-racing writes), the header is never captured and the entire session is dropped from `agf list`. This is the same bug class as the `extract_first_prompt` `.ok()?` regression fixed in v0.11.3 for the Cursor scanner. Replaced with `let Ok(line) = line_result else { continue; };` so each bad line is skipped individually. Regression test: `parse_session_skips_invalid_utf8_lines`.
- **`scanner/codex`: `~/.codex/history.jsonl` no longer scales with the user's lifetime codex usage** — `read_history_summaries` streamed the entire file, parsed every line into a `HistoryEntry`, and accumulated `(f64, String)` tuples for every session_id ever seen — including thousands of sessions that no longer have a rollout JSONL on disk. For power users who run codex daily, the file reaches tens of MB; v0.11.3's `CACHE_VERSION` bump to 6 forces a cold rescan on every upgrader, which would otherwise pay this cost on first launch. The function now takes the same `live_session_ids` set already collected by `collect_live_session_ids` and short-circuits any line whose `session_id` is not in it. When `live_session_ids` is `None` (transient I/O on the sessions tree), the legacy "keep everything" behavior is preserved, mirroring `scan_sqlite`'s same-condition fallback. Regression tests: `read_history_summaries_pre_filters_against_live_session_ids` and `read_history_summaries_keeps_all_when_live_set_is_none`.

### Docs

- **README: Kiro row now surfaces "no per-session resume — always opens the latest session for the cwd"** — `kiro-cli` ignores `session_id`, so selecting a specific older Kiro entry in the TUI silently launches a different session. The caveat was previously only in `Agent::Kiro::resume_cmd`'s inline comment; it now appears in the top agents table where users actually read.
- **README: Hermes row now surfaces "cwd-independent — resumes in your current shell directory"** — documented in the expanded `Full session storage paths` section but missing from the discoverable top table.

### Internal

- **`.gitignore`: `/.claude/` added** — every contributor running Claude Code locally was seeing `~/.claude/` show up as untracked in `git status`, with the latent risk of an accidental `git add .` committing a personal agent state directory.

## [0.11.3] - 2026-05-27

### Fixed

- **Cursor scanner: walk both legacy `.txt` and current Composer 2+ `.jsonl` layouts** ([#45](https://github.com/subinium/agf/pull/45), by @rooty0 / Stan) — current Cursor stores transcripts at `~/.cursor/projects/*/agent-transcripts/<uuid>/<uuid>.jsonl` (depth 4) rather than the legacy `~/.cursor/projects/*/agent-transcripts/<uuid>.txt` (depth 3). The scanner walked depth 3 with a `.txt`-only filter, so on current Cursor it returned **zero sessions**. Verified against live data: `~/.cursor/projects` had 2 JSONL transcripts at depth 4 and 0 TXT, and `agf list --agent cursor-agent` returned `No sessions found.` before this release. Closes [#35](https://github.com/subinium/agf/issues/35).
- **Cursor scanner: read chat metadata from the right table** ([#45](https://github.com/subinium/agf/pull/45), by @rooty0) — the previous code queried `SELECT value FROM cursorDiskKV WHERE key = 'composerData'`, which is the **IDE's** `state.vscdb` schema, not the CLI's `store.db`. Cursor CLI's `store.db` actually exposes a `meta(key TEXT PRIMARY KEY, value TEXT)` table with a single `key = '0'` row whose value is a hex-encoded JSON containing `agentId`, `name`, `createdAt`, `mode`, and `lastUsedModel`. Verified via `sqlite3` against a real store.db on disk.
- **Cursor scanner: skip JSONL transcripts whose `store.db` is missing** ([#45](https://github.com/subinium/agf/pull/45), by @rooty0) — `cursor-agent --resume` only surfaces sessions that have BOTH a transcript and a `~/.cursor/chats/<workspace>/<session_id>/store.db` entry; reporting orphaned transcripts that the CLI itself refuses to resume just confuses the listing. Legacy `.txt` sessions are unaffected (they predate the `chats/` directory).
- **Cursor scanner: fall back to the first user prompt when `store.db` has no usable metadata** ([#45](https://github.com/subinium/agf/pull/45), by @rooty0) — the JSONL is parsed for the first `role: user` text part, with `<user_info>` system injections skipped and `<user_query>` wrappers stripped.
- **`extract_first_prompt` no longer panics on inverted `<user_query>` tags** — `str::find` returns the FIRST occurrence of each substring independently, so a text part where `</user_query>` byte-precedes `<user_query>` (e.g. a pasted log or AI-generated code sample) gave `start > end` and `text[s+12..e]` panicked with `begin > end`. Confirmed via a standalone rustc reproducer. The closing tag is now searched **after** the opening one. Regression test: `extract_first_prompt_does_not_panic_on_inverted_tags`.
- **`extract_first_prompt` no longer aborts on the first malformed or non-UTF-8 line** — both the per-line IO read and `serde_json::from_str` used `.ok()?`, which propagates `None` out of the whole function on the first error instead of skipping the bad line. A single corrupted/truncated/non-UTF-8 line at the top of the JSONL silently disabled the blank-summary fallback for the rest of the file. Replaced with `let Ok(...) else { continue; };` (matching `scanner/pi.rs`). Regression tests: `extract_first_prompt_skips_malformed_json_lines` and `extract_first_prompt_skips_invalid_utf8_lines`.
- **`extract_first_prompt` now bounded by a 512 KiB byte budget** — pi.rs added this safeguard in v0.11.2 after large Claude logs stalled the TUI; Cursor transcripts can carry multi-MB tool-result blobs, and the `CACHE_VERSION` bump in this release forces a cold rescan for every upgrader, so the same precaution applies.
- **Cursor delete: legacy `.txt` transcripts now actually get removed** — `delete_cursor_agent_session` called `remove_dirs_matching_name(&projects_dir, &session.session_id)`, but that helper filters on `path.is_dir()` AND `file_name == name`. Legacy sessions live at `agent-transcripts/<uuid>.txt` (a file named `<uuid>.txt`), so it never matched. Delete returned `Ok(())`, the orphan file persisted on disk, and the next scan resurrected it. A new sibling helper `remove_files_matching_name` removes the file form alongside the directory form. Regression test: `delete_cursor_agent_removes_legacy_txt_transcript`.
- **Cursor scanner: enforce stem == parent UUID invariant on the `.jsonl` arm** — the previous check only required the grandparent to be named `agent-transcripts`. A stray `agent-transcripts/<uuidA>/<uuidB>.jsonl` would produce `session_id = uuidB`, which mismatches both the store.db lookup and what `cursor-agent --resume` expects. Real Cursor always writes them equal, but the invariant is now explicit. Regression test: `scan_from_rejects_jsonl_with_stem_mismatched_to_parent`.
- **`decode_dash_path` test coverage for hyphenated project segments** — added `decode_dash_path_resolves_hyphenated_segments` which places `agent`, `agent-tui`, and `agent-tui-finder` as sibling directories and asserts the backtracking decoder resolves to the longest existing match. This is the load-bearing case for this very repo's path.

### Changed

- **`CACHE_VERSION` bumped to 6** — the new orphan-skip rule fires only on fresh scans; cached `0.11.x` cursor entries persist with their old summaries until each transcript's mtime changes. Bumping the version forces a one-time rescan on upgrade so the "35 orphans → 0" effect actually lands for upgraders.

### Docs

- **README: Cursor CLI doc link + transcript paths updated** ([#45](https://github.com/subinium/agf/pull/45), by @rooty0) — `docs.cursor.com/agent` no longer resolves; switched to `cursor.com/docs/cli/overview`. Storage column now lists both the current JSONL layout and the legacy `.txt` form.

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
