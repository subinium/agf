# Adding an agent

`agf` discovers sessions by reading the files an agent already writes locally — no
plugin binaries, no config. Adding a new agent (harness) is a self-contained
change: implement one scanner and register it in a handful of match arms. The
compiler enforces most of the wiring — every `match self { Agent::… }` becomes
non-exhaustive until you add the new arm — but two registrations live in plain
arrays/`Vec`s the compiler can't check, so they're called out below.

Community integrations are welcome even if the agent isn't a "main" harness;
keep them scoped like the existing ones (read-only scan, deletion limited to the
validated session).

## Checklist

Say the new agent is `Foo`, CLI `foo`, sessions under `~/.foo/sessions/`.

1. **`src/model.rs` — the `Agent` enum.** Add `Foo` and fill in every arm the
   compiler now flags: `Display`, `color()`, `all()` *(plain array — add it)*,
   `cli_name()`, `resume_cmd()`, `new_session_cmd()`. Add `resume_mode_options()`
   only if `foo` has permission/approval flags.
   - `resume_cmd()` **must** quote the id via the passed `shell` (`shell.quote(session_id)`),
     never raw `'{session_id}'` — session ids come from parsed files and may contain
     shell metacharacters.

2. **`src/config.rs`** — add a `foo_sessions_dir()` (or `_dir()`) helper returning
   the on-disk location. Use `dirs::` for platform-correct paths, and
   `AgfError::NoDataDir` (not `NoHomeDir`) when a `dirs::data_dir()` lookup fails.
   Then add the `data_sources()` arm: the paths whose mtime decides cache
   freshness.
   - It must cover **every** file your scanner reads, not just its primary index —
     a source you omit can change without invalidating the cache, so `agf` keeps
     serving a stale payload. Within that constraint keep it narrow: each path is
     stat-walked on every launch.

3. **`src/scanner/foo.rs`** — implement `pub fn scan() -> Result<Vec<Session>, AgfError>`.
   - Return a `Session` per resumable session. Set `timestamp` (Unix **ms**) to the
     **last-activity** time (file mtime or the newest in-file event) — not creation
     time — so time sort is consistent with the other agents.
   - Bound reads on large transcripts with the shared `read_head_tail` / bounded-read
     helpers in `scanner/mod.rs`; never slurp multi-MB logs whole.
   - Skip malformed lines, don't panic on bad input.
   - Register the module in **`src/scanner/mod.rs`**: add `pub mod foo;` **and** a
     `thread::spawn(|| foo::scan().unwrap_or_default())` line in `scan_all()`
     *(plain `Vec` — the compiler won't remind you)*.

4. **`src/cache.rs`** — add the `Agent::Foo => scanner::foo::scan().unwrap_or_default()`
   arm in `start_stale_scan()`. Bump `CACHE_VERSION` only if the cached payload
   *shape* can change within a single released package version (a new agent key
   alone doesn't require it — the `agf_version` stamp forces a rescan on upgrade).

5. **`src/delete.rs`** — add the `Agent::Foo` arm in `delete_agent_sessions()` and
   implement `delete_foo_sessions(ids: &HashSet<&str>)`.
   - It receives a **batch**: bulk delete does one pass per agent, so do the walk
     or open the database once and act on every id in `ids`.
   - **Scope deletion to validated sessions** — match by id in file content or by a
     validated directory name. `is_safe_session_id` already rejects traversal, but
     if you join an id onto a path, re-check it against your own id format first
     (see `delete_yolop_session_from`).
   - Bound what you read: the id lives in a header, so use `read_first_line` /
     `read_head_lines` rather than slurping transcripts.
   - Add a test proving a sibling session survives.

6. **Tests + docs** — unit-test the scanner against a fixture session, add a
   `resume_cmd` test, add a row to the *Supported agents* and storage tables in
   `README.md`, and add the CLI name to the Requirements list.

## Verify

```bash
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo fmt --all -- --check
# Real-data smoke test (scans all agents regardless of install):
AGF_DEBUG=1 cargo run -- list --agent foo --format json
```

`agf list` runs every scanner unconditionally, so you can verify `foo` against a
fixture `$HOME` without installing the CLI:

```bash
HOME=/tmp/agf-fixture cargo run -- list --agent foo --format json
```
