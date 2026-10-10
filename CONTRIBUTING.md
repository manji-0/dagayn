# Contributing to dagayn

## Issues

Issues are the primary way to participate in the project. Bug reports, questions, and feature suggestions are all welcome.

When filing an issue, include enough context for maintainers to reproduce or understand the problem. Feature requests are read and considered, but there is no commitment to implement them.

## Pull requests

Pull requests are not accepted at this stage of development. The maintainers manage all changes directly. If the project matures to broader OSS adoption, this will be revisited.

## Security issues

Do not file sensitive vulnerabilities as public issues. Follow `SECURITY.md`.

## Development setup (maintainers)

The toolchain comes from the Nix flake (`flake.nix`): Python 3.14, uv, the
Rust toolchain pinned by `rust-toolchain.toml` (plus `cargo-llvm-cov`), Node 22
with corepack for pnpm, prek, and jj. Enter it with `nix develop`, or let
direnv load it from `.envrc` (`direnv allow`). Inside the shell uv uses the
flake's Python (`UV_PYTHON`) and `PYO3_PYTHON` already points at it.

<!-- constrained-by ./prek.toml -->

```bash
nix develop
uv sync --extra dev
prek install
```

```bash
uv run ruff check .
uv run ruff format --check .
uv run pyrefly check
uv run pytest --tb=short -q -n auto --dist loadfile
```

Type checking runs on Pyrefly, whose Pydantic integration (>= 0.33.0) applies
Pydantic model semantics — `BaseModel`, `Field`, `ConfigDict`, and
`pydantic_settings.BaseSettings` — statically, so schema violations in
`dagayn/contracts/state_types.py` and the tool dispatchers surface as type errors.

`uv sync --extra dev` builds the PyO3 extension (`dagayn._core`) and vendors
pinned Tree-sitter grammars. The first build fetches grammars over the network.
It uses the `dev-fast` Cargo profile (`[tool.uv]` in `pyproject.toml`: thin
LTO, no single codegen unit), so a rebuild after a Rust edit takes seconds;
release wheels are built with `maturin build --release` and keep fat LTO.

Tests run in parallel with pytest-xdist (`-n auto --dist loadfile`); pass
`-n 0` to debug a single test in-process.

### Rust workspace

Requires a Rust toolchain (1.98+) and a C compiler; the flake provides both.
`uv sync` is enough for the Python test path (maturin). For
`cargo test --workspace` or `cargo clippy --workspace --all-targets -- -D warnings`,
`dagayn-py` must link `libpython`. The flake's shell sets `PYO3_PYTHON` for
that; outside it, point PyO3 at uv's interpreter:

```bash
export PYO3_PYTHON="$(uv run python -c 'import sys; print(sys.executable)')"
```

CI runs the Rust tests under `cargo llvm-cov` and fails below 82% line coverage.
The PyO3 layer (`dagayn-py`) is left out of that figure because the Python
tests exercise it:

```bash
cargo llvm-cov --workspace --summary-only --ignore-filename-regex 'dagayn-py/'
```

### VS Code extension (`dagayn-vscode/`)

Requires Node 22+ and pnpm. In the flake's shell, corepack supplies the pnpm
version pinned by `packageManager`, which it resolves from the current
directory, so run pnpm inside `dagayn-vscode/` rather than with `-C`.

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
