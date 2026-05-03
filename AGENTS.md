# dagayn agent guide

<!-- constrained-by ./docs/USAGE.md -->
<!-- constrained-by ./docs/COMMANDS.md -->

This repository ships `dagayn`, a fork of `code-review-graph` with extra emphasis on Terraform, Markdown, and mixed-language monorepos.

## How agents should work with this repo

- use `dagayn` in all user-facing commands
- build or update the graph before asking graph-backed questions
- treat graph paths as repo-root-relative where the fork expects registered file trees
- use targeted graph tools before reading broad file sets
- keep docs aligned with fork behavior, not upstream prose

## Useful commands

```bash
dagayn build
dagayn update
dagayn status
dagayn detect-changes --base HEAD~1
dagayn serve
```

## Useful MCP flows

- `get_minimal_context` for quick orientation
- `detect_changes` or `get_review_context` for review work
- `query_graph`, `traverse_graph`, `list_flows`, and `list_communities` for exploration
- `refactor_tool` for rename previews and dead-code analysis

## Documentation rule

If you update features, command names, integrations, or supported languages, update the fork's docs in the same change.
