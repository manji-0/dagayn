---
name: explore-codebase
description: Navigate and understand codebase structure using the knowledge graph
---

## Explore Codebase

Use the dagayn MCP tools to explore and understand the codebase.

<!-- derived-from ../../docs/plans/ANALYSIS-TOOL-STRATEGY.md#exploration-analysis -->

<!-- dagayn skill embedding context -->
## Installed Search Mode

This packaged skill is mode-neutral. `dagayn install` rewrites this section with
the selected embedding mode so exploration chooses the right search strategy.
<!-- /dagayn skill embedding context -->

## Decision Model

- Unknown entity, fuzzy concept, or process-language query: use
  `semantic_search_nodes_tool` first, then pick a concrete `qualified_name`.
- Known entity plus a specific relationship: use `query_graph_tool` with the
  narrowest pattern (`callers_of`, `callees_of`, `imports_of`, `tests_for`,
  `docs_for`, `implementations_of`, `children_of`, or `file_summary`).
- Changed code, review risk, or blast radius: use `review_tool` before raw
  traversal.
- Architecture health or structural risk: use
  `architecture_analysis_tool(mode="overview")` before metric drill-downs.
- Reachable-set flow: use `flow_tool(mode="list")`, then `flow_tool(mode="get")`
  only after choosing a concrete flow. Treat `path` / `steps` as BFS visit
  order, not a runtime call sequence, and read `truncated` / `truncation_reason`.
- Neighborhood exploration: use `traverse_graph_tool` only after choosing a
  concrete start node, only when a specific relationship query would be too
  narrow, and only when the advanced MCP surface (or `dagayn tool`) exposes it.

### Steps

1. Run `get_minimal_context_tool(task="<what you need to understand>")` to see graph
   freshness, risk, major communities, and suggested next tools. If
   `graph_health.status` is `empty` (or `ensure_graph_tool` is the first
   next-tool hint), call `ensure_graph_tool()` and re-orient before exploration.
2. Pick **one** next move from the Decision Model — do not run the whole ladder:
   - Unknown / fuzzy / process language → `semantic_search_nodes_tool`, then a
     concrete `qualified_name`.
   - Known entity + relationship → `query_graph_tool` with the narrowest pattern.
   - Review risk / blast radius → `review_tool(mode="changes")` and read
     `analysis_summary` first.
   - Architecture / structural risk only →
     `architecture_analysis_tool(mode="overview", detail_level="minimal")`.
     Read `architecture_health` first; use the Architecture Analysis skill for
     drill-down mode selection.
   - Reachable-set flow → `flow_tool(mode="list", detail_level="minimal")`, then
     `flow_tool(mode="get")` only after choosing a concrete flow name.
3. After you have a concrete node, use `query_graph_tool` patterns such as
   `callers_of`, `callees_of`, `imports_of`, `docs_for`, or `implementations_of`
   to verify relationships. Prefer these over raw traversal.
4. Fall back to `rg`/file reads when graph output is stale, ambiguous, truncated,
   or missing exact source text.

### Tips

- Start from `get_minimal_context_tool`, then follow the Decision Model. Do not
  open with architecture overview unless the question is about structure or
  health.
- Use `children_of` on a file to see all its functions and classes.
- Use `find_large_functions_tool` (advanced surface / `dagayn tool`) to identify
  complex code.
- For Markdown ↔ code traceability, treat `dagayn:` directives as authored
  `CROSS_ARTIFACT` evidence. Markdown comments such as
  `<!-- dagayn: implemented-by path::symbol -->` point from a doc section to a
  code point; Python/Terraform comments such as
  `# dagayn: implements docs/spec.md#Section` point from code to a Markdown
  section. Query tools expose inverse labels, so do not assume both directions
  are stored. Read `evidence_type` (`authored`, `extracted`, or
  `heuristic_reachable`) and `missingness` before treating a traceability edge
  as contract evidence.
- For empty or not-found relationship queries, use `zero_result_reason` and
  `next_action` to decide the next lookup; absence is limited to the current
  graph.
- For tiny literal lookups, one `rg` is fine after minimal context; switch back
  to graph tools once you have a file, function, or class name.
- Do not use raw BFS/DFS as the first exploration move. Prefer search for
  discovery and `query_graph_tool` for relationship verification; raw traversal
  is a follow-up for a bounded neighborhood.
- Treat graph output as evidence: cite counts, thresholds, reason codes, and
  truncation flags when making architectural claims.

## CLI Fallback

Default MCP already exposes `flow_tool`, `architecture_analysis_tool`,
`query_graph_tool`, and `semantic_search_nodes_tool`. Use `dagayn tool` when the
server allow-list omitted a tool, or for advanced helpers such as
`find_large_functions_tool` / `traverse_graph_tool`:

```bash
dagayn tool find_large_functions_tool --arg min_lines=80
dagayn tool traverse_graph_tool --arg query='"auth handler"' --arg depth=2
dagayn tool query_graph_tool --arg pattern='"docs_for"' --arg target='"src/app.py::handler"'
dagayn tool query_graph_tool --arg pattern='"implementations_of"' --arg target='"docs/spec.md::contract-section"'
```

## Token Efficiency Rules
- ALWAYS start with `get_minimal_context_tool(task="<your task>")` before any other graph tool.
- If the graph was empty, count tool calls **after** `ensure_graph_tool` returns.
- Use `detail_level="minimal"` on all calls. Only escalate to "standard" when minimal is insufficient.
- Target: complete any explore task in ≤5 tool calls and ≤800 total output tokens
  after ensure.
