---
name: worktree-sync
description: Make a git worktree or jj workspace usable with dagayn — inherit the main checkout's graph and MCP config instead of rebuilding, then catch up the branch diff. Use when an agent works in a linked worktree (EnterWorktree, Cursor parallel agents, `git worktree add`, `jj workspace add`) or dagayn there returns an empty graph, another checkout's results, or no MCP server.
argument-hint: "[worktree path]"
---

# Worktree Sync

A linked worktree starts without `.dagayn/` or the gitignored MCP config.
Rebuilding from scratch works but is slow; seeding from the main checkout's
graph and catching up only the branch diff takes seconds.

## Steps

1. **Confirm where you are**: `dagayn worktree info`. In the main checkout,
   stop and use `ensure_graph_tool` / the build-graph skill instead. jj
   workspaces count as linked worktrees too.
2. **Check whether a hook already did it**. Claude Code installs a PostToolUse
   hook on `EnterWorktree|ExitWorktree` that runs `dagayn session prepare
   --from-hook`, and Cursor's `.cursor/worktrees.json` runs `dagayn session
   prepare` in each parallel-agent worktree. If `get_minimal_context_tool`
   already shows a healthy graph, skip to step 4.
3. **Seed and catch up**: `dagayn session prepare --budget-seconds 45` seeds the
   worktree (config + graph) and catches up HEAD and worktree drift within the
   budget. For an explicit inherit without the budget, use
   `dagayn worktree sync`:
   - `--build-if-missing` — build here when the main checkout has no graph
     (without it, sync stops with "No graph available")
   - `--seed-only` — inherit the graph, skip the incremental catch-up
   - `--no-copy-config` — skip copying MCP/skill files that are already there
   - `--base <ref>` — catch up from a specific base
4. **Orient**: `get_minimal_context_tool(task="worktree session")`. If
   `graph_health.status` is still `empty` or `sync.state` is `unbuilt`, call
   `ensure_graph_tool()` once; then review and explore as usual.
5. **If no host wiring exists**, fix it in the **main** checkout:
   `dagayn install --platform claude` (or `cursor`, `all`). That writes
   `.worktreeinclude`, listing whichever MCP config files exist and are
   gitignored (e.g. `.mcp.json`, `.cursor/mcp.json`) so new worktrees get a
   copy — commit it — and `.cursor/worktrees.json` for Cursor. Don't commit
   local MCP config itself.

## Notes

- `dagayn serve`, `update`, `status`, and `session prepare` also seed a
  worktree automatically unless `DAGAYN_WORKTREE_SEED=0`;
  `session prepare --no-seed-worktree` skips it for one run.
- `CRG_DATA_DIR` keeps graph data outside the working tree; each repository and
  each worktree gets its own subdirectory (and its own graph) there.
- Git hooks live in the shared hooks directory, so one install covers every
  worktree of a checkout.
- After sync, review against the PR base, not against an empty graph. Don't
  start embedding-enabled rebuilds to "fix" an empty worktree graph.
