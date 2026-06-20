---
name: debug-issue
description: Systematically debug issues using graph-powered code navigation
---

## Debug Issue

Use the knowledge graph to systematically trace and debug issues.

<!-- dagayn skill embedding context -->
## Installed Search Mode

This packaged skill is mode-neutral. `dagayn install` rewrites this section with
the selected embedding mode so bug searches balance semantic recall with speed.
<!-- /dagayn skill embedding context -->

### Steps

1. Run `get_minimal_context_tool(task="<bug or symptom>")` to check graph freshness,
   risk, and suggested next tools.
2. Use `semantic_search_nodes_tool` to find code related to the issue.
3. Use `query_graph_tool` with `callers_of` and `callees_of` to trace call chains.
4. Use `traverse_graph_tool` only after selecting a concrete suspected node and
   only when a bounded neighborhood is more useful than a specific caller,
   callee, import, test, or documentation relationship.
5. Use `flow_tool(mode="list", detail_level="minimal")` or
   `review_tool(mode="affected_flows")` to identify candidate flow names before
   calling `flow_tool(mode="get")`.
6. If the suspected code point has documentation bridges, use
   `query_graph_tool(pattern="docs_for", target="<path::symbol>", detail_level="minimal")`
   to pull in linked specs, runbooks, explanations, and issue notes. If the bug
   report starts from a Markdown contract section, use
   `query_graph_tool(pattern="implementations_of", target="<doc.md>::<section-slug>", detail_level="minimal")`
   to find the implementation nodes linked by `implemented_by` /
   `implements_contract`.
7. Run `review_tool(mode="changes")` to check if recent changes caused the issue. Read
   `analysis_summary` for risk reasons, affected-flow rankings, hotspot
   proximity, and recommended tests.
8. Use `review_tool(mode="impact")` on suspected files only when `analysis_summary` or
   the call trace leaves the blast radius unclear.
9. Read source directly once graph evidence identifies the likely failing path.

### Tips

- Check both callers and callees to understand the full context.
- Prefer relationship queries over raw traversal when the question names a
  relationship. Raw traversal is for "what else is nearby?" after the likely
  node is known.
- Look at affected flows to find the entry point that triggers the bug.
- Recent changes are the most common source of new issues.
- When the symptom starts from a log line, CLI command, or UI action, use `rg`
  once to map that literal string to a graph node, then return to graph tools.
- Treat `dagayn:` documentation directives as typed traceability evidence. Code
  comments such as `# dagayn: explained-by docs/runbook.md#Failure Mode` can
  point to runbooks or problem statements that are more useful than another
  caller hop.
- For documentation bridge query results, check `evidence_type`: `authored`
  contract links are stronger than `extracted` explanatory links, while
  `heuristic_reachable` links require source confirmation.
- If `query_graph_tool` returns no result or `status="not_found"`, read
  `zero_result_reason`, `next_action`, `answerability`, and `missingness`
  before ruling out a path.
- Do not infer root cause from graph centrality alone; require an observed
  failing path, changed behavior, or source-level defect.

## CLI Fallback

Use MCP tools first. If the current MCP server profile does not expose a tool
such as `flow_tool` or `review_tool`, run the same implementation
through the CLI without restarting the agent:

```bash
dagayn tool get_minimal_context_tool --arg 'task="debug login timeout"'
dagayn tool flow_tool --arg mode='"list"' --arg detail_level='"minimal"'
dagayn tool flow_tool --arg mode='"get"' --arg 'flow_name="handle_request"'
dagayn tool review_tool --arg mode='"impact"' --arg 'changed_files=["src/auth.py"]'
dagayn tool query_graph_tool --arg pattern='"docs_for"' --arg target='"src/auth.py::handler"'
dagayn tool query_graph_tool --arg pattern='"implementations_of"' --arg target='"docs/auth.md::login-contract"'
```

## Token Efficiency Rules
- ALWAYS start with `get_minimal_context_tool(task="<your task>")` before any other graph tool.
- Use `detail_level="minimal"` on all calls. Only escalate to "standard" when minimal is insufficient.
- Target: complete any review/debug/refactor task in ≤5 tool calls and ≤800 total output tokens.
