# dagayn agent guide

<!-- constrained-by ./docs/USAGE.md -->
<!-- constrained-by ./docs/COMMANDS.md -->
<!-- constrained-by ./docs/LOCAL-EMBEDDINGS.md -->

This repository ships `dagayn`, a fork of `code-review-graph` with extra emphasis on Terraform, Markdown, and mixed-language monorepos.

## How agents should work with this repo

- use `dagayn` in all user-facing commands
- build or update the graph before asking graph-backed questions
- start broad tasks with `get_minimal_context_tool` so the agent sees graph
  freshness, risk, and suggested next tools before spending tokens elsewhere
- treat graph paths as repo-root-relative where the fork expects registered file trees
- use targeted graph tools before reading broad file sets
- treat graph analysis as evidence-ranked leads: cite thresholds, counts,
  reason codes, truncation state, `answerability`, and `missingness` when
  drawing conclusions
- treat `query_graph_tool` zero-result and not-found responses as graph-limited:
  read `zero_result_reason`, `next_action`, and missingness before concluding
  absence
- fall back to `rg`/file reads when graph output is stale, ambiguous, truncated,
  or lacks exact source text
- keep docs aligned with fork behavior, not upstream prose

## Useful commands

```bash
dagayn build
dagayn update
dagayn status
dagayn detect-changes --base HEAD~1
dagayn serve
```

## Embedding and rebuild discipline

- Do not run embedding-enabled full rebuilds as a routine verification step.
- Treat `dagayn build --force-full-build --local-embedding low` as an explicit
  embedding-quality or end-to-end maintenance operation, not a normal parser,
  flow, or documentation-edge check.
- For ordinary implementation verification, prefer focused tests, `dagayn tool`
  queries, `dagayn update`, or `dagayn build --local-embedding none` when a clean
  graph parser/postprocess check is truly necessary.
- If new files must be included in the graph for verification, stage or otherwise
  make them visible first, then run the smallest graph refresh that proves the
  claim. Do not compensate for untracked files by starting an embedding rebuild.
- Before any command likely to refresh local embeddings for many nodes, state the
  reason and get explicit confirmation from the user.

## Useful MCP flows

- `get_minimal_context_tool` for quick orientation
- `ensure_graph_tool` when `graph_health` is empty (safe bootstrap; no embeddings)
- `review_tool(mode="changes")` or `review_tool(mode="context")` for review work
- `query_graph_tool`, `semantic_search_nodes_tool`, and `flow_tool(mode="list")` for exploration
- `architecture_analysis_tool(mode="overview")` and its drill-down modes for
  evidence-backed architecture analysis
- `refactor_tool` for rename previews, dead-code analysis, and refactor suggestions
- `traverse_graph` and maintenance tools are available through explicit
  `--tools` allow-lists or `dagayn tool`, not the default MCP surface

## How to judge dagayn analysis

- Hub scores are degree-based; bridge scores are betweenness-based.
- Knowledge-gap hotspots are based on repository-relative degree thresholds and
  explicit test/documentation exclusions.
- Architecture analysis modes should expose their metric formulas or reason
  codes; use them as review leads, not automatic edit approval.
- Refactor suggestions must be verified against public APIs, dynamic dispatch,
  generated code, test artifacts, and framework entry points before applying.

## Documentation rule

If you update features, command names, integrations, or supported languages, update the fork's docs in the same change.

## Subagent delegation rules

<!-- derived-from ~/.pi/agent/AGENTS.md -->

Subagent delegation rules are maintained in the global `~/.pi/agent/AGENTS.md`. See that file for the current agent roster, models, use-when rules, and tool constraints.

## Cursor Cloud specific instructions

This repo ships two products: the primary `dagayn` CLI/MCP server (Python package
with a PyO3 Rust core, `dagayn._core`) and the secondary `dagayn-vscode` editor
extension (Node/pnpm). The Rust toolchain (1.92), Node 22, and pnpm are preinstalled;
`uv` is installed to `~/.local/bin` (already on the default login-shell `PATH`).

Standard lint/test/build/run commands are already documented — see `CONTRIBUTING.md`
(verification commands + VS Code extension), `README.md`, and `docs/USAGE.md`. The
notes below only capture the non-obvious, environment-specific gotchas.

- Setup is `uv sync --extra dev` (the update script runs this). It builds the PyO3
  extension and, as a side effect of `crates/dagayn-grammars/build.rs`, vendors the
  pinned tree-sitter grammars into the gitignored `dagayn/_vendor_grammars/`. Because
  of this, no separate `python -m dagayn.vendor_grammars` prefetch is needed after a
  successful `uv sync`. The first build fetches grammars over the network.
- Python is pinned to 3.14 via `.python-version`; `uv` downloads/manages it. Always
  invoke Python tooling through `uv run ...` so the built `dagayn._core` extension and
  the correct interpreter are used.
- Running the full `pytest` suite: the environment's global git config enables SSH
  commit signing (`commit.gpgsign=true` via a Cursor helper). Under full-suite load the
  git-backed tests (`tests/test_worktree.py`, `tests/test_integration_git.py`) can hit
  10s `git commit` timeouts. Run the suite with signing disabled per-invocation (no
  config files touched):
  `GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=commit.gpgsign GIT_CONFIG_VALUE_0=false uv run pytest --tb=short -q`.
  Tests pass individually without this; it only matters for the concurrent full run.
- Rust workspace checks (`cargo test --workspace`, `cargo clippy --workspace -- -D warnings`):
  the pure-Rust crates build as-is, but the `dagayn-py` pyo3 crate fails to link
  (`unable to find library -lpython3.12`) unless pointed at a Python with a shared
  `libpython`. Export `PYO3_PYTHON="$(uv run python -c 'import sys; print(sys.executable)')"`
  first (uv's CPython 3.14 ships `libpython3.14.so`). Not needed for the Python-side
  `uv sync`/pytest flow, which uses maturin.
- `dagayn-vscode` `pnpm test` runs plain mocha via ts-node (`.mocharc.json`), not the
  Electron/`@vscode/test-electron` runner, so it needs no display. Install its deps with
  `pnpm -C dagayn-vscode install --frozen-lockfile` (not part of the update script).
