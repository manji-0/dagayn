# Changelog

All notable changes to `dagayn` are documented here.

## Unreleased

## 4.4.1 — 2026-07-20

### Fixes

- Re-export `_git_branch_info` from the incremental facade so `dagayn status`
  no longer raises `ImportError` when printing branch metadata.
- Warn from `dagayn status` when the working copy git commit or SVN
  path/revision has drifted from the graph build metadata.
- Drop removed in-process `local:` embedding specs from the material-model
  benchmark and require OpenAI-compatible sidecar URLs instead.
- Widen Cursor `beforeShellExecution` matching so path-qualified `git`
  binaries (for example nix-profile absolute paths) still trigger the
  pre-commit graph refresh, and replace existing `crg-*` hook entries on
  reinstall so matcher updates take effect.

### Documentation

- Document Cursor install hooks and VCS drift warnings for `dagayn status`.

## 4.4.0 — 2026-07-18

### Improvements

- Update Terraform parsing to `tree-sitter-terraform` v0.2.0 and recognize 17
  additional block types.
- Expand the VS Code extension with multi-root workspace support, module
  dependency visualization, saved custom queries, node documentation panels,
  blast-radius snapshot save/compare, editor context-menu commands, and
  auto-update failure notifications.
- Align skill examples with current tool signatures and remove the obsolete
  `wiki-research` skill.

### Fixes

- Pass `--flash-attn on` to managed `llama-server` sidecars so current
  llama.cpp CLI parsers no longer treat the next flag as the Flash Attention
  mode and exit before MCP/`serve` becomes ready. Startup failures now include
  a stderr tail for diagnosis.
- Satisfy ty 0.0.61 sort-key and redundant-cast diagnostics in analysis,
  local-embedding, and state-type helpers.
- Include the VS Code test stub package in git so extension CI can run
  reliably.

### Documentation

- Update vector-search backend documentation and remove the outdated system
  BLAS requirement from the README.
- Move the architecture analysis report from the repository root into `docs/`.

## 4.3.0 — 2026-06-21

### Improvements

- Add a Rust native embedding-search backend with process-level vector caching,
  prewarm support, macOS Accelerate matrix-vector search, and SIMD dot-product
  fallbacks; remove the Python-side numpy search path while keeping a
  pure-Python fallback mode through `DAGAYN_EMBEDDING_SEARCH_BACKEND` for A/B
  testing.
- Parallelize embedding search for large matrices.
- Add typed Rust DTOs and enum-backed parser contracts for stricter
  cross-language interfaces.
- Add Pydantic boundary DTOs for state transitions, dispatchers, architecture
  analysis, refactor outputs, and answerability guidance.
- Split the embeddings, incremental build, and review tool modules into
  focused submodules.

### Fixes

- Drop the system BLAS dependency in the native embedding search backend.
- Keep the Rust embedding backend clippy-clean.
- Align typed contracts with CI checks.

### Testing

- Add an embedding search micro-benchmark.
- Expand test coverage for impact analysis scoring.
- Update markdown resolution contract assertions.

## 4.2.8 — 2026-06-15

### Improvements

- Recognize public API coverage for bare helper contracts across Rust, Python,
  JavaScript, TypeScript, Go, Java, C#, C++, PHP, Ruby, and Kotlin test styles,
  reducing false quality-check gaps when helpers are intentionally private.
- Share edge-record normalization between graph storage paths so rich edge
  metadata such as line ranges, confidence, scope, and evidence is preserved
  consistently.

### Fixes

- Keep the Rust graph backend clippy-clean by deriving the default confidence
  tier directly on the enum variant.

## 4.2.6 — 2026-06-09

### Fixes

- Keep `get_minimal_context_tool` lightweight by avoiding automatic change
  impact analysis unless callers explicitly provide `changed_files`, preventing
  CLI and MCP entry-point hangs in repositories with costly coverage inference.

## 4.2.5 — 2026-06-08

### Fixes

- Include local embedding sidecar arguments in generated AI-tool update hooks
  when installing with `--mode local-embedding` or
  `--mode local-embedding-llama`, so hook-triggered graph refreshes keep local
  vectors current.
- Preserve sidecar port, binary, and startup-timeout options consistently
  between the MCP `serve` command and generated update hooks.

## 4.2.4 — 2026-06-08

### Improvements

- Run bare BGE-M3 local embeddings through a managed `llama-server` GGUF Q8
  sidecar instead of in-process sentence-transformers, keeping Apple Metal
  execution outside the dagayn Python process.
- Document the BGE-M3 Q8 sidecar benchmark, including search-quality parity
  with sentence-transformers BGE-M3 on the smaller quality set and much lower
  observed resident memory than the PyTorch MPS path.
- Update GitHub workflow actions for the Node 24 runtime.

### Fixes

- Keep generated hook updates graph-only so MCP `serve --local-embedding`
  settings are not reused by edit-time hook refreshes.
- Default explicit in-process local sentence-transformers loading to CPU unless
  `CRG_LOCAL_EMBEDDING_DEVICE` is set.

## 4.2.3 — 2026-06-08

### Improvements

- Clarify installed skill guidance so agents consistently use semantic search
  for start-node discovery, relationship queries for specific graph evidence,
  workflow dispatchers for review/architecture/refactor decisions, and raw
  graph traversal only for bounded neighborhood follow-ups.

## 4.2.2 — 2026-05-31

### Improvements

- Include unstaged, staged, and untracked files in change review and
  incremental graph update detection.
- Add `change_file_sources` buckets so base-ref changes remain distinguishable
  from local worktree, staged, unstaged, and untracked files.
- Annotate changed nodes and relevant edges with `change_status` and summarize
  existing vs added graph entities in `change_entity_summary`.

## 4.2.1 — 2026-05-27

### Fixes

- Update the `wiki-research` skill for the simplified static `dagayn visualize`
  export surface and explicitly avoid the removed interactive HTML/webserver
  workflow.

## 4.2.0 — 2026-05-27

### Changes

- Remove the interactive HTML graph visualization from `dagayn visualize`.
  Static exports now require an explicit `--format` value: `graphml`,
  `mermaid-c4`, `svg`, `cypher`, or `obsidian`.
- Remove `dagayn visualize --serve` and the local webserver path.

### Improvements

- Reduce architecture-analysis noise by separating code/docs/test scopes,
  classifying low-signal knowledge-gap findings, and adding edge-shape metrics
  for single-file communities.
- Add compact `_runtime` metadata to tool responses so agents can distinguish
  long-lived MCP processes from direct CLI runs.
- Add Rust parser type-reference edges for local type identifiers to reduce
  false isolated-node leads.

### Fixes

- Keep the Rust parser clippy-clean after the new type-reference traversal.

## 4.1.4 — 2026-05-25

### Fixes

- Calibrate `refactor_tool(mode="dead_code")` confidence so public APIs,
  ambiguous symbol names, and candidates without readable source are reported
  as lower-confidence review leads instead of ordinary deletion candidates.

## 4.1.3 — 2026-05-25

### Fixes

- Classify Rust `#[test]` functions as `Test` nodes even when their names do
  not start with `test`, reducing false-positive graph quality test gaps.

## 4.1.2 — 2026-05-25

### Fixes

- Apply repository-wide ruff formatting so main CI passes after the 4.1.x
  remediation releases.

## 4.1.1 — 2026-05-25

### Fixes

- Restore strict CI compatibility for ruff, ty, and Rust clippy after the
  interface remediation release.

## 4.1.0 — 2026-05-25

### Improvements

- Align the MCP and CLI fallback tool surface around public `*_tool` names,
  bounded `top_n` output, and shared `detail_level` arguments.
- Make fallback responses easier to act on by adding result counts, confidence,
  zero-result reasons, and next-action hints where they were missing.
- Keep installed Codex skills and MCP interface guidance in sync with the
  remediated tool contracts.

### Fixes

- Preserve calibrated documentation evidence types in minimal outputs.
- Normalize zero-result and dispatcher error contracts across graph query,
  flow, review, architecture, semantic search, and refactor tools.
- Keep `dagayn.__version__` aligned with the released package version.

## 4.0.6 — 2026-05-24

### Improvements

- Ship numpy as a standard dependency so embedding search uses the vectorized
  cosine-similarity path in default installs.
- Prefer persisted hub and bridge centrality scores even when callers pass a
  graph snapshot, avoiding runtime betweenness centrality during suggested
  question generation.
- Sample large-graph Rust centrality sources by stable hash instead of sorted
  prefix so approximate bridge scoring does not miss connected regions.
- Generate suggested architecture questions as a native Rust analysis unit when
  using the Rust graph store, avoiding Python snapshot materialization.

### Fixes

- Keep `dagayn serve` importable on Python 3.14.0b4 by applying compatibility
  shims before importing FastMCP dependencies that still reference older
  CPython typing and `collections.abc` surfaces.

## 4.0.5 — 2026-05-24

### Fixes

- Revert the managed local embedding runtime to a single `llama-server`
  implementation and remove the slow `mlx-openai-server` path.
- Update local embedding documentation and tests so the `low` preset consistently
  uses the llama.cpp GGUF model on every platform.

## 3.2.0 — 2026-05-17

### Features

- Add `dagayn build --force` to force a full graph rebuild even when an existing
  graph database is present.
- Teach installed skills how to follow explicit Markdown ↔ code documentation
  traceability through `dagayn:` directives, `docs_for`, and
  `implementations_of`.

### Improvements

- Clarify Markdown `CROSS_ARTIFACT` resolution semantics for unresolved or
  ambiguous code-span references.
- Extend review, exploration, debug, refactor, and architecture skills so
  documentation bridge edges are considered when they affect the task.

### Fixes

- Pass install-time skill rendering context explicitly so `ty` can type-check
  the `dagayn install` command path.

## 3.1.0 — 2026-05-17

### Features

- Add install-time skill rendering so search and graph-build skills adapt to
  the selected embedding mode (`fts`, `local`, or `remote`).
- Add operational skills for dagayn installation, semantic search, wiki
  research, and cross-repository workflows.
- Make `build_or_update_graph_tool()` inherit the local embedding preset from
  `dagayn serve --local-embedding low|high`; explicit
  `local_embedding="none"` remains available for deliberate FTS-only refreshes.
- Prefer the globally installed `dagayn serve` command in generated MCP config
  when `dagayn` is available on `PATH`, matching `uv tool install` workflows.

### Improvements

- Align packaged skills with the 3.x MCP dispatcher surface and remove stale
  split-tool names.
- Refresh graph exploration, debugging, review, refactor, and Markdown authoring
  skills with token-efficient graph-first workflows.
- Make Markdown symbol checks robust under hybrid semantic search by requiring
  exact symbol matches for `CROSS_ARTIFACT` resolution.

### Fixes

- Avoid duplicate AGENTS/CLAUDE instruction injection when an existing dagayn
  heading is present without a marker.
- Ensure `dagayn install` rewrites managed skill content while preserving
  unrelated user-created skills.

## 2.3.5 — 2026-04-30

### Performance

- `get_impact_radius_sql`: replace `get_edges_among` with temp table JOIN (B5)
- `find_dependents`: batch frontier lookups, reducing N+1 to 3 queries/hop
- `store_file_batch`: batch processing in `full_build` and `incremental_update` (A2)
- Add mtime-based incremental skip (migration v11, `mtime_ns` column)
- `find_dead_code`: batch-preload edges, reducing O(N) SQL to O(1) lookups
- Batch postprocessing `CROSS_ARTIFACT` ref resolution (N+1 → 3 queries)
- `get_local_subgraph`: batch DFS traversal via recursive CTE
- Vectorize embedding search with numpy BLAS and process-level matrix cache
- Batch flow INSERT/DELETE with `executemany` and `IN (?,…)`
- Batch community INSERT/UPDATE in `_clear_and_store_communities`
- migration v10: add `idx_nodes_parent_name(parent_name, name)` index
- Cache `parse_diff_ranges` and `embed_query` results with `lru_cache`
- Pin `EmbeddingStore` per-process; batch `embed_nodes` hash lookups
- Bulk-insert nodes/edges with `executemany`, use `RETURNING id` in upsert
- SQLite PRAGMA tuning, parser worker singleton, token-estimate fix

## 2.3.4 — 2026-04-30

### Features

- Add `writing/reading-markdown-document` skills with global install support (#1)

### Performance

- Pin sqlite connection, batch node lookups, add profiler scaffold (#2)

### Refactoring

- Split megafiles into focused packages (Phase 1–2)
- Split `parser/core.py` (3476 → 1269 lines) into focused modules
- Split `extension.ts::registerCommands` into feature modules (Phase 3-1)
- Extract `graphWebview` HTML/CSS to static assets (Phase 3-2)
- Add Protocol classes to lift SAP abstractness (Phase 4)

### Fixes

- Resolve lint and type-check errors
- Update notebook parity snapshot
- Apply ruff-format to `parser/_protocol.py`

## 2.3.3 — 2026-04-27

### Features

- Expand bridge detection to 13 languages (file_io, subprocess, FFI)
- Add cohesion filter and file-I/O bridge detection
- Add unified multi-language CROSS_LANGUAGE edge extractor
- Add Markdown → code `CROSS_ARTIFACT` edges (doc-to-symbol bridge)
- Add SAP metrics and unify SDP/SAP edge set
- Add ADP/SDP analysis tools; fix Python relative import resolution
- Add Mermaid C4 export
- Add markdown documentation policy to `dagayn install`
- Add markdown parser support

### Refactoring

- Extract tree-sitter language extractors and walkers (A.3+A.4)
- Extract bespoke-parser languages into `dagayn/parser/` (A.2)
- Split parser shared infra into dispatch/grammars/bridges modules
- Split `cli.py` into `cli/` package with command modules
- Split `graph.py` into `graph/` package (types/helpers/core)
- Convert `dagayn/parser.py` monolith to `parser/` package
- Rename `CROSS_LANGUAGE` → `CROSS_ARTIFACT` edge kind
- Migrate vendored grammar sources to dynamic loading system
- Rename VS Code extension and rebrand to `dagayn`

### Fixes

- Exclude dev-environment artifacts from vendor grammar sources
- Filter short plain-word identifiers from markdown code-span candidates
- Resolve grammar archive structure and binding injection
- Enforce HTTPS-only URL before `urlopen` in `vendor_grammars`
- Skip grammar vendor during editable install
- Remove dead code modules; fix test expectation
- Resolve architecture lint and typing

### Tests

- Add cross-package Java integration tests for SAP/SDP
- Add comprehensive test coverage for analysis and exports
- Add multi-file language tests

## 0.1.0 — Initial dagayn fork

- Forked from `code-review-graph` by Tirth Kanani and established `dagayn` as an independent project
- First-class Terraform parsing support
- Markdown structure parsing with directive-based dependency extraction
- Graph registration paths unified to repository-root-relative
- Package name, CLI, and storage directory unified under `dagayn`
- See [NOTICE](NOTICE) for upstream attribution

## 2.9.0 — 2026-05-13

### Features

- Embedding progress bar: `dagayn update --local-embedding` now shows a live
  progress bar (nodes/s, ETA) on stderr when running interactively
- `CRG_OPENAI_TIMEOUT` env var: controls the per-call HTTP timeout for the
  OpenAI-compatible embedding provider (default 120 s); local embedding runs
  automatically inherit `--local-embedding-timeout` so large/slow models no
  longer hit the 120 s ceiling
- `dagayn serve --local-embedding {low,high}`: managed llama-server sidecar
  for semantic search during MCP sessions
- `dagayn install --local-embedding {low,high}`: bakes the llama-server flags
  into the generated MCP server entry so the model is available after restart
- FTS search-quality benchmark (`dagayn/eval/`) with 12 labelled queries;
  results published in `docs/LOCAL-EMBEDDINGS.md` and `README.md`

### Fixes

- Local embedding runs now set `CRG_OPENAI_TIMEOUT` to the startup-timeout
  value, preventing spurious HTTP timeouts on slow/large GGUF models

## 2.9.1 — 2026-05-14

### Improvements

- MCP semantic search now inherits the embedding mode selected for `dagayn serve`:
  `--local-embedding {low,high}` defaults search to the managed local
  OpenAI-compatible sidecar, and `--remote-embedding {openai,google,minimax}`
  defaults search to the selected remote provider.
- `hybrid_search` test deboost: nodes detected as test code (`is_test=True`)
  are multiplied by 0.6× during boosting so source ranks above the tests
  that exercise it. Tests remain visible (deboost, not filter). Each
  result dict now exposes `is_test: bool`.
- RRF default `k` changed from 60 to 10 so the `score` field spreads over
  ~0.05–0.2 instead of being compressed into 0.015–0.016 with L2-normalised
  embeddings. Item order is preserved.
- Natural-language identifier extraction: `hybrid_search` now extracts
  snake_case / camelCase / PascalCase tokens from the query and fires an
  extra FTS arm per identifier, so phrases like
  "tests for embed_graph" still match the `embed_graph` symbol directly.

## 2.10.0 — 2026-05-14

### Features

- `dagayn install --mode {fts,local,remote}` makes the three install
  patterns first-class CLI choices.  Each mode has paired sub-options:
  `--preset {low,high}` for local, `--provider {openai,google,minimax}`
  for remote.  When `--mode` is omitted on a TTY, the user is prompted
  interactively; under `-y` or a non-TTY stdin the install fails fast
  with a helpful error.  Legacy `--local-embedding low|high` continues
  to work as a shortcut for `--mode local --preset $X`.
- `--mode remote` prints the provider-specific environment variables the
  user needs to set in the shell that launches their AI coding tool
  (e.g. `CRG_OPENAI_API_KEY`, `CRG_OPENAI_BASE_URL`, `CRG_OPENAI_MODEL`
  for `openai`).  The MCP server inherits those at launch time.

## 2.10.1 — 2026-05-15

### Fixes

- Re-running `dagayn install` with local or remote embedding enabled now updates
  existing MCP config entries and dagayn-generated hooks instead of leaving the
  old `dagayn serve` / `dagayn update` arguments in place.
- Re-running `dagayn install` now refreshes dagayn-managed skill Markdown
  placements from the packaged source, replacing stale skill content while
  preserving unrelated user-created skills.
- Align `TESTED_BY` scoring semantics across Python and Rust analysis paths:
  production symbols now consistently point to the test symbols that cover
  them, matching parser output and schema documentation.
- Make sampled bridge-centrality scoring deterministic.
- Avoid reporting SAP violations for scopes with no eligible concrete or
  abstract types.

## 2.11.0 — 2026-05-16

### Features

- Add explicit documentation bridge directives for Markdown, Python comments,
  and Terraform comments. Directives emit `CROSS_ARTIFACT` edges for
  `implemented_by`, `implements_contract`, `explained_by`, `has_runbook`,
  `problem_described_by`, `discussed_by`, `discusses_artifact`, and
  `raises_issue_for` relationships.
- Add `query_graph` patterns `docs_for` and `implementations_of` so agents can
  traverse documentation relationships in either direction without storing
  duplicate inverse edges.

### Documentation

- Document the documentation bridge role model and authoring-site policy in the
  cross-artifact edge specification.
