---
name: review-pr
description: Review a pull request (by number, URL, or branch name) with the dagayn knowledge graph — risk ranking, blast radius across every commit in the PR, missing tests, breaking public-API changes, and linked docs — and write a structured PR review. Use this whenever the user hands you a PR number or link, says "review PR 123", "review this branch against main", or asks for a pre-merge review of someone else's branch. For your own uncommitted edits use review-delta; for a general branch review with a merge recommendation use review-changes.
argument-hint: "[PR number or branch name]"
---

# Review PR

A PR review has to cover every commit on the branch, not just the last one,
and has to compare against where the branch actually left `main`. Get that
base right first; then let the graph rank what deserves attention.

<!-- dagayn skill embedding context -->
## Installed Search Mode

This packaged skill is mode-neutral. `dagayn install` rewrites this section with
the selected embedding mode so related-code search matches the installed
retrieval setup.
<!-- /dagayn skill embedding context -->

## Steps

1. **Orient**: `get_minimal_context_tool(task="<PR review>")`.
2. **Check out the PR and find its base**. `review_tool` diffs `base` against
   the current working tree, so the PR branch must be checked out
   (`gh pr checkout <number>` or `git switch <branch>`). Use the merge base as
   `base`: `git merge-base main HEAD`. Plain `base="main"` is only right when
   the branch is up to date with `main`; otherwise commits that landed on
   `main` after the branch point show up as reversed changes.
3. **Refresh only when needed**: if `graph_health.status` is `empty` or
   `sync.state` is `unbuilt` / `commit_drift` (e.g. right after the checkout),
   follow `recommended_action`, or call `ensure_graph_tool(force=True)` to wait
   for the refresh. Otherwise skip ensure and go to review.
   Do not call `ensure_graph_tool(force=True)` on every PR when the graph is
   already current — it re-parses the changed files each time.
4. **Rank the change**: `review_tool(mode="changes", base="<merge-base>")`. At
   the default `detail_level="standard"` read `analysis_summary`: reason codes,
   `recommended_tests`, affected-flow rankings, documentation update
   candidates, hotspot proximity, and architecture risks. At `"minimal"` the
   same fields are flattened to the top level (`risk_level`, `reason_codes`,
   `recommended_tests`, `affected_flow_rankings`,
   `documentation_update_candidates`, `review_priorities`, `next_drill_downs`)
   and lists are capped at five.
5. **Read only the risky parts**: `review_tool(mode="context",
   base="<merge-base>")` for change-set snippets; for one `qualified_name`,
   `query_graph_tool(pattern="source_of")`. Open a whole file only when that
   span is truncated, stale, or you need its neighbors.
6. **Drill into the highest-risk changes**:
   - Blast radius: `review_tool(mode="impact", base=...)`; flows:
     `review_tool(mode="affected_flows", base=...)` or `flow_tool(mode="get",
     flow_name=...)`.
   - Callers of a changed public function:
     `query_graph_tool(pattern="callers_of", target=..., depth=3)` (up to 6) —
     check `reachability` before calling it the full set. Call targets marked
     `resolved_by: "scip"` are index-backed; when call accuracy matters and SCIP
     indexers are installed, `dagayn build --scip` settles the rest.
   - Coverage: start with `recommended_tests`, confirm doubtful cases with
     `query_graph_tool(pattern="tests_for")`.
   - Renamed or moved symbols: check every caller was updated.
   - Docs: `docs_for` on changed code, `implementations_of` on changed
     Markdown sections. Markdown `implemented-by` means the doc owns the
     contract; code `implements` means the code declares conformance;
     `explained-by` / `has-runbook` / `problem-described-by` docs may now be
     stale. Weigh each result by `evidence_type` (`authored`, `extracted`,
     `heuristic_reachable`).
7. **Write the review**:

   ```
   ## PR Review: <title>

   ### Summary
   <1-3 sentences>

   ### Risk Assessment
   - Overall risk: Low / Medium / High (and the metric behind it)
   - Blast radius: X files, Y functions impacted
   - Test coverage: N of M changed functions covered

   ### File-by-File Review
   #### <file_path>
   - Changes / Impact (who depends on it) / Issues

   ### Missing Tests
   - <function> in <file>

   ### Docs to Update (or deferred, with path + role)

   ### Recommendations
   1. <actionable suggestion>
   ```

## Judgment

<!-- dagayn trust tiers -->
Reach comes from the graph, correctness from `source_of`, and user-visible
effect from a reproduction; keep them apart in a claim. Full rules:
`get_docs_section_tool(section_name="trust")`.

- **Highest** — on a current graph (`sync.state` is `commit_synced` or
  `worktree_ahead`): `HIGH` and `EXTRACTED` edges, whether the parser or a
  SCIP index (`resolved_by: "scip"`) settled them; `source_of` spans;
  authored doc contracts (`evidence_type=authored`).
- **Medium** — structure, not correctness: `MEDIUM` (inferred) edges,
  `reason_codes`, blast radius, flows, communities, metrics, suggestions,
  explanatory doc links, and search hits.
- **Low** — a hypothesis until `source_of` or a reproduction confirms it:
  `LOW` edges, `heuristic_reachable` links, a file-level `tests_for` of 0,
  `truncated` or `ambiguous` results, and answers about files changed since
  the graph was built (`sync.state` `commit_drift` or `worktree_behind`).
<!-- /dagayn trust tiers -->

- Risk labels prioritize; they don't prove. Confirm a behavioral issue with
  `source_of` or a test before reporting it as a finding.
- When a result is bounded (`truncated`, `total`, thresholds) say so in the
  review; when a query comes back empty, report `zero_result_reason` and
  `next_action` rather than concluding the thing doesn't exist.
- On large PRs, triage from the summary and cap drill-downs to the top few
  impacted functions per risk area; list the rest as residual uncertainty.
- For doc candidates, follow "Docs update after code change" in review-changes,
  or list each deferred doc path and role.
- Use `semantic_search_nodes_tool` for fuzzy related-code questions; for exact
  renamed symbols, relationship queries (or a literal `rg`) are more reliable.

## CLI fallback

```bash
BASE=$(git merge-base main HEAD)
dagayn tool review_tool --arg mode='"changes"' --arg base="\"$BASE\"" --arg detail_level='"minimal"'
dagayn tool review_tool --arg mode='"context"' --arg base="\"$BASE\"" --arg detail_level='"minimal"'
dagayn tool query_graph_tool --arg pattern='"source_of"' --arg target='"src/app.py::handler"'
dagayn tool query_graph_tool --arg pattern='"callers_of"' --arg target='"src/app.py::handler"' --arg depth=3
dagayn tool query_graph_tool --arg pattern='"docs_for"' --arg target='"src/app.py::handler"'
```
