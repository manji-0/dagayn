---
name: review-changes
description: Risk-ranked review of a branch or set of commits with the dagayn graph — blast radius, affected flows, missing tests, and stale linked docs — ending in a merge recommendation. Use when the user asks to review their branch or changes before merging, what a change breaks, or which tests to run. For only the uncommitted delta use review-delta; for a PR number or link use review-pr.
---

# Review Changes

The graph already knows who calls the changed code, which flows pass through
it, which tests cover it, and which docs are linked to it. Read that summary
first and open source only where it can change the verdict.

## Steps

1. **Orient**: `get_minimal_context_tool(task="<review goal>")`. If
   `graph_health.status` is `empty` or `sync.state` is `unbuilt` /
   `commit_drift`, follow `recommended_action`; call `ensure_graph_tool()`
   only when you must wait for the refresh. A dirty worktree is not refreshed
   automatically: use `ensure_graph_tool(force=True)` when uncommitted edits
   matter and hooks have not caught up.
2. **Summarize the change**: `review_tool(mode="changes", base=...)`. Change
   detection is the `base` diff plus staged, unstaged, and untracked files
   (with no `base`: `HEAD` while tracked files have uncommitted edits, else
   `HEAD~1`), so pass the merge base (`git merge-base main HEAD`) to
   review a whole branch; plain `base="main"` also counts commits that landed
   on `main` after the branch point, as reversed changes. At `detail_level="minimal"` the result is flat: read `risk_level`,
   `reason_codes`, `recommended_tests`, `affected_flow_rankings`,
   `documentation_update_candidates`, `stability_contracts`, `guidance`,
   `architecture_delta`, and `next_drill_downs`. Use the default `"standard"`
   when you need the nested `analysis_summary` (it adds hotspot and
   cross-artifact proximity).
3. **Fetch source only where needed**: `review_tool(mode="context")` for
   change-set snippets, `query_graph_tool(pattern="source_of")` for one symbol.
4. **Drill down only on a concrete question**: `review_tool(mode="impact")` for
   blast radius (`max_depth` sets the hops), `mode="affected_flows"` for flows,
   `query_graph_tool(pattern="tests_for")` for uncertain coverage, and
   `callers_of` with `depth` up to 6 for transitive callers.
5. **Follow documentation links**: `docs_for` on changed code symbols,
   `implementations_of` on changed Markdown sections (`<doc.md>::<section-slug>`).
6. **Suggest tests** for changed behavior that nothing covers.
7. **Docs update after code change**: when doc candidates or authored
   `docs_for` links appear, don't stop at "docs may be stale":
   1. Rank them: `implemented_by` / `implements_contract` first, then
      `explained_by` / `has_runbook` / `problem_described_by`, then weaker
      `extracted` / `heuristic_reachable` hits.
   2. Fetch only the sections that affect the decision (`source_of` on the
      DocSection).
   3. Edit them with the `writing-markdown-document` skill, keeping `dagayn:`
      directives and heading slugs accurate.
   4. `ensure_graph_tool(force=True)`, then re-check `docs_for` /
      `implementations_of` on the touched paths.
   If the docs work is deferred, list each doc path and role in the review.

## Output

Group findings by risk (high / medium / low), each with what changed and why it
matters, test coverage, documentation updates required or deferred (path +
role), and suggested improvements; end with a merge recommendation.

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

- Tie each risk label to a metric: `reason_codes`, blast-radius counts, an
  affected flow, a test gap, a changed public surface, or a dependency
  direction change.
- Confirm behavior with `source_of` before calling something a bug; graph
  structure alone shows reach, not correctness.
- `CROSS_ARTIFACT` documentation roles are typed evidence, not duplicate
  inverse facts: `implemented_by` / `implements_contract` are authored
  contracts, explanatory roles are usually `extracted`, and
  `heuristic_reachable` stays tentative. Cite the role and the query used.
- Before claiming something is absent, read `zero_result_reason`,
  `next_action`, `answerability`, and `missingness`; report `truncated` /
  `total` when a result is incomplete. Unresolved (`LOW`) calls can hide
  callers — `dagayn build --scip` settles them where SCIP indexers exist.
- Function concern profiles (`concern_separation`, the
  `function_concern_pressure` reason code) come from
  `refactor_tool(mode="suggest")`, not from review, and are
  not correctness evidence.
  Do not report a function concern profile as a bug by itself; if you ran it,
  mention it as a refactor lead.

## Output budget

Start with `get_minimal_context_tool`, stay on the `changes` summary until it
raises a concrete question, and aim for about five graph calls after the graph
is ready.

## CLI fallback

```bash
dagayn tool review_tool --arg mode='"changes"' --arg base='"main"' --arg detail_level='"minimal"'
dagayn tool review_tool --arg mode='"context"' --arg detail_level='"minimal"'
dagayn tool review_tool --arg mode='"impact"' --arg 'changed_files=["src/app.py"]' --arg detail_level='"minimal"'
dagayn tool query_graph_tool --arg pattern='"source_of"' --arg target='"src/app.py::handler"'
dagayn tool query_graph_tool --arg pattern='"docs_for"' --arg target='"src/app.py::handler"'
```
