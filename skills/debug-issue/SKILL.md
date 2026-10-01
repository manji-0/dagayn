---
name: debug-issue
description: Debug a bug, failing test, error message, stack trace, crash, or unexpected behavior in a repository dagayn has indexed — locate the code behind the symptom, trace callers and callees, check affected flows and recent changes, and confirm the failing path in source. Consult this skill before running the failing test, grepping for the error text, or calling graph tools whenever the user reports something broken or asks why X happens, where an error comes from, or why a tool or command returns the wrong result — even a one-line "why does this fail" question.
---

# Debug Issue

The graph turns a symptom into a short list of code points and the paths
between them. Use it to decide where to look, then prove the cause in source:
centrality or a call edge alone never establishes a root cause.

<!-- dagayn skill embedding context -->
## Installed Search Mode

This packaged skill is mode-neutral. `dagayn install` rewrites this section with
the selected embedding mode so bug searches balance semantic recall with speed.
<!-- /dagayn skill embedding context -->

## Steps

1. **Orient**: `get_minimal_context_tool(task="<bug or symptom>")`. If
   `graph_health.status` is `empty` or `sync.state` is `unbuilt` /
   `commit_drift`, follow `recommended_action` (call `ensure_graph_tool()` when
   you need to wait for it).
2. **Find the code**: `semantic_search_nodes_tool` for the symptom; for a log
   line, CLI command, or UI string, one `rg` to map the literal to a node, then
   back to graph tools.
3. **Read and trace**: `query_graph_tool(pattern="source_of")` for the suspect,
   then `callers_of` / `callees_of`. `callers_of` with `depth` (up to 6) walks
   the caller chain in one call; check `reachability` before calling it the
   whole chain. Calls into packages (`subprocess`, `std`, `builtins`) appear in
   `unresolved_targets` because packages are not nodes — that is expected, not
   a gap. If a target is `status="ambiguous"`, re-query with a
   `qualified_name` from `candidates`.
4. **Find the entry point**: `flow_tool(mode="list", detail_level="minimal")`
   or `review_tool(mode="affected_flows")`, then `flow_tool(mode="get")` with
   the chosen `flow_name` (or `flow_id`). A flow is a reachable set in BFS order, not an execution
   trace; read `truncated`. Flows exist only after full post-processing — if
   the list is empty right after a bootstrap, run `dagayn postprocess`.
5. **Check recent changes**: `review_tool(mode="changes",
   detail_level="minimal")` and read `risk_level`, `reason_codes`,
   `affected_flow_rankings`, and `recommended_tests`; use `mode="impact"` only
   when the blast radius is still unclear.
6. **Follow linked docs** when they can explain the behavior: `docs_for` from
   the suspect code point (runbooks and problem statements are often more
   useful than another caller hop), `implementations_of` when the report starts
   from a Markdown contract section.
7. **Confirm the failing span** with `source_of`; open the file only when the
   span is truncated or stale, or you need the surrounding code.

## Tips

- Prefer relationship queries over raw traversal when the question names a
  relationship; `traverse_graph_tool` (advanced surface) is for "what else is
  nearby?" once the likely node is known.
- Check both directions: callers show how the bad input arrives, callees show
  what the code depends on.
- Doc links carry `evidence_type`: `authored` contracts beat `extracted`
  explanations; `heuristic_reachable` needs source confirmation.
- If a query returns nothing, read `zero_result_reason`, `next_action`,
  `answerability`, and `missingness` before ruling a path out. Unresolved
  (`LOW`) calls can hide the real callee; where SCIP indexers are installed,
  `dagayn build --scip` settles them (settled edges carry `resolved_by: "scip"`).
- Pass `detail_level="minimal"` to tools that take it; use `"standard"` on
  `query_graph_tool` when you need every related node.

## CLI fallback

```bash
dagayn tool get_minimal_context_tool --arg 'task="debug login timeout"'
dagayn tool flow_tool --arg mode='"list"' --arg detail_level='"minimal"'
dagayn tool flow_tool --arg mode='"get"' --arg 'flow_name="handle_request"'
dagayn tool query_graph_tool --arg pattern='"callers_of"' --arg target='"src/auth.py::handler"' --arg depth=4
dagayn tool query_graph_tool --arg pattern='"source_of"' --arg target='"src/auth.py::handler"'
```
