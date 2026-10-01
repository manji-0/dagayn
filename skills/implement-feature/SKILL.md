---
name: implement-feature
description: Add new behavior in a repository dagayn has indexed — a flag, option, command, endpoint, handler, tool, query pattern, integration, or UI flow — by finding the extension point that already does something similar, making the smallest change in that pattern, then verifying blast radius, tests, and linked docs. Consult this skill before searching or editing whenever the user asks to add, implement, support, or extend something, including "make X also handle Y" or "add support for Z like the others".
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
   `commit_drift`, follow `recommended_action` (call `ensure_graph_tool()` when
   you need to wait for it).
2. **Find extension points** — pick the one path that fits:
   - Product language or a fuzzy concept →
     `semantic_search_nodes_tool(query="<concept>", detail_level="minimal")`.
   - A known symbol or file → `query_graph_tool` with `source_of`,
     `children_of`, `callers_of`, `callees_of`, or `file_summary`.
   - An existing user journey → `flow_tool(mode="list")`, then
     `flow_tool(mode="get", flow_name=...)` (or `flow_id=`) for one flow.
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
   `risk_level`, `reason_codes`, `recommended_tests`, `affected_flow_rankings`,
   and `documentation_update_candidates` (use the default `"standard"` for the
   nested `analysis_summary`). Check `tests_for` on the new symbols when
   coverage is unclear. For doc candidates, follow "Docs update after code
   change" in review-changes.
7. **Done when** the behavior is reachable from an existing flow or a
   deliberate new entry point, the high-risk blast radius is understood, and
   linked specs or runbooks are updated or explicitly deferred.

## Notes

- `callers_of` / `importers_of` take `depth` (up to 6) for transitive reach.
  Calls into packages target the package name and show up in `callees_of` as
  `unresolved_targets` (packages are not nodes).
- Where SCIP indexers are installed, `dagayn build --scip` makes call targets
  compiler-accurate; unresolved (`LOW`) calls are leads, not absence.
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
