# Troubleshooting

## `dagayn install` did not touch my editor

<!-- derived-from ./USAGE.md#register-mcp-integration -->

Run with `--dry-run` first and confirm the platform was detected. Some integrations are repo-level and only activate when their config directories already exist.

## MCP tools report `database disk image is malformed`

<!-- constrained-by ./USAGE.md#register-mcp-integration -->

This is SQLite `SQLITE_CORRUPT`. A long-lived `dagayn serve` (the Cursor MCP
process) can keep poisoned connections or leftover WAL file descriptors even
after the on-disk `.dagayn/graph.db` is healthy. CLI commands in a new process
often still succeed.

1. Retry the tool. MCP tools now close live handles and retry once.
2. If it still fails, reload the MCP server (or restart `dagayn serve`) so
   leftover WAL handles are dropped.
3. Rebuild only when a fresh process also fails:

```bash
sqlite3 .dagayn/graph.db "PRAGMA quick_check;"
dagayn build --local-embedding none
```

`quick_check` returning `ok` means a rebuild will not help until the MCP
process is restarted.

## The graph is empty or stale

<!-- derived-from ./USAGE.md#build-and-refresh-the-graph -->

Start with:

```bash
dagayn build
dagayn status
```

If the repository moved on disk, rebuild so stored metadata matches the current root.

## MCP tools cannot find docs sections

Ensure `docs/LLM-OPTIMIZED-REFERENCE.md` exists in the repo or installed package layout.

## `dagayn._core` is missing

<!-- derived-from ./USAGE.md#use-the-rust-backend -->

Install a wheel that includes the Rust extension, or rebuild from source with a
Rust toolchain and C compiler available:

```bash
pip install git+https://github.com/manji-0/dagayn.git
```

```bash
uv tool install --from git+https://github.com/manji-0/dagayn.git dagayn
```

For a source checkout without the native extension, rebuild the editable
extension. The old Python parser implementation is no longer shipped.

```bash
uvx --from git+https://github.com/manji-0/dagayn.git dagayn --help
```

For a checkout, rebuild the editable extension:

```bash
uvx maturin develop --release
```

## Type checking fails locally but not in CI

CI uses `pyrefly` with Python 3.13 assumptions and ignores unresolved third-party imports (see `[tool.pyrefly]` in `pyproject.toml`). Runtime support currently starts at Python 3.12.

Match that command locally:

```bash
uv run pyrefly check
```

## Bandit warnings about comments or `nosec`

Bandit may log informational warnings while still exiting successfully. The repository config decides which rules are enforced.

## Notebook cell indexes look wrong

<!-- derived-from ./ARCHITECTURE.md#parsing-model -->

Rebuild after notebook fixture or formatting changes. The parser now tags cells by span overlap to stay stable when line endings shift.
