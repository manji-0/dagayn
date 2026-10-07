---
name: review-delta
description: Fast review of the working delta — the uncommitted edits, or the last commit on a clean tree — and what it reaches, with the dagayn graph. Use before git diff when the user asks to look over, sanity-check, or double-check what they just changed, staged, or are about to commit, or whether an edit broke callers or tests. For a whole branch with a merge recommendation use review-changes.
argument-hint: "[file or function name]"
---

# Review Delta

Review only what changed and what it reaches, so the review stays small enough
to run after every edit. `get_docs_section_tool(section_name="review-delta")`
has the compact workflow if you want it in context.

## Steps

<!-- constrained-by ../../docs/plans/REVIEW-TOOL-TARGET.md#target-contract -->

1. **Orient**: `get_minimal_context_tool(task="<review goal>")`.
2. **Refresh only when needed**: if `graph_health.status` is `empty` or
   `sync.state` is `unbuilt` / `commit_drift`, follow `next`. If
   the working tree is newer than the graph (edit hooks only queue an async
   update; `dagayn queue status` shows it), call
   `ensure_graph_tool(force=True)`. Otherwise skip ensure and go straight to
   review. Do not call `ensure_graph_tool(force=True)` on every review: a
   forced refresh re-parses the changed files each time.
3. **Get the findings**: `review_tool(mode="changes",
   detail_level="minimal")`. With no `base`, a tree with uncommitted edits to
   tracked files reviews only that work in progress (`HEAD`); a clean tree
   reviews the last commit (`HEAD~1`). Untracked files count either way; pass
   `base=` to widen it, or `changed_files=` to scope it to some files. Read
   `findings`: each names a place to look that the diff does not show
   (`dangling_reference`, `unchanged_caller`, `contract_doc_not_updated`,
   `bridge_touched`, `unstable_dependency`, `untested_change`, `tests_to_run`),
   its `evidence` or `sites`, and an `action`. An empty list means nothing beyond the diff needs
   checking; say so and stop. The score-first fields (`analysis_summary`,
   `risk_level`, ...) are deprecated and only in `detail_level="verbose"`.
4. **Fetch source only for what can change the verdict**:
   `review_tool(mode="context")` for the change set, `query_graph_tool(
   pattern="source_of")` for one symbol.
5. **Check each finding**: open the `sites` of a `dangling_reference` or
   `unchanged_caller` with `source_of`; read the doc behind a
   `contract_doc_not_updated`; check the other side of a `bridge_touched`;
   ask whether an `unstable_dependency` can point the other way.
   `review_tool(mode="impact")` (`max_depth`, default 2, is the hop count)
   only when a behavior change has no finding but may still reach callers.
6. **Check coverage**: run the `command` of each `tests_to_run` finding;
   confirm an `untested_change` with `query_graph_tool(pattern="tests_for")`
   before flagging it.

## Report

- **Summary**: one line.
- **Risk**: low / medium / high, your judgment from the confirmed findings
  (their kinds and counts).
- **Issues**: bugs (confirmed with `source_of`), missing tests, style.
- **Blast radius**: impacted files and functions.
- **Docs**: linked docs to update, or explicit deferrals.

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

`contract_doc_not_updated` findings and authored doc links are not optional
reading: update them (see the "Docs update
after code change" steps in review-changes) or list them as deferred. Before
claiming something is missing, read `zero_result_reason`, `next`,
and `missingness`, and narrow truncated results first.

## CLI fallback

```bash
dagayn tool review_tool --arg mode='"changes"' --arg detail_level='"minimal"'
dagayn tool review_tool --arg mode='"context"' --arg detail_level='"minimal"'
dagayn tool review_tool --arg mode='"impact"' --arg detail_level='"minimal"'
dagayn tool query_graph_tool --arg pattern='"tests_for"' --arg target='"src/app.py::handler"'
```
