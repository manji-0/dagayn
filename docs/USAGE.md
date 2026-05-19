# Usage

<!-- constrained-by ./COMMANDS.md -->

## Install the package

<!-- derived-from #install-the-package -->

```bash
pip install dagayn
```

For a persistent isolated CLI environment:

```bash
uv tool install dagayn
```

For an isolated invocation without a persistent environment:

```bash
uvx --from dagayn dagayn --help
```

To run from the Git repository instead of a published wheel:

```bash
pip install git+https://github.com/manji-0/dagayn.git
```

```bash
uv tool install --from git+https://github.com/manji-0/dagayn.git dagayn
```

```bash
uvx --from git+https://github.com/manji-0/dagayn.git dagayn --help
```

Git/source installs build the PyO3 Rust extension locally. Install a Rust
toolchain, a C compiler, and the macOS Command Line Tools first when no
prebuilt wheel is available for your platform.

## Register MCP integration

<!-- constrained-by ./DAEMON-CONFIG.md -->

```bash
dagayn install
```

Useful flags:

- `--platform <name>` to target one integration
- `--dry-run` to preview generated config
- `--no-skills`, `--no-hooks`, `--no-instructions` to skip optional setup steps

Claude instruction injection writes to `~/.claude/CLAUDE.md`; Codex and OpenCode write global `AGENTS.md` files under `~/.codex/` and `~/.config/opencode/`; repo-local rule files such as `QODER.md` are still written in the workspace when their platforms are selected. Pi MCP config is written to `.pi/mcp.json`, dagayn skills are installed to `~/.pi/agent/skills/`, and pi-yaml-hooks-compatible hook files are written under `~/.pi/agent/hook/`. Hermes Agent MCP config is written to `~/.hermes/config.yaml`, dagayn skills are installed to `~/.hermes/skills/`, and shell hooks are added to the `hooks:` block in `~/.hermes/config.yaml`.

When Codex is selected, `dagayn install` also writes global hooks to `~/.codex/hooks.json` and enables `[features].hooks` in `~/.codex/config.toml`. Claude hooks are written to global `~/.claude/settings.json`. The hooks mirror the graph refresh flow: `dagayn update --skip-flows` after edits or before commit-time checks, graph status at session start, and a full `dagayn update` from the installed git `post-commit` hook. The generated update hook allows up to 300 seconds so large documentation or mixed-language changes can refresh without the host tool killing the hook at 30 seconds.

## Build and refresh the graph

<!-- constrained-by ./ARCHITECTURE.md#pipeline-overview -->

```bash
dagayn build
dagayn update
dagayn watch
dagayn status
```

Use `build` the first time, `update` for change-driven refreshes, and `watch` during active development.
Use `dagayn build --force-full-build` (or `--force`) to delete the existing
graph database and SQLite sidecar files before running a clean full parse.

## Use the Rust backend

<!-- constrained-by ./RUST-CORE-MIGRATION-WIP.md -->

The Rust backend is the default. It uses the Rust-backed graph store and
Rust-owned parser paths for Markdown, Terraform,
Rust, Python/notebooks, Bash/Go/Java/Ruby/C#/PHP/Kotlin/Swift/Scala/Solidity/Dart/Lua/Luau/C/C headers/Perl XS/C++/Objective-C/Elixir/GDScript/R/Julia/Perl/Vue/Svelte/Zig/PowerShell, extensionless shebang scripts for supported scripting languages, and core JavaScript/JSX/TypeScript/TSX/Astro files:

```bash
dagayn build
dagayn update
```

Source checkouts without `dagayn._core` fail clearly instead of falling back
to the removed Python parser implementation.

## Review changes

<!-- constrained-by ./ARCHITECTURE.md#pipeline-overview -->
<!-- derived-from ./plans/ANALYSIS-TOOL-STRATEGY.md#phase-1-document-the-default-path -->

```bash
dagayn detect-changes --base HEAD~1
```

In MCP clients, start with `get_minimal_context_tool`, then choose
`review_tool`, `architecture_analysis_tool`, `refactor_tool`, or
`query_graph_tool`. Follow response hints to drill-down modes only when needed.

## Start the MCP server

<!-- derived-from ./plans/ANALYSIS-TOOL-STRATEGY.md#mcp-tool-surface-plan -->

```bash
dagayn serve
```

By default the server runs over stdio and exposes only the compact workflow
surface: `get_minimal_context_tool`, `review_tool`, `flow_tool`,
`architecture_analysis_tool`, `refactor_tool`, `query_graph_tool`, and
`semantic_search_nodes_tool`. Use `--tools` when you need an exact
comma-separated allow-list:

```bash
dagayn serve --tools query_graph_tool,semantic_search_nodes_tool
dagayn serve --tools all
```

The same allow-list can be supplied with `CRG_TOOLS`; `all`, `full`, and `*`
restore the full advanced/maintenance tool surface. Use the HTTP flags if you
explicitly need local HTTP transport. Dagayn v3 removed named MCP tool
profiles; dispatcher tools keep the default surface small enough for ordinary
agent use while preserving drill-down access through `mode` arguments.

In dagayn 3.0, v2 split architecture MCP/CLI tools were removed. Use
`architecture_analysis_tool(mode=...)`, for example
`architecture_analysis_tool(mode="overview")` or
`architecture_analysis_tool(mode="sdp_violations")`.

## Visualize or export the graph

<!-- constrained-by ./ARCHITECTURE.md#post-processing -->

```bash
dagayn visualize --serve
dagayn visualize --mode community
dagayn visualize --format graphml
dagayn visualize --format mermaid-c4
dagayn visualize --format svg
dagayn visualize --format cypher
dagayn visualize --format obsidian
```

Notes:

- default output is `.dagayn/graph.html`
- HTML modes are `auto`, `full`, `community`, and `file`
- built-in export formats are `html`, `graphml`, `mermaid-c4`, `svg`, `cypher`, and `obsidian`
- `mermaid-c4` emits Mermaid `C4Component` code using files as components
- Graphviz / DOT is not a built-in export target
- Jupyter / Databricks notebooks are graph inputs, not report outputs
- `svg` export requires matplotlib, available via `dagayn[eval]`

## Multi-repo workflows

<!-- constrained-by ./DAEMON-CONFIG.md -->

```bash
dagayn register /path/to/repo --alias app
dagayn repos
dagayn daemon start
```

The registry is useful when you want cross-repo search or long-running watch management.
