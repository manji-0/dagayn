# Session / worktree graph freshness

<!-- constrained-by ./USAGE.md -->
<!-- constrained-by ./COMMANDS.md -->
<!-- derived-from ./RECIPES.md#single-repo-watch--session-prepare -->

How dagayn keeps the knowledge graph **structurally ready** before an agent
starts work across session start/resume, worktree create/switch/delete, HEAD
moves, Subagent/parallel-agent launch, and MCP first-tool calls.

## Ready-to-work definition

<!-- derived-from #use-case-catalog -->

| Layer | Ready when | Not ready when |
| ----- | ---------- | -------------- |
| **Structure** (required) | `assess_graph_sync().status == "synced"` | `empty`, `git_drift`, or `dirty_worktree` |
| **Worktree** | Linked worktree seeded (if needed) and catch-up so `git_head_sha` matches that worktree's HEAD | Graph missing, still describing main, or HEAD drifted |
| **Embeddings** (best-effort) | Indexed when serve/install embedding mode is on | `pending` / `skipped_budget` — structure may still be ready; finish via MCP `ensure_graph_tool` / `get_minimal_context_tool` |

`session_prepare` / `ensure_graph` may return `status=partial` when the wall-clock
budget or a hook lock leaves structure unsynced, or when embeddings stay
deferred. Callers must retry (UC-M2) before treating the graph as ready.

`build_if_missing` is accepted by `session_prepare` / `ensure_graph` for API
symmetry with `dagayn worktree sync`; the prepare path currently always builds
an empty graph when structure prepare runs — do not rely on that flag as a
gate.

## Sync status model

<!-- constrained-by ./COMMANDS.md#git-worktrees -->

Authority: `assess_graph_sync` in `dagayn/tools/sync_status.py`.

| `sync.status` | Meaning |
| ------------- | ------- |
| `empty` | No nodes/files in the graph |
| `git_drift` | Stored `git_head_sha` ≠ current HEAD (or missing metadata) |
| `dirty_worktree` | HEAD matches but staged/unstaged/untracked files exist |
| `synced` | Non-empty graph, HEAD match, clean worktree relative to HEAD |

Structure refresh runs when `needs_structure_prepare` is true (`empty` /
`git_drift` / `dirty_worktree`, or `force=True`).

## Lifecycle flow

```mermaid
flowchart TD
  trigger[LifecycleTrigger] --> resolve[resolve_repo_or_hook_repo]
  resolve --> seed{linked_worktree_and_empty_graph}
  seed -->|yes| seedGraph[seed_worktree_graph]
  seed -->|no| assess[assess_graph_sync]
  seedGraph --> assess
  assess -->|empty_git_drift_dirty| structure[build_or_update_graph minimal]
  assess -->|synced| embedCheck{embedding_needs_refresh}
  structure --> embedCheck
  embedCheck -->|yes_and_budget| embed[Phase2 embeddings]
  embedCheck -->|no_or_defer| ready[structure synced ready]
  embed --> ready
```

## Use-case catalog

| ID | Trigger | Entry point | Freshness guarantee |
| -- | ------- | ----------- | ------------------- |
| UC-S1 | Session start | Cursor `sessionStart` / Claude `SessionStart` / OpenCode `session.created` → `dagayn session prepare` | Structure refresh if needed; hook budget 45s |
| UC-S2 | Session resume | Same hooks (no dedicated resume event) | Re-assess dirty/drift; same prepare path |
| UC-H1 | HEAD relocate mid-session | Cursor `crg-relocate.sh` / OpenCode after HEAD-moving git → `session prepare` | `git_drift` → structure refresh |
| UC-W1 | Worktree create | Cursor `.cursor/worktrees.json` setup / Claude `EnterWorktree` / `worktree sync` / `session prepare` | Seed from main + catch-up to worktree HEAD |
| UC-W2 | Worktree switch / re-enter | EnterWorktree / `session prepare` in existing worktree | Seed skipped if graph exists; catch-up from stored `git_head_sha` |
| UC-W3 | Worktree delete | `git worktree remove` (no dagayn hook) | Main checkout graph unchanged; orphaned worktree `.dagayn` discarded with the tree |
| UC-A1 | Subagent / parallel agent | Worktree create + MCP in that tree | Seed + `get_minimal_context(auto_prepare=True)` → structure synced |
| UC-M1 | MCP first tool | `get_minimal_context_tool` | `auto_prepare=True` (300s) when not synced |
| UC-M2 | MCP explicit sync | `ensure_graph_tool` | Same prepare path with MCP budget; retry after `partial` |
| UC-E1 | File edit (ongoing) | `dagayn update --skip-flows` | Out of bootstrap scope — keeps graph current during a session, not a start gate |

Automated coverage lives in `tests/test_session_graph_freshness.py` (test names
carry the UC id). Hook wiring for UC-S1 / UC-W1 / UC-H1 is asserted against
install-generated configs in `tests/test_skills.py`.

## Out of scope

- Spawning real Cursor/Claude Subagents in CI
- Treating embedding sidecar health as a readiness gate
- A dedicated worktree-delete cleanup hook
- Changing the unused `build_if_missing` gate inside `session_prepare`
