# Commands and surfaces

## CLI commands

<!-- constrained-by ./ARCHITECTURE.md -->

### Core graph lifecycle

- `dagayn build`
- `dagayn update`
- `dagayn postprocess`
- `dagayn watch`
- `dagayn status`

Use `dagayn build --force-full-build` when you need a clean graph rebuild from
scratch. It removes the existing `graph.db` plus SQLite sidecar files before
running the normal full parse. `--force` is accepted as a shorter alias.
The CLI does not keep a second GraphStore open across that rebuild: a leftover
WAL+mmap connection used to corrupt `sqlite_master` during postprocess.
Postprocess SQL stays on the build store's connection; local embeddings start
only after that store is closed. Graph reads (MCP tools, `dagayn status`) take
a shared lock and writes take an exclusive lock, so the two do not overlap.
`dagayn update` detects tracked diffs, staged changes, unstaged changes, and
untracked files together, so new files do not need to be staged before an
incremental graph refresh can parse them. `dagayn build`, `dagayn update`, and
`dagayn watch` share one file-scope authority: git's indexable set (tracked plus
untracked, excluding gitignored). `.dagaynignore` is an extra restriction on
that set, not a replacement for `.gitignore`. A file that becomes gitignored
after it was indexed is removed on the next update or build. Incremental results include
`change_file_sources` so base-ref diffs remain distinguishable from local
worktree changes. Its diff base defaults to the commit the graph was built at
(`git_head_sha`), falling back to `HEAD~1` for a graph that has none: a
hard-coded `HEAD~1` skipped every commit in between, so an edit hook firing
after a multi-commit `git pull` parsed the last commit only. Pass `--base`
explicitly to narrow or widen it; a base that does not reach the graph's own
commit leaves the graph's recorded commit untouched, so
`dagayn status` keeps reporting `commit_drift` until a prepare catches up.
`dagayn status` also prints `Graph state:` — the assessed freshness state with
the command that clears it, and the files needing a re-index when there are any.
It comes from the same `assess_graph_sync` the hooks and MCP tools act on, so
status cannot disagree with them.
`dagayn status` prints graph totals and embedding coverage for the same
database, including the current state (`complete`, `partial`, `stale`, `empty`,
or `not_indexed`) and provider-level vector counts. It also prints the VCS
branch/revision recorded at build time and warns when the working copy has
moved to a different git branch/commit or SVN path/revision.

The installed `dagayn` command runs `build`, `update`, and `status` in the
Rust CLI compiled into `dagayn._core`, so a hook's `dagayn update` does not load
the Python CLI. Anything the Rust CLI does not handle yet (other flags such as
`--scip` or `--local-embedding`, jj and SVN working copies, `CRG_DATA_DIR`, a
run without `--repo` outside a plain git checkout or with `CRG_REPO_ROOT` or an
editor workspace variable set, a corrupt graph) runs the Python CLI instead,
with the same output. `dagayn queue add` of `update`, `postprocess`, or
`prepare` with `--repo`, which edit hooks run, likewise enqueues without loading
the whole CLI. `dagayn serve` over stdio answers
the MCP handshake, the tool and prompt listings, and `list_graph_stats_tool` and
`get_docs_section_tool` in Rust, and starts the Python server only for the first
call it does not answer itself. Set `DAGAYN_PYTHON_CLI=1` to run
every command through the full Python CLI; `python -m dagayn` always does.

### SCIP call resolution

<!-- derived-from ./plans/SCIP-CALL-RESOLUTION.md -->

`dagayn build --scip` settles call targets with SCIP indexers before
post-processing. For every project of the repository (a directory holding
the marker file) it runs the indexer, when this machine has it:

| Language | Indexer | Project marker | Answer |
| -------- | ------- | -------------- | ------ |
| Rust | `rust-analyzer scip` | outermost `Cargo.toml` | replaces the extractor's |
| TypeScript / JavaScript | `scip-typescript` (or `npx @sourcegraph/scip-typescript`) | `tsconfig.json` with `node_modules` | replaces the extractor's |
| Go | `scip-go` | `go.mod` | fills |
| Python | `scip-python` (or `npx @sourcegraph/scip-python`) | outermost `pyproject.toml` / `setup.py` / `setup.cfg` | fills |
| Java / Kotlin | `scip-java` (or `coursier launch`) | outermost `pom.xml` / `build.gradle(.kts)` / `settings.gradle(.kts)` | fills; runs only with `DAGAYN_SCIP_ALLOW_BUILD=1` |
| C / C++ | `scip-clang` | `compile_commands.json` or `build/compile_commands.json` | fills |
| C# / VB | `scip-dotnet` | `*.sln`, `*.slnx`, `*.csproj`, `*.vbproj` | fills |
| Ruby | `scip-ruby` (or the project's `bin/scip-ruby`) | `Gemfile` | fills |
| Dart | `scip_dart` (or `dart pub global run scip_dart`) | `pubspec.yaml` with `.dart_tool/package_config.json` | fills |
| PHP | the project's `vendor/bin/scip-php` | `composer.json` with `composer.lock` and `vendor/` | fills |
| R | `scip-r` | `DESCRIPTION` with `R/` | fills |

An indexer whose answer *fills* keeps every target the extractor resolved
and settles the rest before the inference passes; one that *replaces* moves
any target it disagrees with. When a project's indexer is not installed, the
build prints a `hint:` line with the install command and the language keeps
dagayn's own resolution; a failed run is a warning and does the same.
`scip-java` runs the project's build with `clean`, so it needs
`DAGAYN_SCIP_ALLOW_BUILD=1`; `scip-dotnet` runs `dotnet restore`. The indexes
are written under `.dagayn/scip/`. `DAGAYN_SCIP_<LANGUAGE>`
(`DAGAYN_SCIP_RUST`, `DAGAYN_SCIP_GO`, `DAGAYN_SCIP_CSHARP`, ...) replaces an
indexer's command, and `DAGAYN_SCIP_TIMEOUT` bounds each run (seconds,
default 900). The overlay runs only on a full `build`; `update` re-resolves
the files it re-parses as before, until the next `build --scip`. See
[CALL-RESOLUTION.md](./CALL-RESOLUTION.md#scip-overlay).

### Local embedding refresh

<!-- derived-from ./LOCAL-EMBEDDINGS.md -->

`dagayn build` and `dagayn update` can also generate local embeddings after the
graph refresh. Passing `--local-embedding` without a value runs the recommended
BGE-M3 GGUF model through a managed llama.cpp sidecar with the measured
`material` text mode:

```bash
dagayn build --local-embedding
dagayn update --local-embedding
```

To use the managed llama.cpp sidecar with Qwen3-Embedding-0.6B, pass the mode
explicitly. The legacy `low` value remains an alias for this behavior:

```bash
dagayn build --local-embedding --mode llama-qwen3
dagayn update --local-embedding low
```

Use `--local-embedding none` to keep the default graph-only behavior. The
server startup timeout and each embedding request timeout are separate knobs:
`--local-embedding-timeout` controls readiness, while
`--local-embedding-request-timeout` controls a single `/v1/embeddings` call.
Managed sidecar requests use `--local-embedding-batch-size 1` by default,
regardless of any ambient `CRG_OPENAI_BATCH_SIZE`. `--local-embedding-bin auto`
selects `llama-server`. Default sidecar ports are 18080 for BGE-M3 and 18081
for Qwen (`low` / `--mode llama-qwen3`); a server whose `/v1/models` catalog
does not name the requested alias is refused even when the vector length
matches. After a model switch, search keeps ranking in the new partition
(reporting `partial_coverage` / `degraded` until the corpus is fully
re-embedded) and a completed `embed_all_nodes` run deletes retired partitions.

### Background task queue

<!-- constrained-by ./SESSION-GRAPH-FRESHNESS.md#use-case-catalog -->

- `dagayn queue add`
- `dagayn queue run`
- `dagayn queue status`
- `dagayn queue clear`

Per-repository task queue for background graph processing. The edit-triggered
hooks (Claude `PostToolUse`, Cursor `afterFileEdit`, pi/hermes file-change,
OpenCode `file.edited`) call `dagayn queue add update` instead of running
`dagayn update` themselves: the add is a single SQLite insert that coalesces
with an already-pending `update` task, then one detached worker per
repository (spawned on demand, guarded by a flock) drains the queue. A burst
of edits therefore collapses into one structure-only update
(`postprocess=minimal`, no embeddings) instead of one `dagayn update` process
per edit, and the last edit of a burst can no longer be left unindexed by a
skipped overlapping run. After that structure update, if the graph already
holds a managed localhost-sidecar partition, the worker enqueues a
file-scoped `embed` for the changed and dependent files (`text_hash` skip,
no whole-corpus scan), inferring BGE-M3 vs Qwen from the stored provider.
That task embeds straight away instead of repeating the structure update it
was queued by; it runs one again only after coalescing with an `embed` that
did not come from an update.
Two scoped `embed` tasks union their file lists when they coalesce; mixing a
scoped task with a whole-corpus `embed` keeps the whole-corpus pass. The
worker applies hook-update semantics to the task
it executes (`DAGAYN_HOOK_UPDATE=1`: budget watchdog, non-blocking write
lock) and honors the `.dagayn/hook-skip` opt-out. It exits after the queue
has been empty for the idle window (default 60s), so it does not hold the
graph lock between bursts.

Task kinds: `update` (structure-only incremental update — the hook path),
`embed` (explicit embedding pass using the payload's local-embedding
configuration), and `postprocess` (flows/communities/FTS). The worker is one
serial lane, so kinds carry a default priority (`update` 10, `embed` and
`postprocess` 0) and an edit-triggered update is claimed ahead of an `embed`
queued before it; `--priority` overrides this. A task already running is not
preempted, so an update enqueued mid-`embed` waits for it (bounded by the
embed budget). A failing task is retried up to 3 times, waiting 1s longer per
attempt spent, before it is parked `dead`; other queued tasks run while a
retry waits. A worker that died mid-task (budget
watchdog, crash) leaves its task `running`; the next worker requeues it, or
parks it `dead` when its attempts are already spent. `dagayn queue status`
shows pending/running/dead counts and the last 10 log entries (`--json` for
machine output). The queue lives at `.dagayn/task_queue.db` (or the
`CRG_DATA_DIR` location), deliberately separate from `graph.db` so it stays
writable while a build holds the graph write lock. `dagayn queue clear`
drops all queued tasks. The pre-commit paths (git hook, Cursor
`beforeShellExecution`, OpenCode pre-commit) still run
`dagayn update --skip-flows` synchronously, because `detect-changes` needs a
fresh graph at commit time.

### Analysis and review

- `dagayn detect-changes`
- `dagayn tool`
- `dagayn visualize`
- `dagayn wiki`
- `dagayn eval`

<!-- derived-from ./EVALUATION-SEMANTICS.md -->

`dagayn detect-changes` uses the same combined change detection as
`dagayn update`: tracked diffs plus staged, unstaged, and untracked working-tree
files. Untracked files are reviewed as whole-file changes because Git has no
line hunks for files it does not yet track. The CLI output includes
`change_file_sources.base_diff`, `worktree`, `staged`, `unstaged`, and
`untracked` buckets alongside the compatibility `changed_files` list;
`review_tool(mode="changes")` counts them in `change_file_source_counts` and
lists the buckets only at `detail_level="verbose"`.
Change analysis also annotates changed nodes and relevant edges with
`change_status` (`existing`, `added`, or `unknown`) and summarizes those counts
in `change_entity_summary`, making before/after risk changes easier to read.

`dagayn eval --benchmark doc_fuzzy_search` compares FTS and deterministic
embedding retrieval for fuzzy natural-language queries against Markdown
documentation sections and bodies. Configure queries with
`doc_fuzzy_search_queries` in an eval YAML file; `relevant` entries provide
graded alternate targets, `doc_fuzzy_search_include_paths` /
`doc_fuzzy_search_exclude_paths` constrain the documentation corpus, and
`doc_fuzzy_search_query_variants` compares embedding query prefixes.

`dagayn eval --benchmark embedding_materials` compares deterministic embedding
quality across generated material strategies before changing embedding models.
It varies Markdown section/paragraph/sentence granularity, code symbol text,
mechanical predicate text, adjacent comments, and split-vs-combined
symbol/comment materials. Configure unrelated calibration queries with
`embedding_material_negative_queries`; their rows report `top_score` and
`mean_top_5_score` so overconfident matches are visible even when no target is
expected.

#### Graph construction scale

<!-- derived-from ./GRAPH-EFFICIENCY-PLAN.md -->

`dagayn eval --benchmark query_performance` records p95 for `traverse_graph`
(depth 1/3/6), `get_impact_radius`, and `get_affected_flows`. Embedding search
is a skipped row; use `embedding_materials` for that axis.

`dagayn eval --benchmark scale_performance` builds a synthetic Python graph
(default ~10k nodes). Set `DAGAYN_SCALE_LARGE=1` or `scale_include_100k` for
100k, and `DAGAYN_SCALE_1M=1` only for manual/nightly 1M runs. Rows split
parse/write vs postprocess, report `nodes_per_second` / `edges_per_second` /
peak RSS / incremental changed-node/sec, and attach query + MCP p95. Do not
mix embedding generation into these numbers.

`dagayn eval --benchmark guidance_precision` measures precision@k for
review-guidance outputs such as recommended tests, documentation update
candidates, refactor suggestions, calibrated `guidance` items, stable-contract
warnings, architecture leads, answerability warnings, and guidance field
coverage. Configure cases with `guidance_precision_cases` in an eval YAML file.
It reads review output at `detail_level="verbose"`, the only level that still
carries the deprecated score-first fields, and goes when they do. The
`findings` contract has its own gate: `eval/run_review_eval.py` scores each
finding kind on the `tests/fixtures/review_eval` cases, and CI fails when a kind
drops below its precision or recall floor in `eval/review_thresholds.yaml`
(0.8; `DAGAYN_REVIEW_EVAL=1 uv run pytest -q tests/test_review_eval.py`).

```yaml
guidance_precision_cases:
  - name: review-guidance-contract
    kind: guidance_items
    changed_files: ["dagayn/tools/review_dispatcher.py"]
    expected: ["test_gaps", "documentation_update_candidates"]
    k: 3
  - name: answerability-warning
    kind: answerability_warnings
    changed_files: ["dagayn/tools/query.py"]
    expected: ["missing_test_edges"]
    k: 5
  - name: field-coverage
    kind: guidance_field_coverage
    changed_files: ["dagayn/tools/review_dispatcher.py"]
    expected: ["1.0"]
    k: 1
```

`dagayn eval --report` generates a semantic evaluation report by default. The
report separates capability scores, efficiency/cost metrics, gates,
diagnostics, and proxy/synthetic metrics instead of producing one misleading
global score. Use `--profile` to focus the profile summary:

```bash
dagayn eval --report
dagayn eval --report --profile search
dagayn eval --report --profile review
dagayn eval --report --profile operability
```

Available profile values are `search`, `review`, `architecture`,
`operability`, `regression`, and `all`. The default is `all`.

Semantic reports also write machine-readable outputs:

- `evaluate/reports/profile_summary.json`
- `evaluate/reports/metric_semantics.json`

Pass `--no-semantic-report` to keep the older simple report shape:

```bash
dagayn eval --report --no-semantic-report
```

`dagayn tool <mcp-tool-name>` invokes the same underlying implementation as an
MCP tool and prints JSON. This gives agents and scripts a CLI path to run a
single tool directly, including when a running MCP server was started with a
narrow `--tools` allow-list:

```bash
dagayn tool review_tool --arg mode='"impact"' --arg 'changed_files=["src/app.py"]' --arg max_depth=3
dagayn tool flow_tool --arg mode='"entry_points"' --arg target='"save_user"'
dagayn tool architecture_analysis_tool --arg mode='"overview"' --format summary
```

Tool responses at `detail_level="verbose"` (and the maintenance tools'
responses) include compact `_runtime` metadata (`version`, `pid`, `python`,
and `package_root`) and the full `_repo` (`db_path`, how the root was found)
so agents can compare a direct CLI run with a running MCP server. A long-lived MCP process keeps the implementation it loaded
at startup; after editing or upgrading dagayn, restart `dagayn serve` before
treating MCP output as the same truth source as `dagayn tool`.

`dagayn visualize` is the static graph export command. It requires `--format`
and generates:

- GraphML, Mermaid C4, SVG, Neo4j Cypher, or Obsidian exports via `--format`

Jupyter / Databricks / marimo notebooks (Python `.py` and Markdown `.md`) are supported as graph inputs rather than report
output formats.

`dagayn wiki` writes Markdown pages under `.dagayn/wiki/` from detected graph
communities. Each community page includes members, reachable-set execution
flows (BFS visit order from an entry point, not a runtime call sequence),
cross-community dependencies, and code-scoped package-level ADP/SDP/SAP
architecture metrics filtered to the scopes represented by that community.

### Architecture metrics

<!-- derived-from ./USAGE.md -->

These CLI commands answer with the matching `architecture_analysis_tool`
mode (`adp_violations`, `sdp_metrics`, `sdp_violations`, `sap_metrics`,
`sap_violations`), so the CLI and MCP clients see the same scopes and
numbers. `--format json` prints the MCP answer without the fields meant for
an agent (`_hints`, `next_tool_suggestions`, `_runtime`, `_repo`). A
`package` is a declared unit (Cargo crate, npm package, Go module, Python
import package, Terraform module, or a top-level directory no manifest
covers), not a directory; `sap-metrics` and `detect-sap` keep directory
scopes under `--scope-kind directory`. They default to
`artifact_scope="code"` so Markdown dependencies are not mixed into code
design metrics; pass `--artifact-scope docs` or `--artifact-scope all` for
documentation or legacy mixed-graph analysis.

- `dagayn detect-adp` — Detect cyclic dependencies (Acyclic Dependencies
  Principle violations). Reports cycles with length and severity. Deprecated
  with the `adp_violations` mode it reads, and removed with it after one
  release; the overview's `import_cycle` findings replace it
  (`dagayn tool architecture_analysis_tool --arg 'mode="overview"'`). It warns
  on stderr.
  - `--granularity {package,file}` (default `package`)
  - `--artifact-scope {code,docs,all}` (default `code`)
  - `--min-cycle-size N` (default 2)
  - `--max-cycle-length N` (default 10)
  - `--top-n N` (default: every cycle)
  - `--format {json,text}` (default `json`)
- `dagayn sdp-metrics` — Compute per-scope instability (Stable Dependencies
  Principle) scores. Returns `instability`, `Ca`, and `Ce` per scope, sorted,
  limited by `--top-n`.
  - `--granularity {package,file}` (default `package`)
  - `--artifact-scope {code,docs,all}` (default `code`)
  - `--top-n N` (default 30)
  - `--format {json,text}` (default `json`)
- `dagayn detect-sdp` — Detect stability-direction violations: dependencies
  from a more-stable scope to a less-stable scope whose instability gap meets
  `--min-delta`.
  - `--granularity {package,file}` (default `package`)
  - `--artifact-scope {code,docs,all}` (default `code`)
  - `--min-delta FLOAT` (default 0.1)
  - `--top-n N` (default: every violation)
  - `--format {json,text}` (default `json`)
<!-- derived-from ./SAP-METRICS.md -->

- `dagayn sap-metrics` — Compute per-scope abstractness, instability, and
  distance from the main sequence (Stable Abstractions Principle). Returns
  `A`, `I`, and `D` per scope, limited by `--top-n`; scopes the metric does
  not apply to (no eligible types, or isolated) are listed apart in
  `inapplicable_metrics`.
  - `--scope-kind {package,file,directory}` (default `package`)
  - `--unit-filter PREFIXES` — comma-separated scope_key prefixes to restrict
    output
  - `--artifact-scope {code,docs,all}` (default `code`)
  - `--top-n N` (default 30)
  - `--format {json,text}` (default `json`)
- `dagayn detect-sap` — Detect scopes whose distance from the main sequence
  meets `--min-distance`, with the zone (`pain` or `uselessness`) each is in.
  - `--scope-kind {package,file,directory}` (default `package`)
  - `--artifact-scope {code,docs,all}` (default `code`)
  - `--min-distance FLOAT` (default 0.5)
  - `--top-n N` (default: every violation)
  - `--format {json,text}` (default `json`)

All five commands accept `--repo` to override the repository root
(auto-detected by default). `detect-sap` suppresses test-scope and
fixture-scope entries from the violation list, as
`architecture_analysis_tool(mode="sap_violations")` does.

### Profiling

- `dagayn profile`

`dagayn profile` wraps any other dagayn subcommand in a
[pyinstrument](https://pyinstrument.readthedocs.io/) CPU profile and writes an
HTML report. The profiler is an optional dev dependency; install it with
`uv sync --extra dev` or `pip install pyinstrument`.

```bash
dagayn profile build
dagayn profile update --local-embedding
dagayn profile tool review_tool --arg mode='"changes"'
```

Flags:

- `--output-dir DIR` — directory for the HTML report (default
  `.dagayn/profiles`)
- `--interval SECONDS` — sampling interval (default `0.001`)
- `--open` — open the HTML report in a browser when finished

The subcommand and its arguments are passed after the `profile` command name.
The wrapped subcommand re-enters the dagayn CLI under the profiler, so the
profile reflects the full nested command execution. If `pyinstrument` is not
installed, the command prints a clear install hint and exits non-zero.

### Integration and serving

- `dagayn install`
- `dagayn init`
- `dagayn serve`

`dagayn install --platform cursor` merges graph-refresh hooks into
`~/.cursor/hooks.json` and writes scripts under `~/.cursor/hooks/`:
`afterFileEdit` enqueues a structure-only update (`dagayn queue add update`)
so edit bursts coalesce into one worker pass, `sessionStart` runs
`dagayn session prepare --budget-seconds 45` and
returns the result as `additional_context`, `beforeShellExecution` matches bare
or path-qualified `git commit` commands before running update +
`detect-changes --brief`, and `afterShellExecution` matches HEAD-moving git
commands (`checkout` / `switch` / `reset` / `pull` / …) to re-prepare the graph
after HEAD has moved. Every script resolves the repository from the hook payload
via `dagayn hook-repo` and passes it as `--repo`, because user-level Cursor hooks
run from `~/.cursor` rather than the project directory. Existing unrelated
Cursor hooks are preserved. MCP config is written to `.cursor/mcp.json` and
synced to `~/.cursor/mcp.json` without a hardcoded `--repo`; at serve time
dagayn resolves the open workspace from Cursor's `WORKSPACE_FOLDER_PATHS`.

`dagayn install --platform codex` configures the Codex MCP server, installs
Codex skills, and writes global Codex hooks in `~/.codex/hooks.json` with the
required `~/.codex/config.toml` feature flag. Claude hooks are written to
`~/.claude/settings.json`, and dagayn's Claude Code skills to
`~/.claude/skills/<name>/SKILL.md` only: Claude Code also loads
`<repo>/.claude/skills`, so install removes the untracked dagayn copies an
earlier install left there (committed copies stay). Git hooks installed by `dagayn install` refresh
cheaply with `dagayn update --skip-flows` before commit-time checks and run a
full `dagayn update` after a commit. Generated AI-tool edit hooks enqueue a
structure-only update (`dagayn queue add update`) instead of running one
inline; the queue worker marks the run it executes with
`DAGAYN_HOOK_UPDATE=1` (budget watchdog, skip-when-busy write lock) and
honors `.dagayn/hook-skip`. Local embedding sidecar arguments are applied to
the session-start prepare only, and remote embedding modes are only baked
into the MCP serve command.
`--no-hooks` skips the hook files.

Install embedding modes are baked into the generated MCP serve command:
`dagayn install --mode local-embedding` writes
`dagayn serve --local-embedding` for the managed BGE-M3 GGUF sidecar;
`dagayn install --mode local-embedding-llama --preset low` writes
`dagayn serve --local-embedding --mode llama-qwen3` for the managed Qwen
sidecar; `--mode remote-embedding --provider <provider>` writes the
corresponding remote provider flag. Legacy mode names such as `fts`, `local`,
`llama-qwen3`, and `remote` remain accepted as aliases.

`dagayn install --platform pi` writes `.pi/mcp.json`, installs skills under
`~/.pi/agent/skills/`, and writes pi-yaml-hooks-compatible hooks under
`~/.pi/agent/hook/`. Install `pi-yaml-hooks` in Pi to activate those hooks.
`dagayn install --platform hermes` writes `~/.hermes/config.yaml` under
`mcp_servers`, installs skills under `~/.hermes/skills/`, and adds shell hooks
to the same config's `hooks:` block.

### Git worktrees

- `dagayn worktree sync`
- `dagayn worktree info`
- `dagayn hook-repo`
- `dagayn session prepare`

<!-- constrained-by ./SESSION-GRAPH-FRESHNESS.md -->

Agent hosts run parallel sessions in linked git worktrees: `claude --worktree`,
the `EnterWorktree` tool, subagents with `isolation: worktree`, Claude Code
desktop sessions, and Cursor's parallel agents. A worktree is a fresh checkout,
so gitignored files — `.mcp.json`, `.cursor/mcp.json`, and the whole `.dagayn/`
graph directory — are not there. Git-backed jj workspaces (`jj workspace add`,
`track` in `vcs-mode=jj`) are treated as linked worktrees of the colocated main
checkout; see [USAGE.md](./USAGE.md#jj-workspaces).

`dagayn session prepare` is the session-lifecycle entry point: seed a linked
worktree when needed, build an empty graph (`postprocess=minimal`), or
incrementally refresh when the graph is `commit_drift` or `worktree_behind`
(a `worktree_ahead` tree is already indexed and stays a noop). Hooks
pass `--budget-seconds 45` and optional `--local-embedding` args from
`dagayn install`. Use `--embedding auto|defer|skip|inline` to control whether
Phase 2 vector refresh runs inside the budget. MCP `ensure_graph_tool`
waits on the same prepare path with a longer budget. MCP
`get_minimal_context_tool` enqueues that prepare on the background queue
and returns immediately (current `sync` plus `repair`/`prepare` queued
state); call `ensure_graph_tool` if you must wait. Both inherit
`serve --local-embedding`. See
[SESSION-GRAPH-FRESHNESS.md](./SESSION-GRAPH-FRESHNESS.md) for the use-case
catalog and structure-ready contract.

`dagayn worktree sync` makes a worktree usable in seconds. It copies the main
checkout's gitignored MCP config and skill files (never overwriting files the
worktree already has), copies `graph.db` via the SQLite backup API so
write-ahead-log content and every embedding come along, then runs an incremental
update against the commit that graph was built at so only the branch diff is
re-parsed. `--seed-only` copies without updating, `--no-copy-config` skips the
config copy, `--build-if-missing` falls back to a full build when the main
checkout has no graph, and `--json` emits a machine-readable result for hook
integrations. Set `DAGAYN_WORKTREE_SEED=0` to disable graph inheritance.

`CRG_DATA_DIR` keeps graph data outside the working tree. Each repository —
and each worktree — gets its own subdirectory of it,
`<CRG_DATA_DIR>/<name>-<identity digest>`: the digest is the directory's
inode identity (falling back to a case-folded path), so one checkout maps to
one graph even when the path is spelled with different case on macOS/Windows.
Looking up where a graph would live (`db_path_for`) creates nothing; a stale
registry entry for a deleted or moved repo is reported as stale instead of
resurrecting `<gone>/.dagayn`. The variable used to be honored verbatim, so
every checkout that saw it shared a single `graph.db` and one project's nodes
silently mixed into another's. Because each worktree now has its own graph,
inheritance applies under `CRG_DATA_DIR` too. A pre-existing
`<CRG_DATA_DIR>/graph.db` moves into the subdirectory when its `repo_root`
metadata names that repository, so a single-repository setup keeps its graph;
a graph belonging to another repository is left untouched.

The same inheritance runs automatically at `dagayn serve` startup and before
`dagayn update` / `dagayn status`, so an MCP session that opens in a worktree
has a graph without any manual step. An empty schema-only `graph.db` stub
(0 nodes) — the kind `status` creates when the file is missing — is treated as
absent and replaced by inheritance. Session-start hooks call
`dagayn session prepare` (budgeted structure sync + optional embeddings),
which seeds linked worktrees and refreshes HEAD/worktree drift.

`dagayn install` wires both hosts' worktree-bootstrap mechanisms:

- **Claude Code** — a managed block in `.worktreeinclude` listing the gitignored
  MCP config files dagayn wrote. Claude Code copies files matching that file into
  every worktree it creates; commit it so worktree sessions keep their MCP tools.
- **Cursor** — a `dagayn session prepare --budget-seconds 45` entry in
  `.cursor/worktrees.json`, which Cursor runs inside each worktree it creates
  for a parallel agent. User commands in that file are preserved; when it
  delegates to a setup script, install prints the line to add yourself.

Running `dagayn install` from inside a worktree also configures the main
checkout, and git hooks are installed into the repository's shared hooks
directory (resolved through `git rev-parse --git-common-dir`, honoring
`core.hooksPath`), so one install covers every worktree.

`dagayn hook-repo` reads an agent hook's JSON payload on stdin and prints the
repository root it refers to, resolving `workspace_roots`, `file_path`, and
`EnterWorktree` tool responses through `git rev-parse --show-toplevel`. The
generated Cursor hook scripts use it because user-level Cursor hooks run with
their working directory set to `~/.cursor` rather than the project.

### Multi-repo management

- `dagayn register`
- `dagayn unregister`
- `dagayn repos`
- `dagayn daemon ...`

Copy-paste register → search and daemon recipes: [RECIPES.md](./RECIPES.md#multi-repo-registry--search).

## MCP tools

<!-- constrained-by ./ARCHITECTURE.md#query-surfaces -->
<!-- derived-from ./refactor-tool-suggest-spec.md -->
<!-- Plan context: ./plans/ANALYSIS-TOOL-STRATEGY.md#tool-tiers; not a graph dependency because stable command docs are canonical. -->

The compact default MCP surface exposes tools for:

- minimal context retrieval
- safe graph bootstrap (`ensure_graph_tool`)
- impact radius and review context
- graph queries and traversal
- semantic search
- flows and communities
- architectural hotspot analysis
- refactor previews and suggestions

Advanced and maintenance tools for graph build/post-processing, embeddings,
wiki generation, refactor application, and cross-repo search remain available
when explicitly requested with `--tools`.

When the server is launched with `dagayn serve --local-embedding`,
search-oriented MCP tools default to the managed OpenAI-compatible BGE-M3 GGUF
endpoint. Use `dagayn serve --local-embedding --mode llama-qwen3` for the
managed OpenAI-compatible Qwen endpoint. Either path makes `semantic_search_nodes`,
`traverse_graph`, and `cross_repo_search` run hybrid FTS + embedding retrieval
unless the client explicitly passes another provider or model.

A long-lived `dagayn serve` that hits SQLite `SQLITE_CORRUPT` (`database disk
image is malformed`) closes live graph handles and retries the tool once.
Restart the MCP server if the retry still fails; rebuild the graph only when
`PRAGMA quick_check` on `.dagayn/graph.db` is not `ok`. See
[TROUBLESHOOTING.md](./TROUBLESHOOTING.md#mcp-tools-report-database-disk-image-is-malformed).

Default tool names are:

- `get_minimal_context_tool`
- `ensure_graph_tool`
- `review_tool`
- `flow_tool`
- `architecture_analysis_tool`
- `refactor_tool`
- `query_graph_tool`
- `semantic_search_nodes_tool`
- `get_docs_section_tool`

`refactor_tool(mode="rename")` returns a `refactor_id` and an edit list. The
`refactor_id` is session-scoped (in-memory, expires after 10 minutes), so
apply the preview with `apply_refactor_tool` in the same `dagayn serve` MCP
session (expose it via `dagayn serve --tools all`); a fresh `dagayn tool`
process cannot resolve an id from an earlier invocation. Prompts, hints, and
next-step suggestions on the default surface name only default-surface tools.

`ensure_graph_tool` bootstraps an empty graph, or refreshes when HEAD/worktree
has drifted (and always with `force=True`), using `postprocess="minimal"`.
Local embedding mode inherits `dagayn serve --local-embedding`. Prefer it on
the default MCP surface; keep `build_or_update_graph_tool` for explicit
maintenance via `--tools all` or `dagayn tool`.

`dagayn session prepare` is the CLI entry point used by SessionStart /
EnterWorktree / relocate hooks. It runs a budgeted Phase 1 structure sync and
an optional Phase 2 embedding refresh (`--embedding auto|defer|skip|inline`,
`--budget-seconds`). Hooks default to a 45s budget; MCP auto-prepare uses a
longer budget so deferred embeddings can finish.

`get_docs_section_tool` reads a section from `docs/LLM-OPTIMIZED-REFERENCE.md`
so skills can fetch their optimized workflow without duplicating it in the
skill file.

`query_graph_tool` includes documentation-aware bridge patterns in addition to
ordinary code relationships. Use `source_of` to fetch the live worktree span
for one chosen node (function, class, Markdown section, or file) instead of
re-reading the whole file; the slice is capped and sets `truncated` /
`source_stale` when the graph span or file hash is no longer authoritative.
Use `docs_for` to find specifications, runbooks,
issue notes, and explanations linked to a code, Terraform, or artifact node.
Use `implementations_of` to find code or Terraform nodes linked to a Markdown
contract section through `implemented_by` / `implements_contract`
`CROSS_ARTIFACT` edges. Use `bridges_from` to follow high-confidence Terraform
→ application-code bridges (`maps_entrypoint`, `invokes_binary`) emitted from
`filename` / `source_dir` / `handler` / `entry_point` / `local-exec` patterns.
Review impact / flow / architecture guidance also surfaces reportable bridges
as first-class transitions and keeps low-confidence bridges as missingness
caveats rather than hard claims.

`callers_of` and `importers_of` accept `depth` (1 to 6, default 1) to follow
the chain transitively in one call. Rows past hop 1 carry `depth` and `via`
(the node they reach the previous hop through), and each node appears once, at
its shortest hop. The response adds `depth` and a `reachability` object:
`state` is `complete` or `truncated` (500 rows past hop 1), and
`depth_limit_reached` is true when the walk stopped at `depth` with nodes whose
own callers or importers were not checked. Hops past the first follow resolved
edges only; the bare-name fallback that hop 1 of `callers_of` uses when a node
has no resolved callers is not repeated further out. Other patterns reject
`depth` other than 1.

`query_graph_tool` has three detail levels. `standard` (default) returns one
row per related node; for `callers_of`, `callees_of`, `inheritors_of`, and
`importers_of`, the edge lines are folded into the row as `lines` with the
edge's `confidence_tier`, and rows drop `id`, `language`, default
`parent_name` / `is_test`, and a `file_path` already in `qualified_name`.
It omits `answerability` and `edges`. `minimal` keeps fewer row fields, drops `guidance` and
`description`, and returns every row that fits a 2,000-token budget. `full`
returns the earlier `standard` shape: one row per edge, the `edges` list, full
`answerability`, and `_hints`.

`traverse_graph_tool` returns both the legacy top-level `truncated` boolean and a
typed `reachability` object. `reachability.state` is `complete`, `truncated`, or
`not_found`, so callers can distinguish a fully explored budgeted neighborhood
from a partial traversal or a missing start node without inferring from text.

### MCP tool surface

<!-- Plan context: ./plans/ANALYSIS-TOOL-STRATEGY.md#mcp-tool-surface-plan; not a graph dependency because stable command docs are canonical. -->

`dagayn serve` exposes a compact workflow surface by default. Dagayn v3 removed
named tool profiles; specialized analysis now lives behind dispatcher tools
such as `review_tool`, `flow_tool`, and `architecture_analysis_tool`.

```bash
dagayn serve
dagayn serve --tools query_graph_tool,semantic_search_nodes_tool
dagayn serve --tools all
dagayn tool architecture_analysis_tool --arg mode='"overview"'
dagayn tool architecture_analysis_tool --arg mode='"adp_violations"' --arg artifact_scope='"docs"'
dagayn serve --local-embedding
dagayn serve --local-embedding --mode llama-qwen3
dagayn serve --remote-embedding openai
```

`--tools` is an exact comma-separated allow-list for deployments that need a
different public surface. The same allow-list can be supplied with `CRG_TOOLS`;
use `all`, `full`, or `*` to expose every registered advanced/maintenance tool.
Tool filtering is applied when `dagayn serve` starts; a running MCP server does
not reload a broader allow-list dynamically. Use
`dagayn tool <tool-name>` for ad-hoc shell access without restarting the
agent's MCP server.

When `dagayn serve --local-embedding --mode llama-qwen3` starts a managed local
embedding sidecar, MCP `semantic_search_nodes_tool` automatically searches with
the matching OpenAI-compatible provider, Qwen model, and `material` text mode. When
`--remote-embedding {openai,google,minimax}` is set, MCP search automatically
uses that remote provider unless the client explicitly passes a different
`provider`. If no `--remote-embedding` flag is supplied, `serve` infers a remote
default only when exactly one provider's required environment variables are
configured.

`review_tool(mode="changes")` is the primary change-analysis surface. It lists
what a reviewer must check before merging that the diff does not show (target
contract: [REVIEW-TOOL-TARGET.md](./plans/REVIEW-TOOL-TARGET.md#target-contract)).
Read `findings` first. Each finding is one checkable claim: a `kind`, the place
to look (`qualified_name` and/or `file`; a grouped finding lists `targets`), a
`claim`, the graph facts behind it (`evidence`, or the referencing `sites`), and
an `action`. Kinds:

- `dangling_reference`: a symbol or file the change removed, renamed, or moved
  is still referenced outside the change. One finding per removed symbol, with
  its `sites`. The base side comes from re-parsing each changed file at `base`.
- `unchanged_caller`: a function gained a required parameter or lost
  parameters, and callers outside the change were not edited. A new optional
  or defaulted parameter does not fire.
- `contract_doc_not_updated`: an authored contract doc (`implemented-by` /
  `implements`) is linked to changed code and was not edited.
- `bridge_touched`: the change edits the source side of a reportable
  cross-artifact bridge (manifest, Terraform, FFI) and not the other side.
- `unstable_dependency`: the change makes a declared unit depend on a less
  stable one (instability gap above 0.1, `strict_static` imports): every
  edge behind the dependency is on a line the diff touches, and the base
  version of those files did not name the unit depended on. Evidence: both
  units' afferent and efferent counts and instability, the sites, and the
  SAP abstractness and distance of the unit depended on where SAP applies.
- `untested_change`: changed production functions with no test reaching them,
  directly or through callers up to 4 hops. A dunder method or a property or
  validator counts as called wherever its class is, and a Python module that
  builds the instance at its top level (or defines the `__getattr__`) as
  called wherever it is imported. One finding per file; tests,
  `build.rs`, `examples/`, benches, fixtures, and generated code are excluded.
- `tests_to_run`: direct tests of the changed code, and changed tests. One
  finding per test file (Rust unit tests: one per crate), with a `command`.

Comment- and layout-only edits are not changes (Python is compared by syntax
tree). Each kind keeps 10 findings; `findings_omitted` counts the rest per
kind. An empty `findings` list means nothing beyond the diff needs checking,
and `summary` says "Nothing beyond the diff needs checking."; otherwise it
reads "N changed file(s), M changed symbol(s). Findings: ...". `next` makes
the first findings runnable: `source_of` on a finding's symbol, or a
`tests_to_run` command as `{"tool": "shell"}`. Findings rest on graph edges and the base-side
re-parse: confirm one with `source_of` or a reproduction before calling it a
bug.

Every detail level also carries `base` (the base actually used),
`changed_file_count`, `changed_files`, `change_file_source_counts`,
`change_entity_summary`, `affected_flow_count`, `unmapped_changed_files`,
`next`, and `missingness`. `standard`
(the default) adds `changed_functions` and `affected_flows`; a flow there keeps
its summary and `changed_steps` (only the steps the change touches), and
`mode="affected_flows"` returns the full steps. Output is bounded by size, not
only by item count.

`detail_level="verbose"` adds the deprecated score-first fields of the earlier
contract: `analysis_summary` (with `risk_level`, `reason_codes`, `guidance`,
recommended tests, documentation candidates, and `stability_contracts`),
`risk_score`, `review_priority_score`, `score_semantics`, `review_priorities`,
`test_gaps`, `test_gap_evidence`, and `changed_edges`, plus `symbol_delta`,
`change_file_sources`, and `deprecated_fields`, which names them. They stay for
one release and then go; do not build new consumers on them.

With no `base`, a checkout whose tracked files have staged or unstaged edits is
reviewed against `HEAD` (the work in progress); a clean one against `HEAD~1`
(the last commit). Untracked files count either way. To review a branch, pass
its merge base with `main`. An explicit `changed_files` list scopes the review
to those files; the base diff only narrows them to their changed lines. The
`context`, `impact`, and `affected_flows` modes are unchanged.

`get_minimal_context_tool` routes common English and Japanese task descriptions
for review, debugging, exploration, feature addition, and refactoring to the
first calls of that workflow, as `next` (each with its arguments: the
task's text as a search query, a review at `detail_level="minimal"`,
`ensure_graph_tool` first on an empty or drifted graph). It also returns
`workflow`, `why`, and `confidence`. It includes compact
`graph_health` answerability metadata and a `sync` object carrying the
freshness `state` (`unbuilt` / `commit_drift` / `commit_synced` /
`worktree_behind` / `worktree_ahead`) alongside the legacy `status`
(`empty` / `git_drift` / `dirty_worktree` / `synced`) for older clients. On the
MCP surface it auto-runs `session prepare` when unbuilt or commit-drifted
(inheriting `serve --local-embedding`). When `graph_health.status` is still
`empty` / `sync.state` is still `unbuilt` after prepare, it points at
`ensure_graph_tool` first.
`parse` is `[files, languages,
has_last_updated]`; `answerability` is `[flows, communities, test_edges,
reportable_cross_artifact_edges, unresolved_cross_artifact_ratio]`. Unresolved
Markdown code-span candidates are excluded from these answerability counts
because post-processing treats them as prose vocabulary unless they resolve
uniquely to a non-Markdown symbol.

Responses of `query_graph_tool`, `semantic_search_nodes_tool`, `review_tool`,
`flow_tool`, `architecture_analysis_tool`, and `refactor_tool` include a
`missingness` list, including error and not-found paths: the gaps that limit
that answer (a graph of another commit, uncommitted edits, missing test edges,
unresolved cross-artifact edges, missing embeddings, truncated output). Gaps in
communities and stored flows (`missing_flows`, `missing_communities`,
`stale_derived_structures`) appear only on answers read from them. The
graph-wide `answerability` summary (`status`, `score`, `reason_codes`, counts)
is `get_minimal_context_tool`'s `graph_health`; the six tools add it only at
`detail_level="verbose"` (`"full"` for `query_graph_tool`) and never on an
error. A zero-result response should be read as "not
found in the current graph" unless the surrounding source review confirms
absence.

`architecture_analysis_tool` is the primary architecture-analysis surface. Start
with `mode="overview"` and `detail_level="minimal"`. Output includes
`architecture_health`, which composes community coupling, hubs, bridges,
knowledge gaps, surprising connections, and ADP/SDP/SAP signals into a bounded
health summary with drill-down mode hints. These warnings are review leads, not
verdicts. The overview reports formulas, thresholds, `artifact_scope`, guidance
items, and `stable_component_policy` so review, architecture, and refactor
surfaces use the same stability expectations.

ADP/SDP/SAP modes default to `artifact_scope="code"` so Markdown dependencies
and code dependencies are not mixed in design-principle metrics. Pass
`artifact_scope="docs"` to inspect documentation dependency cycles or stability,
or `artifact_scope="all"` for the legacy mixed projection.
They also accept `dependency_profile`: `strict_static` preserves the historical
`IMPORTS_FROM` / `DEPENDS_ON` / `INHERITS` / `IMPLEMENTS` edge set,
`implementation` adds `CALLS`, `infra_dataflow` adds Terraform-style
`REFERENCES`, and `artifact_trace` adds high-confidence `CROSS_ARTIFACT` edges.
Unknown profile names are rejected instead of falling back silently.
`sap_metrics` separates SAP-inapplicable scopes (`no-eligible-types` and
`isolated`) into `inapplicable_metrics` by default; use
`detail_level="verbose"` for raw metric inspection.

The minimal search-ranking scaffold lives in `eval/search_queries.yaml`,
`eval/search_judgments.yaml`, and `eval/run_search_eval.py`. It reports MRR@10,
NDCG@10, Recall@20, exact-symbol success@5, prose-intent success@10,
doc-vs-code confusion rate, and test crowding rate. Follow-up work should add
larger judgment sets before changing ranking weights.

Migration note for dagayn 3.0: v2 split architecture MCP/CLI tools such as
`get_architecture_overview_tool`, `list_communities_tool`,
`get_hub_nodes_tool`, `compute_sdp_metrics_tool`, and
`detect_sap_violations_tool` were removed from the public surface. Use
`architecture_analysis_tool(mode=...)` instead.

Review and execution-flow drill-downs are also dispatcher-based in v3. Use
`review_tool(mode="changes"|"context"|"affected_flows"|"impact")` and
`flow_tool(mode="list"|"get")` instead of the v2 split MCP/CLI tools.

`flow_tool(mode="entry_points", target=...)` answers which entry points reach
a symbol: the nearest `main`, framework handler, FFI export, conventionally
named entry, uncalled function, or method only a trait or framework calls
(`dispatched_method`) on each path, with one shortest call chain each. It is
computed at query time and needs no stored flows; test code is never walked.
Without `target` it lists the repository's entry points per declared unit,
counted by kind. `mode="list"` and `mode="get"` read stored flows and are
deprecated.

`refactor_tool(mode="suggest")` answers with `findings`, refactors worth doing,
10 per kind with the rest in `findings_omitted`: `unused_symbol` (the verified
dead-code report without test fixtures), `complex_hotspot` (a function past the
split thresholds whose lines changed in 5 or more commits in the last 90 days,
from `git log -L`), and `undocumented_surface` (one of the three symbols other
units use most from a unit, without a docstring or doc comment). An empty list
means nothing worth doing. The earlier size-based `suggestions` (remove, split,
document) are only in `detail_level="verbose"` for one release, listed in
`deprecated_fields`. Findings and suggestions are leads (structure, not correctness): confirm them
with `source_of` or a reproduction on a current graph (see the `trust` section
of `get_docs_section_tool`). Verify public APIs,
test artifacts, dynamic dispatch, and generated entry points before changing
source. `plans` states, once per suggestion type, the minimum safe steps,
safety checks, rollback guidance, and defer conditions; `work_packs` gives the
first five suggestions a first commit scope, blast radius, required tests, and
verification commands (`detail_level="minimal"` leaves it out).
`detail_level="verbose"` returns the earlier layout: an `execution_plan` and
`work_pack` on every suggestion, and for function splits a
`concern_separation` profile in `evidence`. A rename preview lists the first
20 `edits` with `edits_omitted` and per-file `files` counts; the pending store
keeps every edit for `apply_refactor_tool`, and `verbose` returns them all.

`semantic_search_nodes_tool` and `query_graph_tool` report result counts,
exactness or ambiguity, evidence type, zero-result reason, and `next`: the
first hits' `source_of`, or one retry per candidate of an ambiguous target. Mixed docs/code hits are labelled so a Markdown body hit is not confused
with a code symbol hit.
For `query_graph_tool`, missing targets use the same consumer contract as empty
relationship results: `status="not_found"`, `result_count=0`, `results=[]`,
`zero_result_reason="target_not_found_in_graph"`, `next`, and
`missingness`.
A bare target name resolves to its node when exactly one node carries that exact
name (`resolution="exact_name"`, with `original_target`), even when fuzzy search
ranks look-alike names higher; several exact-name matches return
`status="ambiguous"` with only those as `candidates`. Successful responses carry
`results_complete`, which is false only when the output budget trimmed the
result list itself; at `detail_level="full"`, `truncated` also turns true when
only `edges` were trimmed. `standard` and `full` list the same related nodes,
and `full` reaches the output budget sooner.
With `depth` above 1, `reachability` reports whether the transitive set is
closed (`state="complete"`: no other node is reachable over graph edges), cut
off by the row limit or output budget, or stopped at `depth` with nodes still
ahead; in the last case `next` starts with the same call at `depth=6`.

`architecture_analysis_tool(mode="knowledge_gaps", top_n=20)` returns bounded
structural weakness categories with explicit thresholds and raw counts.
Untested-hotspot candidates are ranked against the repository's observed
production-node degree distribution rather than a fixed language-specific size
rule; scoped runs still use each scoped code node's full graph degree for this
hotspot ranking so documentation and test relationships can contribute to
impact without being returned as code findings. In `artifact_scope="code"`,
structural modes exclude test-like nodes by default and report low-signal
findings separately under
`classified_noise_counts` / `classified_noise_examples`, including public API
candidates, conventional entry points, Rust `#[cfg(test)]` nodes,
implementation-block containers, and small single-file clusters. Single-file
community findings include `internal_edges`, `external_edges`,
`external_degree`, `cohesion`, and `external_edge_ratio`; large one-file
communities with enough external graph connectivity are classified as
`integrated_single_file_component` noise instead of being returned as knowledge
gaps. The returned category order favors review value: `untested_hotspots`,
`single_file_communities`, `isolated_nodes`, then `thin_communities`.

## MCP prompts

<!-- constrained-by ./ARCHITECTURE.md#pipeline-overview -->

The fork ships prompt surfaces for:

- review changes
- architecture mapping
- issue debugging
- onboarding
- pre-merge review
