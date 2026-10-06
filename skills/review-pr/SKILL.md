---
name: review-pr
description: Review a pull request by number or link, or someone else's branch, with the dagayn graph — findings across every commit (dangling references, unedited callers), missing tests, breaking public-API changes, linked docs — and write a structured PR review. Use when the user hands over a PR or asks for a pre-merge review of another author's work. For your own branch use review-changes.
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

<!-- constrained-by ../../docs/plans/REVIEW-TOOL-TARGET.md#target-contract -->

1. **Orient**: `get_minimal_context_tool(task="<PR review>")`.
2. **Check out the PR and find its base**. `review_tool` diffs `base` against
   the current working tree, so the PR branch must be checked out
   (`gh pr checkout <number>` or `git switch <branch>`). Use the merge base as
   `base`: `git merge-base main HEAD`. Plain `base="main"` is only right when
   the branch is up to date with `main`; otherwise commits that landed on
   `main` after the branch point show up as reversed changes.
3. **Refresh only when needed**: if `graph_health.status` is `empty` or
   `sync.state` is `unbuilt` / `commit_drift` (e.g. right after the checkout),
   follow `recommended_action`, or call `ensure_graph_tool()` to wait for the
   refresh (it already re-syncs a moved HEAD; `force=True` is only for
   uncommitted edits). Otherwise skip ensure and go to review.
   Do not call `ensure_graph_tool(force=True)` on every PR when the graph is
   already current — it re-parses the changed files each time.
4. **Get the findings**: `review_tool(mode="changes", base="<merge-base>")`.
   Read `findings` first: each is one claim to check that the diff does not
   show, with a `kind`, the place to look, `evidence` or `sites`, and an
   `action`. Kinds: `dangling_reference` (a removed, renamed, or moved symbol
   still referenced outside the PR), `unchanged_caller` (a new required
   parameter or fewer parameters, callers not edited),
   `contract_doc_not_updated`, `bridge_touched` (one side of a manifest,
   Terraform, or FFI bridge), `untested_change`, and `tests_to_run` (with a
   `command`). Each kind keeps 10; `findings_omitted` counts the rest. An empty
   list means nothing beyond the diff needs checking. The default
   `detail_level="standard"` adds `changed_functions` and `affected_flows`;
   `"minimal"` drops them. The score-first fields (`analysis_summary`,
   `risk_level`, `review_priorities`, ...) are deprecated and only in
   `"verbose"`.
5. **Read only the parts the findings name**: `review_tool(mode="context",
   base="<merge-base>")` for change-set snippets; for one `qualified_name`,
   `query_graph_tool(pattern="source_of")`. Open a whole file only when that
   span is truncated, stale, or you need its neighbors.
6. **Confirm each finding, then drill down where it raises a question**:
   - Blast radius: `review_tool(mode="impact", base=...)`; flows:
     `review_tool(mode="affected_flows", base=...)` or `flow_tool(mode="get",
     flow_name=...)`.
   - Callers of a changed public function:
     `query_graph_tool(pattern="callers_of", target=..., depth=3)` (up to 6) —
     check `reachability` before calling it the full set. Call targets marked
     `resolved_by: "scip"` are index-backed; when call accuracy matters and SCIP
     indexers are installed, `dagayn build --scip` settles the rest.
   - Coverage: run or list each `tests_to_run` `command`; confirm an
     `untested_change` with `query_graph_tool(pattern="tests_for")`.
   - Renamed, moved, or reshaped symbols: open every site of a
     `dangling_reference` or `unchanged_caller` finding with `source_of`.
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
   - Overall risk: Low / Medium / High (your judgment from the confirmed
     findings: their kinds and counts)
   - Findings: N confirmed, by kind (or "nothing beyond the diff")
   - Tests to run / untested changes

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

Rank results with the Highest / Medium / Low trust tiers in the installed
dagayn instructions (full rules: `get_docs_section_tool(section_name="trust")`).

- A finding is a claim to check, not a verdict. Confirm a behavioral issue
  with `source_of` or a test before reporting it in the review.
- When a result is bounded (`truncated`, `total`, thresholds) say so in the
  review; when a query comes back empty, report `zero_result_reason` and
  `next_action` rather than concluding the thing doesn't exist.
- On large PRs, triage from the findings and cap drill-downs to the first
  few per kind; report `findings_omitted` and list the rest as residual
  uncertainty.
- For `contract_doc_not_updated` findings and authored doc links, follow "Docs update after code change" in review-changes,
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
