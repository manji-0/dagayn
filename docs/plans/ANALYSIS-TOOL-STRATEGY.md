# Analysis tool strategy plan

<!-- constrained-by ../COMMANDS.md#mcp-tools -->
<!-- constrained-by ../ARCHITECTURE.md#query-surfaces -->
<!-- derived-from ../audits/mcp-tool-heuristic-review-2026-05-05.md#low-interface-count-is-high-but-mostly-purposeful -->

## Goal

Define which analyses dagayn should expose as user-facing tools without making
agents or humans choose from a large undifferentiated tool list.

The plan treats the graph as a shared analysis substrate. Individual metrics are
useful, but they should not automatically become separate first-choice tools.
The default surface should answer workflow questions, then expose specialized
metrics only as drill-down evidence.

## Product principle

Dagayn should optimize for a small number of goal-oriented entry points:

1. What should I do next?
2. What is risky in this change?
3. What is unhealthy in this architecture?
4. What refactor is worth considering?
5. What exact graph relationship do I need to inspect?

Any analysis that does not map cleanly to one of those questions should be an
internal signal, a response section, or an advanced drill-down tool.

## Tool tiers

### Tier 1: default workflow tools

These are the tools that should appear in the recommended default path:

- `get_minimal_context_tool`
- `detect_changes_tool`
- `get_review_context_tool`
- `get_architecture_overview_tool`
- `refactor_tool`
- `query_graph_tool`
- `semantic_search_nodes_tool`

This set keeps the user-facing shape small: one orientation tool, two review
tools, one architecture tool, one refactor tool, and two exploration tools.

### Tier 2: drill-down tools

These tools should remain available, but they should be reached through hints
from Tier 1 results rather than presented as equal first choices:

- `get_impact_radius_tool`
- `get_affected_flows_tool`
- `list_flows_tool`
- `get_flow_tool`
- `list_communities_tool`
- `get_community_tool`
- `get_hub_nodes_tool`
- `get_bridge_nodes_tool`
- `get_knowledge_gaps_tool`
- `get_surprising_connections_tool`
- `get_suggested_questions_tool`
- `traverse_graph_tool`

Their output is valuable when the user already has a target. They are not good
first choices for a user who is still deciding what kind of analysis is needed.

### Tier 3: metric and maintenance tools

These tools should be documented as expert or maintenance surfaces:

- `compute_sdp_metrics_tool`
- `detect_sdp_violations_tool`
- `compute_sap_metrics_tool`
- `detect_sap_violations_tool`
- `detect_adp_violations_tool`
- `embed_graph_tool`
- `generate_wiki_tool`
- `get_wiki_page_tool`
- `cross_repo_search_tool`
- `build_or_update_graph_tool`
- `run_postprocess_tool`
- `list_graph_stats_tool`
- `get_docs_section_tool`
- `apply_refactor_tool`
- `list_repos_tool`

They should stay callable because agents and advanced workflows need precise
controls. They should not be marketed as the ordinary analysis starting point.

## Analysis products

### Change analysis

<!-- constrained-by ../COMMANDS.md#mcp-tools -->

The primary change-analysis surface should be `detect_changes_tool`.

It should compose these signals:

- changed files and changed graph nodes
- impact radius
- affected flows
- direct and indirect test coverage
- hub and bridge proximity
- new or worsened architecture violations
- documentation drift through Markdown and cross-artifact edges

The output should include:

- a single risk level and score
- reason codes for the score
- recommended tests to run
- affected flows ranked by criticality
- documentation sections likely to need updates
- the next drill-down tool to call for each finding

Do not add separate top-level tools such as `recommend_tests_tool`,
`detect_docs_drift_tool`, or `score_pr_risk_tool` until this composed surface
proves too large to use. These are change-analysis sections, not separate user
questions.

### Architecture health analysis

<!-- constrained-by ../ARCHITECTURE.md#post-processing -->

The primary architecture-analysis surface should be
`get_architecture_overview_tool`.

It should summarize:

- community shape and cohesion
- cross-community coupling
- hub nodes
- bridge nodes
- knowledge gaps
- surprising connections
- ADP, SDP, and SAP violations

The specialized architecture tools should remain drill-downs. The overview
should return counts, top examples, reason codes, scoring-policy metadata, and
links to the exact tool that can expand each section.

### Refactor opportunity analysis

<!-- derived-from ../refactor-tool-suggest-spec.md#suggestion-types -->

The primary refactor-analysis surface should remain `refactor_tool`.

The existing `suggest` mode is the right shape because the user is asking for
actionable improvement candidates, not raw metrics. The analysis should keep
combining remove, move, split, and document suggestions behind one tool, with
evidence fields that explain why each candidate was ranked.

Do not add separate first-choice tools for dead code, move suggestions, split
suggestions, or documentation gaps. Those are categories inside the refactor
workflow.

### Exploration analysis

The primary exploration surfaces should remain `query_graph_tool` and
`semantic_search_nodes_tool`.

They serve different questions:

- `semantic_search_nodes_tool`: find the entity when the user does not know the
  exact name.
- `query_graph_tool`: inspect a known relationship such as callers, callees,
  imports, tests, or file contents.

`traverse_graph_tool` should be an advanced follow-up when the user wants a
neighborhood rather than a specific relationship.

## Tool profile plan

The current `dagayn serve --tools` allow-list is useful but too manual. Add
named tool profiles that expand to explicit allow-lists:

| Profile | Purpose | Tools |
| --- | --- | --- |
| `default` | General agent use with low tool-choice overhead | Tier 1 plus graph lifecycle basics |
| `review` | PR and local diff review | `default` plus impact, flow, and suggested-question drill-downs |
| `architecture` | Architecture mapping and cleanup | `default` plus community, flow, hub, bridge, gap, surprise, ADP, SDP, and SAP drill-downs |
| `refactor` | Refactor planning and safe apply flows | `default` plus large-function, impact, and apply-refactor surfaces |
| `full` | Current behavior for power users and compatibility | All registered tools |

Keep explicit `--tools` support. Profiles should be a convenience layer, not a
replacement for exact allow-lists.

## Selection rules for new analyses

A new top-level analysis tool is justified only when all of these are true:

1. The user question is stable and common.
2. The result has a distinct workflow owner.
3. The output cannot fit cleanly as a section of an existing Tier 1 tool.
4. The tool can return bounded output by default.
5. The tool exposes thresholds, reason codes, counts, and truncation state.
6. The tool can recommend its own next drill-down step.

If any condition fails, implement the analysis as a composed signal inside
`detect_changes_tool`, `get_architecture_overview_tool`, or `refactor_tool`.

## Implementation phases

### Phase 1: document the default path

Update command and usage docs so the recommended flow is:

1. start with `get_minimal_context_tool`
2. choose `detect_changes_tool`, `get_architecture_overview_tool`,
   `refactor_tool`, or `query_graph_tool`
3. follow response hints to Tier 2 tools only when needed

### Phase 2: add tool profiles

Add `dagayn serve --tool-profile default|review|architecture|refactor|full`.
The existing `--tools` flag should override or refine the profile.

Status: implemented. `dagayn serve` now defaults to the `default` profile,
while `--tool-profile full` preserves the legacy all-tools behavior.

### Phase 3: enrich change analysis

Extend `detect_changes_tool` with recommended tests, affected-flow ranking,
architecture-delta summaries, and documentation-drift hints.

Status: implemented. `detect_changes_tool` now returns `analysis_summary` in
standard mode and compact risk/test/flow/doc fields in minimal mode.

### Phase 4: enrich architecture overview

Extend `get_architecture_overview_tool` so it composes the current specialized
architecture tools into one bounded health report.

Status: implemented. `get_architecture_overview_tool` now returns
`architecture_health`, which composes community coupling, hubs, bridges,
knowledge gaps, surprising connections, and ADP/SDP/SAP signals.

### Phase 5: evaluate whether new top-level tools are needed

After the composed surfaces are in use, inspect repeated user requests and
agent traces. Add a new top-level analysis tool only if users repeatedly ask for
the same analysis and the existing Tier 1 tools cannot express it cleanly.

Status: implemented as a policy gate in this plan. No new top-level analysis
tool was added for the composed signals; the implementation keeps them inside
existing Tier 1 surfaces.

## Summary

<!-- derived-from #product-principle -->
<!-- derived-from #tool-tiers -->
<!-- derived-from #analysis-products -->
<!-- derived-from #tool-profile-plan -->

Dagayn should provide more analysis by composing existing graph signals into
fewer workflow tools, not by exposing every metric as a separate first-choice
tool. The default path should be small, the drill-down path should remain rich,
and every result should explain why the next tool is worth calling.
