# Contributing to dagayn

## Issues

Issues are the primary way to participate in the project. Bug reports, questions, and feature suggestions are all welcome.

When filing an issue, include enough context for maintainers to reproduce or understand the problem. Feature requests are read and considered, but there is no commitment to implement them.

## Pull requests

Pull requests are not accepted at this stage of development. The maintainers manage all changes directly. If the project matures to broader OSS adoption, this will be revisited.

## Security issues

Do not file sensitive vulnerabilities as public issues. Follow `SECURITY.md`.

## Development setup (maintainers)

<!-- constrained-by ./prek.toml -->

```bash
uv sync --extra dev
uv tool install prek
prek install
```

```bash
uv run ruff check .
uv run ruff format --check .
uv run pyrefly check
uv run pytest --tb=short -q
```

Type checking runs on Pyrefly, whose Pydantic integration (>= 0.33.0) applies
Pydantic model semantics — `BaseModel`, `Field`, `ConfigDict`, and
`pydantic_settings.BaseSettings` — statically, so schema violations in
`dagayn/state_types.py` and the tool dispatchers surface as type errors.

`uv sync --extra dev` builds the PyO3 extension (`dagayn._core`) and vendors
pinned Tree-sitter grammars. The first build fetches grammars over the network.

### Rust workspace

Requires a Rust toolchain (1.92+) and a C compiler. `uv sync` is enough for
the Python test path (maturin). For `cargo test --workspace` or
`cargo clippy --workspace -- -D warnings`, point PyO3 at uv's interpreter so
`dagayn-py` can link `libpython`:

```bash
export PYO3_PYTHON="$(uv run python -c 'import sys; print(sys.executable)')"
```

### VS Code extension (`dagayn-vscode/`)

Requires Node 22+ and pnpm.

```bash
cd dagayn-vscode
pnpm install
pnpm compile
pnpm lint
pnpm fmt:check
pnpm test
pnpm test:compile
```

The `prek` hooks (configured in `prek.toml`) run ruff/pyrefly on Python changes and
the VS Code checks when files under `dagayn-vscode/` change. Pre-push pytest
runs tests related to the files being pushed, not the full suite. CI still
runs `uv run pytest --tb=short -q`. To auto-fix VS Code formatting:

```bash
cd dagayn-vscode && pnpm fmt
```

The formatter is Biome (configured in `biome.json`).
