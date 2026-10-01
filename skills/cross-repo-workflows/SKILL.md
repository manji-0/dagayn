---
name: cross-repo-workflows
description: Work across several repositories with dagayn — register repos, keep their graphs fresh with the watch daemon, search all of them at once for a symbol or concept, then confirm hits in the owning repo. Use this whenever a task spans multiple repositories, a shared library and its consumers, a client and a server in different checkouts, or the user asks "who else uses this", "where is this defined in our other repos", or wants to set up the multi-repo registry or daemon.
argument-hint: "[repo or query]"
---

# Cross-Repo Workflows

Cross-repo search narrows candidates across every registered repository; the
owning repo's own graph then confirms them. Each repo needs its own graph for
search to see it.

<!-- dagayn skill embedding context -->
## Installed Search Mode

This packaged skill is mode-neutral. `dagayn install` rewrites this section with
the selected embedding mode so cross-repo search advice matches the installed
retrieval setup.
<!-- /dagayn skill embedding context -->

## Workflow

1. **See what is registered**: `dagayn repos` (or `dagayn tool list_repos_tool`).
2. **Register and build missing repos**. `register` feeds cross-repo search;
   `daemon add` only feeds the watch daemon, so run both if you want both:
   ```bash
   dagayn register /path/to/repo --alias short-name
   dagayn build --repo /path/to/repo   # once; search skips repos without a graph
   dagayn daemon add /path/to/repo
   ```
3. **Keep graphs fresh**: `dagayn daemon status`, `dagayn daemon start`,
   `dagayn daemon logs`.
4. **Search across repos**:
   ```bash
   dagayn tool cross_repo_search_tool --arg query='"billing client"'
   ```
   Check `repos_searched`, `repos_skipped` (`no_graph`, `stale_registry_entry`,
   `search_failed`), and `missingness`: a skipped repo means absence there is
   not evidence. Scores from different `repo_search_modes` are not comparable.
5. **Confirm in the owning repo** without switching checkouts: every local tool
   takes `repo_root`, so pass the hit's `repo_path`, e.g.
   `query_graph_tool(pattern="source_of", target=..., repo_root="<repo_path>")`.
   Refresh that repo first with `ensure_graph_tool(repo_root="<repo_path>")`
   (`force=True` only for uncommitted edits there).

## Rules

- A registered repo is not necessarily fresh: check `dagayn daemon status` or
  ensure it before relying on its results.
- Cross-repo search is candidate discovery; confirm behavior in the owning repo
  before recommending edits.
- Report each finding with its repo alias so the user can tell where it lives.
- Use cross-repo search before any broad `rg` across several checkouts, and
  keep the file reads that remain targeted and repo-local.
