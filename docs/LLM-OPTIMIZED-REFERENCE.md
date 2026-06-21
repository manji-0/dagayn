# dagayn LLM reference

<!-- derived-from ./COMMANDS.md -->

<section name="usage">
Install with `pip install dagayn` or `uv tool install dagayn`, then run `dagayn install` and `dagayn build`.

Use `dagayn update` for change-driven refreshes and `dagayn watch` for live development.

`dagayn serve` exposes the compact workflow MCP surface by default. Use an exact `--tools` or `CRG_TOOLS` allow-list for a different surface; `all`, `full`, or `*` exposes every advanced/maintenance tool.

Use `dagayn` in all user-facing guidance.
</section>

<section name="review-delta">
Recommended sequence for reviewing a delta:

1. ensure the graph is up to date
2. call `review_tool(mode="changes")` or `review_tool(mode="context")`
3. inspect affected nodes, flows, and tests
4. read only the files that remain ambiguous after graph queries

`review_tool(mode="changes")` returns an `analysis_summary` with risk reasons, recommended
tests, affected-flow rankings, documentation update candidates, hotspot
proximity, and architecture risks in changed scopes.

The fork is designed to work well when docs, app code, and Terraform all change together.
</section>

<section name="review-pr">
For larger reviews, start with `get_minimal_context`, then use `review_tool`, `flow_tool`, and `architecture_analysis_tool(mode="communities")` as needed.

If the PR touches infrastructure, assume Terraform nodes and references are part of the review surface.
</section>

<section name="commands">
Important CLI commands:

- `dagayn install`
- `dagayn build`
- `dagayn update`
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
</section>

<section name="languages">
The fork supports mainstream app languages plus Markdown, notebooks, and Terraform.

Jupyter and Databricks notebooks are parsed as graph inputs rather than report output formats.

Terraform and Markdown are notable differentiators for this fork's review workflows.
</section>

<section name="troubleshooting">
If results look stale, rebuild or update the graph.

If integrations are missing, re-run `dagayn install --dry-run` first.

If local type checks disagree with CI, use the repository's `ty` command line with the documented excludes.
</section>
