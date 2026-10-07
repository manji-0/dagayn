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

Instruction injection manages two marked sections (`<!-- dagayn MCP tools -->` and `<!-- dagayn markdown policy -->`). Re-running `dagayn install` replaces a marked section whose text is out of date and leaves the rest of the file alone; a marked section runs to the next dagayn marker or the next level-2 (`## `) heading, so keep your own notes under a separate `##` heading. Claude instruction injection writes to `~/.claude/CLAUDE.md`, and Claude Code skills are installed to `~/.claude/skills/` only (untracked dagayn copies an earlier install left in `<repo>/.claude/skills` are removed); Codex and OpenCode write global `AGENTS.md` files under `~/.codex/` and `~/.config/opencode/`; repo-local rule files such as `QODER.md` are still written in the workspace when their platforms are selected. Pi MCP config is written to `.pi/mcp.json`, dagayn skills are installed to `~/.pi/agent/skills/`, and pi-yaml-hooks-compatible hook files are written under `~/.pi/agent/hook/`. Hermes Agent MCP config is written to `~/.hermes/config.yaml`, dagayn skills are installed to `~/.hermes/skills/`, and shell hooks are added to the `hooks:` block in `~/.hermes/config.yaml`.

Cursor MCP config is written to both `.cursor/mcp.json` and `~/.cursor/mcp.json`. Entries omit a hardcoded `--repo` so the shared user-level MCP server can follow the currently open workspace via Cursor's `WORKSPACE_FOLDER_PATHS` environment variable (multi-root workspaces prefer a folder that already has a `.dagayn` graph).

When Codex is selected, `dagayn install` also writes global hooks to `~/.codex/hooks.json` and enables `[features].hooks` in `~/.codex/config.toml`. Claude hooks are written to global `~/.claude/settings.json`. The hooks mirror the graph refresh flow: edit-triggered hooks enqueue a structure-only update (`dagayn queue add update`) that a single detached queue worker drains per repository, commit-time checks run `dagayn update --skip-flows` synchronously, `dagayn session prepare` runs at session start (and on EnterWorktree / HEAD-moving git commands where the host supports them), and the installed git `post-commit` hook runs a full `dagayn update`. Session prepare uses a self-budget (45s by default) so the editor is not blocked; deferred embeddings finish via the queue worker or `ensure_graph_tool`; `get_minimal_context_tool` enqueues repair and returns immediately. Queue-worker updates set `DAGAYN_HOOK_UPDATE=1` (budget watchdog, skip-when-busy write lock) and honor the `.dagayn/hook-skip` opt-out; local embedding sidecar arguments are applied to the session-start prepare only.

## Build and refresh the graph

<!-- constrained-by ./ARCHITECTURE.md#pipeline-overview -->
<!-- constrained-by ./RECIPES.md#single-repo-watch--session-prepare -->

```bash
dagayn build
dagayn update
dagayn watch
dagayn status
```

Use `build` the first time, `update` for change-driven refreshes, and `watch` during active development.
`build`, `update`, and `watch` index the same git-indexable set: tracked plus
untracked working-tree files, excluding gitignored paths, then apply
`.dagaynignore`. Gitignored generated code is out of scope for watch as well as
for a full rebuild.
See [RECIPES.md](./RECIPES.md#single-repo-watch--session-prepare) for session
prepare and MCP serve variants.
Use `dagayn build --force-full-build` (or `--force`) to delete the existing
graph database and SQLite sidecar files before running a clean full parse.
Use `dagayn build --scip` to have SCIP indexers settle call targets during the
full build: Rust and TypeScript answers replace the extractor's, other
languages' fill what it left unresolved, and a project whose indexer is not
installed prints a `hint:` and keeps dagayn's own resolution. `update` and
`watch` do not run indexers; files they re-parse keep dagayn's resolution
until the next `build --scip`. See
[COMMANDS.md](./COMMANDS.md#scip-call-resolution) for the indexers, their
prerequisites, and the `DAGAYN_SCIP_*` variables.
Do not keep another GraphStore open across that rebuild. Postprocess uses the
same connection; embeddings run after it is closed. MCP queries wait for an
in-flight build, and a build waits for in-flight MCP queries.
`dagayn status` also reports embedding coverage from the current graph database,
including provider counts, missing embeddable nodes, and orphaned embedding rows.
When VCS metadata is present, it warns if the working copy branch/commit (git)
or path/revision (SVN) no longer matches the graph build. A branch name that
differs while the commit matches is not treated as drift, which is the normal
state in a worktree that inherited the main checkout's graph.

## Work in git worktrees

<!-- derived-from #build-and-refresh-the-graph -->
<!-- constrained-by ./SESSION-GRAPH-FRESHNESS.md -->
<!-- constrained-by ./COMMANDS.md#git-worktrees -->

Parallel agent sessions run in linked git worktrees: `claude --worktree`, the
`EnterWorktree` tool, subagents with `isolation: worktree`, and Cursor's
parallel agents. A worktree checks out tracked files only, so the gitignored
`.dagayn/` graph and MCP config files are missing there.

```bash
dagayn worktree info    # is this a worktree? what can it inherit?
dagayn worktree sync    # inherit the main checkout's graph, then catch up
```

`sync` copies the main checkout's gitignored MCP config and `graph.db` —
embeddings included — and runs an incremental update against the commit that
graph was built at, so only the branch diff is re-parsed. Graph inheritance also
happens automatically when `dagayn serve` starts and before `dagayn update` /
`dagayn status`, so an MCP session that opens in a worktree finds a working
graph. Set `DAGAYN_WORKTREE_SEED=0` to disable it.

`dagayn install` wires this into both hosts so it happens without you asking: a
managed block in `.worktreeinclude` (Claude Code copies matching gitignored files
into new worktrees — commit that file) and a `dagayn session prepare
--budget-seconds 45` entry in `.cursor/worktrees.json` (Cursor runs it when
creating a worktree for a parallel agent). Installing from inside a worktree
also configures the main checkout, and git hooks go into the repository's shared
hooks directory so one install covers every worktree.

### jj workspaces

<!-- derived-from #work-in-git-worktrees -->

A git-backed jj workspace created with `jj workspace add` — for example the
`.worktrees/<slug>` workspaces `track` creates in `vcs-mode=jj` — is handled
like a linked worktree of the colocated main checkout. It has `.jj/` but no
`.git`, so plain git commands there answer for the main checkout; dagayn stops
the repository-root walk at the workspace instead and reads its state from jj:

- `@-` is the commit the graph is built at (`git_head_sha`), `@-..@` is the
  uncommitted change, and the tree of `@` is the indexable file set.
- `dagayn worktree sync`, `session prepare`, and graph inheritance work as for
  git worktrees, seeding from the main checkout's graph.
- Hooks narrow `git rev-parse --show-toplevel` to a nested `jj workspace root`,
  and `dagayn hook-repo` resolves edited files to the workspace.
- A stale workspace (another workspace rewrote its commit) is reported as
  `commit_drift`; `build` and `update` stop with jj's reason until you run
  `jj workspace update-stale` there.

The main checkout must be colocated (`jj git init --colocate`), and `jj` must
be on `PATH`.

Session start/resume, worktree create/switch/delete, Subagent launch, and MCP
first-tool readiness are defined in
[SESSION-GRAPH-FRESHNESS.md](./SESSION-GRAPH-FRESHNESS.md).

## Native graph store

<!-- constrained-by ./RUST-CORE-MIGRATION-WIP.md -->

The graph store, parsers, FTS, flows, and post-processing run in the native
Rust extension (`dagayn._core`). There is no Python graph engine to fall back
to: `DAGAYN_BACKEND=python` is rejected. Hybrid search ranking and
manifest-bridge extraction stay in Python.

Parsers cover Markdown, Terraform, Rust, Python/notebooks, Bash, Go, Java,
Ruby, C#, PHP, Kotlin, Swift, Scala, Dart, Lua, C / C headers /
Perl XS, C++, Objective-C, Elixir, GDScript, R, Julia, Perl, Vue, Svelte, Zig,
PowerShell, extensionless shebang scripts for supported scripting languages,
and core JavaScript / JSX (`.js`, `.jsx`, `.mjs`, `.cjs`) / TypeScript (`.ts`, `.mts`, `.cts`, and `.d.ts` / `.d.mts` / `.d.cts`) / TSX / Astro files:

```bash
dagayn build
dagayn update
```

Source checkouts without `dagayn._core` fail clearly.

Zig is parsed structurally: `struct` / `enum` / `union` / `opaque` / error-set
declarations become Class nodes (with `type_role`), nested and type-function
containers keep dotted parents (`Point.Origin`, `Stack.push`), functions carry
`pub` / `extern` / `export` modifiers, `test "name"` and doctest `test decl`
blocks become Test nodes with `TESTED_BY` edges, `@import("x.zig")` resolves to
the imported file, and calls such as `Point.init(...)` or `self.axis(...)`
resolve to same-file declarations. PowerShell is still File-node-only.

## Review changes

<!-- constrained-by ./ARCHITECTURE.md#pipeline-overview -->
<!-- Plan context: ./plans/ANALYSIS-TOOL-STRATEGY.md#phase-1-document-the-default-path; not a graph dependency because usage docs are canonical. -->

```bash
dagayn detect-changes --base HEAD~1
```

Change review includes tracked diffs, staged changes, unstaged changes, and
untracked files. Untracked files are treated as whole-file changes because Git
does not provide line-level hunks for files it is not tracking yet. Inspect
`change_file_sources` (CLI output; `review_tool` counts them in
`change_file_source_counts`) when you need to distinguish base-ref changes from
local worktree, staged, unstaged, or untracked changes. Changed nodes and relevant
edges include `change_status` (`existing`, `added`, or `unknown`), with counts
grouped in `change_entity_summary`.

In MCP clients, start with `get_minimal_context_tool`, then choose
`review_tool`, `architecture_analysis_tool`, `refactor_tool`, or
`query_graph_tool`. Follow response hints to drill-down modes only when needed.
For change review, prefer `review_tool(mode="changes", detail_level="minimal")`
first and read `findings`: each names a place to look that the diff does not
show (a dangling reference, an unedited caller, a contract doc, a bridge, an
untested change, tests to run), the graph facts behind it, and an action. An
empty list means nothing beyond the diff needs checking. Use
`detail_level="standard"` to add the changed functions and affected flows. With
no `base`, uncommitted edits to tracked files are reviewed against `HEAD`, a
clean checkout against `HEAD~1`. The full field list is in
[COMMANDS.md](./COMMANDS.md).

## Start the MCP server

<!-- Plan context: ./plans/ANALYSIS-TOOL-STRATEGY.md#mcp-tool-surface-plan; not a graph dependency because usage docs are canonical. -->

```bash
dagayn serve
```

By default the server runs over stdio and exposes only the compact workflow
surface: `get_minimal_context_tool`, `ensure_graph_tool`, `review_tool`,
`flow_tool`, `architecture_analysis_tool`, `refactor_tool`,
`query_graph_tool`, `semantic_search_nodes_tool`, and `get_docs_section_tool`.
Use `--tools` when you need an exact comma-separated allow-list:

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
Architecture and flow outputs are calibrated leads: the architecture overview's
`findings` are claims to confirm at the location each names, and its SDP/SAP
modes report formulas and thresholds per declared unit; `flow_tool(mode="entry_points")`
returns the nearest entry points reaching a symbol with one call chain each,
computed at query time; its stored-flow modes (`list`, `get`) report a CALLS
reachable set (not an ordered execution path), disclose truncation via
`truncated` / `truncation_reason`, and remain for one release.
Tool responses also include `_runtime` metadata (`version`, `pid`, `python`,
and `package_root`) so you can spot when a long-lived MCP server is still
serving an older dagayn process than a direct `dagayn tool ...` CLI check.
Restart `dagayn serve` after local edits or upgrades before comparing MCP and
CLI results as the same implementation.

### Migrating response consumers

Existing fields such as `summary`, `_hints`, and `next_tool_suggestions`
remain available; `refactor_tool`'s per-suggestion `work_pack` and
`execution_plan` moved to `detail_level="verbose"`. New consumers should read `guidance`,
`answerability`, and `missingness` first, then fall back to the older raw
sections only when a drill-down needs more detail.

`review_tool(mode="changes")` is the exception: it answers with `findings`.
Its score-first fields (`analysis_summary`, `recommended_tests`,
`documentation_update_candidates`, `stability_contracts`, `risk_score`,
`review_priorities`, `test_gaps`, ...) appear only at
`detail_level="verbose"`, listed in `deprecated_fields`, for one release.
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
for finding in result["findings"]:
    if finding["kind"] == "tests_to_run" and finding.get("command"):
        run(finding["command"])
    else:
        check(finding["claim"], finding.get("sites") or finding.get("evidence"))
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

After a search or relationship hit, fetch one node's live span rather than
opening the whole file:

```python
result = query_graph_tool(pattern="source_of", target="src/app.py::handler")
body = result["results"][0]["source"]
```

The text is a worktree slice of the graph span, capped at 4,000 characters.
`source_stale` means the file hash moved; `truncated` means the tail was
omitted. Read the file only for surrounding context, edits, or the omitted
tail.

For everything that reaches a file or function, ask once with `depth` instead
of calling `importers_of` or `callers_of` again for each file it returns:

```python
result = query_graph_tool(pattern="importers_of", target="src/changes.py", depth=6)
files = {row["file"] for row in result["results"]}  # each row has depth and via
if result["reachability"]["depth_limit_reached"]:
    ...  # nodes at the last hop were not expanded
```

Rows are one per related node, with the edge lines in `lines`. Pass
`detail_level="full"` for one row per edge and the raw `edges` list.

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
- Jupyter / Databricks / marimo notebooks (Python `.py` and Markdown `.md`) are graph inputs, not report outputs
- `svg` export requires matplotlib, available via `dagayn[eval]`

## Multi-repo workflows

<!-- constrained-by ./DAEMON-CONFIG.md -->
<!-- constrained-by ./RECIPES.md -->

```bash
dagayn register /path/to/repo --alias app
dagayn repos
dagayn daemon start
```

The registry is useful when you want cross-repo search or long-running watch management.
Copy-paste recipes for single-repo watch, registry → search, optional embedding
providers, and common failure modes are in [RECIPES.md](./RECIPES.md).
