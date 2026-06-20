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
- Execution path: use `flow_tool(mode="list")`, then `flow_tool(mode="get")`
  only after choosing a concrete flow.
- Neighborhood exploration: use `traverse_graph_tool` only after choosing a
  concrete start node and only when a specific relationship query would be too
  narrow.

### Steps

1. Run `get_minimal_context_tool(task="<what you need to understand>")` to see graph
   freshness, risk, major communities, and suggested next tools.
2. Run `architecture_analysis_tool(mode="overview", detail_level="minimal")`
   for high-level architecture questions. Read `architecture_health` first; it
   summarizes coupling, hubs, bridges, knowledge gaps, surprising connections,
   and ADP/SDP/SAP signals. Use the Architecture Analysis skill for mode
   selection before drilling down.
3. Use `semantic_search_nodes_tool` to find specific functions or classes.
4. Use `query_graph_tool` with patterns like `callers_of`, `callees_of`, `imports_of`
   to trace relationships. Use `docs_for` when starting from a code/Terraform
   node and you need linked specs, runbooks, explanations, or issue notes. Use
   `implementations_of` when starting from a Markdown section and you need the
   code points linked by `implemented_by` / `implements_contract` documentation
   bridges.
5. Use `flow_tool(mode="list", detail_level="minimal")` to find candidate
   execution paths. Call `flow_tool(mode="get")` only after choosing a concrete
   flow name.
6. Fall back to `rg`/file reads when graph output is stale, ambiguous, truncated,
   or missing exact source text.

### Tips

- Start broad (minimal context, architecture health) then narrow down to
  specific areas.
- Use `children_of` on a file to see all its functions and classes.
- Use `find_large_functions_tool` to identify complex code.
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

Use MCP tools first. If the current MCP server profile does not expose a
drill-down tool such as `flow_tool`, `architecture_analysis_tool`, or
`find_large_functions_tool`, run the same implementation through the CLI without
restarting the agent:

```bash
dagayn tool flow_tool --arg mode='"list"' --arg detail_level='"minimal"'
dagayn tool architecture_analysis_tool --arg mode='"overview"' --arg detail_level='"minimal"'
dagayn tool architecture_analysis_tool --arg mode='"communities"'
dagayn tool find_large_functions_tool --arg min_lines=80
dagayn tool query_graph_tool --arg pattern='"docs_for"' --arg target='"src/app.py::handler"'
dagayn tool query_graph_tool --arg pattern='"implementations_of"' --arg target='"docs/spec.md::contract-section"'
```

## Token Efficiency Rules
- ALWAYS start with `get_minimal_context_tool(task="<your task>")` before any other graph tool.
- Use `detail_level="minimal"` on all calls. Only escalate to "standard" when minimal is insufficient.
- Target: complete any review/debug/refactor task in ≤5 tool calls and ≤800 total output tokens.
