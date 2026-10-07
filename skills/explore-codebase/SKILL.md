---
name: explore-codebase
description: Explore and explain code in a repository dagayn indexes — how a mechanism works end to end, where something is defined, what calls or imports it, what a module does, which tests and docs cover it — through graph relationships instead of whole-file reads. Use before the first search when the user asks how something works, where X lives, what uses X, or is onboarding.
---

# Explore Codebase

Answer structural questions from the graph and read source only for the spans
that matter. The decision model below picks the cheapest tool that can answer
the question; running the whole ladder wastes context.

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
  narrowest pattern (`callers_of`, `callees_of`, `imports_of`, `importers_of`,
  `inheritors_of`, `tests_for`, `docs_for`, `implementations_of`,
  `bridges_from`, `children_of`, `file_summary`, or `source_of`).
- Everything that reaches a function or file, not just direct neighbours: pass
  `depth` (up to 6) to `callers_of` or `importers_of` instead of calling once
  per discovered node. `reachability.depth_limit_reached` says whether a deeper
  walk could find more.
- Known entity whose body you need to inspect: use
  `query_graph_tool(pattern="source_of")` before opening the file.
- Changed code, review risk, or blast radius: use `review_tool` before raw
  traversal.
- Architecture health or structural risk: use
  `architecture_analysis_tool(mode="overview")` before metric drill-downs.
- Where code is entered from: `flow_tool(mode="entry_points", target=...)`
  lists the nearest entry points reaching a symbol, each with one call
  `chain` in call order. `flow_tool(mode="list")` / `"get"` read stored
  reachable sets (BFS visit order, not a call sequence) and remain for one
  release.
- Neighborhood exploration: use `traverse_graph_tool` only after choosing a
  concrete start node, only when a specific relationship query would be too
  narrow, and only when the advanced MCP surface (or `dagayn tool`) exposes it.

### Steps

1. Orient with `get_minimal_context_tool(task="<what you need to understand>")`.
   If `graph_health.status` is `empty` or `sync.state` is `unbuilt` /
   `commit_drift`, follow `recommended_action` (call `ensure_graph_tool()` when
   you need to wait for it).
2. Pick **one** next move from the Decision Model above — for structure or
   health questions only, that is
   `architecture_analysis_tool(mode="overview", detail_level="minimal")` (read
   `units` and `unit_edges` for the layering, then `findings`; the
   architecture-analysis skill covers the rest).
3. With a concrete node, take its `source_of` span, then verify relationships
   with the narrow patterns. Read files only when `source_of` reports
   `source_coverage` as truncated or stale, or when you need surrounding code or
   are about to edit.

## Following replies

<!-- dagayn workflow -->
Orient → locate → read → trace → judge → confirm. Enter at the phase the task
needs, and close the loop after an edit with `review_tool`.

1. **Orient**: `get_minimal_context_tool(task=...)` reports `sync.state` and
   the first calls for the task.
2. **Locate**: `semantic_search_nodes_tool` turns a description into a
   `qualified_name`.
3. **Read**: `query_graph_tool(pattern="source_of")` returns the live span.
4. **Trace**: `query_graph_tool` (`callers_of`, `callees_of`, `tests_for`,
   `docs_for`) and `flow_tool(mode="entry_points")`.
5. **Judge**: `review_tool`, `architecture_analysis_tool`, or
   `refactor_tool` answer with `findings`; an empty list means nothing to act
   on.
6. **Confirm**: `source_of` on each finding's place, a test, or a
   reproduction.

Every reply ends with `next`: at most three calls with complete arguments and
a `why`. Follow it unless the task points elsewhere; `[]` means the answer is
complete. `status="ambiguous"` puts one retry per candidate in `next`, and
`missingness` lists only the gaps that limit that reply. Full contract:
`get_docs_section_tool(section_name="workflow")`.
<!-- /dagayn workflow -->

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

- Don't open with architecture overview unless the question is about
  structure or health.
- `children_of` on a file lists its functions and classes.
- `callees_of` lists repository nodes in `results`; calls into packages
  (`subprocess`, `std`, `react`) appear in `unresolved_targets` because
  packages are not nodes.
- A bare name matching several symbols returns `status="ambiguous"` with
  `candidates`; re-query with one of their `qualified_name`s.
- Flows and communities need full post-processing; if they come back empty
  right after a bootstrap, run `dagayn postprocess`.
- Markdown ↔ code traceability: `dagayn:` directives are authored
  `CROSS_ARTIFACT` evidence — an HTML comment in a Markdown section
  (`dagayn: implemented-by` with a `path::symbol` target) points to code, and a
  code comment in Python, Terraform, or C# (`dagayn: implements` with a
  `docs/spec.md#Section` target) points to a doc. Query tools expose the inverse
  labels, so don't assume both directions are stored; read `evidence_type`
  (`authored`, `extracted`, `heuristic_reachable`) and `missingness`.
- An empty or not-found result is limited to the current graph: use
  `zero_result_reason` and `next_action` to choose the next lookup.
- Cite counts, thresholds, reason codes, and truncation flags for structural
  claims.

## CLI fallback

```bash
dagayn tool query_graph_tool --arg pattern='"children_of"' --arg target='"src/app.py"'
dagayn tool query_graph_tool --arg pattern='"importers_of"' --arg target='"src/app.py"' --arg depth=3
dagayn tool query_graph_tool --arg pattern='"source_of"' --arg target='"src/app.py::handler"'
dagayn tool traverse_graph_tool --arg query='"auth handler"' --arg depth=2
```
