---
name: refactor-safely
description: Plan and execute refactors in a repository dagayn indexes — renames, moves, splits, extractions, dead-code removal — by previewing every caller, import, test, and linked doc the change touches before editing. Use before any grep, sed, or edit when the user asks to rename, move, split, extract, or delete something, whether it is still used, or what breaks if X changes.
---

# Refactor Safely

A refactor is safe when you know every place that depends on what you change
before you change it. The graph gives you callers, importers, tests, and linked
docs in a few calls; the steps below use it to find candidates, preview the
edit set, and verify impact afterwards.

## Steps

1. **Orient**: `get_minimal_context_tool(task="<refactor goal>")`. If
   `graph_health.status` is `empty` or `sync.state` is `unbuilt` /
   `commit_drift`, follow `next` (the server has usually queued a
   refresh already); call `ensure_graph_tool()` only when you must wait for it.
2. **Find candidates**: `refactor_tool(mode="suggest")` answers with
   `findings`: `unused_symbol` (verified dead code, test fixtures left out),
   `complex_hotspot` (a long function changed in 5+ commits in 90 days), and
   `undocumented_surface` (a symbol other units use most, with no doc). An
   empty list means nothing worth doing. Use `mode="dead_code"` to see what
   the dead-code check left out and why.
3. **Map the blast radius** before touching public code:
   - `query_graph_tool(pattern="callers_of", target=..., depth=6)` (also
     `importers_of`) returns the transitive set in one call; stop when
     `reachability.state` is `complete`.
   - `tests_for`, `children_of`, and `source_of` for the symbol itself.
   - Linked docs: `docs_for` from code, `implementations_of` from a Markdown
     section (`<doc.md>::<section-slug>`).
   Unresolved (`LOW`) calls are possible hidden callers, not proof of absence.
   When the repository has SCIP indexers installed, `dagayn build --scip` makes
   call targets compiler-accurate and is worth running before a large rename.
4. **Preview renames**: `refactor_tool(mode="rename", old_name=..., new_name=...)`.
   `old_name` is resolved by search, so check `ambiguous`, `candidates`, and
   `warnings` (the preview uses the first match). Review every edit with
   `confidence: "medium"` (bare calls, imports by name) by reading its source.
5. **Apply** with `apply_refactor_tool`, which is on the advanced surface
   (`dagayn serve --tools all`). The `refactor_id` lives in the same `dagayn
   serve` session for 10 minutes: call it with `dry_run=True`, then again
   without. A separate `dagayn tool` process cannot see that id; over the CLI,
   apply the preview's `edits` yourself.
6. **Keep docs in step**: preserve `dagayn:` directives; update Markdown
   `implemented-by path::symbol` targets after a code rename and code
   `implements docs/spec.md#section` targets after a heading or path change.
7. **Verify**: `review_tool(mode="changes")` and read `findings`: a
   `dangling_reference` or `unchanged_caller` names a site the refactor
   missed, `contract_doc_not_updated` a doc to fix, and `tests_to_run` the
   tests to run. Drill into `mode="impact"` or `"affected_flows"` only when a
   finding points there.

## Judging suggestions

Rank results with the Highest / Medium / Low trust tiers in the installed
dagayn instructions (full rules: `get_docs_section_tool(section_name="trust")`).

Suggestions are **Medium** leads. Public APIs, dynamic dispatch, generated
code, test fixtures, and framework entry points often have no static caller,
so verify before removing or moving. Prefer suggestions that carry counts,
thresholds, callers, and reason codes over bare names.

### Function Concern Separation Profiles

A split suggestion's `evidence.concern_separation` is a
role-aware refactoring profile, not a verdict that the function is bad:

- Read `role` first: boundary functions, CLI handlers, adapters, coordinators,
  and test helpers may legitimately combine IO and orchestration.
- Compare `score` with `evidence.split_score_threshold`; above it, concern
  pressure justifies looking at extraction, not that extraction is safe.
- `reason_codes` name the pressure (callee spread, branches, side effects,
  implicit context); `evidence.purity_likelihood` is side-effect evidence, not
  proof of purity.
- Check `missingness`: thin source or call evidence calls for `source_of`
  before you recommend an edit. Start from `action`, and extract one cohesive
  decision or transformation before moving IO or changing signatures.

`line_count` is on the suggestion; `find_large_functions_tool` (advanced
surface) is only needed for a wider sweep.

## Output budget

Start every task with `get_minimal_context_tool`. Use `detail_level="minimal"`
unless it omits a field you need; aim for about five graph calls after the
graph is ready.

## CLI fallback

The default MCP surface already has `refactor_tool`, `review_tool`, and
`query_graph_tool`; use `dagayn tool` when an allow-list omitted them:

```bash
dagayn tool refactor_tool --arg mode='"suggest"' --arg limit=10
dagayn tool refactor_tool --arg mode='"rename"' --arg old_name='"old_symbol"' --arg new_name='"new_symbol"'
dagayn tool query_graph_tool --arg pattern='"callers_of"' --arg target='"src/app.py::handler"' --arg depth=6
dagayn tool query_graph_tool --arg pattern='"docs_for"' --arg target='"src/app.py::handler"'
```
