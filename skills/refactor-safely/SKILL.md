---
name: Refactor Safely
description: Plan and execute safe refactoring using dependency analysis
---

## Refactor Safely

Use the knowledge graph to plan and execute refactoring with confidence.

### Steps

1. Run `get_minimal_context_tool(task="<refactor goal>")` to check graph freshness,
   risk, and suggested next tools.
2. Use `refactor_tool` with mode="suggest" for evidence-ranked remove, move,
   split, and document candidates.
3. Use `refactor_tool` with mode="dead_code" only when the suggested remove
   candidates need a deeper dead-code drill-down.
4. For renames, use `refactor_tool` with mode="rename" to preview all affected locations.
5. Use `apply_refactor_tool` with `dry_run=True` first, then apply with the
   refactor_id only after the diff is acceptable.
6. Before renaming, moving, or deleting public code, follow documentation bridge
   edges when present: `query_graph_tool(pattern="docs_for", target="<path::symbol>", detail_level="minimal")`
   for specs/runbooks/issue notes attached to code, and
   `query_graph_tool(pattern="implementations_of", target="<doc.md>::<section-slug>", detail_level="minimal")`
   when the refactor starts from a Markdown contract section.
7. After changes, run `review_tool(mode="changes")` and inspect `analysis_summary` to verify
   impact, recommended tests, affected flows, and architecture risks.

### Safety Checks

- Always preview before applying (rename mode gives you an edit list).
- Use `review_tool(mode="changes").analysis_summary` first; call
  `review_tool(mode="impact")` or `review_tool(mode="affected_flows")` only
  when a wider drill-down is needed.
- Run `find_large_functions_tool` to identify decomposition targets.
- Preserve authored `dagayn:` documentation directives. Update Markdown
  `implemented-by path::symbol` targets after code renames, and update code
  `implements docs/spec.md#Section` targets after doc heading/path changes.
  Do not add duplicate inverse directives unless there are two distinct claims.
- Treat suggestions as leads, not approval. Verify public APIs, dynamic
  dispatch, generated code, test artifacts, and framework entry points before
  removing or moving code.
- Prefer suggestions with explicit counts, thresholds, callers, communities, and
  reason codes; narrow truncated output with `top_n` or follow-up graph queries.

### Function Concern Separation Profiles

When a split suggestion contains `evidence.concern_separation`, treat it as a
role-aware refactoring profile, not a verdict that the function is bad.

- Read `role` first. Boundary functions, CLI handlers, adapters, coordinators,
  and test helpers may legitimately combine IO and orchestration.
- Read `score` against `split_score_threshold`. A score over the threshold means
  concern pressure can justify reviewing a large function for extraction; it
  does not prove that extraction is safe or required.
- Use `reason_codes` to identify the suspected pressure: callee community spread,
  callee scope spread, branch pressure, side-effect pressure, implicit context,
  or low context clarity.
- Use `evidence.purity_likelihood` only as side-effect evidence. It is not a
  proof of functional purity.
- Check `missingness` before acting. Low source, call, community, or unresolved
  call evidence lowers confidence and should trigger a source read or graph
  follow-up before recommending an edit.
- Use `action` as the first investigation step. Prefer extracting one cohesive
  decision, transformation, or context object before moving IO or changing public
  signatures.
- Do not turn this profile into a standalone review finding. Cite exact source
  behavior or a failing test when claiming a bug; cite the profile only as
  refactoring prioritization evidence.

## CLI Fallback

Use MCP tools first. If the current MCP server profile does not expose a
refactor-only tool such as `apply_refactor_tool` or `find_large_functions_tool`,
run the same implementation through the CLI without restarting the agent:

```bash
dagayn tool refactor_tool --arg mode='"suggest"' --arg limit=10
dagayn tool refactor_tool --arg mode='"rename"' --arg old_name='"old_symbol"' --arg new_name='"new_symbol"'
dagayn tool apply_refactor_tool --arg refactor_id='"refactor_123"' --arg dry_run=true
dagayn tool query_graph_tool --arg pattern='"docs_for"' --arg target='"src/app.py::handler"'
dagayn tool query_graph_tool --arg pattern='"implementations_of"' --arg target='"docs/spec.md::contract-section"'
```

## Token Efficiency Rules
- ALWAYS start with `get_minimal_context_tool(task="<your task>")` before any other graph tool.
- Use `detail_level="minimal"` on all calls. Only escalate to "standard" when minimal is insufficient.
- Target: complete any review/debug/refactor task in ≤5 tool calls and ≤800 total output tokens.
