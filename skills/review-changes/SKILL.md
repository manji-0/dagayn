---
name: review-changes
description: Review of a branch or set of commits with the dagayn graph — references the change left dangling, unedited callers, missing tests, tests to run, and stale linked docs — ending in a merge recommendation. Use when the user asks to review their branch or changes before merging, what a change breaks, or which tests to run. For only the uncommitted delta use review-delta; for a PR number or link use review-pr.
---

# Review Changes

The graph already knows who calls the changed code, which flows pass through
it, which tests cover it, and which docs are linked to it. Read the
`findings` first and open source only where it can change the verdict.

## Steps

<!-- constrained-by ../../docs/plans/REVIEW-TOOL-TARGET.md#target-contract -->

1. **Orient**: `get_minimal_context_tool(task="<review goal>")`. If
   `graph_health.status` is `empty` or `sync.state` is `unbuilt` /
   `commit_drift`, follow `next`; call `ensure_graph_tool()`
   only when you must wait for the refresh. A dirty worktree is not refreshed
   automatically: use `ensure_graph_tool(force=True)` when uncommitted edits
   matter and hooks have not caught up.
2. **Summarize the change**: `review_tool(mode="changes", base=...)`. Change
   detection is the `base` diff plus staged, unstaged, and untracked files
   (with no `base`: `HEAD` while tracked files have uncommitted edits, else
   `HEAD~1`), so pass the merge base (`git merge-base main HEAD`) to
   review a whole branch; plain `base="main"` also counts commits that landed
   on `main` after the branch point, as reversed changes. An explicit
   `changed_files` list scopes the review to those files. Read `findings`:
   each is one claim to check that the diff does not show, with a `kind`, the
   place to look (`qualified_name` / `file`, or `targets`), `evidence` or
   `sites`, and an `action`. An empty list (summary "Nothing beyond the diff
   needs checking.") means the diff is the whole review. `findings_omitted`
   counts what each kind's cap of 10 left out. `detail_level="standard"` adds
   `changed_functions` and `affected_flows`.
3. **Work through the findings**, each with its own check:
   - `dangling_reference` / `unchanged_caller`: open each listed site with
     `query_graph_tool(pattern="source_of")`; a site still using the old name
     or signature is a bug.
   - `contract_doc_not_updated`: read the linked section (step 7).
   - `bridge_touched`: check the other side of the bridge still matches.
   - `unstable_dependency`: the change makes a unit depend on a less stable
     one; ask whether the dependency can point the other way (move what is
     needed into the stable unit, or an interface it owns).
   - `untested_change`: confirm with `query_graph_tool(pattern="tests_for")`,
     then suggest a test.
   - `tests_to_run`: run its `command`, or list it in the review.
4. **Fetch source only where needed**: `review_tool(mode="context")` for
   change-set snippets, `query_graph_tool(pattern="source_of")` for one symbol.
5. **Drill down only on a concrete question**: `review_tool(mode="impact")` for
   blast radius (`max_depth` sets the hops), `mode="affected_flows"` for the entry points that reach the change,
   `query_graph_tool(pattern="tests_for")` for uncertain coverage, and
   `callers_of` with `depth` up to 6 for transitive callers.
6. **Follow documentation links**: `docs_for` on changed code symbols,
   `implementations_of` on changed Markdown sections (`<doc.md>::<section-slug>`).
7. **Docs update after code change**: when a `contract_doc_not_updated`
   finding or authored `docs_for` links appear, don't stop at "docs may be
   stale":
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
role), and suggested improvements; end with a merge recommendation. The risk
label is your judgment from the confirmed findings (their kinds and counts,
e.g. "2 dangling_reference, 1 untested_change"); the tool no longer rates the
change. With no findings, say the diff needs nothing beyond itself.

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

- Tie each risk label to evidence: a finding and its sites, a blast-radius
  count, an affected flow, a changed public surface, or a dependency direction
  change.
- Confirm behavior with `source_of` before calling something a bug; graph
  structure alone shows reach, not correctness.
- `CROSS_ARTIFACT` documentation roles are typed evidence, not duplicate
  inverse facts: `implemented_by` / `implements_contract` are authored
  contracts, explanatory roles are usually `extracted`, and
  `heuristic_reachable` stays tentative. Cite the role and the query used.
- Before claiming something is absent, read `zero_result_reason`,
  `next`, and `missingness`; report `truncated` /
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
