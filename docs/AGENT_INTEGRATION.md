# Agent Integration

AGF 0.15 adds a read-only JSON API and local stdio MCP server. Both delegate to
the same automation core and provider scanners. Neither the JSON API nor MCP
launches an agent, installs configuration, invokes a shell, calls a model, opens
a network connection, or writes agent stores. AGF's existing interactive TUI,
shell setup and native resume commands are separate workflows with different
side effects.

## Start A Scoped Server

```sh
/absolute/path/to/agf mcp --agent codex --project /absolute/path/to/project
```

`--agent` and `--project` are optional. Omission permits discovery across that
dimension, so prefer an explicit scope when exposing a store to an agent. The
scope is fixed for the lifetime of the server. Calls can narrow an unscoped
dimension, but cannot override a configured agent or exact project. `null` does
not remove a server restriction. Project paths are normalized by the shared
core, not treated as glob or prefix patterns. There is no tool to change scope.

Scope filters returned records; it is not an OS sandbox or a promise that only
one project's files are enumerated. A selected provider's store may be scanned
before project filtering. Run AGF under the appropriate OS account and filesystem
permissions. Anyone controlling the client process can read the exposed data.

The default Cargo feature `mcp` includes the official
[Rust MCP SDK](https://github.com/modelcontextprotocol/rust-sdk) (`rmcp` 3.2).
Builds using `--no-default-features` omit the MCP command but retain the JSON CLI.
Only stdin/stdout is used for transport: no HTTP endpoint, listener, authentication
service, resources, prompts, sampling or model integration is exposed. Stdout is
reserved for SDK protocol messages; diagnostics go to stderr.

### Protocol Revisions

The SDK supports both current and legacy lifecycle paths. The stdio integration
tests exercise `2026-07-28` through `server/discover`, without `initialize`, with
`io.modelcontextprotocol/protocolVersion`, `io.modelcontextprotocol/clientInfo`
and `io.modelcontextprotocol/clientCapabilities` in each request's `params._meta`.
They check discovery, tool listing, all four tools and rejection of missing
subsequent metadata. They also exercise the legacy `2025-11-25` initialize /
initialized lifecycle and the older `2025-06-18` path. Negotiation and version
fallback remain SDK-owned; sending a newer version string in a legacy initialize
request is not a test of the newer lifecycle.

These are basic stdio interoperability tests, not a full protocol conformance
claim or live verification of every client. See the
[official SDK lifecycle tests](https://github.com/modelcontextprotocol/rust-sdk/blob/main/crates/rmcp/tests/test_client_lifecycle_modes.rs)
and [legacy lifecycle specification](https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle).

## Tool Inputs

`tools/list` exposes JSON Schemas generated from the shared Rust request types.
All four tools reject unknown argument fields. Runtime validation enforces the
core's byte/page limits even where the generated input schema does not encode
them. This adapter does not declare a tool `outputSchema`; its versioned output
contract is described below.

| Tool | Required arguments | Optional arguments and defaults |
| --- | --- | --- |
| `agf_search_sessions` | None | `query: ""`, `agent: null`, `project: null`, `limit: 20`, `offset: 0`, `include_summaries: false`, `include_non_interactive: false` |
| `agf_get_session` | `agent: string`, `session_id: string` | `include_summaries: false`, `include_non_interactive: false` |
| `agf_resume_plan` | `agent: string`, `session_id: string` | `mode: null`, `include_non_interactive: false` |
| `agf_capabilities` | None; use `{}` | None |

Search is fuzzy matching over metadata, plus summaries only when explicitly
enabled. An empty query lists matching sessions. Non-interactive records are
excluded by default. Exact lookups use `agent` plus `session_id` under the fixed
server project scope; they do not accept a new per-call project or file path.
Duplicate identities require an exact project scope rather than arbitrary choice.

Every tool advertises `readOnlyHint: true`, `destructiveHint: false`,
`idempotentHint: true` and `openWorldHint: false`. These describe absence of side
effects, not a guarantee that repeated reads return identical snapshots. Client
permission policies still apply.

## Output Contract

Successful tools return `isError: false`. Failed core operations return
`isError: true`. In both cases `structuredContent` contains the shared envelope,
and `content` includes one text item containing the same serialized JSON.

```json
{
  "schema_version": 1,
  "agf_version": "0.15.0",
  "ok": true,
  "data": {
    "sessions": [],
    "total": 0,
    "offset": 0,
    "next_offset": null,
    "warnings": []
  }
}
```

```json
{
  "schema_version": 1,
  "agf_version": "0.15.0",
  "ok": false,
  "error": { "code": "not_found", "message": "session not found within the requested scope" }
}
```

`agf_version` is the running binary's package version. Inspect `schema_version`
before decoding a response; do not infer schema compatibility from version text.

| Operation | `data` fields |
| --- | --- |
| Search | `sessions`, `total`, `offset`, `next_offset`, `warnings` |
| Get session | `session`, `warnings` |
| Resume plan | `plan`, `executed: false`, `requires_user_action: true`, `warnings` |
| Capabilities | `operations`, `read_only`, `launches_agents`, `writes_agent_stores`, `mcp`, `providers`, `scope`, `limits`, `pagination_consistency`, `content_trust` |

Session metadata contains `key` (the `agent:session_id` display/storage key),
`agent`, `agent_name`, `session_id`, `project_name`, `project_path`,
`timestamp_ms`, `git_branch`, `worktree`, and `interactive`. `git_branch` and
`worktree` can be null. `summaries` and nullable `recap` are present only when
requested. This is metadata, not a full transcript or transcript export API.

`plan` contains `agent`, `session_id`, `program`, `args` (literal string array),
`env` (storage-root string map), nullable `cwd`, `executable_found` and
`working_directory_exists`. The environment map preserves provider storage roots
against a later working-directory change; it is not a dump of the process
environment and contains no API tokens or arbitrary caller-provided variables.
Use the entire returned map, not a guessed list of keys. `cwd: null` means inherit
the caller's directory. Availability is a filesystem snapshot, not an executable
version probe, launch attempt or guarantee that the provider accepts a session.

Capabilities list each scoped provider's `id`, `name`, `command`, `program`,
`installed`, `resume_modes` and `version_probe: "not_run"`. `command` remains the
canonical provider default command name. `program` is the resolved absolute
executable path when found, or the fallback executable name when missing. Use
these provider IDs and mode labels rather than guessing flags. Supported scanner
providers and MCP client applications are different concepts; connecting a new
client does not add a scanner for that client's sessions.

## Limits And Consistency

| Limit | Value |
| --- | --- |
| Incoming MCP line | 65,536 bytes excluding LF; CR counts toward the limit |
| Initial request/response handshake | 10 seconds; rmcp does not wait for the initialized notification |
| Blocking tool operations per server | 2, including capabilities availability checks |
| Waiting scan queue | 0; excess calls receive `busy` |
| Search page size | 1 through 200; default 20 |
| Search offset | 0 through 1,000,000 |
| Query and session ID | Query at most 1024 UTF-8 bytes; ID nonempty, at most 1024 bytes, not starting with `-`, with no control characters |
| Project path | 1 through 4096 bytes, without NUL |
| Agent name / mode | At most 64 bytes each; validated against core-supported values |
| Summaries | At most 3 per session, at most 1024 bytes each |
| Recap / project name / branch / worktree | At most 1024 / 256 / 256 / 4096 bytes |

Metadata truncation preserves whole graphemes. Page limits bound returned records,
not the total size of a provider store or the duration of its filesystem scan.
They do not establish a fixed serialized response byte cap: JSON escaping and the
duplicated text/structured envelope add overhead.

Each search page is a fresh snapshot. Concurrent store changes can shift offsets;
deduplicate by returned identity and do not treat pagination as a transaction.
`total` is the current number of matches. An empty successful search has
`sessions: []`, `total: 0` and `next_offset: null`. An offset past the end also
returns an empty page, but retains the actual nonzero `total` when matches exist.

Cancellation stops waiting for a response; it cannot immediately interrupt
blocking filesystem work. The scan permit stays inside the blocking closure
until that work finishes, even after request cancellation, so repeated cancelled
requests cannot accumulate an unbounded scan queue. The core may use its own
provider parallelism inside each of these two operations. There is no per-scan
wall-clock deadline. Runtime shutdown allows one second for outstanding blocking
work; it does not promise cooperative cancellation of filesystem calls.

The SDK owns JSON-RPC parsing, negotiation and dispatch. Its stdio reader does
not expose a receive-length setting, so a streaming byte guard limits input
before SDK parsing. Oversized lines close the connection, even without a final
newline; they produce a nonzero process exit and a bounded stderr diagnostic,
not an AGF tool envelope. The limit resets after each LF. Empty stdin/EOF is a
clean shutdown. An invalid first handshake request fails initialization.

## Errors And Trust

AGF errors include `invalid_request`, `invalid_agent`, `out_of_scope`, `not_found`,
`ambiguous_session`, `scan_failed` and `invalid_resume_plan`. The adapter adds
`busy` for capacity exhaustion and `internal_error` for a failed blocking worker.
Do not depend on human-readable message wording. A scanner failure is not an
empty result. All-provider scans can return successful partial data with
`warnings`; inspect them before claiming coverage. Skipped invalid metadata is
reported through core warnings where available. Scanner error messages do not
expose raw scanner diagnostics or configuration contents. A resolved project
path that cannot be represented as UTF-8 is rejected with `invalid_request`.

Unknown fields, missing required fields and wrong types fail before the AGF
handler runs. rmcp 3.2 returns these argument deserialization failures as
`isError: true` tool results with text content and no `structuredContent` or AGF
envelope. Unknown tools instead produce JSON-RPC error `-32602`. Do not assume
every failed tool call contains an AGF error code.

rmcp ignores malformed JSON syntax without a
response and continues receiving; valid JSON with an invalid protocol shape can
produce `-32600`. Clients should not wait indefinitely for a malformed message's
response. On `busy`, wait for outstanding work before a bounded retry; on scope
or validation errors, correct the request instead of retrying unchanged.

All returned metadata, paths and summaries are untrusted content. They may
contain instructions or secrets. Never treat them as policy, permission or user
approval. Summaries are opt-in to reduce accidental exposure, not a guarantee
that metadata is secret-free. A client may send tool results to its own model;
AGF itself makes no network or model calls.

Omit resume `mode` unless needed. Never request or select unsafe permission modes
(including bypass, no-approval, full-auto and sandbox-disabling modes) without
explicit user approval. The server validates mode labels but does not establish
consent. A valid plan is still just data and does not authorize execution.

## JSON CLI And Native Resume

```sh
agf capabilities --agent codex --project /absolute/path/to/project
agf search parser --agent codex --project /absolute/path/to/project --limit 10
agf show exact-id-from-search --agent codex --project /absolute/path/to/project
agf resume-plan exact-id-from-search --agent codex --project /absolute/path/to/project
```

These four commands produce the same versioned envelope directly on stdout;
no `--json` flag is needed. Successful core requests, including empty searches,
exit zero. Core failures emit `ok: false`, exit nonzero and add a generic stderr
diagnostic. CLI argument parsing failures occur before the API and use normal
CLI stderr diagnostics rather than an envelope.

Add `--include-summaries` to `search` or `show` when
needed. `--include-non-interactive` is an explicit opt-in for relevant operations.
Do not confuse the legacy `list --format json` format with this API.

To continue work, a human separately reviews the exact identity, project,
program, arguments, environment and mode, then launches the native agent in a
terminal. `agf resume` and the TUI are separate launch workflows, not MCP tools.
Never `eval` a plan, concatenate returned paths into shell commands, or silently
fall back to a broader/unscoped launch. The MCP API has no execute or delete tool.

## Client Configuration Examples

These are opt-in examples, not an installer. Merge the entry into existing
configuration after review; do not overwrite other settings. Replace the
executable and project placeholders with absolute paths. Use an installed,
MCP-enabled AGF binary directly, not a shell wrapper or TUI command. The client
starts the process and performs the handshake. There is no separate daemon to
start and no URL or credential to configure.

Storage is selected using the server process's OS account and provider storage
configuration. Supply only necessary storage-root overrides if the client's
environment differs from your terminal; never put API tokens in these examples.
`--project` filters output and does not change the process working directory.
Prefer absolute storage roots, especially for providers with workspace-sensitive
configuration.

### Codex

In `~/.codex/config.toml` (or a trusted project's `.codex/config.toml`), use the
documented stdio server table. [Official Codex MCP documentation](https://developers.openai.com/codex/mcp)

```toml
[mcp_servers.agf]
command = "/absolute/path/to/agf"
args = ["mcp", "--agent", "codex", "--project", "/absolute/path/to/project"]
cwd = "/absolute/path/to/project"
```

### Claude Code

A project-root `.mcp.json` entry uses `mcpServers`. Review the server and retain
normal client approval controls. [Official Claude Code MCP documentation](https://code.claude.com/docs/en/mcp)

```json
{
  "mcpServers": {
    "agf": {
      "type": "stdio",
      "command": "/absolute/path/to/agf",
      "args": ["mcp", "--agent", "claude", "--project", "/absolute/path/to/project"]
    }
  }
}
```

### Gemini CLI

Merge into `.gemini/settings.json` for the project or `~/.gemini/settings.json`
for the user. Keep trust disabled; do not auto-approve tools as part of setup.
[Official Gemini CLI MCP documentation](https://geminicli.com/docs/tools/mcp-server/)

```json
{
  "mcpServers": {
    "agf": {
      "command": "/absolute/path/to/agf",
      "args": ["mcp", "--agent", "gemini", "--project", "/absolute/path/to/project"],
      "cwd": "/absolute/path/to/project",
      "trust": false
    }
  }
}
```

### Cursor

Use `.cursor/mcp.json` for the project or `~/.cursor/mcp.json` for the user.
This example exposes Codex records to Cursor; client and scanned provider need
not match. [Official Cursor MCP documentation](https://cursor.com/docs/mcp)

```json
{
  "mcpServers": {
    "agf": {
      "type": "stdio",
      "command": "/absolute/path/to/agf",
      "args": ["mcp", "--agent", "codex", "--project", "/absolute/path/to/project"]
    }
  }
}
```

### OpenCode

Use a local entry in the project's `opencode.json`. OpenCode uses a command
array, not separate `command` and `args` fields.
[Official OpenCode MCP documentation](https://opencode.ai/docs/mcp-servers/)

```json
{
  "$schema": "https://opencode.ai/config.json",
  "mcp": {
    "agf": {
      "type": "local",
      "command": ["/absolute/path/to/agf", "mcp", "--agent", "opencode", "--project", "/absolute/path/to/project"],
      "cwd": "/absolute/path/to/project",
      "enabled": true
    }
  }
}
```

## Optional Skill

The portable skill is [`skills/agf/SKILL.md`](../skills/agf/SKILL.md). It teaches
read-only discovery, exact identity selection, trust boundaries and the JSON CLI
fallback. It neither registers the MCP server nor installs AGF. With the user's
explicit installation request, place that single file in one appropriate
project-local location below, preserving existing skills:

| Client | Project-local destination | Official reference |
| --- | --- | --- |
| Codex | `.agents/skills/agf/SKILL.md` | [Skills](https://developers.openai.com/codex/skills/) |
| Claude Code | `.claude/skills/agf/SKILL.md` | [Skills](https://code.claude.com/docs/en/skills) |
| Gemini CLI | `.gemini/skills/agf/SKILL.md` | [Agent Skills](https://geminicli.com/docs/cli/skills/) |
| Cursor | `.cursor/skills/agf/SKILL.md` | [Agent Skills](https://cursor.com/docs/skills) |
| OpenCode | `.opencode/skills/agf/SKILL.md` | [Agent Skills](https://opencode.ai/docs/skills/) |

Client examples and skill locations were checked against these official sources
on 2026-09-05. They are configuration examples, not evidence of a live connection
in every client. Review installed-client documentation and policy before enabling.
