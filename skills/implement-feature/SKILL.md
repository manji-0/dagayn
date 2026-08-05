---
name: implement-feature
description: Add behavior using the knowledge graph — find extension points, implement with minimal blast radius, then verify with change review and docs bridges.
argument-hint: "[feature goal]"
---

# Implement Feature

Use this when adding behavior, endpoints, commands, or UI flows. Goal: find the
right extension points before editing, then prove impact with graph review.

<!-- dagayn skill embedding context -->
## Installed Search Mode

This packaged skill is mode-neutral. `dagayn install` rewrites this section with
the selected embedding mode so related-code search matches the installed
retrieval setup.
<!-- /dagayn skill embedding context -->

## Steps

1. **Orient** with `get_minimal_context_tool(task="<feature goal>")`.
   If `graph_health.status` is `empty` (or `ensure_graph_tool` is the first
   next-tool hint), call `ensure_graph_tool()` and re-orient.

2. **Find extension points** (pick one path, do not run all):
   - Fuzzy / product language → `semantic_search_nodes_tool(query="<concept>", detail_level="minimal")`
   - Known symbol / file → `query_graph_tool` with `children_of`, `callers_of`,
     `callees_of`, or `file_summary`
   - Existing user journey → `flow_tool(mode="list")`, then `flow_tool(mode="get")`
     for one concrete flow
   - Spec-driven work → `query_graph_tool(pattern="implementations_of", target="<doc.md>::<section>")`
     or `pattern="docs_for"` from a nearby code symbol

3. **Read only the chosen extension surface** — prefer
   `review_tool(mode="context")` / targeted file reads over broad tree walks.

4. **Implement the smallest change** that hooks into the existing pattern
   (same module, same flow entry, same interface style). Preserve any nearby
   `dagayn:` documentation directives; update targets if you rename symbols.

5. **Refresh the graph for the edit**:
   - Default MCP: `ensure_graph_tool(force=True)`
   - Or rely on hooks / `dagayn update` if they already ran

6. **Verify**:
   - `review_tool(mode="changes")` — read `analysis_summary` for risk, tests,
     affected flows, and documentation update candidates
   - `query_graph_tool(pattern="tests_for", target="<new-or-changed-symbol>")`
     when coverage is unclear
   - If `analysis_summary` lists documentation candidates, follow the
     **Docs update after code change** section in `review-changes` (or call
     `query_graph_tool(pattern="docs_for", ...)` and edit those docs next)

7. **Stop when**:
   - The new behavior is reachable from an existing flow or a deliberate new
     entry point
   - High-risk blast radius is understood
   - Linked specs/runbooks are updated or explicitly deferred

## Efficiency Rules

- ALWAYS start with `get_minimal_context_tool(task="<feature goal>")`.
- If the graph was empty, count tool calls **after** `ensure_graph_tool` returns.
- Cap discovery to ≤3 search/relationship calls before editing.
- After edits, prefer one `review_tool(mode="changes")` over repeated impact
  drills unless `analysis_summary` points to a concrete risk.
- Use `detail_level="minimal"` unless minimal omits the field you need.

## CLI Fallback

Default MCP already exposes the tools above. Use `dagayn tool` when the server
allow-list omitted them:

```bash
dagayn tool get_minimal_context_tool --arg 'task="add billing webhook"'
dagayn tool ensure_graph_tool
dagayn tool semantic_search_nodes_tool --arg query='"webhook handler"' --arg detail_level='"minimal"'
dagayn tool review_tool --arg mode='"changes"' --arg detail_level='"minimal"'
dagayn tool query_graph_tool --arg pattern='"docs_for"' --arg target='"src/billing.py::handle_webhook"'
```
