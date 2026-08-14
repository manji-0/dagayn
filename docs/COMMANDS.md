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
selects `llama-server`.

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
line hunks for files it does not yet track. Standard and minimal tool responses
include `change_file_sources.base_diff`, `worktree`, `staged`, `unstaged`, and
`untracked` buckets alongside the compatibility `changed_files` list.
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

`dagayn eval --benchmark guidance_precision` measures precision@k for
review-guidance outputs such as recommended tests, documentation update
candidates, refactor suggestions, calibrated `guidance` items, stable-contract
warnings, architecture leads, answerability warnings, and guidance field
coverage. Configure cases with `guidance_precision_cases` in an eval YAML file.

```yaml
guidance_precision_cases:
  - name: review-guidance-contract
    kind: guidance_items
    changed_files: ["dagayn/tools/review.py"]
    expected: ["test_gaps", "documentation_update_candidates"]
    k: 3
  - name: answerability-warning
    kind: answerability_warnings
    changed_files: ["dagayn/tools/query.py"]
    expected: ["missing_test_edges"]
    k: 5
  - name: field-coverage
    kind: guidance_field_coverage
    changed_files: ["dagayn/tools/review.py"]
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
dagayn tool flow_tool --arg mode='"list"' --arg detail_level='"minimal"'
dagayn tool architecture_analysis_tool --arg mode='"knowledge_gaps"' --arg top_n=10 --format summary
```

Tool responses include compact `_runtime` metadata (`version`, `pid`,
`python`, and `package_root`) so agents can compare a direct CLI run with a
running MCP server. A long-lived MCP process keeps the implementation it loaded
at startup; after editing or upgrading dagayn, restart `dagayn serve` before
treating MCP output as the same truth source as `dagayn tool`.

`dagayn visualize` is the static graph export command. It requires `--format`
and generates:

- GraphML, Mermaid C4, SVG, Neo4j Cypher, or Obsidian exports via `--format`

Jupyter / Databricks notebooks are supported as graph inputs rather than report
output formats.

`dagayn wiki` writes Markdown pages under `.dagayn/wiki/` from detected graph
communities. Each community page includes members, execution flows,
cross-community dependencies, and code-scoped package-level ADP/SDP/SAP
architecture metrics filtered to the scopes represented by that community.

### Architecture metrics

<!-- derived-from ./USAGE.md -->

These CLI commands compute and detect package/file-level architecture-principle
metrics directly from the graph, independently of the MCP
`architecture_analysis_tool`. They default to `artifact_scope="code"` so
Markdown dependencies are not mixed into code design metrics; pass
`--artifact-scope docs` or `--artifact-scope all` for documentation or legacy
mixed-graph analysis.

- `dagayn detect-adp` — Detect cyclic dependencies (Acyclic Dependencies
  Principle violations). Reports cycles with length and severity.
  - `--granularity {package,file}` (default `package`)
  - `--artifact-scope {code,docs,all}` (default `code`)
  - `--min-cycle-size N` (default 2)
  - `--max-cycle-length N` (default 10)
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
  - `--format {json,text}` (default `json`)
<!-- derived-from ./SAP-METRICS.md -->

- `dagayn sap-metrics` — Compute per-scope abstractness, instability, and
  distance from the main sequence (Stable Abstractions Principle). Returns
  `A`, `I`, and `D` per scope, limited by `--top-n`.
  - `--scope-kind {package,file,directory}` (default `package`)
  - `--unit-filter PREFIXES` — comma-separated scope_key prefixes to restrict
    output
  - `--artifact-scope {code,docs,all}` (default `code`)
  - `--top-n N` (default 30)
  - `--format {json,text}` (default `json`)
- `dagayn detect-sap` — Detect scopes whose distance from the main sequence
  meets `--min-distance`.
  - `--scope-kind {package,file,directory}` (default `package`)
  - `--artifact-scope {code,docs,all}` (default `code`)
  - `--min-distance FLOAT` (default 0.5)
  - `--format {json,text}` (default `json`)

All five commands accept `--repo` to override the repository root
(auto-detected by default). `detect-sap` suppresses test-scope and
fixture-scope entries from the violation list, consistent with the matching
`architecture_analysis_tool(mode="sap_violations")` MCP mode.

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
`afterFileEdit` runs `dagayn update --skip-flows` detached so the editor never
waits, `sessionStart` runs `dagayn session prepare --budget-seconds 45` and
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
`~/.claude/settings.json`. Git hooks installed by `dagayn install` refresh
cheaply with `dagayn update --skip-flows` before commit-time checks and run a
full `dagayn update` after a commit. Generated AI-tool update hooks use a
300-second timeout to tolerate large documentation or mixed-language refreshes,
mark hook-triggered runs with `DAGAYN_HOOK_UPDATE=1`, skip overlapping hook
updates, and reuse local embedding sidecar arguments when a local embedding
install mode is selected. Remote embedding modes are only baked into the MCP
serve command.
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
graph directory — are not there.

`dagayn session prepare` is the session-lifecycle entry point: seed a linked
worktree when needed, build an empty graph (`postprocess=minimal`), or
incrementally refresh when the graph is `commit_drift` or `worktree_behind`
(a `worktree_ahead` tree is already indexed and stays a noop). Hooks
pass `--budget-seconds 45` and optional `--local-embedding` args from
`dagayn install`. Use `--embedding auto|defer|skip|inline` to control whether
Phase 2 vector refresh runs inside the budget. MCP `ensure_graph_tool` /
`get_minimal_context_tool` call the same prepare path with a longer budget and
inherit `serve --local-embedding`. See
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

`refactor_tool(mode="rename")` returns a `refactor_id` and an edit list. Apply
the preview with `dagayn tool apply_refactor_tool` (or expose it on MCP via
`dagayn serve --tools all`). Prompts, hints, and next-step suggestions on the
default surface name only default-surface tools.

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
ordinary code relationships. Use `docs_for` to find specifications, runbooks,
issue notes, and explanations linked to a code, Terraform, or artifact node.
Use `implementations_of` to find code or Terraform nodes linked to a Markdown
contract section through `implemented_by` / `implements_contract`
`CROSS_ARTIFACT` edges. Use `bridges_from` to follow high-confidence Terraform
→ application-code bridges (`maps_entrypoint`, `invokes_binary`) emitted from
`filename` / `source_dir` / `handler` / `entry_point` / `local-exec` patterns.
Review impact / flow / architecture guidance also surfaces reportable bridges
as first-class transitions and keeps low-confidence bridges as missingness
caveats rather than hard claims.

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

`review_tool(mode="changes")` is the primary change-analysis surface. Standard
output includes `analysis_summary` with risk level, reason codes, recommended
tests, affected-flow rankings, documentation update candidates, hotspot
proximity, and architecture risks in changed scopes. It also includes
`analysis_summary.guidance`, a bounded list of calibrated items. Each guidance
item has `claim`, `evidence`, `confidence`, `missingness`, `action`,
`reason_codes`, and `counts`; `_hints.next_steps` is derived from those actions
when guidance is available. `risk_score` remains as a compatibility alias for
`review_priority_score`: it ranks review attention from flow participation,
callers, test evidence, security keywords, and community crossing, and is not a
standalone changeability score. Recommended tests and documentation candidates
keep their existing sections for compatibility, but now expose evidence type
distinctions such as `authored`, `extracted`, and `heuristic_reachable`.
Default guidance uses authored/extracted documentation evidence;
heuristic-reachable Markdown and unresolved low-confidence Markdown code-span
candidates are exploratory leads rather than quality-policy signals. Stable
or should-be-stable components, identified from package-level SDP/SAP metrics,
also produce `stability_contracts` so reviewers can see whether highly
depended-on code has enough test and documentation density. Stable-component
policy gates on `direct_test_density`; `heuristic_test_density` and
`transitive_test_density` are supplemental signals so naming/source leads and
call-chain reachability do not inflate the main density metric. Standard review
output does not run the heavier heuristic/transitive density scan; use
`detail_level="verbose"` when those exploratory coverage leads are needed.
Verbose supplemental density is bounded and reports whether the scope was
sampled/truncated.

`get_minimal_context_tool` routes common English and Japanese task descriptions
for review, debugging, exploration, feature addition, and refactoring to the
next small set of MCP tools. It also returns `workflow`,
`recommended_action`, `why`, and `confidence` so clients can show the next
step without requiring users to know tool names. It includes compact
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

Most dispatcher responses now include `answerability` and `missingness` blocks,
including error and not-found paths. `answerability.status` and `score` describe
how much graph evidence is available for the requested `repo_root`;
`reason_codes` call out partial graphs, missing flows, missing communities,
missing test edges, unresolved cross-artifact edges, missing embeddings, and
truncation-sensitive output. A zero-result response should be read as "not
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

`refactor_tool(mode="suggest")` returns graph-backed remove, move, split, and
document candidates. Treat them as evidence-ranked leads; verify public APIs,
test artifacts, dynamic dispatch, and generated entry points before changing
source. Suggestions include `execution_plan` with minimum safe steps, required
tests, rollback guidance, and defer conditions. Suggestion payloads also include
`work_pack` so agents can pick a first commit scope, success criteria, and
verification commands without inventing a separate planning tool. Work packs
include `blast_radius`, `required_tests`, `documentation_obligations`,
`safe_first_commit`, `rollback_path`, and `defer_conditions`. Split
suggestions for functions may also include a `concern_separation` profile that
reports role-aware single-responsibility pressure, side-effect evidence,
purity likelihood, context clarity, missingness, and a first extraction action.

`semantic_search_nodes_tool` and `query_graph_tool` report result counts,
exactness or ambiguity, evidence type, zero-result reason, and a `next_action`
lead. Mixed docs/code hits are labelled so a Markdown body hit is not confused
with a code symbol hit.
For `query_graph_tool`, missing targets use the same consumer contract as empty
relationship results: `status="not_found"`, `result_count=0`, `results=[]`,
`zero_result_reason="target_not_found_in_graph"`, `next_action`,
`answerability`, and `missingness`.

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
