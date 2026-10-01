---
name: explore-codebase
description: Explore and explain code in a repository dagayn has indexed — how a mechanism works end to end, where something is defined, what calls or imports it, what a file or module does, which tests and docs cover it — using graph relationships instead of reading whole files. Consult this skill before the first search, grep, or graph call whenever the user asks how something works, walks through a process, asks where X lives or what uses X, wants a module overview, or is onboarding onto the codebase.
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
- Reachable-set flow: use `flow_tool(mode="list")`, then `flow_tool(mode="get")`
  only after choosing a concrete flow. Treat `path` / `steps` as BFS visit
  order, not a runtime call sequence, and read `truncated` / `truncation_reason`.
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
   `architecture_health`, then use the architecture-analysis skill).
3. With a concrete node, take its `source_of` span, then verify relationships
   with the narrow patterns. Read files only when `source_of` reports
   `source_coverage` as truncated or stale, or when you need surrounding code or
   are about to edit.

## Evidence

Reach comes from the graph; correctness from `source_of`; user-visible
effect from a reproduction or CLI output. Do not mix those in one claim.

- **Highest** — assert freely: on a graph whose orientation shows no
  `graph_describes_another_commit` or `graph_built_by_older_extractor`,
  `CALLS` / importer edges with `resolved_by: "scip"` at `HIGH` or
  `EXTRACTED`; the `source_of` span those edges point at; authored
  `CROSS_ARTIFACT` contracts (`implemented_by` / `implements_contract`,
  `evidence_type=authored`).
- **Medium** — structure only, not correctness: `review_tool`
  `reason_codes`, blast radius, affected flows; `EXTRACTED` `TESTED_BY`
  and directive dependencies; FTS hits when embeddings are empty or
  `embedding_health` is not available (keyword candidates, not semantic
  ranking).
- **Low** — hypothesis until `source_of` or a reproduction: `MEDIUM` /
  `LOW` or non-SCIP calls; `heuristic_reachable`; a file-level
  `tests_for` of 0; `truncated`, `status="ambiguous"`, or absences on a
  degraded orientation.

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
