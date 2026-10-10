# dagayn LLM reference

<!-- derived-from ./COMMANDS.md -->

<section name="usage">
Install with `pip install dagayn` or `uv tool install dagayn`, then run `dagayn install`.

First graph: `ensure_graph_tool()` on the default MCP surface, or `dagayn build` from the CLI.
Routine refresh: hooks, `dagayn update`, `dagayn watch`, or `ensure_graph_tool(force=True)` when stale.

Linked worktrees: `dagayn worktree sync` (or the `worktree-sync` skill) inherits the main checkout graph/MCP config before analysis.

Feature work: find extension points with search/query/flow, implement, then `review_tool(mode="changes")` (see the `implement-feature` skill). After code changes, follow `contract_doc_not_updated` findings and `docs_for` links with the review-changes docs-update flow.

`dagayn serve` exposes the compact workflow MCP surface by default (`get_minimal_context_tool`, `ensure_graph_tool`, `review_tool`, `flow_tool`, `architecture_analysis_tool`, `refactor_tool`, `query_graph_tool`, `semantic_search_nodes_tool`, `get_docs_section_tool`). Use an exact `--tools` or `CRG_TOOLS` allow-list for a different surface; `all`, `full`, or `*` exposes every advanced/maintenance tool.

After a search hit, `query_graph_tool(pattern="source_of")` returns the live
worktree span for one node (capped). Prefer it over opening the whole file.

Use `dagayn` in all user-facing guidance.
</section>

<section name="workflow">
Orient → locate → read → trace → judge → confirm. Enter at the phase the task
needs, and close the loop after an edit with `review_tool`.

1. **Orient**: `get_minimal_context_tool(task=...)` reports `sync.state` and
   the first calls for the task.
2. **Locate**: `semantic_search_nodes_tool` turns a description into a
   `qualified_name`.
3. **Read**: `query_graph_tool(pattern="source_of")` returns the live span.
4. **Trace**: `query_graph_tool` (`callers_of`, `callees_of`, `tests_for`,
   `docs_for`) and `flow_tool(mode="entry_points")`.
5. **Judge**: `review_tool`, `architecture_analysis_tool`, or
   `refactor_tool` answer with `findings`; an empty list means nothing to act
   on.
6. **Confirm**: `source_of` on each finding's place, a test, or a
   reproduction.

Every reply ends with `next`: at most three calls with complete arguments and
a `why`. Follow it unless the task points elsewhere; `[]` means the answer is
complete. `status="ambiguous"` puts one retry per candidate in `next`, and
`missingness` lists only the gaps that limit that reply. Full contract:
`get_docs_section_tool(section_name="workflow")`.

The reply contract of `query_graph_tool`, `semantic_search_nodes_tool`,
`review_tool`, `flow_tool`, `architecture_analysis_tool`, and `refactor_tool`:

- `next` items are `{"tool", "args", "why"}`; pass `args` unchanged.
  Arguments at their defaults are left out. A test to run is
  `{"tool": "shell", "args": {"command": ...}}`.
- Size: `detail_level="minimal"` fits in 8K characters and `"standard"` in
  32K, whatever the input. What does not fit is counted (`truncated`,
  `_truncation`, `*_omitted`, `total`), never dropped silently: narrow the
  call before concluding from a trimmed list.
- `missingness` names the gaps that limit this reply (another commit,
  uncommitted edits, a truncated search, an ambiguous target). Gaps in
  communities and stored flows appear only on answers read from them. The
  graph-wide `answerability` summary is `get_minimal_context_tool`'s
  `graph_health`; the six tools add it only at `detail_level="verbose"`
  (`"full"` for `query_graph_tool`).
- `query_graph_tool(pattern="tests_for")` carries `test_reach`: the nearest
  test as `review_tool`'s `untested_change` counts it (`hops`,
  `nearest_test`, `counts_as_tested` within `hop_limit`, which is `null`,
  no limit, for Rust). No test there
  means no path in the graph, which a test run may still cover.
- The earlier next-step fields (`next_action`, `exactness.next_action`,
  `next_drill_downs`, `next_tool_suggestions`, `_hints`) appear only at
  `detail_level="verbose"` (`"full"` for `query_graph_tool`), named in
  `deprecated_fields`, for one release. `get_minimal_context_tool` no
  longer returns `recommended_action` or `next_tool_suggestions`.
- Below `verbose` a reply leaves out what is diagnosis or restatement:
  `_runtime`, `called_subtool`, `guidance` (its claim is `summary`, its
  caveats `missingness`, its action `next`), and search's
  `embedding_health` (`search_mode` and `missingness` say when embeddings
  limit a search); `_repo` names only `repo_root`.
</section>

<section name="trust">
How much a graph answer proves. Reach comes from the graph, correctness from
`source_of`, and user-visible effect from a reproduction or CLI output; a claim
should say which of the three it rests on.

Freshness first: `get_minimal_context_tool` reports `sync.state`. On
`commit_synced` or `worktree_ahead` the graph describes the working tree. On
`commit_drift` or `worktree_behind`, edges of files changed since the build may
be out of date, so answers about those files are Low until `ensure_graph_tool()`
refreshes them (`force=True` for `worktree_behind`: a plain call leaves
uncommitted edits to the edit hooks). `graph_health.reason_codes` says why the
graph is short: `missing_flows` / `missing_communities` mean post-processing
has not run (not that edges are wrong), and `status` can still read `ok` with
them.

Highest (state it as fact):
- `HIGH` and `EXTRACTED` edges. The parser settles them from what the code
  writes (same-file definitions, imports, typed receivers, standard-library and
  package calls); `dagayn build --scip` lets a compiler-grade index settle them
  (`resolved_by: "scip"`). Both count.
- `source_of` spans: the live source of a node.
- Authored doc contracts: `implemented_by` / `implements_contract` links with
  `evidence_type=authored` whose target exists (a section or symbol missing
  from an indexed file is demoted to `LOW`). `evidence_type=authored` alone
  is not a contract: every Markdown result carries it, search hits included.

Medium (structure, not correctness):
- `MEDIUM` edges: inferred from usage (observed methods, return-type tables).
- `review_tool` `findings`: each is a claim to check, resting on graph edges
  and a base-side re-parse; open its `sites` or place with `source_of` before
  calling it a bug. An empty list means nothing beyond the diff.
- Blast radius and affected flows; `reason_codes` from architecture,
  refactor, and answerability; architecture metrics and rankings; refactor
  suggestions and dead-code candidates.
- `TESTED_BY` edges: they follow the calls a test makes, so they are as good
  as those calls.
- Explanatory doc links (`explained_by`, `has_runbook`, `describes_symbol`, …)
  and directive dependencies.
- Search hits: discovery, not proof, whether hybrid or `fts_only`.

Low (a hypothesis until `source_of` or a reproduction confirms it):
- `LOW` edges: calls nothing settled; possible hidden callers.
- `heuristic_reachable` doc links (bare code-span mentions).
- A file-level `tests_for` of 0, an empty `callers_of`, or any other absence:
  read `zero_result_reason`, `next`, and `missingness` before claiming
  something does not exist.
- `truncated` results (narrow first) and `status="ambiguous"` (pick a
  candidate first).
- Answers about files changed since the build (see freshness).

Promote a Medium or Low lead by confirming it: `source_of` for behavior, a
test or CLI run for an effect.
</section>

<section name="review-delta">
Recommended sequence for reviewing a delta:

1. `get_minimal_context_tool(task=...)` — enqueues background prepare when empty or out of sync (`sync.status`) and returns immediately; call `ensure_graph_tool()` if you must wait for the graph to be ready
2. `review_tool(mode="changes")` and read `findings` first; an empty list means nothing beyond the diff needs checking
3. Call `review_tool(mode="context")` / `mode="impact"` / `query_graph_tool` only for concrete source, blast-radius, or coverage questions
4. Read only the files that remain ambiguous after graph queries. After a
   concrete `qualified_name`, prefer `query_graph_tool(pattern="source_of")`
   over a whole-file read.

`review_tool(mode="changes")` returns `findings`, each a checkable claim with a
`kind`, the place to look, its `evidence` or `sites`, and an `action`:
`dangling_reference` (a removed, renamed, or moved symbol still referenced),
`unchanged_caller` (new required parameter or fewer parameters, callers not
edited), `contract_doc_not_updated`, `bridge_touched`, `unstable_dependency`
(the change makes a unit depend on a less stable one), `untested_change`, and
`tests_to_run` (with a `command`). Each kind keeps 10; `findings_omitted`
counts the rest. With no `base`, uncommitted edits to tracked files are
reviewed against `HEAD`, a clean checkout against `HEAD~1`.

The fork is designed to work well when docs, app code, and Terraform all change together.
</section>

<section name="review-pr">
Recommended sequence for reviewing a PR or branch:

1. `get_minimal_context_tool(task="PR review")`
2. Refresh only when empty/stale: `ensure_graph_tool()` or `ensure_graph_tool(force=True)`; use `build_or_update_graph_tool(base="main")` only on the advanced surface when an explicit base ref is required
3. `review_tool(mode="changes", base=<merge base of the branch and main>)` and read `findings` first (plain `base="main"` also counts commits that landed on `main` after the branch point, as reversed changes)
4. Prefer `review_tool(mode="context")` snippets, or `query_graph_tool(pattern="source_of")` for one `qualified_name`, over full-file reads; use `mode="impact"` and relationship `query_graph_tool` only to follow up a finding

If the PR touches infrastructure, assume Terraform nodes and references are part of the review surface.
</section>

<section name="commands">
Important CLI commands:

- `dagayn install`
- `dagayn build`
- `dagayn update`
- `dagayn session prepare`
- `dagayn watch`
- `dagayn status`
- `dagayn detect-changes`
- `dagayn tool`
- `dagayn visualize`
- `dagayn serve`
- `dagayn sdp-metrics` / `dagayn detect-sdp`
- `dagayn sap-metrics` / `dagayn detect-sap`
- `dagayn profile`
- `dagayn register` / `dagayn repos` / `dagayn daemon`

`dagayn build --scip` settles call targets with the SCIP indexers installed for the repository's projects (Rust and TypeScript answers replace the extractor's; Go, Python, Java/Kotlin, C/C++, C#, Ruby, Dart, PHP, and R fill what it left unresolved). Missing indexers print a `hint:` and keep dagayn's own resolution. Edges an index settled carry `resolved_by: "scip"`.

`dagayn serve` exposes the compact workflow MCP surface by default. Use `--tools` when a deployment needs an exact allow-list; `--tools all` exposes every advanced/maintenance tool.

Tool filtering is fixed at MCP server startup. For ad-hoc CLI access, use `dagayn tool <mcp-tool-name>` with
`--arg KEY=VALUE` or `--json-args '{...}'` to invoke the same implementation
from the CLI.

`dagayn install --platform codex` also writes `~/.codex/hooks.json` and enables
Codex hooks in `~/.codex/config.toml`, unless `--no-hooks` is used. Claude hooks
are written to `~/.claude/settings.json`.

`architecture_analysis_tool(mode="overview")` returns a map of the units the
repository declares (`units`: Cargo crates, npm packages, Go modules, Python
import packages, Terraform modules; `unit_edges`: the calls, imports,
references, and bridges between them, `declared` when a manifest lists the
dependency) and `findings`: `import_cycle` (modules that import each other at
load time, with the imports to cut), `unstable_dependency` (a unit that depends
on a less stable one, with the imports that make it and the SAP position of
the unit depended on), `untested_core` (code used from many files
that no test reaches through its callers), and `broken_doc_link` (a directive
pointing at a file, section, or symbol that is gone). An empty `findings` list
means nothing structural to act on. `detail_level="standard"` adds each unit's
`surface`; the community health report (`architecture_health`) is at
`"verbose"` until the next release. SDP and SAP modes compute per declared unit.

`dagayn visualize` is the static graph export surface. It requires `--format` and supports `graphml`, `mermaid-c4`, `svg`, `cypher`, and `obsidian`.
</section>

<section name="legal">
`dagayn` is an MIT-licensed fork of `code-review-graph`.

The graph database is local by default. Optional embedding providers may call remote services only when explicitly configured.
</section>

<section name="watch">
Use `dagayn watch` when you want continuous graph refresh during active development.

Use `dagayn update` when you want a one-shot incremental refresh tied to a change set.
</section>

<section name="embeddings">
<!-- derived-from ./LOCAL-EMBEDDINGS.md -->
Embeddings are optional.

Embeddings are additive: with embeddings built, `semantic_search_nodes` merges BM25 and cosine via RRF and returns `search_mode: "hybrid"`; without them it falls back to FTS5-only (`"fts_only"`). The per-result `source` field (`"fts"`, `"embedding"`, `"both"`, `"keyword"`) shows which arm produced each hit. If provider imports are unavailable, keyword-based graph search still works.

For local embeddings during graph refresh, use `dagayn build --local-embedding`
or `dagayn update --local-embedding`. A bare `--local-embedding` runs the
managed BGE-M3 llama.cpp GGUF sidecar with the measured `material` text mode.

For the managed local Qwen sidecar, use
`dagayn build --local-embedding --mode llama-qwen3` or the legacy
`dagayn build --local-embedding low`. dagayn reuses a compatible local
OpenAI-compatible embedding server on localhost or starts one as a subprocess
for the command; the managed preset starts llama.cpp GGUF via `llama-server`.

`ensure_graph_tool` inherits the active `dagayn serve --local-embedding` mode
when refreshing vectors. Direct CLI `dagayn session prepare` / `ensure_graph`
callers default to `none` unless `--local-embedding` is passed.
</section>

<section name="languages">
The fork supports mainstream app languages plus Markdown, notebooks, and Terraform.

Jupyter, Databricks, and marimo notebooks (Python `.py` and Markdown `.md`) are parsed as graph inputs rather than report output formats.

Terraform and Markdown are notable differentiators for this fork's review workflows. Native FTS indexes Japanese with Lindera IPADIC morphemes and overlapping CJK bigrams.
</section>

<section name="troubleshooting">
If results look stale, call `ensure_graph_tool(force=True)` or run `dagayn update` / `dagayn build`.

If integrations are missing, re-run `dagayn install --dry-run` first.

If local type checks disagree with CI, use `uv run pyrefly check` (see `[tool.pyrefly]` in `pyproject.toml`).
</section>
