---
name: implement-feature
description: Add behavior in a repository dagayn indexes — a flag, option, command, endpoint, handler, tool, integration, or UI flow — by finding the extension point that already does something similar, making the smallest change in that pattern, and checking blast radius, tests, and linked docs. Use when the user asks to add, implement, support, or extend something.
argument-hint: "[feature goal]"
---

# Implement Feature

Most feature bugs come from extending the wrong place. Find the existing
pattern the feature belongs to first, edit only that surface, then let the
graph show what the edit reaches.

<!-- dagayn skill embedding context -->
## Installed Search Mode

This packaged skill is mode-neutral. `dagayn install` rewrites this section with
the selected embedding mode so related-code search matches the installed
retrieval setup.
<!-- /dagayn skill embedding context -->

## Steps

1. **Orient**: `get_minimal_context_tool(task="<feature goal>")`. If
   `graph_health.status` is `empty` or `sync.state` is `unbuilt` /
   `commit_drift`, follow `next` (call `ensure_graph_tool()` when
   you need to wait for it).
2. **Find extension points** — pick the one path that fits:
   - Product language or a fuzzy concept →
     `semantic_search_nodes_tool(query="<concept>", detail_level="minimal")`.
   - A known symbol or file → `query_graph_tool` with `source_of`,
     `children_of`, `callers_of`, `callees_of`, or `file_summary`.
   - An existing user journey → `flow_tool(mode="entry_points",
     target=<a symbol on it>)` for the commands or handlers that already
     reach it, with the call chain from each.
   - A spec → `query_graph_tool(pattern="implementations_of",
     target="<doc.md>::<section-slug>")`, or `docs_for` from nearby code.
3. **Read only that surface**: `source_of` for one symbol, or
   `review_tool(mode="context")` for a change set, instead of whole files.
4. **Implement the smallest change** in the existing pattern (same module, same
   flow entry, same interface style). Keep nearby `dagayn:` directives and
   update their targets if you rename anything.
5. **Refresh the graph**: edit hooks only queue an async update
   (`dagayn queue status` shows it), so call `ensure_graph_tool(force=True)`
   when the review must see your edit.
6. **Verify**: `review_tool(mode="changes", detail_level="minimal")` and read
   `findings`. Fix each `dangling_reference` or `unchanged_caller` site, run
   each `tests_to_run` `command`, and add a test for an `untested_change` (or
   confirm one with `tests_for`). For `contract_doc_not_updated`, follow "Docs
   update after code change" in review-changes. An empty list means nothing
   beyond the diff needs checking.
7. **Done when** the behavior is reachable from an existing flow or a
   deliberate new entry point, every finding is fixed or explained, and
   linked specs or runbooks are updated or explicitly deferred.

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

## Notes

- `callers_of` / `importers_of` take `depth` (up to 6) for transitive reach.
  Calls into packages target the package name and show up in `callees_of` as
  `unresolved_targets` (packages are not nodes).
- Where SCIP indexers are installed, `dagayn build --scip` makes call targets
  compiler-accurate; unresolved (`LOW`) calls are **Low** trust leads, not
  absence.
- Keep discovery to about three search or relationship calls before editing,
  and prefer one `review_tool(mode="changes")` afterwards over repeated impact
  drills.

## CLI fallback

```bash
dagayn tool get_minimal_context_tool --arg 'task="add billing webhook"'
dagayn tool semantic_search_nodes_tool --arg query='"webhook handler"' --arg detail_level='"minimal"'
dagayn tool query_graph_tool --arg pattern='"source_of"' --arg target='"src/billing.py::handle_webhook"'
dagayn tool review_tool --arg mode='"changes"' --arg detail_level='"minimal"'
```
