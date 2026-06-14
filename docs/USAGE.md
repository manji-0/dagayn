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

When Codex is selected, `dagayn install` also writes global hooks to `~/.codex/hooks.json` and enables `[features].hooks` in `~/.codex/config.toml`. Claude hooks are written to global `~/.claude/settings.json`. The hooks mirror the graph refresh flow: `dagayn update --skip-flows` after edits or before commit-time checks, graph status at session start, and a full `dagayn update` from the installed git `post-commit` hook. The generated update hook allows up to 300 seconds so large documentation or mixed-language changes can refresh without the host tool killing the hook at 30 seconds. Hook-triggered updates set `DAGAYN_HOOK_UPDATE=1`, skip overlapping hook refreshes, and reuse local embedding sidecar arguments when a local embedding install mode is selected.

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
`dagayn status` also reports embedding coverage from the current graph database,
including provider counts, missing embeddable nodes, and orphaned embedding rows.

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
<!-- Plan context: ./plans/ANALYSIS-TOOL-STRATEGY.md#phase-1-document-the-default-path; not a graph dependency because usage docs are canonical. -->

```bash
dagayn detect-changes --base HEAD~1
```

Change review includes tracked diffs, staged changes, unstaged changes, and
untracked files. Untracked files are treated as whole-file changes because Git
does not provide line-level hunks for files it is not tracking yet. Inspect
`change_file_sources` when you need to distinguish base-ref changes from local
worktree, staged, unstaged, or untracked changes. Changed nodes and relevant
edges include `change_status` (`existing`, `added`, or `unknown`), with counts
grouped in `change_entity_summary`.

In MCP clients, start with `get_minimal_context_tool`, then choose
`review_tool`, `architecture_analysis_tool`, `refactor_tool`, or
`query_graph_tool`. Follow response hints to drill-down modes only when needed.
For change review, prefer `review_tool(mode="changes", detail_level="minimal")`
first. Its `guidance` list gives the next test, doc, architecture, or flow
action in the shared `claim` / `evidence` / `confidence` / `missingness` /
`action` shape. Use `detail_level="standard"` when you need the full raw
sections behind those recommendations.

## Start the MCP server

<!-- Plan context: ./plans/ANALYSIS-TOOL-STRATEGY.md#mcp-tool-surface-plan; not a graph dependency because usage docs are canonical. -->

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
ADP/SDP/SAP modes use `artifact_scope="code"` by default; pass
`artifact_scope="docs"` when reviewing Markdown dependency structure.
SAP metrics also mark each row with `sap_applicable` and
`applicability_reason`; default `sap_metrics` output separates
`no-eligible-types` and `isolated` scopes from the main metric list, while
`detail_level="verbose"` includes those raw rows.
Use `dependency_profile="implementation"`, `"infra_dataflow"`, or
`"artifact_trace"` only when the analysis needs CALLS, Terraform REFERENCES, or
high-confidence CROSS_ARTIFACT traceability; the default `strict_static` profile
keeps design-principle metrics on static dependency edges.
Architecture and flow outputs are calibrated leads: `architecture_health`
reports formulas, thresholds, and stable-component policy; `flow_tool` reports
whether source snippets were included and reminds clients that criticality is a
ranking signal, not a coverage guarantee.
Tool responses also include `_runtime` metadata (`version`, `pid`, `python`,
and `package_root`) so you can spot when a long-lived MCP server is still
serving an older dagayn process than a direct `dagayn tool ...` CLI check.
Restart `dagayn serve` after local edits or upgrades before comparing MCP and
CLI results as the same implementation.

### Migrating response consumers

Existing fields such as `summary`, `_hints`, `next_tool_suggestions`,
`recommended_tests`, `documentation_update_candidates`, `stability_contracts`,
and `work_pack` remain available. New consumers should read `guidance`,
`answerability`, and `missingness` first, then fall back to the older raw
sections only when a drill-down needs more detail.
Dispatcher error paths and graph-limited not-found paths still carry
`answerability` and `missingness`, computed for the requested `repo_root` when
one is supplied.

Before:

```python
result = review_tool(mode="changes", detail_level="minimal")
for test in result.get("recommended_tests", []):
    run(test["qualified_name"])
```

After:

```python
result = review_tool(mode="changes", detail_level="minimal")
for item in result.get("guidance", []):
    if item["confidence"] != "unknown":
        follow(item["action"], evidence=item["evidence"])
```

Before:

```python
result = query_graph_tool(pattern="callers_of", target="handler")
if not result["results"]:
    conclude_absent()
```

After:

```python
result = query_graph_tool(pattern="callers_of", target="handler")
if result.get("zero_result_reason"):
    follow(result["next_action"])
```

`query_graph_tool` keeps the same zero-result contract for both empty
relationship results and missing targets. Missing targets return
`status="not_found"`, `result_count=0`, `results=[]`,
`zero_result_reason="target_not_found_in_graph"`, `next_action`, and
`answerability` / `missingness`; do not treat that as proof the symbol cannot
exist outside the current graph.

## Export the graph

<!-- constrained-by ./ARCHITECTURE.md#post-processing -->

```bash
dagayn visualize --format graphml
dagayn visualize --format mermaid-c4
dagayn visualize --format svg
dagayn visualize --format cypher
dagayn visualize --format obsidian
```

Notes:

- `--format` is required
- built-in export formats are `graphml`, `mermaid-c4`, `svg`, `cypher`, and `obsidian`
- `mermaid-c4` emits Mermaid `C4Component` code using files as components
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
