---
name: architecture-analysis
description: Assess a repository's architecture with the dagayn knowledge graph — module boundaries and communities, coupling, hubs and bridges (blast-radius hotspots), dependency cycles (ADP), stability direction (SDP), abstraction balance (SAP), and knowledge gaps — with counts and thresholds behind every claim. Use this whenever the user asks about architecture, layering, modularity, coupling, cyclic dependencies, "what are the riskiest parts of this codebase", where boundaries should be, or wants an architecture review or health check.
---

# Architecture Analysis

`architecture_analysis_tool` is the single entry point. Start with the overview,
then open one metric mode only when the overview or the user's question points
at a specific signal: each mode answers a narrow question, and running them all
buries the answer.

## Steps

1. **Orient**: `get_minimal_context_tool(task="<architecture goal>")`. If
   `graph_health.status` is `empty` or `sync.state` is `unbuilt` /
   `commit_drift`, follow `recommended_action`.
   Community and hub results need full post-processing: right after an
   `ensure_graph_tool` bootstrap (minimal post-processing) they come back empty
   and `graph_health.status` is `degraded`. Run `dagayn postprocess` first.
2. **Overview**: `architecture_analysis_tool(mode="overview",
   detail_level="minimal")`. Read `architecture_health.reason_codes`, `counts`,
   `top_examples`, `guidance`, and `drill_downs`.
3. **One follow-up mode** for a specific question:
   - `communities` / `community`: boundaries, large clusters, cohesion
   - `hubs` / `bridges`: high-degree hotspots and betweenness chokepoints
   - `knowledge_gaps`: isolated nodes, thin communities, untested hotspots
   - `surprising_connections`: unexpected cross-boundary coupling
   - `adp_violations`: dependency cycles
   - `sdp_metrics` / `sdp_violations`: dependency stability direction
   - `sap_metrics` / `sap_violations`: abstraction / stability balance
   Structural modes default to `artifact_scope="code"` (no docs, no tests);
   pass `"docs"` for documentation cycles or `"all"` for the mixed graph. For
   ADP / SDP / SAP, `dependency_profile` chooses the edges: `strict_static`
   (imports, inheritance, `DEPENDS_ON`; default), `implementation` (+ calls),
   `infra_dataflow` (+ Terraform references), `artifact_trace` (+ high-
   confidence doc links). `granularity="file" | "package"` sets the unit.
4. **Verify the concrete thing** the metric named with `query_graph_tool`
   (`callers_of`, `importers_of` with `depth` for transitive reach,
   `source_of` for one node's body). `traverse_graph_tool` (advanced surface)
   is for a bounded neighborhood around one chosen hub or bridge.
5. **Doc ↔ code boundaries**: `docs_for` from code or Terraform nodes,
   `implementations_of` from a Markdown section. These are traceability links,
   not architectural coupling.

## Evidence

Reach comes from the graph; correctness from `source_of`; user-visible
effect from a reproduction or CLI output. Do not mix those in one claim.

- **Highest** — assert freely: on a graph whose orientation shows no
  `graph_describes_another_commit` or `graph_built_by_older_extractor`,
  `CALLS` / importer edges with `resolved_by: "scip"` at `HIGH` or
  `EXTRACTED`; the `source_of` span those edges point at; authored
  `CROSS_ARTIFACT` contracts (`implemented_by` / `implements_contract`,
  `evidence_type=authored`).
- **Medium** — structure only, not correctness: architecture
  `reason_codes`, hub/bridge/knowledge-gap rankings, ADP/SDP/SAP metrics,
  blast-style community structure; `EXTRACTED` directive dependencies; FTS
  hits when embeddings are empty (keyword candidates, not semantic ranking).
- **Low** — hypothesis until `source_of` or a reproduction: `MEDIUM` /
  `LOW` or non-SCIP calls; `heuristic_reachable`; `truncated`,
  `status="ambiguous"`, or absences on a degraded orientation.
- Architecture signals are **Medium** leads, not proof of a design bug: cite
  counts, thresholds, reason codes, and `total` / `truncated`.
- Start with small `top_n`; a truncated `adp_violations` result's first
  `next_tool_suggestions` entry repeats the call with the full count.
  `sap_metrics` lists scopes the metric doesn't apply to under
  `inapplicable_metrics`.
- Go broad to narrow: overview, one metric mode, then a relationship query or
  `source_of` before turning structure into a recommendation.
- Name the stored role when citing a doc link (`implemented_by`,
  `implements_contract`, `describes_symbol`, `explained_by`, `has_runbook`,
  `problem_described_by`, `discusses_artifact`, `discussed_by`,
  `raises_issue_for`) and its `evidence_type`.
- For a zero-result query, cite `zero_result_reason` and `next_action` rather
  than treating it as proof that no relationship exists.
- Call-based profiles (`implementation`) are only as good as call resolution;
  `dagayn build --scip` makes them compiler-accurate where indexers exist.

## CLI fallback

```bash
dagayn tool architecture_analysis_tool --arg mode='"overview"' --arg detail_level='"minimal"'
dagayn tool architecture_analysis_tool --arg mode='"adp_violations"' --arg dependency_profile='"implementation"'
dagayn tool architecture_analysis_tool --arg mode='"community"' --arg community_name='"auth"'
dagayn tool query_graph_tool --arg pattern='"source_of"' --arg target='"src/app.py::handler"'
```
