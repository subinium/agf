# agf 0.15.0

## Agent Tools

- Versioned, read-only JSON search, exact session metadata, resume plans and
  capabilities. No hidden TUI or native-agent execution.
- Local stdio MCP using the official Rust SDK, with fixed scopes, bounded
  requests, modern/legacy interoperability tests and a portable Skill.
- Resume plans preserve literal arguments, the selected executable, cwd and
  storage-root environment without modifying the parent shell environment.

## Compatibility And Safety

- SLT 0.24.0 and current/legacy Gemini session formats.
- Verified provider root overrides, independent Codex SQLite storage, explicit
  Cursor executable selection and readonly source scans.
- Configuration preservation and secret-safe diagnostics, Unicode boundary
  fixes, bounded SQLite title queries, cache recovery and stdout error handling.
- Correct browse viewport/click boundaries, grapheme search cursors and explicit
  unknown process state on unsupported backends.

## Boundaries

The existing 14 scanner providers remain; no new Copilot scanner or web port.
Gemini deletion uses its native workflow. Codex user configuration is supported,
not its full project/profile/managed stack; Oh My Pi profile/XDG extensions are
not emulated. Physical OS IME and real provider execution are distinct from
fixture-based protocol and terminal tests.

See [Agent Integration](https://github.com/subinium/agf/blob/v0.15.0/docs/AGENT_INTEGRATION.md)
for contracts and setup, and [the changelog](https://github.com/subinium/agf/blob/v0.15.0/CHANGELOG.md)
for details. The release workflow gates artifact publication on quality checks
and an exact registry-installed JSON/MCP consumer.
