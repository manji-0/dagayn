---
name: debug-issue
description: Debug a bug, failing test, error, stack trace, crash, or unexpected behavior in a repository dagayn indexes — reproduce it if that is cheap, locate the code behind the symptom, trace callers and callees, check recent changes, and confirm the failing path in source. Use whenever the user reports something broken or asks why X happens or where an error comes from.
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
4. **Find the entry point**: `flow_tool(mode="entry_points",
   target=<suspect>)` returns the nearest entry points that reach it (`main`,
   framework handlers, FFI exports, uncalled functions, trait-dispatched
   methods), each with one shortest call `chain` to read in order. It needs no
   stored flows and never walks test code. `dispatched_method` means a trait,
   interface, or framework calls it, which the graph cannot see; an empty list
   means only tests (or dynamic calls) reach the suspect.
5. **Check recent changes**: `review_tool(mode="changes",
   detail_level="minimal")` and read `findings`: a `dangling_reference` or
   `unchanged_caller` near the suspect is a likely cause, and `tests_to_run`
   names the tests to rerun. With no `base` it reviews uncommitted edits, or
   the last commit on a clean tree; pass `base=` for older changes. Use
   `mode="impact"` only when the blast radius is still unclear.
6. **Follow linked docs** when they can explain the behavior: `docs_for` from
   the suspect code point (runbooks and problem statements are often more
   useful than another caller hop), `implementations_of` when the report starts
   from a Markdown contract section.
7. **Confirm the failing span** with `source_of`; open the file only when the
   span is truncated or stale, or you need the surrounding code.

## Evidence

<!-- dagayn trust tiers -->
Reach comes from the graph, correctness from `source_of`, and user-visible
effect from a reproduction; keep them apart in a claim. Full rules:
`get_docs_section_tool(section_name="trust")`.

- **Highest** — on a current graph (`sync.state` is `commit_synced` or
  `worktree_ahead`): `HIGH` and `EXTRACTED` edges, whether the parser or a
  SCIP index (`resolved_by: "scip"`) settled them; `source_of` spans;
  authored doc contracts (`implemented_by` / `implements_contract` links whose
  target exists — `evidence_type=authored` alone is on every Markdown result).
- **Medium** — structure, not correctness: `MEDIUM` (inferred) edges,
  `reason_codes`, blast radius, flows, communities, metrics, suggestions,
  explanatory doc links, and search hits.
- **Low** — a hypothesis until `source_of` or a reproduction confirms it:
  `LOW` edges, `heuristic_reachable` doc links whatever their edge confidence,
  a file-level `tests_for` of 0, `truncated` or `ambiguous` results, and
  answers about files changed since the graph was built (`sync.state`
  `commit_drift` or `worktree_behind`).
<!-- /dagayn trust tiers -->

## Tips

- Prefer relationship queries over raw traversal when the question names a
  relationship; `traverse_graph_tool` (advanced surface) is for "what else is
  nearby?" once the likely node is known.
- Check both directions: callers show how the bad input arrives, callees show
  what the code depends on.
- Doc links carry `evidence_type`: `authored` contracts beat `extracted`
  explanations; `heuristic_reachable` needs source confirmation.
- If a query returns nothing, read `zero_result_reason`, `next_action`,
  and `missingness` before ruling a path out. Unresolved
  (`LOW`) calls can hide the real callee; where SCIP indexers are installed,
  `dagayn build --scip` settles them (settled edges carry `resolved_by: "scip"`).
- Pass `detail_level="minimal"` to tools that take it; use `"standard"` on
  `query_graph_tool` when you need every related node.

## CLI fallback

```bash
dagayn tool get_minimal_context_tool --arg 'task="debug login timeout"'
dagayn tool flow_tool --arg mode='"entry_points"' --arg target='"src/auth.py::handler"'
dagayn tool query_graph_tool --arg pattern='"callers_of"' --arg target='"src/auth.py::handler"' --arg depth=4
dagayn tool query_graph_tool --arg pattern='"source_of"' --arg target='"src/auth.py::handler"'
```
