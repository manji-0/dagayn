---
name: Explore Codebase
description: Navigate and understand codebase structure using the knowledge graph
---

## Explore Codebase

Use the dagayn MCP tools to explore and understand the codebase.

<!-- dagayn skill embedding context -->
## Installed Search Mode

This packaged skill is mode-neutral. `dagayn install` rewrites this section with
the selected embedding mode so exploration chooses the right search strategy.
<!-- /dagayn skill embedding context -->

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
   to trace relationships.
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
- For tiny literal lookups, one `rg` is fine after minimal context; switch back
  to graph tools once you have a file, function, or class name.
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
```

## Token Efficiency Rules
- ALWAYS start with `get_minimal_context_tool(task="<your task>")` before any other graph tool.
- Use `detail_level="minimal"` on all calls. Only escalate to "standard" when minimal is insufficient.
- Target: complete any review/debug/refactor task in ≤5 tool calls and ≤800 total output tokens.
