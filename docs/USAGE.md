# Usage

## Install the package

```bash
pip install git+https://github.com/manji-0/dagayn.git
```

## Register MCP integration

```bash
dagayn install
```

Useful flags:

- `--platform <name>` to target one integration
- `--dry-run` to preview generated config
- `--no-skills`, `--no-hooks`, `--no-instructions` to skip optional setup steps

## Build and refresh the graph

```bash
dagayn build
dagayn update
dagayn watch
dagayn status
```

Use `build` the first time, `update` for change-driven refreshes, and `watch` during active development.

## Review changes

```bash
dagayn detect-changes --base HEAD~1
```

In MCP clients, the equivalent workflow usually starts with `detect_changes`, `get_review_context`, or `get_minimal_context`.

## Start the MCP server

```bash
dagayn serve
```

By default the server runs over stdio. Use the HTTP flags if you explicitly need local HTTP transport.

## Visualize or export the graph

```bash
dagayn visualize --serve
dagayn visualize --format graphml
dagayn visualize --format svg
dagayn visualize --format cypher
dagayn visualize --format obsidian
```

## Multi-repo workflows

```bash
dagayn register /path/to/repo --alias app
dagayn repos
dagayn daemon start
```

The registry is useful when you want cross-repo search or long-running watch management.
