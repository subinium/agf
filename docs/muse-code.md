# Muse Code compatibility

AGF discovers retained root sessions under
`${XDG_DATA_HOME:-$HOME/.local/share}/muse/sessions/YYYY/MM/DD/<uuid>/session.jsonl`
and resumes the exact UUID with `muse resume <uuid>`. The default follows Muse's
XDG layout, including on macOS. An explicit relative `XDG_DATA_HOME` is resolved
before the launch changes into the recorded workspace.

The format and resume flags were checked against the official Muse Code
**1.4.2-R4684.1** Linux binary on 2026-10-01. Its SHA-256 was
`dfb3096c91f4767c4d98006460800b7ba906a0b1a408280a926a8dc19a1af64f`.
The binary was downloaded into a temporary directory; no Muse installation,
authentication, or model request is required to use AGF's scanner.

## Storage contract

- `stream.kind = "session"` and `stream.id` must match the UUID directory.
- `runtime.session.metadata` supplies `payload.record.workspace_root`.
  The flattened `payload.workspace_root` form is also recognized by Muse's
  bundled session reader and accepted here.
- `runtime.session` records with `payload.kind = "run"` supply user prompts
  (`event.kind = "started"`) and last-response previews
  (`event.kind = "assistant_message_committed"`). Task events and tool output
  are excluded from these previews.
- `recorded_at` is Unix **microseconds**, converted to AGF's milliseconds.
  Last observed valid activity wins, with file modification time as fallback.
- Reads are limited to 256 KiB at each end of a log. Previews can omit messages
  in the middle, oversized records, and older metadata updates outside those
  windows. They are not a full transcript export. Malformed JSON lines are
  skipped; filesystem read failures are reported as scan failures.

Root logs created by the terminal UI, `exec`, and `serve` can be resumed through
the terminal. Their metadata does not reliably identify the launch surface, so
AGF includes these retained root sessions in its default view. It does not scan
nested `subagent/<child>/session.jsonl` logs as standalone sessions.

AGF does not read the session index or infer generated titles, session names,
branches, or worktree labels. It displays the recorded workspace and bounded
prompt/response previews. Direct deletion is disabled: Muse coordinates its
index, writer leases, subagent logs, and other retained artifacts.

The mode picker offers native defaults, `--disable-approval` (sandbox remains
enabled), and `--yolo` (no approvals or sandbox). None is added unless selected.

## Evidence and fixtures

The [fixture](../tests/fixtures/muse/session.jsonl) contains selected envelope
records captured using the native `muse exec --provider echo --json` command,
with the prompt `AGF fixture prompt 한국어`. The echo provider runs locally
without a model request. Session identity, workspace, and timestamps were
normalized; resource-usage measurements and unrelated records were removed.
It tests the raw log format rather than the separate `--json` stdout format.

Official references:

- [Audit and resume agent sessions](https://dev.meta.ai/docs/cookbook/audit-agent-sessions)
  documents the retained layout, envelope fields, and exact-UUID resume.
- [Deterministic replay in CI](https://dev.meta.ai/docs/cookbook/deterministic-replay)
  documents XDG storage and nested run events.
- [Sessions and turns](https://meta-models.github.io/muse-code-sdk/next/guides/msp-concepts/sessions-and-turns/)
  explains session identity and exclusive writer leases.
- The binary's `muse --help` and `muse resume --help` confirm the launch modes
  and root options accepted before or after `resume`.
