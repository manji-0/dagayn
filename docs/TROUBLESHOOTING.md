# Troubleshooting

## `dagayn install` did not touch my editor

<!-- derived-from ./USAGE.md#register-mcp-integration -->

Run with `--dry-run` first and confirm the platform was detected. Some integrations are repo-level and only activate when their config directories already exist.

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

For a source checkout without the native extension, set
`DAGAYN_BACKEND=python` explicitly to use the Python compatibility backend.

```bash
uvx --from git+https://github.com/manji-0/dagayn.git dagayn --help
```

For a checkout, rebuild the editable extension:

```bash
uvx maturin develop --release
```

## Type checking fails locally but not in CI

CI uses `ty` with Python 3.13 assumptions and ignores unresolved third-party imports. Runtime support currently starts at Python 3.12.

Match that command locally:

```bash
ty check dagayn --python-version 3.13 --ignore unresolved-import
```

## Bandit warnings about comments or `nosec`

Bandit may log informational warnings while still exiting successfully. The repository config decides which rules are enforced.

## Notebook cell indexes look wrong

<!-- derived-from ./ARCHITECTURE.md#parsing-model -->

Rebuild after notebook fixture or formatting changes. The parser now tags cells by span overlap to stay stable when line endings shift.
