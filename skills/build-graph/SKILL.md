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

1. **Check graph status** with `get_minimal_context_tool` (or
   `list_graph_stats_tool` when the advanced surface is available).
   - If `graph_health.status` is `empty` / `last_updated` is null, proceed with
     bootstrap.
   - If the graph exists, prefer hooks/`dagayn update` for routine refresh.

2. **Bootstrap or refresh**:
   - Default MCP surface (preferred): `ensure_graph_tool()` for first-time /
     empty graphs, or `ensure_graph_tool(force=True)` for a safe incremental
     refresh. This always uses `postprocess="minimal"` and
     `local_embedding="none"`.
   - Maintenance / advanced surface: `build_or_update_graph_tool` when you need
     full postprocess, embeddings, or explicit rebuild controls:
     - First-time: `build_or_update_graph_tool(full_rebuild=True, local_embedding="none")`
     - Routine: `build_or_update_graph_tool(local_embedding="none")`
   - Do not run embedding-enabled full rebuilds as a routine verification step.
     When the MCP server was started with `--local-embedding`, omitting
     `local_embedding` on `build_or_update_graph_tool` may inherit that mode and
     trigger a large embedding refresh. Pass `local_embedding="bge-m3"` only when
     the task explicitly requires embedding quality or hybrid-search freshness,
     and state that reason first.

3. **Verify** with `get_minimal_context_tool` (or `list_graph_stats_tool`) and
   report:
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
dagayn tool ensure_graph_tool
dagayn tool list_graph_stats_tool
dagayn tool build_or_update_graph_tool --arg full_rebuild=true
dagayn tool run_postprocess_tool --arg fts=true
```

## Efficiency Rules

- Prefer `ensure_graph_tool()` on the default MCP surface; use incremental
  `build_or_update_graph_tool()` only when maintenance options are required.
- For parser, flow, documentation-edge, or review verification, keep
  `local_embedding="none"` so hooks and local embedding refresh do not turn a
  graph check into an expensive embedding rebuild.
- Use `postprocess="minimal"` while iterating; run full postprocess only when
  flow/community freshness matters.
- Report node, edge, file, language, and error counts instead of reading the
  graph database or generated artifacts directly.
