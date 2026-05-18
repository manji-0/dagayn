# Commands and surfaces

## CLI commands

<!-- constrained-by ./ARCHITECTURE.md -->

### Core graph lifecycle

- `dagayn build`
- `dagayn update`
- `dagayn postprocess`
- `dagayn watch`
- `dagayn status`

Use `dagayn build --force-full-build` when you need a clean graph rebuild from
scratch. It removes the existing `graph.db` plus SQLite sidecar files before
running the normal full parse. `--force` is accepted as a shorter alias.

### Local embedding refresh

<!-- derived-from ./LOCAL-EMBEDDINGS.md -->

`dagayn build` and `dagayn update` can also generate local Qwen embeddings
after the graph refresh:

```bash
dagayn build --local-embedding low
dagayn update --local-embedding low
```

Use `--local-embedding none` to keep the default graph-only behavior. The
server startup timeout and each embedding request timeout are separate knobs:
`--local-embedding-timeout` controls readiness, while
`--local-embedding-request-timeout` controls a single `/v1/embeddings` call.
Local embedding requests use `--local-embedding-batch-size 1` by default,
regardless of any ambient `CRG_OPENAI_BATCH_SIZE`.

### Analysis and review

- `dagayn detect-changes`
- `dagayn tool`
- `dagayn visualize`
- `dagayn wiki`
- `dagayn eval`

`dagayn tool <mcp-tool-name>` invokes the same underlying implementation as an
MCP tool and prints JSON. This gives agents and scripts a CLI path to run a
single tool directly, including when a running MCP server was started with a
narrow `--tools` allow-list:

```bash
dagayn tool review_tool --arg mode='"impact"' --arg 'changed_files=["src/app.py"]' --arg max_depth=3
dagayn tool flow_tool --arg mode='"list"' --arg detail_level='"minimal"'
dagayn tool architecture_analysis_tool --arg mode='"knowledge_gaps"' --arg top_n=10 --format summary
```

`dagayn visualize` is the main report/export command. It generates:

- interactive HTML output in `auto`, `full`, `community`, or `file` mode
- GraphML, Mermaid C4, SVG, Neo4j Cypher, or Obsidian exports via `--format`

Graphviz / DOT is not a built-in export target. Jupyter / Databricks notebooks are supported as graph inputs rather than report output formats.

`dagayn wiki` writes Markdown pages under `.dagayn/wiki/` from detected graph
communities. Each community page includes members, execution flows,
cross-community dependencies, and package-level ADP/SDP/SAP architecture
metrics filtered to the scopes represented by that community.

### Integration and serving

- `dagayn install`
- `dagayn init`
- `dagayn serve`

`dagayn install --platform codex` configures the Codex MCP server, installs
Codex skills, and writes global Codex hooks in `~/.codex/hooks.json` with the
required `~/.codex/config.toml` feature flag. Claude hooks are written to
`~/.claude/settings.json`. Git hooks installed by `dagayn install` refresh
cheaply with `dagayn update --skip-flows` before commit-time checks and run a
full `dagayn update` after a commit. `--no-hooks` skips the hook files.

### Multi-repo management

- `dagayn register`
- `dagayn unregister`
- `dagayn repos`
- `dagayn daemon ...`

## MCP tools

<!-- constrained-by ./ARCHITECTURE.md#query-surfaces -->
<!-- derived-from ./refactor-tool-suggest-spec.md -->
<!-- derived-from ./plans/ANALYSIS-TOOL-STRATEGY.md#tool-tiers -->

The MCP server exposes tools for:

- graph build and post-processing
- minimal context retrieval
- impact radius and review context
- graph queries and traversal
- embeddings and semantic search
- flows and communities
- architectural hotspot analysis
- change detection
- refactor previews and apply flows
- wiki generation and wiki page lookup
- multi-repo registry and cross-repo search

When the server is launched with `dagayn serve --local-embedding low`,
search-oriented MCP tools default to the managed OpenAI-compatible local
embedding endpoint. This makes `semantic_search_nodes`, `traverse_graph`, and
`cross_repo_search` run hybrid FTS + embedding retrieval unless the client
explicitly passes another provider or model.

Representative tool names include:

- `build_or_update_graph`
- `get_minimal_context`
- `review_tool`
- `query_graph`
- `flow_tool`
- `architecture_analysis`
- `refactor_tool`
- `generate_wiki`
- `cross_repo_search`

`query_graph` includes documentation-aware bridge patterns in addition to
ordinary code relationships. Use `docs_for` to find specifications, runbooks,
issue notes, and explanations linked to a code, Terraform, or artifact node.
Use `implementations_of` to find code or Terraform nodes linked to a Markdown
contract section through `implemented_by` / `implements_contract`
`CROSS_ARTIFACT` edges.

### MCP tool surface

<!-- derived-from ./plans/ANALYSIS-TOOL-STRATEGY.md#mcp-tool-surface-plan -->

`dagayn serve` exposes every public MCP main tool by default. Dagayn v3 removed
named tool profiles; specialized analysis now lives behind dispatcher tools
such as `review_tool`, `flow_tool`, and `architecture_analysis_tool`.

```bash
dagayn serve
dagayn serve --tools query_graph_tool,semantic_search_nodes_tool
dagayn tool architecture_analysis_tool --arg mode='"overview"'
dagayn serve --local-embedding low
dagayn serve --remote-embedding openai
```

`--tools` is an exact comma-separated allow-list for deployments that need to
hide some public tools. The same exact allow-list can be supplied with
`CRG_TOOLS`. Tool filtering is applied when `dagayn serve` starts; a running MCP
server does not reload a broader allow-list dynamically. Use
`dagayn tool <tool-name>` for ad-hoc shell access without restarting the
agent's MCP server.

When `dagayn serve --local-embedding low` starts a managed local
embedding sidecar, MCP `semantic_search_nodes_tool` automatically searches with
the matching OpenAI-compatible provider and Qwen model. When
`--remote-embedding {openai,google,minimax}` is set, MCP search automatically
uses that remote provider unless the client explicitly passes a different
`provider`. If no `--remote-embedding` flag is supplied, `serve` infers a remote
default only when exactly one provider's required environment variables are
configured.

`review_tool(mode="changes")` is the primary change-analysis surface. Standard output
includes `analysis_summary` with risk level, reason codes, recommended tests,
affected-flow rankings, documentation update candidates, hotspot proximity, and
architecture risks in changed scopes.

`get_minimal_context_tool` routes common English and Japanese task descriptions
for review, debugging, exploration, feature addition, and refactoring to the
next small set of MCP tools. It also returns `workflow`,
`recommended_action`, `why`, and `confidence` so clients can show the next
step without requiring users to know tool names.

`architecture_analysis_tool` is the primary architecture-analysis surface. Start
with `mode="overview"` and `detail_level="minimal"`. Output includes
`architecture_health`, which composes community coupling, hubs, bridges,
knowledge gaps, surprising connections, and ADP/SDP/SAP signals into a bounded
health summary with drill-down mode hints.

Migration note for dagayn 3.0: v2 split architecture MCP/CLI tools such as
`get_architecture_overview_tool`, `list_communities_tool`,
`get_hub_nodes_tool`, `compute_sdp_metrics_tool`, and
`detect_sap_violations_tool` were removed from the public surface. Use
`architecture_analysis_tool(mode=...)` instead.

Review and execution-flow drill-downs are also dispatcher-based in v3. Use
`review_tool(mode="changes"|"context"|"affected_flows"|"impact")` and
`flow_tool(mode="list"|"get")` instead of the v2 split MCP/CLI tools.

`refactor_tool(mode="suggest")` returns graph-backed remove, move, split, and
document candidates. Treat them as evidence-ranked leads; verify public APIs,
test artifacts, dynamic dispatch, and generated entry points before changing
source. Suggestions include `execution_plan` with minimum safe steps, required
tests, rollback guidance, and defer conditions.

`architecture_analysis_tool(mode="knowledge_gaps", top_n=20)` returns bounded
structural weakness categories with explicit thresholds and raw counts.
Untested-hotspot candidates are ranked against the repository's observed
production-node degree distribution rather than a fixed language-specific size
rule.

## MCP prompts

<!-- constrained-by ./ARCHITECTURE.md#pipeline-overview -->

The fork ships prompt surfaces for:

- review changes
- architecture mapping
- issue debugging
- onboarding
- pre-merge review
