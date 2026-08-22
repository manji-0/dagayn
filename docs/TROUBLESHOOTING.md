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

## Search results come from a different repository

Every MCP response carries `_repo` — the `repo_root` it answered from, the
`db_path` it read, and whether that root was `explicit` (the client passed
`repo_root`) or `auto` (resolved from the server's working directory). Check it
first: `source: "auto"` with an unexpected `repo_root` means the server had no
idea which project you meant.

That happens when the MCP entry has neither `cwd` nor `--repo`, because the
server then inherits a working directory from whatever launched it — Cursor
launches user-level servers with `cwd=$HOME`, and an editor started from a
terminal inherits that shell's directory.

`dagayn serve` without an explicit `--repo` no longer freezes that guess into
every later tool call. Each call re-resolves from IDE workspace hints
(`WORKSPACE_FOLDER_PATHS`, `CURSOR_PROJECT_DIR`, `CLAUDE_PROJECT_DIR`). A
single open folder still works when `cwd` is `$HOME`. Two or more unrelated
hints, with `cwd` inside none of them, are an error rather than "whichever
graph was built last".

Project-level `.cursor/mcp.json` (written by `dagayn install`) sets
`cwd` to `${workspaceFolder}` so that process starts in the repository.
Do **not** put `cwd`/`--repo ${workspaceFolder}` on the user-level
`~/.cursor/mcp.json` entry: there `${workspaceFolder}` is the folder
containing that file, and pinning it indexes the wrong tree. The install
strips those keys from the user copy on purpose.

If a tool still needs a specific checkout, pass `repo_root` or pin the
server:

```json
{"mcpServers": {"dagayn": {"command": "dagayn", "args": ["serve", "--repo", "${workspaceFolder}"], "cwd": "${workspaceFolder}", "type": "stdio"}}}
```

in **project-level** `.cursor/mcp.json` only. `"cwd": "${workspaceFolder}"`,
or `CRG_REPO_ROOT` in that entry's `env`, work equally well.

Two refusals back this up, both reported as tool errors rather than silently
answering:

- a graph whose recorded `repo_root` is a different existing directory is never
  read (rebuild it, or point at the right root);
- an auto-detected root that is your home directory or the filesystem root is
  refused for reads and builds. Set `DAGAYN_ALLOW_WIDE_ROOT=1` if you really do
  want to index it.

If a stray `~/.dagayn/graph.db` already exists from an earlier run, delete that
file (keep `~/.dagayn/registry.json`).

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
