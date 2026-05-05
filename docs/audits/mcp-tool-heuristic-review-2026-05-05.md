# MCP Tool Heuristic Review, 2026-05-05

<!-- derived-from ./mcp-tool-output-review.md -->
<!-- constrained-by ../COMMANDS.md#mcp-tools -->

## Scope

This review covers the MCP tools exposed by dagayn as of 2026-05-05, with
emphasis on whether tool output is grounded in explicit quantitative or
qualitative evidence, and whether the interface set contains avoidable
duplication.

The graph used for spot checks contained 5,492 nodes, 39,254 edges, 356 files,
and 29 languages. Output-size risks from the 2026-04-30 audit were rechecked
with compact or bounded calls where available.

## Findings

### High: `get_knowledge_gaps` used a weak hotspot threshold

The prior hotspot rule treated any non-test node with degree `>= 5` and no
`TESTED_BY` edge as an untested hotspot. On the current graph, degree `>= 5`
is near the median rather than a hotspot threshold, so the rule over-reported
ordinary nodes as insight.

Measured distribution before the fix:

| Metric | Value |
| --- | ---: |
| Non-file nodes | 5,136 |
| p50 degree | 6 |
| p75 degree | 11 |
| p90 degree | 19 |
| p95 degree | 27 |
| Nodes with degree `>= 5` | 3,156 |
| Untested nodes with degree `>= 5` | 1,878 |

The implementation now derives the default untested-hotspot threshold from the
repository's positive-degree production candidates at p95, with a floor of 5.
The MCP response includes the threshold, candidate count, raw counts, returned
counts, `top_n`, and truncation state.

Current spot check with `top_n=5`:

| Field | Value |
| --- | ---: |
| `untested_hotspot_min_degree` | 37 |
| Candidate positive-degree count | 2,013 |
| Raw isolated nodes | 623 |
| Raw thin communities | 138 |
| Raw untested hotspots | 72 |
| Raw single-file communities | 107 |

The same change also excludes documentation sections and test-like file paths
from untested-hotspot candidates. This removes false positives such as Rust
`tests.rs` helper code without making the rule Rust-specific.

### Medium: centrality tools are quantitatively grounded but need method metadata

`get_hub_nodes` is well grounded: it ranks non-file nodes by total graph degree
and returns `in_degree`, `out_degree`, and `total_degree`.

`get_bridge_nodes` is also grounded: it uses betweenness centrality. For graphs
larger than 5,000 nodes it switches to a 500-node approximation. That is a
reasonable performance tradeoff, but the response should expose
`approximate=true`, sample size, graph node count, and preferably a deterministic
seed so repeated calls are explainable.

### Medium: composite insight tools expose reasons but not enough scoring context

`get_surprising_connections` uses explicit qualitative factors:
cross-community, rare community pair, cross-language, peripheral-to-hub,
degree imbalance, cross-test-boundary, and unusual edge kind. The factors are
reasonable as review leads, but the weights are hand-tuned. The response should
include the scoring policy version and key thresholds such as the
peripheral-to-hub degree threshold.

`get_suggested_questions` is useful as an orchestration surface, but its
priority labels are mostly inherited from the source category rather than
calibrated from a shared severity score. Treat it as a prompt generator, not a
risk ranker.

### Medium: architecture principle tools have defensible quantitative bases

The ADP, SDP, and SAP tools are grounded in named architecture metrics:

| Tool family | Basis | Review |
| --- | --- | --- |
| ADP | Directed dependency cycles and weighted cycle severity | Defensible, though long-cycle severity should stay advisory. |
| SDP | Instability `I = Ce / (Ca + Ce)` and dependency direction deltas | Defensible and standard. |
| SAP | Abstractness, instability, and distance from the main sequence | Defensible and standard. |

These tools should remain separate from generic graph queries because their
domain concepts and output shape are different from raw traversal.

### Low: interface count is high but mostly purposeful

The current tool set is broad, but most pairs have distinct user intent:

| Surface | Keep? | Reason |
| --- | --- | --- |
| `detect_changes` and `get_review_context` | Keep, but document `detect_changes` as the default review entry point | One ranks risk; the other fetches focused context. |
| `query_graph` and `traverse_graph` | Keep | One answers fixed graph questions; the other explores neighborhoods. |
| `list_communities`, `get_community`, `get_architecture_overview` | Keep | Summary, drill-down, and architecture overview are separate workflows. |
| SAP/SDP metric and violation tools | Keep | Raw metrics and filtered violations serve different consumers. |
| Individual analysis tools and `get_suggested_questions` | Keep for now | Atomic evidence tools are needed; the question generator composes them. |

The main redundancy risk is discoverability rather than runtime duplication.
Future cleanup should prefer a documented "default path" over removing tools:
`get_minimal_context` -> `detect_changes` or `query_graph` -> specialized
analysis tools only when the first result points there.

## Changes Made

The review produced one implementation change:

- `get_knowledge_gaps` now accepts category-level `top_n`.
- Untested-hotspot detection uses the p95 degree of eligible production
  candidates instead of fixed `degree >= 5`.
- Test files, test nodes, and Markdown documentation sections are excluded from
  untested-hotspot candidates.
- The response now exposes thresholds, degree distribution, raw counts,
  returned counts, and truncation state.

## Remaining Improvements

1. Add approximation metadata and deterministic sampling to `get_bridge_nodes`.
2. Add scoring-policy metadata to `get_surprising_connections`.
3. Normalize response hints for small outliers such as `get_docs_section` and
   wiki not-found responses.
4. Document the default MCP workflow so the large tool set is easier to choose
   from without deleting useful specialized tools.

## Summary

<!-- derived-from #findings -->
<!-- derived-from #changes-made -->

Most MCP tools are now bounded and usable on the current repository. The weakest
quantitative basis found in this pass was `get_knowledge_gaps` untested-hotspot
detection, which has been corrected. Remaining issues are mainly explainability
metadata and interface discoverability, not evidence-free core analysis.
