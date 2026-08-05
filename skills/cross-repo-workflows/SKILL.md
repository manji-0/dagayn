---
name: cross-repo-workflows
description: Register repositories, maintain multi-repo graph freshness, and search across repos with dagayn.
argument-hint: "[repo or query]"
---

# Cross-Repo Workflows

Use this when a task spans multiple repositories, shared libraries, downstream
consumers, or a multi-repo watch daemon.

<!-- dagayn skill embedding context -->
## Installed Search Mode

This packaged skill is mode-neutral. `dagayn install` rewrites this section with
the selected embedding mode so cross-repo search advice matches the installed
retrieval setup.
<!-- /dagayn skill embedding context -->

## Workflow

1. List known repositories:
   ```bash
   dagayn tool list_repos_tool
   dagayn repos
   ```
2. Register missing repos explicitly:
   ```bash
   dagayn register /path/to/repo --alias short-name
   dagayn daemon add /path/to/repo
   ```
3. Keep graphs fresh:
   ```bash
   dagayn daemon status
   dagayn daemon start
   dagayn daemon logs
   ```
4. Search structurally across repos:
   ```bash
   dagayn tool cross_repo_search_tool --arg query='"billing client"'
   ```
5. After cross-repo candidates are identified, switch back to the relevant repo
   and use local graph tools such as `query_graph_tool`, `review_tool`, or
   `semantic_search_nodes_tool` for source-level verification.

## Safety Rules

- Never assume a registered repo is fresh. Check daemon status, or on the
  default MCP surface run `ensure_graph_tool(force=True)` in that repo before
  relying on a result. Use `build_or_update_graph_tool()` only when the
  advanced surface is available and you need explicit rebuild/embedding controls.
- Cross-repo search is for candidate discovery. Confirm behavior in the owning
  repo before recommending edits.
- Use aliases in reports so users can tell which repo each finding came from.

## Efficiency Rules

- Use cross-repo search to narrow the candidate set before any broad `rg` across
  multiple checkout roots.
- Keep source reads repo-local and targeted after cross-repo discovery.
