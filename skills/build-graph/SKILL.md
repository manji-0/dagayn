---
name: build-graph
description: Build or update the code review knowledge graph. Run this first to initialize, or let hooks keep it updated automatically.
argument-hint: "[full]"
---

# Build Graph

Build or incrementally update the persistent code knowledge graph for this repository.

<!-- dagayn skill embedding context -->
## Installed Search Mode

This packaged skill is mode-neutral. `dagayn install` rewrites this section with
the selected embedding mode so graph builds refresh the right retrieval indexes.
<!-- /dagayn skill embedding context -->

## Steps

1. **Check graph status** by calling the `list_graph_stats_tool` MCP tool.
   - If the graph has never been built (last_updated is null), proceed with a full build.
   - If the graph exists, proceed with an incremental update.

2. **Build the graph** by calling the `build_or_update_graph_tool` MCP tool:
   - For first-time setup: `build_or_update_graph_tool(full_rebuild=True)`
   - For updates: `build_or_update_graph_tool()` (incremental by default)
   - Do not pass `local_embedding` just to match the installed mode. When the
     MCP server was started with `--local-embedding low|high`, the tool inherits
     that preset. Pass `local_embedding="none"` only for a deliberate FTS-only
     refresh, or pass an explicit preset only when overriding the server default.

3. **Verify** by calling `list_graph_stats_tool` again and report the results:
   - Number of files parsed
   - Number of nodes and edges created
   - Languages detected
   - Any errors encountered

## When to Use

- First time setting up the graph for a repository
- After major refactoring or branch switches
- If the graph seems stale or out of sync
- Before semantic search evaluation, wiki generation, or cross-repo comparison
- The graph auto-updates via hooks on edit/commit, so manual builds are rarely needed

## Notes

- The graph is stored as a SQLite database (`.dagayn/graph.db`) in the repo root
- Binary files, generated files, and patterns in `.dagaynignore` are skipped
- Supported languages evolve with the parser registry; check `README.md`
  "Supported languages and file types" rather than relying on this skill as the
  authoritative language list.

## CLI Fallback

Use MCP tools first. If the current MCP server profile does not expose a tool,
run the same implementation through the CLI without restarting the agent:

```bash
dagayn tool list_graph_stats_tool
dagayn tool build_or_update_graph_tool --arg full_rebuild=true
dagayn tool run_postprocess_tool --arg fts=true
```

## Efficiency Rules

- Use incremental `build_or_update_graph_tool()` unless the graph is empty,
  branch state changed heavily, or new files are missing from graph queries.
- Use `postprocess="minimal"` while iterating; run full postprocess only when
  flow/community freshness matters.
- Report node, edge, file, language, and error counts instead of reading the
  graph database or generated artifacts directly.
