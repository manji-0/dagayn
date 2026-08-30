# dagayn LLM reference

<!-- derived-from ./COMMANDS.md -->

<section name="usage">
Install with `pip install dagayn` or `uv tool install dagayn`, then run `dagayn install`.

First graph: `ensure_graph_tool()` on the default MCP surface, or `dagayn build` from the CLI.
Routine refresh: hooks, `dagayn update`, `dagayn watch`, or `ensure_graph_tool(force=True)` when stale.

Linked worktrees: `dagayn worktree sync` (or the `worktree-sync` skill) inherits the main checkout graph/MCP config before analysis.

Feature work: find extension points with search/query/flow, implement, then `review_tool(mode="changes")` (see the `implement-feature` skill). After code changes, follow documentation update candidates via `docs_for` / the review-changes docs-update flow.

`dagayn serve` exposes the compact workflow MCP surface by default (`get_minimal_context_tool`, `ensure_graph_tool`, `review_tool`, `flow_tool`, `architecture_analysis_tool`, `refactor_tool`, `query_graph_tool`, `semantic_search_nodes_tool`, `get_docs_section_tool`). Use an exact `--tools` or `CRG_TOOLS` allow-list for a different surface; `all`, `full`, or `*` exposes every advanced/maintenance tool.

Use `dagayn` in all user-facing guidance.
</section>

<section name="review-delta">
Recommended sequence for reviewing a delta:

1. `get_minimal_context_tool(task=...)` — enqueues background prepare when empty or out of sync (`sync.status`) and returns immediately; call `ensure_graph_tool()` if you must wait for the graph to be ready
2. `review_tool(mode="changes")` and read `analysis_summary` first
3. Call `review_tool(mode="context")` / `mode="impact"` / `query_graph_tool` only for concrete source, blast-radius, or coverage questions
4. Read only the files that remain ambiguous after graph queries

`review_tool(mode="changes")` returns an `analysis_summary` with risk reasons, recommended
tests, affected-flow rankings, documentation update candidates, hotspot
proximity, and architecture risks in changed scopes.

The fork is designed to work well when docs, app code, and Terraform all change together.
</section>

<section name="review-pr">
Recommended sequence for reviewing a PR or branch:

1. `get_minimal_context_tool(task="PR review")`
2. Refresh only when empty/stale: `ensure_graph_tool()` or `ensure_graph_tool(force=True)`; use `build_or_update_graph_tool(base="main")` only on the advanced surface when an explicit base ref is required
3. `review_tool(mode="changes", base="main")` and read `analysis_summary` first
4. Prefer `review_tool(mode="context")` snippets over full-file reads; use `mode="impact"` and `query_graph_tool` only for high-risk follow-ups

If the PR touches infrastructure, assume Terraform nodes and references are part of the review surface.
</section>

<section name="commands">
Important CLI commands:

- `dagayn install`
- `dagayn build`
- `dagayn update`
- `dagayn session prepare`
- `dagayn watch`
- `dagayn status`
- `dagayn detect-changes`
- `dagayn tool`
- `dagayn visualize`
- `dagayn serve`
- `dagayn detect-adp` / `dagayn sdp-metrics` / `dagayn detect-sdp`
- `dagayn sap-metrics` / `dagayn detect-sap`
- `dagayn profile`
- `dagayn register` / `dagayn repos` / `dagayn daemon`

`dagayn serve` exposes the compact workflow MCP surface by default. Use `--tools` when a deployment needs an exact allow-list; `--tools all` exposes every advanced/maintenance tool.

Tool filtering is fixed at MCP server startup. For ad-hoc CLI access, use `dagayn tool <mcp-tool-name>` with
`--arg KEY=VALUE` or `--json-args '{...}'` to invoke the same implementation
from the CLI.

`dagayn install --platform codex` also writes `~/.codex/hooks.json` and enables
Codex hooks in `~/.codex/config.toml`, unless `--no-hooks` is used. Claude hooks
are written to `~/.claude/settings.json`.

`architecture_analysis_tool(mode="overview")` returns `architecture_health`, a
bounded composed summary of coupling, hubs, bridges, knowledge gaps, surprising
connections, and ADP/SDP/SAP signals. v2 split architecture tools were removed;
use `architecture_analysis_tool(mode=...)` for drill-downs.

`dagayn visualize` is the static graph export surface. It requires `--format` and supports `graphml`, `mermaid-c4`, `svg`, `cypher`, and `obsidian`.
</section>

<section name="legal">
`dagayn` is an MIT-licensed fork of `code-review-graph`.

The graph database is local by default. Optional embedding providers may call remote services only when explicitly configured.
</section>

<section name="watch">
Use `dagayn watch` when you want continuous graph refresh during active development.

Use `dagayn update` when you want a one-shot incremental refresh tied to a change set.
</section>

<section name="embeddings">
<!-- derived-from ./LOCAL-EMBEDDINGS.md -->
Embeddings are optional.

Embeddings are additive: with embeddings built, `semantic_search_nodes` merges BM25 and cosine via RRF and returns `search_mode: "hybrid"`; without them it falls back to FTS5-only (`"fts_only"`). The per-result `source` field (`"fts"`, `"embedding"`, `"both"`, `"keyword"`) shows which arm produced each hit. If provider imports are unavailable, keyword-based graph search still works.

For local embeddings during graph refresh, use `dagayn build --local-embedding`
or `dagayn update --local-embedding`. A bare `--local-embedding` runs the
managed BGE-M3 llama.cpp GGUF sidecar with the measured `material` text mode.

For the managed local Qwen sidecar, use
`dagayn build --local-embedding --mode llama-qwen3` or the legacy
`dagayn build --local-embedding low`. dagayn reuses a compatible local
OpenAI-compatible embedding server on localhost or starts one as a subprocess
for the command; the managed preset starts llama.cpp GGUF via `llama-server`.

`ensure_graph_tool` inherits the active `dagayn serve --local-embedding` mode
when refreshing vectors. Direct CLI `dagayn session prepare` / `ensure_graph`
callers default to `none` unless `--local-embedding` is passed.
</section>

<section name="languages">
The fork supports mainstream app languages plus Markdown, notebooks, and Terraform.

Jupyter, Databricks, and marimo notebooks are parsed as graph inputs rather than report output formats.

Terraform and Markdown are notable differentiators for this fork's review workflows. Native FTS indexes Japanese with Lindera IPADIC morphemes and overlapping CJK bigrams.
</section>

<section name="troubleshooting">
If results look stale, call `ensure_graph_tool(force=True)` or run `dagayn update` / `dagayn build`.

If integrations are missing, re-run `dagayn install --dry-run` first.

If local type checks disagree with CI, use `uv run pyrefly check` (see `[tool.pyrefly]` in `pyproject.toml`).
</section>
