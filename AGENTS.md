# dagayn agent guide

<!-- constrained-by ./docs/USAGE.md -->
<!-- constrained-by ./docs/COMMANDS.md -->

This repository ships `dagayn`, a fork of `code-review-graph` with extra emphasis on Terraform, Markdown, and mixed-language monorepos.

## How agents should work with this repo

- use `dagayn` in all user-facing commands
- build or update the graph before asking graph-backed questions
- start broad tasks with `get_minimal_context` so the agent sees graph freshness,
  risk, and suggested next tools before spending tokens elsewhere
- treat graph paths as repo-root-relative where the fork expects registered file trees
- use targeted graph tools before reading broad file sets
- treat graph analysis as evidence-ranked leads: cite thresholds, counts,
  reason codes, and truncation state when drawing conclusions
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

## Useful MCP flows

- `get_minimal_context` for quick orientation
- `review_tool(mode="changes")` or `review_tool(mode="context")` for review work
- `query_graph`, `semantic_search_nodes_tool`, and `flow_tool(mode="list")` for exploration
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
