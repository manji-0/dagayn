# Dagayn Refactoring Priority Report, 2026-05-04

<!-- constrained-by ../AGENTS.md#how-agents-should-work-with-this-repo -->
<!-- derived-from ./refactoring-priority-report.md -->

Generated from dagayn graph tools on 2026-05-04. Graph snapshot:
5,739 nodes, 41,594 edges, 379 files, 30 languages, and 0 embeddings.

Important caveat: the worktree already had unrelated uncommitted changes while
this report was generated, including Rust parser changes and deleted ReScript
parser files. Treat graph-derived findings for those files as snapshot findings
until the graph is rebuilt after those changes settle.

## Executive Summary

The refactoring priority is still concentrated in orchestration, graph storage,
parser boundaries, and tool layering. The highest-risk pattern is not one big
file by itself; it is large symbols that are also hubs, bridge nodes, dependency
cycle participants, or under-tested graph entry points.

Recommended order:

1. Break package cycles through `dagayn`, `dagayn/tools`, `dagayn/cli`,
   `dagayn/cli/commands`, `dagayn/eval`, `dagayn/refactor`, and `dagayn/graph`.
2. Split build and incremental orchestration before changing parser internals.
3. Decompose graph store APIs around query, mutation, impact, review, and
   postprocessing concerns.
4. Stabilize parser boundaries with characterization tests, then extract large
   language handlers.
5. Clean graph/model noise: Markdown dead-code false positives, stale report
   cross-artifact links, and thin/single-file communities.

## Evidence Base

<!-- derived-from #executive-summary -->

Graph tools used:

| Tool | Signal used |
| --- | --- |
| `get_minimal_context_tool` | Overall risk medium at 0.65, 39 test gaps. |
| `list_graph_stats_tool` | Current graph size and language mix. |
| `architecture_analysis_tool(mode="overview")` | Docs/test coupling warnings. |
| `architecture_analysis_tool(mode="communities")` | Low-cohesion communities and large clusters. |
| `architecture_analysis_tool(mode="hubs")` | High-degree nodes with large blast radius. |
| `architecture_analysis_tool(mode="bridges")` | Betweenness chokepoints. |
| `find_large_functions_tool` | Long functions, production and tests. |
| `review_tool(mode="impact")` | File-level risk and impacted node counts. |
| `architecture_analysis_tool(mode="adp_violations")` | Package cycles. |
| `architecture_analysis_tool(mode="sdp_violations")` | Stable modules depending on less stable ones. |
| `architecture_analysis_tool(mode="sap_metrics")` | Pain/uselessness zones. |
| `architecture_analysis_tool(mode="knowledge_gaps")` | Untested hotspots and graph gaps. |
| `refactor_tool` | Dead-code and refactor suggestion quality. |

## Ranked Backlog

<!-- derived-from #evidence-base -->

### P0: Break Package-Level Cycles

Package ADP found 30 cycles, while file-level ADP found no cycles. That means
the immediate problem is package layering, not direct file import loops.

Highest-severity cycles:

| Severity | Cycle |
| ---: | --- |
| 384 | `dagayn/cli/commands -> dagayn/eval -> dagayn/eval/benchmarks -> dagayn/tools -> dagayn -> dagayn/cli` |
| 355 | `dagayn/cli/commands -> dagayn/eval -> dagayn/tools -> dagayn -> dagayn/cli` |
| 244 | `dagayn/cli/commands -> dagayn/tools -> dagayn -> dagayn/cli` |
| 147 | `dagayn/cli/commands -> dagayn/eval -> dagayn/tools -> dagayn/refactor -> dagayn/graph -> dagayn -> dagayn/cli` |
| 114 | `dagayn -> dagayn/tools` |

Refactor direction:

1. Make `dagayn/tools` a leaf-facing API layer that does not import broad root
   package behavior from `dagayn`.
2. Keep CLI command modules as consumers of tool APIs, not providers of shared
   orchestration helpers.
3. Move shared CLI contracts into `dagayn/cli` or a small neutral module; do
   not let `dagayn/cli` depend on volatile command implementations.
4. Keep `dagayn/eval` and benchmark code outside production tool import paths.

Done condition: the top three ADP cycles disappear and `dagayn -> dagayn/tools`
is removed or reduced to a deliberate public facade dependency.

### P1: Split Build And Incremental Orchestration

<!-- derived-from #p0-break-package-level-cycles -->

Primary evidence:

| Target | Signal |
| --- | --- |
| `dagayn/cli/commands/build.py::handle` | 313 lines, degree 140, high file impact risk. |
| `dagayn/incremental.py::incremental_update` | 273 lines, degree 130, bridge node. |
| `dagayn/incremental.py::full_build` | Bridge node, same orchestration file. |
| `dagayn/tools/build.py::_compute_summaries` | 231 lines, response shaping mixed with graph work. |
| `dagayn/tools/build.py::build_or_update_graph` | 185 lines, tool entry point and orchestration mixed. |

Refactor direction:

1. Introduce a build plan object for repo root, candidate files, parser backend,
   postprocess mode, and output mode.
2. Split side effects from result formatting: graph mutation, postprocess,
   progress reporting, and CLI/tool serialization should be separate functions.
3. Extract incremental candidate selection and parser dispatch from
   `incremental_update`.
4. Add characterization tests around build-plan construction before moving
   command behavior.

Done condition: `handle` and `incremental_update` drop below roughly 120 lines
each, while existing CLI and MCP build commands keep the same user-visible
output.

### P1: Decompose Graph Store Boundaries

<!-- derived-from #p0-break-package-level-cycles -->

Primary evidence:

| Target | Signal |
| --- | --- |
| `crates/dagayn-graph/src/lib.rs::GraphStore` | Degree 102, central Rust storage abstraction. |
| `crates/dagayn-graph/src/lib.rs::GraphStore.analyze_changes_json` | Degree 92, 158 lines. |
| `dagayn/graph/core.py::GraphStore` | Degree 85, Python storage counterpart. |
| `dagayn/graph` package | SAP pain zone, distance 0.5143. |

Refactor direction:

1. Keep public `GraphStore` construction stable.
2. Split Rust implementation into internal modules for schema, node/edge CRUD,
   summaries, flows, communities, impact, and review analysis.
3. Split Python graph code into query, mutation, impact/review, and SQL helper
   modules, with `core.py` as a compatibility shell during the transition.
4. Prefer method delegation and re-exported impl blocks over public API churn.

Done condition: module moves do not change public Python/Rust entry points, and
impact/review methods have focused tests before SQL is relocated.

### P1: Stabilize Parser Boundaries Before Extraction

<!-- derived-from #p1-split-build-and-incremental-orchestration -->

Primary evidence:

| Target | Signal |
| --- | --- |
| `dagayn/parser/languages/rescript.py::parse` | 401 lines, 105 degree, high impact risk. |
| `crates/dagayn-parser/src/core.rs::RustOwnedParser.parse_file_in_repo` | 235 lines, high impact risk, no graph-linked tests. |
| `dagayn/parser/languages/julia.py::_extract_julia_constructs` | 273 lines. |
| `dagayn/parser/core.py::CodeParser._extract_from_tree` | 196 lines. |
| `dagayn/parser/core.py::CodeParser._extract_calls` | 143 lines. |

Worktree caveat: current uncommitted changes delete `crates/dagayn-parser/src/rescript.rs`
and `crates/dagayn-parser/src/rescript_legacy.rs`, while the graph still
reported ReScript Rust parser symbols. Rebuild and re-run parser graph checks
after those changes are committed or reverted.

Refactor direction:

1. Add or refresh parity fixtures before moving parser code.
2. Extract parser phases: language detection, parser selection, tree walking,
   node extraction, import resolution, call resolution, and bridge detection.
3. For large language handlers, split by construct category rather than by
   syntax-tree utility helper.
4. Keep Rust/Python parity tests close to parser boundary changes.

Done condition: parser entry points keep stable output for fixture databases,
and each extracted phase has direct unit or parity coverage.

### P2: Improve Refactor Tool Precision

<!-- derived-from #evidence-base -->

`refactor_tool(mode="suggest")` produced 1,174 suggestions, but the first page
was dominated by Markdown headings represented as class-like nodes. Code-only
dead-code mode also reported many Rust API methods that may be public FFI or
externally consumed rather than removable.

Primary evidence:

| Signal | Count or example |
| --- | --- |
| Knowledge gaps | 339 total. |
| Thin communities | 150. |
| Single-file communities | 119. |
| Dead-code function candidates | 545, many likely public API/test artifacts. |
| Noisy suggestions | Markdown sections in `AGENTS.md`, `CHANGELOG.md`, localized READMEs. |

Refactor direction:

1. Exclude Markdown heading nodes from default dead-code suggestions.
2. Add public API/FFI/export awareness for Rust `GraphStore` and grammar APIs.
3. Rank suggestions by production file, degree, public visibility, and test
   evidence instead of raw unreferenced status.
4. Add a separate documentation-orphan report for Markdown heading gaps.

Done condition: the top 20 default refactor suggestions are actionable code
changes, not documentation headings or public API false positives.

### P2: VS Code Extension And UI Graph Code

<!-- derived-from #evidence-base -->

Primary evidence:

| Target | Signal |
| --- | --- |
| `dagayn-vscode/src/features/navigation.ts::registerNavigationCommands` | 301 lines. |
| `dagayn-vscode/src/webview/graph.ts::buildGraph` | Degree 96, 152 lines. |
| `dagayn-vscode/src/webview` | SAP uselessness zone. |
| `dagayn-vscode/test` | SAP uselessness zone. |

Refactor direction:

1. Extract command registration into command descriptors plus small handlers.
2. Split webview graph construction into data conversion, layout, rendering, and
   interaction phases.
3. Add tests around command descriptor registration and graph-data conversion.

Done condition: command registration is declarative enough to test without
activating the whole extension.

## Test Strategy

<!-- derived-from #ranked-backlog -->

Tests should be added before behavior-preserving extraction in these areas:

| Target area | Minimum test before refactor |
| --- | --- |
| Build CLI/tool path | Characterize plan/options/output for build and update modes. |
| Incremental update | Changed-file filtering, backend split, and postprocess selection. |
| Parser boundary | Fixture parity for changed languages and bridge extraction. |
| Graph store | SQL result equivalence for moved query/mutation functions. |
| Refactor suggestions | Golden tests for Markdown exclusion and public API filtering. |
| VS Code extension | Command descriptor and graph conversion tests. |

## Sequencing Notes

<!-- derived-from #ranked-backlog -->

Do not start with parser internals while the Rust parser files are already
changing. The safer sequence is:

1. Rebuild the dagayn graph after current Rust parser changes are resolved.
2. Break the CLI/tools/root package cycles.
3. Split build/incremental orchestration around stable tests.
4. Split graph store internals with compatibility shells.
5. Re-run graph hotspot, ADP, SDP, SAP, and dead-code checks.
6. Then extract parser phases using the refreshed graph.

## Summary

<!-- derived-from #executive-summary -->
<!-- derived-from #ranked-backlog -->
<!-- derived-from #test-strategy -->
<!-- derived-from #sequencing-notes -->

The next refactoring pass should optimize for architectural leverage, not line
count alone. The best first win is reducing package cycles and orchestration
fan-out, because that lowers risk before touching parser and graph-store
internals. The graph also shows that refactor-suggestion quality itself needs
attention: removing Markdown/public-API false positives will make future
prioritization reports more reliable.
