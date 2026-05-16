---
name: Refactor Safely
description: Plan and execute safe refactoring using dependency analysis
---

## Refactor Safely

Use the knowledge graph to plan and execute refactoring with confidence.

### Steps

1. Run `get_minimal_context(task="<refactor goal>")` to check graph freshness,
   risk, and suggested next tools.
2. Use `refactor_tool` with mode="suggest" for evidence-ranked remove, move,
   split, and document candidates.
3. Use `refactor_tool` with mode="dead_code" only when the suggested remove
   candidates need a deeper dead-code drill-down.
4. For renames, use `refactor_tool` with mode="rename" to preview all affected locations.
5. Use `apply_refactor_tool` with `dry_run=True` first, then apply with the
   refactor_id only after the diff is acceptable.
6. After changes, run `review_tool(mode="changes")` and inspect `analysis_summary` to verify
   impact, recommended tests, affected flows, and architecture risks.

### Safety Checks

- Always preview before applying (rename mode gives you an edit list).
- Use `review_tool(mode="changes").analysis_summary` first; call
  `review_tool(mode="impact")` or `review_tool(mode="affected_flows")` only
  when a wider drill-down is needed.
- Run `find_large_functions` to identify decomposition targets.
- Treat suggestions as leads, not approval. Verify public APIs, dynamic
  dispatch, generated code, test artifacts, and framework entry points before
  removing or moving code.
- Prefer suggestions with explicit counts, thresholds, callers, communities, and
  reason codes; narrow truncated output with `top_n` or follow-up graph queries.

## CLI Fallback

Use MCP tools first. If the current MCP server profile does not expose a
refactor-only tool such as `apply_refactor_tool` or `find_large_functions_tool`,
run the same implementation through the CLI without restarting the agent:

```bash
dagayn tool refactor_tool --arg mode='"suggest"' --arg limit=10
dagayn tool refactor_tool --arg mode='"rename"' --arg old_name='"old_symbol"' --arg new_name='"new_symbol"'
dagayn tool apply_refactor_tool --arg refactor_id='"refactor_123"' --arg dry_run=true
```

## Token Efficiency Rules
- ALWAYS start with `get_minimal_context(task="<your task>")` before any other graph tool.
- Use `detail_level="minimal"` on all calls. Only escalate to "standard" when minimal is insufficient.
- Target: complete any review/debug/refactor task in ≤5 tool calls and ≤800 total output tokens.
