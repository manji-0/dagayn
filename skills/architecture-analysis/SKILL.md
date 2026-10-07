---
name: architecture-analysis
description: Assess architecture with the dagayn graph — the declared units (crates, packages, modules) and how they depend on each other, import cycles, widely used untested code, broken doc links, and SDP / SAP stability metrics per unit. Use when the user asks about architecture, layering, modularity, coupling, cycles, the riskiest parts of the codebase, or wants an architecture health check.
---

# Architecture Analysis

`architecture_analysis_tool(mode="overview")` is the single entry point. It
returns a map of the units the repository declares and a list of `findings`,
structural facts worth acting on. Answer from those two first; open a metric
mode only when the user asks for one by name.

## Steps

1. **Orient**: `get_minimal_context_tool(task="<architecture goal>")`. If
   `graph_health.status` is `empty` or `sync.state` is `unbuilt` /
   `commit_drift`, follow `recommended_action`. The overview needs no
   post-processing: it answers right after an `ensure_graph_tool` bootstrap.
2. **Overview**: `architecture_analysis_tool(mode="overview",
   detail_level="minimal")`.
   - `units`: each Cargo crate, npm package, Go module, Python import package,
     or Terraform module, with its files, symbols, and tests. Code no manifest
     covers falls back to its top-level directory (`kind: "directory"`).
   - `unit_edges`: calls, imports, references, inheritance, and bridges between
     two units, heaviest first. `declared: true` marks a dependency a manifest
     lists; such a pair with no counts is one the graph cannot see (a call
     inside a macro).
   - `findings`: `import_cycle` (modules that import each other when they load,
     with `cut`, the imports that break it), `untested_core` (a symbol used
     from many files that no test reaches through its callers), and
     `broken_doc_link` (a directive pointing at a file, section, or symbol that
     is gone). Empty means nothing structural to act on; say so.
   - `detail_level="standard"` adds each unit's `surface`, the symbols other
     units use most: the unit's de facto public API.
3. **Confirm each finding** before stating it: `query_graph_tool`
   (`importers_of` on a cycle's modules, `callers_of` / `tests_for` on an
   untested symbol, `source_of` for the code), or read the directive's line.
4. **Metrics only on request**: `sdp_metrics` / `sdp_violations` (a unit
   depending on a less stable one) and `sap_metrics` / `sap_violations`
   (abstractness vs instability) compute per declared unit;
   `scope_kind="directory"` gives the old per-directory SAP. For these modes
   `dependency_profile` chooses the edges: `strict_static` (imports,
   inheritance, `DEPENDS_ON`; default), `implementation` (+ calls),
   `infra_dataflow` (+ Terraform references), `artifact_trace` (+ high-
   confidence doc links).
5. **Doc ↔ code boundaries**: `docs_for` from code or Terraform nodes,
   `implementations_of` from a Markdown section. These are traceability links,
   not architectural coupling.

`hubs`, `bridges`, `knowledge_gaps`, `surprising_connections`, and
`adp_violations` are deprecated and answer with a `deprecated` note naming
their replacement; do not start from them. `communities` / `community` are
graph clusters for an explicit clustering question and need full
post-processing (`dagayn postprocess`).

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

Rank results with the Highest / Medium / Low trust tiers in the installed
dagayn instructions (full rules: `get_docs_section_tool(section_name="trust")`).

- A finding is a checkable claim with a location: cite its `kind`, `file` /
  `line` or `targets`, and `evidence`, after confirming it at the source.
- The map is aggregated graph structure: cite edge counts as counts, and note
  that a call the extractor cannot resolve is missing from them.
- SDP / SAP values are **Medium** leads, not design bugs: cite the formula's
  inputs, the threshold, and `total` / `truncated`.
- For a zero-result query, cite `zero_result_reason` and `next_action` rather
  than treating it as proof that no relationship exists.
- Call-based profiles (`implementation`) are only as good as call resolution;
  `dagayn build --scip` makes them compiler-accurate where indexers exist.

## CLI fallback

```bash
dagayn tool architecture_analysis_tool --arg mode='"overview"' --arg detail_level='"minimal"'
dagayn tool architecture_analysis_tool --arg mode='"overview"' --arg detail_level='"standard"'
dagayn tool architecture_analysis_tool --arg mode='"sdp_violations"' --arg dependency_profile='"implementation"'
dagayn tool query_graph_tool --arg pattern='"importers_of"' --arg target='"pkg/a.py"'
```
