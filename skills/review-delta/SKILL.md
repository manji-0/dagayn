---
name: review-delta
description: Fast review of the working delta — the last commit plus uncommitted edits — and what it reaches, with the dagayn graph. Use before git diff when the user asks to look over, sanity-check, or double-check what they just changed, staged, or are about to commit, or whether an edit broke callers or tests. For a whole branch with a merge recommendation use review-changes.
argument-hint: "[file or function name]"
---

# Review Delta

Review only what changed and what it reaches, so the review stays small enough
to run after every edit. `get_docs_section_tool(section_name="review-delta")`
has the compact workflow if you want it in context.

## Steps

1. **Orient**: `get_minimal_context_tool(task="<review goal>")`.
2. **Refresh only when needed**: if `graph_health.status` is `empty` or
   `sync.state` is `unbuilt` / `commit_drift`, follow `recommended_action`. If
   the working tree is newer than the graph (edit hooks only queue an async
   update; `dagayn queue status` shows it), call
   `ensure_graph_tool(force=True)`. Otherwise skip ensure and go straight to
   review. Do not call `ensure_graph_tool(force=True)` on every review: a
   forced refresh re-parses the changed files each time.
3. **Get the risk summary**: `review_tool(mode="changes",
   detail_level="minimal")`. The default `base="HEAD~1"` covers the last commit
   plus staged, unstaged, and untracked files; pass `base=` to widen it. Read
   the flat fields: `risk_level`, `reason_codes`, `changed_node_count`,
   `impacted_node_count`, `recommended_tests`, `affected_flow_rankings`,
   `documentation_update_candidates`, `architecture_delta`, `next_drill_downs`
   (the default `"standard"` nests the full set under `analysis_summary`).
4. **Fetch source only for what can change the verdict**:
   `review_tool(mode="context")` for the change set, `query_graph_tool(
   pattern="source_of")` for one symbol.
5. **Check the blast radius** when the summary points at it:
   `review_tool(mode="impact")` (`max_depth`, default 2, is the hop count).
   Look for callers that depend on changed signatures or behavior, subclasses
   of changed classes, files with many dependents, and linked docs
   (`docs_for` on code, `implementations_of` on a Markdown section).
6. **Check coverage**: take `recommended_tests` first; use
   `query_graph_tool(pattern="tests_for")` only where coverage is unclear, and
   flag changed functions nothing tests.

## Report

- **Summary**: one line.
- **Risk**: low / medium / high, with the metric behind it.
- **Issues**: bugs (confirmed with `source_of`), missing tests, style.
- **Blast radius**: impacted files and functions.
- **Docs**: linked docs to update, or explicit deferrals.

## Evidence

Rank results with the Highest / Medium / Low trust tiers in the installed
dagayn instructions (full rules: `get_docs_section_tool(section_name="trust")`).

Doc candidates are not optional reading: update them (see the "Docs update
after code change" steps in review-changes) or list them as deferred. Before
claiming something is missing, read `zero_result_reason`, `next_action`,
`answerability`, and `missingness`, and narrow truncated results first.

## CLI fallback

```bash
dagayn tool review_tool --arg mode='"changes"' --arg detail_level='"minimal"'
dagayn tool review_tool --arg mode='"context"' --arg detail_level='"minimal"'
dagayn tool review_tool --arg mode='"impact"' --arg detail_level='"minimal"'
dagayn tool query_graph_tool --arg pattern='"tests_for"' --arg target='"src/app.py::handler"'
```
