---
name: worktree-sync
description: Make a git worktree usable for dagayn MCP — inherit the main checkout's graph and MCP config, then catch up the branch diff.
argument-hint: "[worktree path]"
---

# Worktree Sync

Use this when an agent opens a Claude Code / Cursor parallel worktree, or when
MCP tools in a linked worktree look empty, missing, or pointed at the wrong
checkout.

## When to Use

- Cursor created a worktree for a parallel agent
- Claude Code entered a worktree and graph/MCP config is missing
- `get_minimal_context_tool` shows an empty graph in a worktree that should
  inherit from the main checkout
- After `git worktree add` for a feature branch

## Steps

1. Confirm you are in a linked worktree:
   ```bash
   dagayn worktree info
   ```
   If this is the main checkout, stop — use `ensure_graph_tool` / `build-graph`
   instead of worktree inheritance.

2. Sync config + graph from the main checkout:
   ```bash
   dagayn worktree sync
   ```
   This copies gitignored MCP config from the main checkout, inherits
   `graph.db` (WAL + embeddings included), and re-parses only the branch diff.

   Options:
   - `--seed-only` — inherit the graph without the incremental catch-up
   - `--no-copy-config` — skip MCP/skill file copy when config is already present

3. Orient with MCP:
   - `get_minimal_context_tool(task="worktree session")`
   - If `graph_health.status` is still `empty`, call `ensure_graph_tool()` once
   - Prefer review / explore tools after the graph is healthy

4. If install never wired host bootstrap, fix that in the **main** checkout:
   ```bash
   dagayn install --platform cursor   # or claude / all
   ```
   Expect:
   - `.worktreeinclude` listing `.cursor/mcp.json` (and related MCP paths)
   - `.cursor/worktrees.json` containing `dagayn worktree sync`
   Commit `.worktreeinclude` (and force-add `.cursor/worktrees.json` when the
   team wants Cursor auto-sync). Do not commit local `.cursor/mcp.json`.

## Notes

- `dagayn serve` / `dagayn update` / `dagayn status` also seed a worktree graph
  automatically unless `DAGAYN_WORKTREE_SEED=0`.
- Git hooks live in the shared hooks directory, so one install covers every
  worktree of the same main checkout.
- After sync, treat the worktree like a normal branch: review against the PR
  base, not against an empty graph.

## Efficiency Rules

- Prefer `dagayn worktree sync` over a full `ensure_graph_tool` / `dagayn build`
  when the main checkout already has a healthy graph.
- One `worktree info` + one `worktree sync` + one `get_minimal_context_tool` is
  enough for most sessions.
- Do not start embedding-enabled rebuilds to "fix" an empty worktree graph.
