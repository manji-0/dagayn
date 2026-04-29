# Rust core migration specification

<!-- constrained-by ./ARCHITECTURE.md -->

> **Status:** Work in progress — core decisions frozen as of 2026-04-26. Phase 0 complete as of 2026-04-27. Phase 1 (Rust graph engine) started with the initial Rust workspace and PyO3 graph-store scaffold.

## Frozen decisions

The following design choices are settled and must not be reopened without explicit rationale.

| Concern | Decision |
|---|---|
| Python ↔ Rust binding | PyO3 / maturin Python extension (`dagayn._core`) |
| Migration order | Graph engine → Post-processing → Parser |
| Parallel operation | `DAGAYN_BACKEND={python,rust}` env switch; no auto-fallback |
| Acceptance criteria | Correctness parity **and** explicit numeric performance targets |
| ipynb / Markdown | Rust-only: `serde_json` for notebook cells, `cc`-built `tree-sitter-md` for Markdown |
| Distribution targets | macOS arm64, macOS x86_64, Linux x86_64, Linux aarch64 (4 wheels) |

## Purpose

This specification defines how dagayn should migrate its **core graph pipeline** from Python to Rust without breaking the product contract that existing users and AI tool integrations rely on.

The immediate target is **not** a full product rewrite, but the end-state
direction is now explicit: Python should shrink to the CLI/MCP interface layer
and compatibility glue, while parsing, graph persistence, post-processing,
query primitives, and normalization move to Rust wherever practical.

The target is the core pipeline only:

1. graph storage and query engine
2. post-processing layers such as FTS, flows, and communities
3. parser and language extraction

## Non-goals

This spec does **not** require an immediate Rust replacement for:

- CLI install flows
- MCP tool definitions and prompt wiring
- editor/platform config injection
- daemon/watch process management
- embedding provider integrations
- wiki generation and other convenience surfaces

Those surfaces may remain in Python until the Rust core reaches parity.

## Migration strategy

dagayn follows a **core-first replacement** strategy, not a big-bang rewrite.

Shape:

1. Define a stable compatibility contract around graph data and behavior.
2. Implement a Rust core behind that contract.
3. Let the existing Python CLI and MCP layers call the Rust core **via PyO3** — not subprocess.
4. Replace outer Python surfaces only after parity is proven.

This keeps user-visible behavior stable while moving the performance-sensitive and correctness-critical path first.

## Current contract that must be preserved

The Rust core must preserve these dagayn-specific behaviors:

- **repo-root-relative graph identity** for files and qualified names
- current node and edge kinds, including fork-specific Markdown and Terraform behavior
- SQLite-backed local graph storage
- compatibility with existing post-processing and query flows
- support for incremental updates
- fork-local grammar provisioning rules for pinned Markdown and Terraform grammars

Behavioral compatibility is more important than literal implementation parity.

## Scope boundaries

### In scope for the Rust core

- file discovery and language detection
- Tree-sitter parser orchestration
- notebook cell-aware extraction
- Markdown and Terraform extraction rules
- node and edge normalization
- SQLite schema creation and migration handling
- graph writes and read-side query primitives
- post-processing:
  - full-text search indexes
  - flow derivation
  - community derivation
  - graph statistics and traversal primitives

### Out of scope for the first Rust milestone

- platform install/config mutation
- `dagayn serve` MCP tool registration layer
- editor skill generation
- daemon supervisor logic
- cloud embedding provider wrappers

## Target architecture

The desired end state is a layered split.

### Layer 1: Rust core library

Responsible for:

- parsing supported files
- emitting normalized node and edge records
- reading and writing the graph database
- running post-processing passes
- exposing a stable programmatic API

### Layer 2: Rust–Python binding via PyO3

A single Python extension module (`dagayn._core`) is built by maturin from the `crates/dagayn-py` entry point. Python callers import this module directly — no subprocess boundary.

A minimal native CLI binary (`dagayn-core`) is kept in `crates/dagayn-core/` for local debugging and A/B comparison against the Python implementation. It is not part of the distributed wheel.

### Layer 3: Python interface shell

Python remains responsible for:

- current CLI UX and command argument handling
- FastMCP tool registration and response shaping
- platform install/config flows only where they are inherently product-interface work
- temporary compatibility adapters while Rust reaches parity

Python should not retain core parsing, graph mutation, query, or post-processing
logic once the corresponding Rust implementation has parity. New performance-
or correctness-sensitive core logic should land in Rust crates first.

## Crate workspace layout

```
crates/
  dagayn-core/      # public API facade; used by dagayn-py and integration tests
  dagayn-graph/     # SQLite I/O, upserts, incremental replacement, migrations (rusqlite)
  dagayn-postproc/  # FTS, flows, communities, traversal helpers
  dagayn-parser/    # tree-sitter orchestration, language detection, ipynb, Markdown
  dagayn-grammars/  # build.rs fetches and builds pinned Markdown and Terraform grammars
  dagayn-py/        # PyO3 entry point; maturin builds this into dagayn._core
```

Dependency rules:

- `dagayn-py` is a thin translation layer only (PyO3 types ↔ internal types). No business logic.
- `dagayn-graph` uses `rusqlite` for SQLite access. It is the sole owner of connection management and schema migrations.
- `dagayn-postproc` depends on `dagayn-graph` only. It must not depend on `dagayn-parser`.
- `dagayn-parser` depends on `dagayn-grammars` for compiled grammar bindings.
- `dagayn-core` depends on all internal crates and is the only public surface for `dagayn-py`.

ipynb parsing lives in `dagayn-parser/src/notebook.rs` using `serde_json`. Markdown grammar wiring lives in `dagayn-grammars/` with a build.rs that fetches and compiles the pinned grammar source via `cc`.

## Compatibility contract

Before implementation starts, each phase must freeze the relevant slice of this contract.

### Graph data contract

The Rust core must preserve:

- the existing SQLite schema or a migration-compatible successor
- node kinds and edge kinds already documented by dagayn
- `extra` metadata fields that downstream tools rely on
- repo-root-relative `file_path` and `qualified_name` normalization

Schema change rule: Rust adds a migration only when the corresponding Python `migrations.py` also changes. The two migration sequences must remain compatible.

### Parser contract

The Rust parser layer must preserve:

- current language detection rules
- pinned grammar provisioning behavior for Markdown and Terraform (see `docs/GRAMMAR-PROVISIONING.md`)
- notebook cell attribution semantics: `serde_json` cell enumeration, cell index, and source line ranges must match the attribution rules in the Python `parser.py`
- Markdown heading slugging and directive-comment dependency extraction: re-implemented in `dagayn-parser/src/markdown.rs` to be equivalent to the Python implementation. The Python binding shim is replaced by the `cc`-built grammar in `dagayn-grammars/`.
- Terraform block naming and reference extraction rules

### Query contract

The Rust engine must return results compatible enough that existing Python tools can keep their current response shapes with at most thin adapter code.

That includes:

- graph stats
- impact radius primitives
- traversal primitives
- flow inputs
- community inputs
- search/index inputs

## `DAGAYN_BACKEND` specification

The env variable `DAGAYN_BACKEND` controls which backend handles each pipeline stage.

Valid values: `python` (default during Phase 1–3), `rust` (default from Phase 4 onward).

Scope: resolved once at process startup. Per-subcommand override is supported by setting the env before each invocation.

**Auto-fallback is not performed.** If `DAGAYN_BACKEND=rust` is set and the Rust extension fails to import or returns an error, dagayn exits with a clear error message. Silent drift is treated as worse than a visible failure.

Backend availability by phase:

| Phase | `graph` | `postproc` | `parser` | `DAGAYN_BACKEND=rust` default? |
|-------|---------|------------|----------|-------------------------------|
| 0     | Python  | Python     | Python   | no                            |
| 1     | Rust    | Python     | Python   | no                            |
| 2     | Rust    | Rust       | Python   | no                            |
| 3     | Rust    | Rust       | Rust     | no                            |
| 4+    | Rust    | Rust       | Rust     | yes                           |

During Phase 4, Python implementations are moved to `legacy_py/` (not deleted) for regression comparison.

## Recommended migration phases

### Phase 0: freeze contracts and parity fixtures

Deliverable: a `tools/parity_export.py` script that:

1. Opens the dagayn SQLite database after a full `build`.
2. Emits a **canonical JSON** snapshot: nodes and edges sorted by a stable key, `extra` fields normalized to alphabetical key order.
3. Writes one file per fixture repository.

Starting point: `dagayn/graph/helpers.py:21-46` (`node_to_dict`, `edge_to_dict`). The canonical export builds a richer dict that adds `extra`, `signature`, `community_id`, `params`, `return_type`, `file_hash`, and `modifiers` beyond what those helpers emit; `id` and `updated_at` are excluded (non-deterministic). `File`-kind node names are canonicalized to `file_path` (relative) because the Python parser stores the absolute path in `name` while `_relativize_parsed_entities` normalizes only `file_path`.

Fixture set (`tests/fixtures/parity/`):

- `python_only/` — Python imports, calls, class hierarchy
- `terraform_only/` — Terraform blocks, `var.*` references, outputs
- `markdown_only/` — Markdown headings, `derived-from` directive edge
- `mixed/` — Python + Terraform + Markdown (cross-artifact edges)
- `notebook/` — Jupyter notebook with multiple code cells (cell attribution)

Committed baselines: `tests/fixtures/parity/__snapshots__/<name>.json` (one per fixture).

Acceptance: `test_build_is_deterministic` in `tests/test_parity_export.py` builds each fixture twice and asserts byte-identical canonical JSON. All five pass as of Phase 0 implementation. `test_export_matches_snapshot` verifies subsequent Python builds do not drift from the committed baseline.

`DAGAYN_BACKEND` behavior: `python` only.

### Phase 1: Rust graph engine

Deliverable: `dagayn-graph` and `dagayn-py` wiring such that `dagayn._core.GraphStore` can replace the Python `GraphStore` in `dagayn/graph.py`.

Initial scaffold:

- root Cargo workspace with `dagayn-core`, `dagayn-graph`, and `dagayn-py`
- pinned Rust toolchain through `rust-toolchain.toml`
- `dagayn-graph` opens the SQLite database, creates the Python-compatible base schema, runs migrations through schema version 9, and supports atomic per-file node/edge replacement
- `dagayn-py` exposes the first `dagayn._core.GraphStore` methods for metadata, file replacement, batch file replacement, and file listing
- Python `full_build` / `incremental_update` buffer parsed file results and call `store_file_batch`, so the Rust backend crosses the PyO3 boundary at coarse DB-write chunks instead of once per parsed file
- incremental hash checks use `get_file_hashes(paths)` and stale-file cleanup uses `remove_files_data(paths)`, avoiding per-file PyO3 calls in the update path
- Rust `GraphStore` now exposes the read methods needed by Python's current
  incremental dependent expansion (`get_node`, `get_nodes_by_file`,
  `get_edges_by_source`, and `get_edges_by_target`) while returning the
  existing Python dataclass shapes through the PyO3 interface.
- Rust `GraphStore` now also owns FTS5 index rebuilds for the Rust backend,
  using the same virtual table definition and atomic rebuild sequence as the
  Python fallback.
- Rust `GraphStore` computes missing node signatures in one backend call for
  Rust-backed post-processing, preserving the current Python signature format.
- Rust `GraphStore` resolves Markdown-to-code CROSS_ARTIFACT placeholders for
  Rust-backed minimal post-processing, preserving the existing strict
  unique-match policy.
- `postprocess="minimal"` no longer re-opens the Python `GraphStore` when the
  Rust backend exposes the required signature, FTS, and Markdown resolver
  methods. Full rebuild post-processing now keeps flow tracing, community
  detection, and summary generation on the Rust store when the required Rust
  read/write methods are available; remaining Python-store fallback is retained
  for incremental community detection and incomplete Rust surfaces.
- Full rebuild community detection can now read from the Rust store and persist
  through Rust's `store_communities_json` API while the detection heuristics
  remain in Python. Incremental community detection now uses Rust's
  `count_affected_communities` API for the affected-community gate before
  re-running the same Python heuristics over Rust read helpers.
- Rust `GraphStore` can populate `community_summaries`, `flow_snapshots`, and
  `risk_index`; full Rust-backed post-processing now runs summary table
  generation on the Rust store after flow/community data has been written.
- Rust `GraphStore` exposes the read surface needed by Python flow tracing
  (`get_all_call_targets`, `get_nodes_by_kind`, and `load_flow_adjacency`) and
  can persist traced flows through `store_flows_json`.
- Incremental flow retracing can now stay on the Rust store for its affected
  flow deletion and append-only flow insertion steps through
  `delete_affected_flows` and `insert_flows_json`. Python still owns the entry
  point filtering and BFS trace heuristics.
- Rust `GraphStore` also exposes stored-flow query JSON APIs for `list_flows`,
  `get_flow`, and `get_affected_flows`, so those Python-facing tools no longer
  require direct SQLite connection access under the Rust backend. The
  `get_minimal_context` MCP entry point now uses those stored-flow APIs instead
  of querying the `flows` table through Python SQLite.
- Rust `GraphStore` exposes community persistence and read APIs used by
  `list_communities`, `get_community`, and architecture overview flows. Python
  still owns the detection heuristics and optional igraph integration.
- Rust `GraphStore` exposes `get_stats`, preserving the Python `GraphStats`
  dataclass shape for MCP/tool entry points such as `get_minimal_context`.
- Rust `GraphStore` exposes the read helpers used by change risk scoring:
  file suffix matching, flow membership counts/criticalities, node community
  lookup, batched community ID lookup, and transitive test lookup.
- Rust `GraphStore` exposes the process-level store-cache lease attributes
  (`_pinned`, `_leases`) and `get_edges_by_endpoints`, matching the current
  Python graph read/cache contract after the mainline performance work.
- Rust `GraphStore` exposes the mainline batch read helpers for node hydration,
  flow membership/criticality maps, node community maps, and community member
  maps. Change-risk, query traversal, flow hydration, and community overview
  paths can now stay on the Rust backend without falling back to per-node Python
  SQLite loops.
- `DAGAYN_BACKEND=rust` is recognized by the Python graph package and fails loudly if the extension has not been built; `python` remains the default

Current local benchmark baseline, measured on 2026-04-28 with `tools/backend_benchmark.py`
against this repository copy (307 files, 4,148 parsed nodes, 25,845 parsed edges):

| Mode | Python avg | Rust avg | Current interpretation |
|---|---:|---:|---|
| full build, `postprocess=none` | 2.509s | 3.105s | Rust is slower because PyO3 object conversion still dominates the coarse writer call |
| full build, `postprocess=minimal` | 12.399s | 11.548s | Mostly Python post-processing variance; Rust graph write is not the bottleneck |
| full build, `postprocess=full` | 11.238s | 11.822s | Mostly Python post-processing; Rust graph write has small visible overhead |
| writer-only `store_file_batch` | 0.302s | 0.986s | Confirms the current Rust path is conversion-bound, not ready as a performance win |

This baseline means the next Phase 1 optimization should reduce Python-object
marshalling before adding more Rust methods. Likely options are a compact
serialized batch format or moving parse output normalization into Rust with the
writer, rather than crossing PyO3 per node/edge object.

Follow-up implementation: the Rust backend now accepts `store_file_batch_json`,
a compact tuple-array JSON batch. Python uses this method when available so Rust
does not perform per-node/per-edge PyO3 `getattr` extraction. A later
`parse_rust_owned_files_compact_json` path also batches Rust-owned Markdown and
Terraform parsing across one PyO3 call per chunk. The current path goes one
step further with `GraphStore.store_rust_owned_files(repo_root, paths)`, which
parses Rust-owned files and writes them inside the same Rust call so parsed
node/edge batches no longer round-trip through Python JSON.

The 2026-04-29 local
benchmarks on this repository (312 files, 4,315 parsed nodes, 27,600 parsed
edges) measured:

| Mode | Python avg | Rust avg | Current interpretation |
|---|---:|---:|---|
| full build, `postprocess=none` | 2.645s | 2.982s | Rust output matches Python counts; Rust is still slower, but the gap narrowed after writer batching |
| writer-only `store_file_batch` | 0.325s | 0.502s | Prepared statements and direct edge inserts cut Rust writer time by roughly half versus the prior ~1.04s |
| FTS rebuild only | 0.017s | 0.027s | Rust owns the operation now, but the current SQLite-equivalent implementation is not a speedup yet |
| missing signature computation only | 12.300s | 0.036s | Rust batches the old Python per-node update loop into one transaction |
| summary table computation only | 0.051s | 0.100s | Rust owns the operation now, but the current port is slower than the already-batched Python implementation |
| flow persistence only | 0.010s | 0.013s | Rust owns the transaction now; JSON handoff and equivalent SQLite work make it slightly slower on this graph |

The FTS-only measurement used the current local `.dagayn/graph.db` snapshot and
indexed 4,265 rows in both backends. The signature-only measurement reset
signatures on a copied local graph and recomputed 4,270 rows in both backends.
The summary-only measurement used copied local graph snapshots after flow and
community data had already been generated. The flow persistence measurement
stored 254 traced flows into copied local graph snapshots.

This did not materially change the conclusion: the next meaningful
optimization is to move more non-Markdown/Terraform parsing and post-processing
into Rust-owned operations, not to add narrow per-item PyO3 methods.

Rust parser grammar design:

- Rust parser extraction must use the same pinned grammar sources as the current
  Python parser path. Markdown uses `manji-0/tree-sitter-markdown` at
  `13a2b8bb44965b75ddba5e70f16411c18e6f09fe` with source subdirectory
  `vendor/tree-sitter-markdown/tree-sitter-markdown`. Terraform uses
  `manji-0/tree-sitter-terraform` at
  `5a5b258a71290999ce58797eafeaa098b2d450b9`.
- Do not replace these grammars with crates.io grammars such as
  `tree-sitter-hcl` or unrelated Markdown crates. They may parse related
  languages, but grammar drift would change dagayn's graph contract.
- `dagayn-grammars` should own the Rust-side pinned grammar provisioning. Its
  metadata must mirror `dagayn/vendor_grammars.py::GRAMMAR_SPECS`, including
  repository owner, repository name, commit, required source files, and
  Markdown source subdirectory.
- The Rust build should compile the pinned grammar C sources with `cc` and
  expose `tree_sitter::Language` constructors for `markdown` and `terraform`.
  Runtime dependency on Python's grammar cache is not acceptable for wheels or
  CI. Packaged grammar sources may be reused, but the Rust crate must have a
  deterministic build path.
- Existing hand-written Markdown and Terraform extractors are transitional.
  They may remain only as fallback or parity scaffolding until the pinned
  tree-sitter extractors cover the same behavior. New parser behavior should
  land against the tree-sitter-backed Rust path first.
- Acceptance for replacing the transitional parser path requires canonical
  export parity for `markdown_only`, `terraform_only`, and `mixed`, plus
  regression fixtures for fenced Markdown code blocks, YAML/TOML frontmatter,
  blockquote-contained headings, skill frontmatter, relative links, Terraform
  block labels, provider source edges, module source imports, references, and
  calls.

Parser migration progress:

- `dagayn-parser` owns parseable-file collection for the Rust backend except
  SVN-specific listing, which remains in Python for compatibility.
- `dagayn-parser` now exposes an initial Markdown extractor through
  `parse_markdown_compact_json(file_path, source)`. It emits compact node/edge
  arrays equivalent to the Python Markdown parser for the current parity
  fixtures. `DAGAYN_BACKEND=rust` now routes Markdown files in `full_build` /
  `incremental_update` through this Rust extractor and stores the compact
  output directly through the Rust graph writer. `markdown_only` and `mixed`
  parity snapshots match through this path.
- `dagayn-parser` now exposes an initial Terraform extractor through
  `parse_terraform_compact_json(file_path, source)`. `DAGAYN_BACKEND=rust`
  routes `.tf` / `.tfvars` files through Rust for block extraction, Terraform
  reference extraction, call extraction, module imports, and provider source
  dependency edges. `terraform_only`, `markdown_only`, and `mixed` parity
  snapshots match with `--skip-postprocess` through this path.
- The Rust parser path now also exposes
  `parse_rust_owned_files_compact_json(repo_root, file_paths)`, so
  `full_build` / `incremental_update` batch Markdown and Terraform files into
  one Rust parser call per chunk before handing the resulting compact batch to
  the Rust graph writer. The preferred Rust backend path now calls
  `GraphStore.store_rust_owned_files(repo_root, paths)` so Rust-owned parse
  output is written without returning node/edge JSON to Python. Python no
  longer crosses PyO3 once per Rust-owned file in the normal build/update path.
- `dagayn-grammars` now compiles the pinned `manji-0/tree-sitter-markdown` and
  `manji-0/tree-sitter-terraform` C sources through Rust build.rs and exposes
  Rust `tree_sitter::Language` constructors.
- The Markdown extractor now collects headings through the pinned Rust
  tree-sitter Markdown grammar, with the previous text scanner retained only as
  fallback. A local full-repo `postprocess=none` smoke benchmark produces
  identical Python/Rust node and edge counts for the current repository.
- The Terraform extractor now collects top-level block kind, labels, body text,
  source ranges, and direct block attributes through the pinned Rust tree-sitter
  Terraform grammar, with the previous text scanner retained only as fallback.
  Call extraction now walks `function_call` AST nodes. Reference extraction now
  walks Terraform traversal expressions and uses template-node compatibility
  scanning only to preserve existing dotted-string behavior such as
  `"t3.micro"`. Terraform provider `source` dependencies are collected from
  nested AST attributes/object elements instead of scanning the block text.
- FTS rebuilds now route through `dagayn._core.GraphStore.rebuild_fts_index`
  when the Rust backend is active. Python's `dagayn.search.rebuild_fts_index`
  keeps the existing SQLite implementation as the fallback for the Python
  backend and tests.
- Missing signature computation now routes through
  `dagayn._core.GraphStore.compute_missing_signatures` when available, avoiding
  one Python-to-store update per unsigned node in Rust-backed post-processing.
- Markdown artifact reference resolution now routes through
  `dagayn._core.GraphStore.resolve_markdown_artifact_refs` when available.
  The Rust path rewrites exactly one matching non-Markdown target to HIGH
  confidence and deletes unmatched or ambiguous placeholder edges, matching the
  Python fallback policy.
- Summary table computation now routes through
  `dagayn._core.GraphStore.compute_summaries` when available, covering
  community summaries, flow snapshots, and risk index rows with the same
  batched aggregate strategy as the Python fallback.
- Flow tracing can now run against the Rust store for full rebuilds while the
  tracing heuristics remain in Python. The traced flow dictionaries are stored
  through `dagayn._core.GraphStore.store_flows_json`, so the flow persistence
  transaction is Rust-owned. Incremental flow retracing now uses Rust-owned
  affected-flow deletion and append-only insertion, avoiding Python `_conn`
  access under `DAGAYN_BACKEND=rust`.
- Stored flow retrieval now routes through Rust JSON methods when available:
  `get_flows_json`, `get_flow_by_id_json`, and `get_affected_flows_json`.
- Community persistence and retrieval now route through Rust JSON methods when
  available: `store_communities_json` and `get_communities_json`, with
  node/edge read helpers exposed for Python's existing detection and overview
  logic. Full rebuild community detection now runs over those Rust read helpers
  under `DAGAYN_BACKEND=rust`; incremental community detection uses
  `count_affected_communities` to avoid Python-only SQLite connection access
  for the skip/redetect decision. `get_minimal_context` also reads top
  communities through this API instead of direct Python SQLite access.
- Graph stats now route through Rust `GraphStore.get_stats` while returning the
  existing Python `GraphStats` dataclass shape.
- Change risk analysis under `get_minimal_context` now has the Rust read
  helpers it needs for risk scoring and test-gap summaries, instead of falling
  back to degraded output when `DAGAYN_BACKEND=rust` is active.

Python modules being replaced: `dagayn/graph.py` (`GraphStore` upsert and replacement logic), `dagayn/incremental.py` (path normalization and VCS metadata helpers such as `_make_repo_relative`), `dagayn/migrations.py`.

Integration path: with `DAGAYN_BACKEND=rust`, the Python parser continues to emit `GraphNode` / `GraphEdge` records, but they are passed to `dagayn._core.GraphStore` for writes. This hybrid path is the first production exposure of the Rust backend.

Performance acceptance:

- `build` DB write phase on a ≥100k-line fixture: wall-clock ≤ 0.4× Python baseline
- `update` single-file replacement p95 latency: ≤ 200 ms

Parity acceptance: canonical JSON from Phase 0 fixtures matches Python output exactly.

`DAGAYN_BACKEND=rust` enables the Rust graph engine; `python` keeps the current Python path.

### Phase 2: Rust post-processing

Deliverable: `dagayn-postproc` implementing FTS rebuild, flow derivation, community derivation, and cached adjacency construction.

Python modules being replaced: `dagayn/postprocessing.py`, `dagayn/flows.py`, `dagayn/communities.py`, `dagayn/search.py`.

Community detection note: algorithm-level bit-identity is not required. Acceptance is based on **output stability** — the Rust and Python community boundaries must be within ±2% on canonical fixture repositories. This resolves the open question about whether to port the exact algorithm.

Performance acceptance (10k-node fixture):

- full `postprocess` run: wall-clock ≤ 0.3× Python baseline
- FTS rebuild only: ≤ 0.2× Python baseline

Parity acceptance:

- flow count and community count within ±2% of Python output on all fixtures
- FTS: same query returns the same top-20 results in the same order

`DAGAYN_BACKEND=rust` enables both graph and post-processing in Rust; parser remains Python.

### Phase 3: Rust parser

Deliverable: `dagayn-parser` and `dagayn-grammars` replacing Python `parser.py` (7 572 lines).

Language introduction order: Python → TypeScript/JS → Java → R → Bash → Markdown → Terraform → ipynb.

Grammar provisioning: `dagayn-grammars/build.rs` fetches pinned grammar archives and compiles them via `cc`. Cache behavior and the `DAGAYN_GRAMMAR_CACHE_DIR` env variable must match the contract in `docs/GRAMMAR-PROVISIONING.md`.

Markdown and ipynb: implement Rust-only paths as described in the Parser contract section above. Cross-artifact edges defined in `docs/CROSS-ARTIFACT-EDGES-WIP.md` must be preserved.

Parity acceptance: canonical JSON for all fixtures matches Python output exactly, including `extra` fields.

`DAGAYN_BACKEND=rust` enables the full Rust pipeline.

### Phase 4: Python compatibility shell

Refit `dagayn/cli.py`, `dagayn/main.py`, and `dagayn/tools/` to call `dagayn._core` directly.

`DAGAYN_BACKEND=python` must remain functional. Both backends must build at Phase 4 release.

Python parser, graph, and post-processing implementations are moved to `legacy_py/` (not deleted). The legacy path is used for ongoing regression comparison.

`DAGAYN_BACKEND=rust` becomes the default.

### Phase 5: optional outer-surface migration

Only after the core is stable should dagayn decide whether to migrate:

- the main CLI
- MCP server implementation
- install/config editing flows
- daemon/watch features

This should be treated as a separate decision, not an automatic consequence of the core migration.

## Distribution and packaging

dagayn uses **maturin** to build `dagayn._core` as a Python extension wheel.

Build targets:

| OS | Architecture |
|---|---|
| macOS | arm64 |
| macOS | x86_64 |
| Linux (manylinux) | x86_64 |
| Linux (manylinux) | aarch64 |

Windows support is deferred to Phase 5 evaluation.

CI: GitHub Actions matrix using `cibuildwheel`-compatible configuration. Rust toolchain version is pinned via `rust-toolchain.toml`.

Source distribution (`sdist`) is maintained alongside wheels. In environments without a Rust toolchain, the build falls back to a Python-only install by setting `DAGAYN_NATIVE=0` in `pyproject.toml` optional build config.

Because PyO3 increases Python packaging complexity, the wheel build runs in CI from Phase 1 onward — not deferred to pre-release.

## Acceptance criteria

The Rust core must not replace the Python core until all of these are true:

1. Graph snapshots match for canonical fixtures within an explicitly documented tolerance.
2. Mixed monorepo fixtures preserve repo-root-relative graph identity.
3. Markdown and Terraform fork behavior matches documented dagayn behavior.
4. Incremental updates match full rebuild semantics on parity fixtures.
5. Post-process outputs are stable enough for existing review/query tools.
6. Python CLI and MCP layers can switch backends without response-shape churn.
7. `build` DB write phase performance meets the Phase 1 numeric target (≤ 0.4× Python).
8. `postprocess` performance meets the Phase 2 numeric target (≤ 0.3× Python).
9. `DAGAYN_BACKEND={python,rust}` both produce the same canonical JSON on all fixtures (Phase 1 onward).
10. Wheel builds succeed on all four distribution targets in CI.

## Key risks

### 1. Semantic drift in parser output

The largest migration risk is not compilation difficulty; it is **graph drift**.

If the Rust parser changes heading slug rules, Terraform reference extraction, notebook cell attribution, path normalization, or type edge semantics, then downstream analysis will become inconsistent even if the system appears to work.

### 2. SQLite compatibility breakage

If the Rust graph engine changes write ordering, uniqueness rules, normalization rules, or migration behavior without a clear compatibility plan, existing Python tools may silently misbehave.

### 3. Incremental update regressions

Incremental update logic is harder than full rebuild logic. The Rust migration should assume that incremental parity needs dedicated fixtures and failure injection tests.

### 4. Post-process divergence

Flows, communities, and search indexes are derived products. Small graph differences can cause large downstream differences, especially in community boundaries and impact paths.

### 5. Packaging and grammar build complexity

Rust adds a second toolchain and new packaging expectations. Tree-sitter grammar provisioning, local builds, CI, and platform distribution all become more complex. PyO3 adoption increases Python packaging risk specifically, which is why wheel CI runs from Phase 1 rather than later.

### 6. Product-surface distraction

Rewriting outer Python surfaces too early would absorb time into install UX, daemon behavior, and MCP framework details before the core migration proves value. PyO3 adoption adds Python packaging risk on top of the Rust toolchain risk, so Phase 1–3 must keep outer surfaces in Python without exception.

## Open questions

One question remains open:

- Whether embeddings metadata should remain fully Python-owned even after search indexes move to Rust, or whether the Rust FTS layer should own the embedding index format as well.

All other questions raised in earlier drafts are resolved by the frozen decisions above.

## Current recommendation

dagayn should proceed with a **staged Rust core migration** in the order Graph → Post-processing → Parser.

The binding method is PyO3/maturin (`dagayn._core`). Parallel operation is controlled by `DAGAYN_BACKEND`. Python remains the compatibility shell until each phase proves parity.
