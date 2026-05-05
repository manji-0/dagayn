# Commands and surfaces

## CLI commands

<!-- constrained-by ./ARCHITECTURE.md -->

### Core graph lifecycle

- `dagayn build`
- `dagayn update`
- `dagayn postprocess`
- `dagayn watch`
- `dagayn status`

### Analysis and review

- `dagayn detect-changes`
- `dagayn visualize`
- `dagayn wiki`
- `dagayn eval`

`dagayn visualize` is the main report/export command. It generates:

- interactive HTML output in `auto`, `full`, `community`, or `file` mode
- GraphML, Mermaid C4, SVG, Neo4j Cypher, or Obsidian exports via `--format`

Graphviz / DOT is not a built-in export target. Jupyter / Databricks notebooks are supported as graph inputs rather than report output formats.

### Integration and serving

- `dagayn install`
- `dagayn init`
- `dagayn serve`

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

Representative tool names include:

- `build_or_update_graph`
- `get_minimal_context`
- `get_impact_radius`
- `query_graph`
- `detect_changes`
- `list_flows`
- `list_communities`
- `get_knowledge_gaps`
- `refactor_tool`
- `generate_wiki`
- `cross_repo_search`

### Tool profiles

<!-- derived-from ./plans/ANALYSIS-TOOL-STRATEGY.md#tool-profile-plan -->

`dagayn serve` exposes the `default` profile unless an exact allow-list or
another profile is selected. Profiles keep the ordinary MCP surface small while
leaving specialized tools available for workflows that need them.

| Profile | Use when |
| --- | --- |
| `default` | General agent use with a small first-choice tool list |
| `review` | PR and local diff review |
| `architecture` | Architecture mapping and cleanup |
| `refactor` | Refactor planning and safe apply flows |
| `full` | Legacy all-tools behavior |

```bash
dagayn serve --tool-profile review
dagayn serve --tool-profile full
dagayn serve --tools query_graph_tool,semantic_search_nodes_tool
```

`--tools` is an exact comma-separated allow-list and overrides profiles. The
same exact allow-list can be supplied with `CRG_TOOLS`. Named profiles can be
supplied with `DAGAYN_TOOL_PROFILE` or `CRG_TOOL_PROFILE`.

`detect_changes_tool` is the primary change-analysis surface. Standard output
includes `analysis_summary` with risk level, reason codes, recommended tests,
affected-flow rankings, documentation update candidates, hotspot proximity, and
architecture risks in changed scopes.

`get_architecture_overview_tool` is the primary architecture-analysis surface.
Output includes `architecture_health`, which composes community coupling, hubs,
bridges, knowledge gaps, surprising connections, and ADP/SDP/SAP signals into a
bounded health summary with drill-down tool names.

`refactor_tool(mode="suggest")` returns graph-backed remove, move, split, and
document candidates. Treat them as evidence-ranked leads; verify public APIs,
test artifacts, dynamic dispatch, and generated entry points before changing
source.

`get_knowledge_gaps(top_n=20)` returns bounded structural weakness categories
with explicit thresholds and raw counts. Untested-hotspot candidates are ranked
against the repository's observed production-node degree distribution rather
than a fixed language-specific size rule.

## MCP prompts

<!-- constrained-by ./ARCHITECTURE.md#pipeline-overview -->

The fork ships prompt surfaces for:

- review changes
- architecture mapping
- issue debugging
- onboarding
- pre-merge review
