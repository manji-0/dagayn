---
name: build-graph
description: Build, refresh, or repair the dagayn graph — first bootstrap, catching up after a pull or branch switch, missing flows or communities, a rebuild, or SCIP-settled call targets (`dagayn build --scip`). Use when the graph is empty or stale, graph tools return nothing, or the user asks to index or rebuild. In a linked git worktree or jj workspace use worktree-sync instead.
argument-hint: "[full]"
---

# Build Graph

Most of the time the graph keeps itself current: edit hooks queue incremental
updates and session start runs `dagayn session prepare`. Build explicitly when
the graph is missing, has fallen behind, or lacks the post-processing a question
needs. Pass `full` (or ask for a clean rebuild) to run `dagayn build`, with
`--force-full-build` to start from an empty database.

<!-- dagayn skill embedding context -->
## Installed Search Mode

This packaged skill is mode-neutral. `dagayn install` rewrites this section with
the selected embedding mode so graph builds refresh the right retrieval indexes.
<!-- /dagayn skill embedding context -->

## Steps

1. **Check the state** with `get_minimal_context_tool` (or `dagayn status`,
   which prints `Graph state:` and embedding coverage). Read
   `graph_health.status` (`ok` / `degraded` / `empty`) and `sync.state`:
   - `unbuilt` (empty graph) or `commit_drift` (HEAD moved): follow
     `next` (`ensure_graph_tool` first, unless a refresh is queued); the server has usually queued a refresh already.
   - `worktree_behind` (uncommitted edits not indexed yet): refresh with
     `ensure_graph_tool(force=True)`.
   - `worktree_ahead` / `commit_synced`: already current; nothing to build.
2. **Bootstrap or refresh**:
   - Default MCP surface: `ensure_graph_tool()`, or `force=True` for
     uncommitted edits (an empty graph still gets a full parse). It inherits
     the server's embedding mode and runs **minimal** post-processing: structure
     and search, but no flows or communities.
   - Flows or communities missing (`graph_health.reason_codes` lists
     `missing_flows` / `missing_communities` — `status` can still read `ok` —
     right after a bootstrap): run `dagayn postprocess` (`run_postprocess_tool()` on the
     advanced surface).
   - CLI, full build with post-processing: `dagayn build`; add `--scip` to let
     installed SCIP indexers (rust-analyzer, scip-typescript, scip-go,
     scip-python, ...) settle call targets. Missing indexers print `hint:` lines
     and that language keeps dagayn's own resolution. The overlay runs only on a
     full build; the MCP build tool has no SCIP option, so use the CLI.
   - Advanced surface: `build_or_update_graph_tool(full_rebuild=True,
     local_embedding="none")` for explicit rebuild controls.
   - Don't run embedding-enabled full rebuilds as routine verification. When
     the server was started with `--local-embedding`, omitting
     `local_embedding` on `build_or_update_graph_tool` may inherit that mode and
     trigger a large embedding refresh. Pass `local_embedding="bge-m3"` only
     when the task needs fresh embeddings, and say why first.
3. **Report** from the build result rather than reading the database: files
   parsed, nodes, edges, `errors`, and any SCIP `hint:` or warning lines. Use
   `dagayn status` or `list_graph_stats_tool` (advanced) for languages.

## Notes

- The graph lives in `.dagayn/graph.db` (or under `CRG_DATA_DIR`). Indexed
  files are git's tracked and untracked files minus gitignored ones;
  `.dagaynignore` narrows that further.
- Edit hooks enqueue `dagayn queue add update` (check `dagayn queue status` if
  the graph seems behind); hooks never rebuild flows or communities.
- Check `README.md` "Supported languages and file types" for the language list.

## CLI

```bash
dagayn build                     # full parse + full post-processing
dagayn build --force-full-build  # delete graph.db first
dagayn build --scip              # also settle call targets with SCIP indexers
dagayn update                    # incremental, from the graph's own commit
dagayn postprocess               # flows, communities, FTS on an existing graph
dagayn status                    # graph state and embedding coverage
dagayn tool ensure_graph_tool
```
