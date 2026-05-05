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
6. After changes, run `detect_changes` and inspect `analysis_summary` to verify
   impact, recommended tests, affected flows, and architecture risks.

### Safety Checks

- Always preview before applying (rename mode gives you an edit list).
- Use `detect_changes.analysis_summary` first; call `get_impact_radius` or
  `get_affected_flows` only when a wider drill-down is needed.
- Run `find_large_functions` to identify decomposition targets.
- Treat suggestions as leads, not approval. Verify public APIs, dynamic
  dispatch, generated code, test artifacts, and framework entry points before
  removing or moving code.
- Prefer suggestions with explicit counts, thresholds, callers, communities, and
  reason codes; narrow truncated output with `top_n` or follow-up graph queries.

## Token Efficiency Rules
- ALWAYS start with `get_minimal_context(task="<your task>")` before any other graph tool.
- Use `detail_level="minimal"` on all calls. Only escalate to "standard" when minimal is insufficient.
- Target: complete any review/debug/refactor task in ≤5 tool calls and ≤800 total output tokens.
