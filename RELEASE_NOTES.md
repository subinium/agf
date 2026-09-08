# agf 0.15.1

## Dependency Refresh

- Upgrade `dirs` 6 -> 7, `rusqlite` 0.39 -> 0.40.2 and `sha2` 0.10 -> 0.11.
- Remove the old transitive `generic-array` dependency. SHA-256 cache and Gemini
  project identifiers keep their existing encoding, verified with fixed vectors.
- Keep Rust 1.88 support: `libsqlite3-sys` 0.38.2 fixes the build-macro issue that
  previously prevented the SQLite upgrade. No unsafe MSRV override is used.

## Installation And Documentation

Use `cargo install agf --locked` to install the release-tested dependency graph,
or use the prebuilt release archives / `brew install subinium/tap/agf`.
Cargo's `(available: ...)` output is version-selection information, not an error.

The README now documents OS-specific configuration paths, optional shell setup,
profile selection limitations, wrapper reloads and Cargo/Homebrew PATH conflicts.
The OpenCode link and JSON envelope examples are refreshed.

No providers, permission defaults or JSON schema fields change in this patch.
See [the changelog](https://github.com/subinium/agf/blob/v0.15.1/CHANGELOG.md)
and [agent integration](https://github.com/subinium/agf/blob/v0.15.1/docs/AGENT_INTEGRATION.md).
