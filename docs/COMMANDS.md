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
- `refactor_tool`
- `generate_wiki`
- `cross_repo_search`

## MCP prompts

<!-- constrained-by ./ARCHITECTURE.md#pipeline-overview -->

The fork ships prompt surfaces for:

- review changes
- architecture mapping
- issue debugging
- onboarding
- pre-merge review
