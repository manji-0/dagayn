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
- `detect_changes` or `get_review_context` for review work
- `query_graph`, `traverse_graph`, `list_flows`, and `list_communities` for exploration
- `get_hub_nodes`, `get_bridge_nodes`, `get_knowledge_gaps`, and
  `get_surprising_connections` for evidence-backed architecture analysis
- `refactor_tool` for rename previews, dead-code analysis, and refactor suggestions

## How to judge dagayn analysis

- Hub scores are degree-based; bridge scores are betweenness-based.
- Knowledge-gap hotspots are based on repository-relative degree thresholds and
  explicit test/documentation exclusions.
- Architecture principle tools should expose their metric formulas or reason
  codes; use them as review leads, not automatic edit approval.
- Refactor suggestions must be verified against public APIs, dynamic dispatch,
  generated code, test artifacts, and framework entry points before applying.

## Documentation rule

If you update features, command names, integrations, or supported languages, update the fork's docs in the same change.
