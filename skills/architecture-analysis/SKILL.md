---
name: Architecture Analysis
description: Evaluate architecture signals through the unified dagayn dispatcher
---

## Architecture Analysis

Use `architecture_analysis_tool` as the single MCP entry point for architecture
evaluation. Start broad, then drill down only when the overview or a concrete
question points to a signal.

### Steps

1. Run `get_minimal_context_tool(task="<architecture goal>")` to check graph
   freshness, risk, and suggested next tools.
2. Run `architecture_analysis_tool(mode="overview", detail_level="minimal")`.
   Read `architecture_health.reason_codes`, counts, examples, and
   `drill_downs`.
3. Choose one follow-up mode only when there is a specific question:
   - `communities`: boundaries, large clusters, cohesion, coupling shape
   - `community`: one community's metadata or member sample
   - `hubs`: high-degree hotspots with broad blast radius
   - `bridges`: betweenness chokepoints between regions
   - `knowledge_gaps`: isolated nodes, thin communities, untested hotspots
   - `surprising_connections`: unexpected cross-boundary coupling
   - `adp_violations`: dependency cycles
   - `sdp_metrics` / `sdp_violations`: dependency stability direction
   - `sap_metrics` / `sap_violations`: abstraction/stability balance
4. Use `query_graph_tool` or source reads only after the metric output identifies a
   concrete node, edge, community, package, or file to verify.
5. For architecture questions that cross documentation/code boundaries, use
   `query_graph_tool(pattern="docs_for", target="<path::symbol>", detail_level="minimal")`
   from code/Terraform nodes and
   `query_graph_tool(pattern="implementations_of", target="<doc.md>::<section-slug>", detail_level="minimal")`
   from Markdown contract sections. Treat `CROSS_ARTIFACT` documentation roles
   as typed traceability evidence, not automatic architectural coupling; read
   `evidence_type` and `missingness` before treating a doc edge as contract
   evidence.

### Evidence Rules

- Treat architecture signals as leads, not proof of a design bug.
- Cite counts, thresholds, reason codes, `total`, `truncated`, and approximation
  metadata when making claims.
- Prefer `detail_level="minimal"` and small `top_n` values first. Increase only
  when the result is too narrow to answer the question.
- Verify source behavior before turning graph structure into a correctness or
  refactor recommendation.
- When citing Markdown ↔ code relationships, name the stored role
  (`implemented_by`, `implements_contract`, `explained_by`, `has_runbook`,
  `problem_described_by`, `discusses_artifact`, or `raises_issue_for`) and
  whether it came from Markdown or code. Include `evidence_type` when it affects
  the strength of the claim.
- For zero-result graph queries, include `zero_result_reason` and `next_action`
  rather than treating the result as proof of no relationship.

## CLI Fallback

Use MCP first. If the current MCP profile does not expose the dispatcher, call
the same implementation through the CLI:

```bash
dagayn tool architecture_analysis_tool --arg mode='"overview"' --arg detail_level='"minimal"'
dagayn tool architecture_analysis_tool --arg mode='"sdp_violations"' --arg top_n=10
dagayn tool architecture_analysis_tool --arg mode='"community"' --arg community_name='"auth"'
dagayn tool query_graph_tool --arg pattern='"docs_for"' --arg target='"src/app.py::handler"'
dagayn tool query_graph_tool --arg pattern='"implementations_of"' --arg target='"docs/spec.md::contract-section"'
```

## Token Efficiency Rules

- ALWAYS start with `get_minimal_context_tool(task="<your task>")`.
- Use `architecture_analysis_tool(mode="overview", detail_level="minimal")`
  before any architecture drill-down mode.
- Target: answer architecture questions in <=5 tool calls unless a concrete
  source verification step requires more.
