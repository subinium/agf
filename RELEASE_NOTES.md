# agf 0.17.0

Find and resume sessions across 16 local coding agents, now including Muse Code.
This release also includes the Antigravity and terminal improvements prepared
for 0.16.0, which was not published as a tagged release.

## Muse Code

- Discover retained Muse Code sessions with `--agent muse` or `--agent muse-code`
  in the TUI, JSON CLI, and MCP server.
- Search bounded prompt previews, inspect the last response, and resume the
  exact session with `muse resume <id>`. Muse is also in the new-session menu.
- Honor `XDG_DATA_HOME`, including relative storage paths, when handing off to
  Muse from the recorded workspace. Offer native default, no-approval, and
  yolo launch modes through the existing picker.
- Read root session logs without modifying Muse's index or writer leases.
  Exclude nested subagent logs and keep direct deletion disabled.
- Verify the retained-log format against the official Muse Code 1.4.2 binary
  with an offline echo fixture. See [compatibility notes](docs/muse-code.md).

## Security Audit

- Fix main-branch audit reporting with job-scoped `checks: write` permission.
  The failing run found no vulnerabilities, but could not report its warning
  because its GitHub token lacked permission to create a check run.
- Replace the yanked transitive dependency `yoke-derive` 0.8.3 with 0.8.4.

## Antigravity And Terminal Runtime

- Add Google Antigravity CLI discovery and exact `agy --conversation <id>`
  resume, building on @VirtoTran's contribution in #95. Thank you for the
  initial integration.
- Read bounded SQLite/transcript metadata without modifying agent stores.
  Preserve read failures, non-interactive filtering and exact session identity.
- Leave Antigravity direct deletion disabled because native cleanup also
  coordinates conversation artifacts and active-session state.
- Upgrade SuperLightTUI from 0.24.0 to 0.25.0 while retaining Rust 1.88 support.

## UI And Navigation

- Align headers, menu columns and responsive footer keys. Keep Help, Settings
  and session details usable in compact terminals, including 20x8 views.
- Use F1 for Help, F2 for search scope, F3/F4 for summaries and Ctrl+L for
  details. Literal `?`, `[` and `]`, plus Left/Right, remain search input.
- Preserve input order around action transitions, reset user searches to the
  best match and prevent Escape from triggering a simultaneous confirmation.
- Agent-menu digits and Enter both open the permission picker. Native
  permission defaults and explicit confirmation requirements are unchanged.
- Distinguish scanning, empty results and provider failures; keep urgent action
  errors visible even in narrow views or during a concurrent refresh failure.

## Appearance And Color Roles

- Add persistent Auto, Dark and Light settings. Auto uses the startup
  `COLORFGBG` hint when available and otherwise selects Dark.
- Keep pointers, menu numbers, pins and selection surfaces neutral. Use agent
  colors only for agent names, cyan for search matches/footer keys, and
  semantic colors only for status messages and destructive actions.
- Maintain readable truecolor/256-color text and a conservative neutral
  fallback for 16-color terminals. NO_COLOR retains structural selection and
  status cues without emitting color codes.
- Apply the same policy to `agf watch`, separating selection from process status.

## Validation And Compatibility

- Keep the existing JSON/MCP schema and read-only provider-storage boundaries.
- Add regression coverage for color roles, Unicode, resizing, input order,
  read-only storage, exact native handoff and terminal cleanup.
- Validate debug/release behavior with isolated synthetic fixtures. Actual
  provider accounts and physical OS IME composition are not covered by these
  automated tests.

Install a published release with `cargo install agf --locked`, use the release
archives, or use `brew install subinium/tap/agf` once its formula is updated.
