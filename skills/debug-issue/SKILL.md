---
name: Debug Issue
description: Systematically debug issues using graph-powered code navigation
---

## Debug Issue

Use the knowledge graph to systematically trace and debug issues.

### Steps

1. Run `get_minimal_context(task="<bug or symptom>")` to check graph freshness,
   risk, and suggested next tools.
2. Use `semantic_search_nodes` to find code related to the issue.
3. Use `query_graph` with `callers_of` and `callees_of` to trace call chains.
4. Use `flow_tool(mode="get")` to see full execution paths through suspected areas.
5. Run `review_tool(mode="changes")` to check if recent changes caused the issue. Read
   `analysis_summary` for risk reasons, affected-flow rankings, hotspot
   proximity, and recommended tests.
6. Use `review_tool(mode="impact")` on suspected files only when `analysis_summary` or
   the call trace leaves the blast radius unclear.
7. Read source directly once graph evidence identifies the likely failing path.

### Tips

- Check both callers and callees to understand the full context.
- Look at affected flows to find the entry point that triggers the bug.
- Recent changes are the most common source of new issues.
- Do not infer root cause from graph centrality alone; require an observed
  failing path, changed behavior, or source-level defect.

## CLI Fallback

Use MCP tools first. If the current MCP server profile does not expose a tool
such as `flow_tool` or `review_tool`, run the same implementation
through the CLI without restarting the agent:

```bash
dagayn tool get_minimal_context_tool --arg 'task="debug login timeout"'
dagayn tool flow_tool --arg mode='"get"' --arg 'flow_name="handle_request"'
dagayn tool review_tool --arg mode='"impact"' --arg 'changed_files=["src/auth.py"]'
```

## Token Efficiency Rules
- ALWAYS start with `get_minimal_context(task="<your task>")` before any other graph tool.
- Use `detail_level="minimal"` on all calls. Only escalate to "standard" when minimal is insufficient.
- Target: complete any review/debug/refactor task in ≤5 tool calls and ≤800 total output tokens.
