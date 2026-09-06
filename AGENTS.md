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
- after a concrete `qualified_name`, fetch the body with
  `query_graph_tool(pattern="source_of")` before opening the file
- fall back to `rg`/file reads when graph output is stale, ambiguous, truncated,
  or `source_of` cannot read the span
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
- `query_graph_tool(pattern="source_of")` after a search hit to fetch a live span
- `architecture_analysis_tool(mode="overview")` and its drill-down modes for
  evidence-backed architecture analysis
- `refactor_tool` for rename previews, dead-code analysis, and refactor suggestions
- apply a rename preview with `dagayn tool apply_refactor_tool` (advanced MCP
  surface: `dagayn serve --tools all`)
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
extension (Node/pnpm). The Rust toolchain (1.95), Node 22, and pnpm are preinstalled;
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

<!-- dagayn MCP tools -->
## MCP Tools: dagayn

**IMPORTANT: This project has a knowledge graph. ALWAYS use the
dagayn MCP tools BEFORE using Grep/Glob/Read to explore
the codebase.** The graph is faster, cheaper (fewer tokens), and gives
you structural context (callers, dependents, test coverage) that file
scanning cannot.

### When to use graph tools FIRST

- **Any new task**: `get_minimal_context_tool` for graph freshness, risk, and next-tool hints
- **Exploring code**: `semantic_search_nodes_tool` or `query_graph_tool` instead of Grep
- **Understanding impact**: `review_tool(mode="impact")` instead of manually tracing imports
- **Code review**: `review_tool(mode="changes")` first; use its `analysis_summary` before
  calling drill-down tools
- **Finding relationships**: `query_graph_tool` with
  callers_of/callees_of/imports_of/tests_for/source_of
- **Architecture questions**: `architecture_analysis_tool(mode="overview")`
  first; use `architecture_health` and the Architecture Analysis skill before
  choosing a drill-down mode

Fall back to Grep/Glob/Read **only** when the graph result is missing, stale,
ambiguous, truncated, or `source_of` cannot supply the span. Do not re-read a
whole file just to inspect a function the graph already located.

### Tool surface

`dagayn serve` exposes the compact workflow tool surface by default. Use
`dagayn serve --tools ...` when a deployment needs an exact allow-list; the same
allow-list can be supplied with `CRG_TOOLS`. Use `all`, `full`, or `*` to expose
advanced/maintenance tools.

### Default workflow tools

| Tool | Use when |
| ------ | ---------- |
| `get_minimal_context_tool` | Start here: graph freshness, risk, communities, next tools |
| `ensure_graph_tool` | Empty or missing graph; safe bootstrap without embeddings |
| `review_tool` | Primary change review and review drill-down dispatcher |
| `flow_tool` | Reachable-set flow lists and BFS membership (not call sequences) |
| `architecture_analysis_tool` | Primary architecture review and drill-down dispatcher |
| `refactor_tool` | Planning renames, finding dead code, and evidence-ranked refactor suggestions |
| `query_graph_tool` | Tracing callers, callees, imports, tests, live source spans |
| `semantic_search_nodes_tool` | Finding functions/classes by name or keyword |

### Drill-down tools

| Tool | Use when |
| ------ | ---------- |
| `review_tool(mode="impact")` | Need a wider or deeper blast-radius view |
| `review_tool(mode="affected_flows")` | Need full affected execution-path details |
| `architecture_analysis_tool(mode=...)` | Architecture drill-downs for boundaries and metrics |

### How to judge analysis output

- Treat graph insights as **evidence-ranked leads**, not automatic truth.
- Prefer outputs that expose metrics, thresholds, counts, reason codes, and
  `truncated`/`total` fields; mention those numbers when making recommendations.
- Check test coverage with `query_graph_tool` pattern="tests_for" before claiming a
  code path is untested.
- For refactors, verify public APIs, dynamic dispatch, generated code, test
  artifacts, and framework entry points before editing.
- If an output is truncated or approximate, narrow with `top_n`, `detail_level`,
  `max_depth`, or a targeted follow-up query before drawing conclusions.

### Workflow

1. Start with `get_minimal_context_tool(task=...)`.
2. Use the suggested next tool or a targeted query.
3. For reviews, use `review_tool(mode="changes")` and read `analysis_summary`
   first. Call `review_tool(mode="context")`, `review_tool(mode="affected_flows")`,
   `review_tool(mode="impact")`, or `query_graph_tool` only when the summary points there.
4. For architecture work, use
   `architecture_analysis_tool(mode="overview", detail_level="minimal")`
   and read `architecture_health` first. Use the Architecture Analysis skill to
   choose drill-down modes only when the health summary identifies a concrete risk.
5. For refactors, use `refactor_tool(mode="suggest")` first, then preview
   renames with `refactor_tool(mode="rename")`. Apply with
   `dagayn tool apply_refactor_tool` (advanced MCP surface: `dagayn serve --tools all`).

<!-- dagayn markdown policy -->
## Markdown documentation policy: declare dependencies via directive comments

When authoring or editing a Markdown document in this repository, declare
inter-section and inter-document dependencies as HTML directive comments so
they are captured by the dagayn graph (`DEPENDS_ON` / `IMPORTS_FROM` edges)
and discoverable via `query_graph_tool` / `review_tool(mode="impact")`.

### Required form

```markdown
<!-- <kind> <target> -->
```

`<kind>` MUST be one of: `constrained-by`, `blocked-by`, `supersedes`,
`derived-from`. Choose the kind whose semantics best match the dependency:

| Kind | Use when |
| ---- | -------- |
| `constrained-by` | This section's design is bounded by the referenced document/section |
| `blocked-by` | This item cannot proceed until the referenced item resolves |
| `supersedes` | This document replaces the referenced content |
| `derived-from` | This section is derived from the referenced source |

### Three target shapes

| Dependency type | Target syntax | Example |
| --------------- | ------------- | ------- |
| Within-document section | `#section-slug` | `<!-- derived-from #background -->` |
| Other document (whole file) | `./relative/path.md` | `<!-- blocked-by ./specs/open-issue.md -->` |
| Other document + section | `./path.md#slug` | `<!-- constrained-by ./adr.md#context -->` |

Slugs follow GitHub Markdown rules: lowercase, non-alphanumerics removed,
spaces and hyphens collapsed to `-`. Place the directive immediately under
the heading whose content depends on the target. External URLs
(`http://`, `https://`) are not graph-resolvable — keep them as ordinary
Markdown links, not directive targets.

### When to add a directive

- Section design references an ADR, spec, or research note → `constrained-by` or `derived-from`.
- A document replaces an older one → `supersedes` (place in the new document).
- A spec/task section is blocked on another being resolved → `blocked-by`.
- A later section extends an earlier one non-obviously → `derived-from #earlier-section`.

If no real dependency exists, do not invent one. Directives are signal, not decoration.
