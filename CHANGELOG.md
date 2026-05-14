# Changelog

All notable changes to `dagayn` are documented here.

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

## Unreleased
