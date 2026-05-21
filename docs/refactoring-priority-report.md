# Dagayn Refactoring Priority Report

<!-- Method context: ../AGENTS.md#how-agents-should-work-with-this-repo; not a graph dependency because AGENTS.md is root-level agent guidance. -->
<!-- derived-from ./USECASE-COHESION-REFACTOR.md#setting-priorities-from-the-readings -->

Generated from the dagayn graph on 2026-05-04. The graph snapshot contained
5,695 nodes, 41,081 edges, 378 files, 30 languages, and 0 embeddings. The
current graph risk score was low at 0.40, but the structural signals point to
several clear refactoring priorities.

## Executive Summary

The highest-value refactoring work is in the core graph, parser, build, and
tooling paths. These areas combine at least two risk signals: large symbols,
high graph degree, central execution flows, package cycles, or missing direct
test coverage.

Priority order:

1. Break package cycles around `dagayn`, `dagayn/tools`, `dagayn/cli`,
   `dagayn/cli/commands`, `dagayn/eval`, `dagayn/refactor`, and `dagayn/graph`.
2. Split graph storage implementations in `crates/dagayn-graph/src/lib.rs` and
   `dagayn/graph/core.py`.
3. Decompose parser monoliths, especially ReScript, Python parser core, and
   language-specific walk/extract functions.
4. Split build and incremental orchestration so CLI handling, graph mutation,
   postprocessing, and reporting are separate units.
5. Add tests before refactoring untested hotspots, especially the Rust ReScript
   parser and VS Code navigation command registration.

## Graph Findings

<!-- derived-from #executive-summary -->

### Size And Coupling

The repo is not uniformly risky. The risk is concentrated in a few files and
symbols.

Largest production files:

| File | Lines | Refactoring interpretation |
| --- | ---: | --- |
| `crates/dagayn-graph/src/lib.rs` | 3,630 | Rust graph store has too many responsibilities in one module. |
| `dagayn/incremental.py` | 1,918 | Build/update/watch orchestration is concentrated in one file. |
| `dagayn/graph/core.py` | 1,911 | Python graph store mirrors Rust complexity and remains large. |
| `crates/dagayn-parser/src/python.rs` | 1,377 | Language parser implementation should be split by extraction concern. |
| `crates/dagayn-py/src/lib.rs` | 1,331 | PyO3 boundary likely combines conversion, API, and orchestration. |
| `dagayn/parser/core.py` | 1,271 | Python parser core still owns broad language dispatch and extraction. |
| `dagayn/main.py` | 1,252 | MCP tool registration and wrappers are too centralized. |
| `dagayn/skills.py` | 1,174 | Platform config/install behavior should be modularized. |
| `dagayn/embeddings.py` | 982 | Provider, store, and API client concerns are in one file. |
| `dagayn/communities.py` | 944 | Community detection, splitting, and reporting logic should be separated. |

Largest production functions:

| Symbol | Lines | Signal |
| --- | ---: | --- |
| `dagayn/parser/languages/rescript.py::parse` | 401 | Legacy/parser logic is too procedural. |
| `dagayn-vscode/src/features/navigation.ts::registerNavigationCommands` | 330 | UI command wiring lacks direct test coverage. |
| `dagayn/cli/commands/build.py::handle` | 313 | CLI command handling includes orchestration and side effects. |
| `crates/dagayn-parser/src/rescript_legacy.rs::parse_rescript` | 280 | High-degree parser hotspot with no direct graph-linked tests. |
| `dagayn/parser/languages/julia.py::_extract_julia_constructs` | 273 | Language-specific extraction needs subroutines. |
| `dagayn/incremental.py::incremental_update` | 273 | Central update path with high blast radius. |
| `dagayn/refactor/dead_code.py::find_dead_code` | 256 | Refactor tool analysis itself is complex and noisy for docs. |
| `crates/dagayn-parser/src/core.rs::RustOwnedParser.parse_file_in_repo` | 235 | Rust parser boundary combines path, parse, and conversion concerns. |
| `dagayn/tools/build.py::_compute_summaries` | 231 | Tool response shaping and summary computation should be isolated. |

### Hotspots And Chokepoints

The top production graph hubs are concentrated in parser, graph, build,
incremental, SAP, embeddings, export/wiki, and VS Code features.

High-degree nodes worth treating as architectural hotspots:

| Symbol | Degree | Why it matters |
| --- | ---: | --- |
| `crates/dagayn-parser/src/rescript_legacy.rs::parse_rescript` | 154 | Untested hub, large function, parser correctness risk. |
| `dagayn/cli/commands/build.py::handle` | 140 | Main build CLI path, large function, broad side effects. |
| `dagayn/incremental.py::incremental_update` | 129 | Central incremental path and bridge node. |
| `crates/dagayn-parser/src/core.rs::RustOwnedParser.parse_file_in_repo` | 103 | Rust parser boundary with high fan-out. |
| `crates/dagayn-graph/src/lib.rs::GraphStore` | 102 | Core storage abstraction and large class/module. |
| `dagayn/sap.py::compute_sap_metrics` | 100 | Architectural metric computation with no direct graph-linked tests. |
| `dagayn/refactor/dead_code.py::find_dead_code` | 99 | Produces noisy suggestions against Markdown headings. |
| `dagayn/embeddings.py::OpenAIEmbeddingProvider` | 93 | Large provider implementation and external API boundary. |
| `dagayn/graph/core.py::GraphStore` | 85 | Python storage counterpart remains large. |
| `dagayn/wiki.py::_generate_community_page` | 83 | Report generation logic has high fan-out. |
| `dagayn-vscode/src/features/navigation.ts::registerNavigationCommands` | 80 | Large UI command registration, no direct tests. |

Bridge nodes reinforce this priority list. Production bridge nodes included
`WatchDaemon`, `incremental_update`, `full_build`, `compute_sap_metrics`,
`get_provider`, `EmbeddingStore`, `hybrid_search`, `analyze_changes`, and
`run_post_processing`.

## Architecture Risks

<!-- derived-from #graph-findings -->

### Package Cycles

`dagayn detect-adp --granularity package` found 29 package-level cycles. The
highest-severity cycles all pass through the same broad layer boundary:

| Severity | Cycle |
| ---: | --- |
| 384 | `dagayn/eval/benchmarks -> dagayn/tools -> dagayn -> dagayn/cli -> dagayn/cli/commands -> dagayn/eval -> dagayn/eval/benchmarks` |
| 355 | `dagayn -> dagayn/cli -> dagayn/cli/commands -> dagayn/eval -> dagayn/tools -> dagayn` |
| 244 | `dagayn -> dagayn/cli -> dagayn/cli/commands -> dagayn/tools -> dagayn` |
| 154 | `dagayn -> dagayn/cli -> dagayn/cli/commands -> dagayn/eval -> dagayn/tools -> dagayn/refactor -> dagayn/graph -> dagayn` |
| 132 | `dagayn -> dagayn/cli -> dagayn/cli/commands -> dagayn/eval -> dagayn/tools -> dagayn/graph -> dagayn` |
| 114 | `dagayn -> dagayn/tools -> dagayn` |
| 38 | `dagayn -> dagayn/graph -> dagayn` |
| 8 | `dagayn -> dagayn/parser -> dagayn` |
| 4 | `dagayn/cli -> dagayn/cli/commands -> dagayn/cli` |

File-level ADP found no cycles for `--max-cycle-length 6`, so the immediate
problem is package layering rather than direct file loops.

### Stability Direction

`detect_sdp` found one package-level stability-direction violation:

| Source | Target | Source I | Target I | Delta |
| --- | --- | ---: | ---: | ---: |
| `dagayn/cli` | `dagayn/cli/commands` | 0.25 | 0.75 | 0.50 |

This suggests the stable CLI package depends on a more volatile command package.
Move shared CLI abstractions down into `dagayn/cli` and keep command modules as
leaf implementations.

### SAP Signals

`detect-sap` reported 42 package-level SAP violations. Many are low-value
fixtures or documentation scopes, but production scopes worth watching are:

| Scope | Distance | Interpretation |
| --- | ---: | --- |
| `crates/dagayn-graph/src` | 1.00 | Stable concrete graph core, high cost to change. |
| `crates/dagayn-parser/src` | 1.00 | Stable concrete parser core, broad blast radius. |
| `crates/dagayn-py/src` | 1.00 | Stable concrete binding layer. |
| `dagayn/graph` | 0.80 | Stable concrete Python graph API. |
| `dagayn/cli` | 0.75 | Stable concrete CLI layer. |
| `dagayn/tools` | 0.625 | Tool layer is concrete and tied into cycles. |
| `dagayn` | 0.625 | Root package participates in cycles and many imports. |
| `dagayn/visualization` | 0.60 | Concrete reporting layer depends on core structures. |

The practical takeaway is not "add abstract classes everywhere." The better
move is to create narrow data and port modules around graph store operations,
parser outputs, and command/tool response DTOs so stable modules depend on
small contracts rather than broad concrete implementation modules.

## Refactoring Backlog

<!-- derived-from #architecture-risks -->

### P0: Break The Core Package Cycles

Target cycles involving `dagayn`, `dagayn/tools`, `dagayn/cli`,
`dagayn/cli/commands`, `dagayn/eval`, `dagayn/refactor`, and `dagayn/graph`.

Recommended sequence:

1. Move CLI-agnostic response shaping from command modules into leaf tool
   modules or a small `dagayn/tool_response` style module.
2. Stop `dagayn/tools` from importing broad root package helpers where possible.
3. Keep `dagayn/eval` and `dagayn/eval/benchmarks` as consumers of public tool
   APIs, not participants in tool or CLI internals.
4. Extract graph-store interfaces or DTO conversion helpers out of
   `dagayn/graph/core.py` if those helpers are imported by higher layers.

Done condition: package ADP count drops materially, and the top three cycles
above disappear.

### P1: Split Graph Store Implementations

Primary targets:

| File | Current signal |
| --- | --- |
| `crates/dagayn-graph/src/lib.rs` | 3,630 lines, `GraphStore` is 2,724 lines, high-degree hub. |
| `dagayn/graph/core.py` | 1,911 lines, `GraphStore` is 1,823 lines. |

Suggested Rust split:

1. Keep public `GraphStore` type and constructor stable.
2. Move schema/migration code into `schema.rs`.
3. Move node/edge CRUD into `nodes.rs` and `edges.rs`.
4. Move summaries, flows, communities, impact, and review analysis into focused
   modules.
5. Re-export methods through `impl GraphStore` blocks to keep the public API
   stable during the first pass.

Suggested Python split:

1. Keep `dagayn/graph/core.py` as compatibility shell.
2. Extract query operations, mutation operations, impact/review operations, and
   SQL helpers into private modules under `dagayn/graph/`.
3. Add characterization tests around any extracted SQL before moving behavior.

### P1: Decompose Parser Monoliths

Highest priority parser targets:

| Target | Signal |
| --- | --- |
| `crates/dagayn-parser/src/rescript_legacy.rs::parse_rescript` | 280 lines, degree 154, no direct graph-linked tests. |
| `dagayn/parser/languages/rescript.py::parse` | 401 lines. |
| `dagayn/parser/core.py::CodeParser` | 1,228 lines, broad dispatch/extraction responsibility. |
| `crates/dagayn-parser/src/core.rs::RustOwnedParser.parse_file_in_repo` | 235 lines, degree 103. |
| `crates/dagayn-parser/src/python.rs` | 1,377 lines. |

Recommended sequence:

1. Add focused fixtures and parity tests for ReScript before changing
   `parse_rescript`.
2. Extract ReScript node handlers by construct type: imports, declarations,
   function-like forms, calls, and export/module resolution.
3. In Python `CodeParser`, separate language detection, parser selection, tree
   walking, import resolution, call resolution, and cross-artifact bridge
   detection.
4. Prefer table-driven per-language handlers where existing Rust modules already
   follow that pattern.

### P1: Split Build And Incremental Orchestration

Targets:

| Target | Signal |
| --- | --- |
| `dagayn/cli/commands/build.py::handle` | 313 lines, degree 140. |
| `dagayn/tools/build.py::_compute_summaries` | 231 lines. |
| `dagayn/tools/build.py::build_or_update_graph` | 185 lines. |
| `dagayn/incremental.py::incremental_update` | 273 lines, degree 129, bridge node. |
| `dagayn/incremental.py::full_build` | 115 lines, bridge node. |

Recommended sequence:

1. Introduce a build plan object that contains repo root, changed files,
   postprocess options, and output mode.
2. Split side-effect orchestration from result formatting.
3. Keep CLI parsing in `dagayn/cli/commands/build.py`; move execution into a
   service module that is shared by CLI and MCP tools.
4. Add tests at the service boundary so CLI and MCP wrappers stay thin.

### P2: Reduce Tool Output And Dead-Code Noise

The graph produced 1,151 dead-code suggestions, but the first 100 were Markdown
headings from `AGENTS.md`, `CHANGELOG.md`, localized READMEs, and similar docs.
That is a product-quality issue in the refactor tooling, not a real deletion
backlog.

Recommended changes:

1. Add `kind` and language-aware filters to dead-code recommendations by
   default.
2. Exclude Markdown section nodes from remove suggestions unless the caller
   explicitly asks for documentation cleanup.
3. Rank production code suggestions before docs and fixtures.
4. Add a "confidence" field that explains whether the candidate is executable
   code, generated graph artifact, fixture, or documentation.

### P2: Split Embeddings And Search Boundaries

Targets:

| Target | Signal |
| --- | --- |
| `dagayn/embeddings.py` | 982 lines. |
| `dagayn/embeddings.py::OpenAIEmbeddingProvider` | 267 lines, degree 93. |
| `dagayn/embeddings.py::EmbeddingStore` | 176 lines, bridge node. |
| `dagayn/search.py::hybrid_search` | 141 lines, bridge node. |

Recommended sequence:

1. Split provider implementations from provider selection.
2. Move HTTP request/response handling out of provider business logic.
3. Keep `EmbeddingStore` focused on persistence and cache semantics.
4. Keep search ranking/scoring separate from embedding retrieval.

### P2: Test And Split VS Code Extension Features

Targets:

| Target | Signal |
| --- | --- |
| `dagayn-vscode/src/features/navigation.ts::registerNavigationCommands` | 330 lines, no direct graph-linked tests. |
| `dagayn-vscode/src/backend/sqlite.ts::SqliteReader` | 436-line class, appears in bridge-node tests. |
| `dagayn-vscode/src/webview/graph.ts::buildGraph` | 152 lines. |
| `dagayn-vscode/src/views/graphWebview.ts::GraphWebviewPanel` | 301-line class. |

Recommended sequence:

1. Split command registration from command implementations.
2. Create testable services for path lookup, symbol lookup, and editor
   navigation.
3. Add unit tests around `registerNavigationCommands` behavior before moving
   command bodies.

### P3: Split Oversized Test Fixtures

Large tests are useful as characterization tests, but several files are now too
large for fast review:

| File | Lines |
| --- | ---: |
| `crates/dagayn-parser/src/core_tests.rs` | 2,649 |
| `tests/test_parser.py` | 2,175 |
| `tests/test_multilang.py` | 2,096 |
| `tests/test_tools.py` | 1,748 |
| `tests/test_skills.py` | 1,552 |

Move these by feature area only after production boundaries are stable. Do not
start here unless a refactor needs local test readability.

## Test Coverage Priorities

<!-- derived-from #refactoring-backlog -->

Add or strengthen tests before touching these nodes:

| Target | Current graph test signal | Priority |
| --- | --- | --- |
| `crates/dagayn-parser/src/rescript_legacy.rs::parse_rescript` | 0 direct tests | P0 before parser refactor |
| `dagayn/sap.py::compute_sap_metrics` | 0 direct tests | P1 before metric refactor |
| `dagayn-vscode/src/features/navigation.ts::registerNavigationCommands` | 0 direct tests | P1 before extension refactor |
| `dagayn/incremental.py::incremental_update` | 1 direct test | P1 add characterization tests |
| `crates/dagayn-graph/src/lib.rs::GraphStore` | 4 direct tests | P1 broaden around extracted modules |
| `dagayn/parser/core.py::CodeParser` | 7 direct tests | P1 add extraction/resolution tests |
| `dagayn/embeddings.py::get_provider` | 2 direct tests | P2 provider split coverage |

## Suggested Milestones

<!-- derived-from #test-coverage-priorities -->

### Milestone 1: Architectural Cycle Reduction

Goal: remove the top package ADP cycles.

Scope:

- `dagayn/tools`
- `dagayn/cli`
- `dagayn/cli/commands`
- `dagayn/eval`
- `dagayn/refactor`
- `dagayn/graph`

Validation:

- `dagayn detect-adp --granularity package`
- targeted unit tests for moved wrappers and DTO formatting

### Milestone 2: Graph Store Modularization

Goal: split storage modules without changing public behavior.

Scope:

- `crates/dagayn-graph/src/lib.rs`
- `dagayn/graph/core.py`

Validation:

- Rust graph crate tests
- Python graph tests
- `dagayn build` on the repo
- `dagayn status`

### Milestone 3: Parser Decomposition

Goal: reduce parser hotspots while preserving language parity.

Scope:

- `crates/dagayn-parser/src/rescript_legacy.rs`
- `crates/dagayn-parser/src/core.rs`
- `crates/dagayn-parser/src/python.rs`
- `dagayn/parser/core.py`
- `dagayn/parser/languages/rescript.py`

Validation:

- parser unit tests
- parity tests
- graph rebuild and postprocess

### Milestone 4: Tooling Quality

Goal: make refactor suggestions actionable.

Scope:

- `dagayn/refactor/dead_code.py`
- `dagayn/tools/*`
- output envelope helpers

Validation:

- dead-code suggestions no longer rank Markdown headings as production removal
  work
- MCP minimal output stays compact

## Non-Goals

<!-- derived-from #suggested-milestones -->

- Do not begin by deleting the 1,151 reported dead-code symbols. The raw list is
  dominated by Markdown section nodes.
- Do not add generic abstract base classes solely to improve SAP scores.
- Do not split tests first unless needed to support a production refactor.
- Do not change public CLI or MCP tool contracts while reducing cycles.

## Summary

<!-- derived-from #executive-summary -->
<!-- derived-from #architecture-risks -->
<!-- derived-from #refactoring-backlog -->
<!-- derived-from #test-coverage-priorities -->

The best first refactor is not a file-size cleanup. The graph points to a
layering cleanup around CLI, tools, eval, graph, and refactor modules, followed
by carefully tested decomposition of graph store and parser hotspots. The
highest-risk individual code targets are the Rust and Python parser monoliths,
the graph store implementations, build/incremental orchestration, dead-code
analysis, embeddings/search boundaries, and VS Code command registration.
