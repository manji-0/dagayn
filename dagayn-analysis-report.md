<!-- constrained-by ./AGENTS.md -->
<!-- derived-from ./docs/ARCHITECTURE.md -->
<!-- derived-from ./docs/COMMANDS.md -->
<!-- derived-from ./docs/SCHEMA.md -->
<!-- derived-from ./docs/MARKDOWN-AUTHORING.md -->

# dagayn 最新分析レポート

作成日: 2026-05-21

対象: dagayn 4.0.2 / branch `main` / local commit `47ae2fd`

根拠: `get_minimal_context_tool`, `list_graph_stats_tool`, `architecture_analysis_tool`, `flow_tool`, `refactor_tool`, `query_graph_tool`, `find_large_functions_tool`, targeted source inspection.

## 1. 要約

dagayn は、AI coding agent が repo 全体を読み直す代わりに、先に構造グラフへ問い合わせるためのローカル知識グラフ基盤である。現時点の強みはかなり明確で、Rust parser / SQLite graph / FTS5 / optional embeddings / MCP dispatcher / review and architecture tools が一つの運用面としてまとまっている。

今回の再分析で一番重要なのは、改善候補の重心が「未実装の高速化」から「大きくなった中核責務をどう小さく安全に扱うか」へ移っている点である。hub / bridge persistence、embedding search vectorization、Markdown artifact ref resolver batching、`parse_diff_ranges` cache、`target_name` index、`mtime_ns` incremental skip、local MCP latency benchmark は実装済みとして扱うべきで、次の主戦場ではない。

最新 graph では docs scope の ADP / SDP は 0 件になった。残る code scope の ADP は `dagayn` と `dagayn/visualization` の軽い 2-cycle 1 件のみで、severity は 6。したがって、今すぐ大規模な architecture cleanup を狙うより、parser / graph store / high-degree tool functions の直接テスト証拠、責務分割、metric noise の制御を優先した方がよい。

## 2. Graph Snapshot

`list_graph_stats_tool` の最新値:

| Metric | Value |
|---|---:|
| nodes | 8,022 |
| edges | 47,133 |
| files | 388 |
| languages | 29 |
| embeddings | 7,634 |
| last updated | 2026-05-22T00:05:56 |

Node kinds:

| Kind | Count |
|---|---:|
| Function | 2,908 |
| Test | 1,766 |
| DocBody | 1,759 |
| DocSection | 745 |
| Class | 456 |
| File | 388 |

Edge kinds:

| Kind | Count |
|---|---:|
| CALLS | 28,849 |
| CONTAINS | 7,704 |
| TESTED_BY | 6,725 |
| IMPORTS_FROM | 1,850 |
| CROSS_ARTIFACT | 1,679 |
| DEPENDS_ON | 140 |
| REFERENCES | 139 |
| INHERITS | 43 |
| IMPLEMENTS | 4 |

Interpretation: production behavior is primarily represented by call and test edges, but Markdown remains a major graph participant. Any architecture result must continue to separate code scope and docs scope.

## 3. Current Shape

Top communities by stored size:

| Rank | Community | Size | Cohesion |
|---:|---|---:|---:|
| 1 | docs-tool | 852 | 0.6730 |
| 2 | tests-detect | 291 | 0.6727 |
| 3 | tests-files | 285 | 0.5198 |
| 4 | tests-nodes | 264 | 0.6346 |
| 5 | docs-returns | 261 | 0.5681 |
| 6 | tests-install | 257 | 0.5463 |
| 7 | docs-confidence | 250 | 0.5260 |
| 8 | graph-nodes | 196 | 0.8015 |
| 9 | src-file | 140 | 0.5152 |
| 10 | tests-provider | 114 | 0.6079 |

`architecture_analysis_tool(mode="overview")` returned 369 communities, 5 coupled pairs shown, 0 warnings, and `truncated=true`. The displayed coupling example is `docs-tool -> tests-files` with 9 edges, mostly `CROSS_ARTIFACT`.

The biggest structural fact is not a failing dependency rule. It is the sheer size and centrality of a few implementation surfaces:

| Unit | Signal |
|---|---|
| `crates/dagayn-graph/src/lib.rs` | 4,095-line file; Rust GraphStore class is 2,936 lines |
| `dagayn/graph/core.py` | 2,062-line file; Python GraphStore class is 1,936 lines |
| `dagayn/cli/commands/build.py::handle` | top hub, degree 161 |
| `dagayn/refactor/dead_code.py::find_dead_code` | degree 121, high branch pressure |
| `dagayn/tools/query.py::query_graph` | degree 96, dispatcher complexity |
| `dagayn/incremental.py::incremental_update` | degree 125, large workflow coordinator |

## 4. Architecture Signals

ADP / SDP / SAP are now useful only when read with scope:

| Scope | Result |
|---|---|
| code ADP | 1 violation: `dagayn <-> dagayn/visualization`, severity 6 |
| code SDP | 0 violations at `min_delta=0.1` |
| docs ADP | 0 violations |
| docs SDP | 0 violations |
| code SAP | 7 violations at `min_distance=0.5` |

The ADP result is small. The visualization cycle is real, but its edge weight is low enough that it is not the best next project.

The SAP result is more interesting, but also noisier. Violations include `dagayn/parser/_base`, `dagayn/graph`, `dagayn/parser`, and `dagayn`, which are plausible design signals, but also `dagayn-vscode/test` and `dagayn-vscode/src/webview`, where abstractness/instability may not mean the same thing as in the Python/Rust core. The next SAP improvement should probably improve scope filtering and explanation before driving large refactors from it.

## 5. Hubs And Bridges

Top hubs:

| Rank | Node | Degree |
|---:|---|---:|
| 1 | `dagayn/cli/commands/build.py::handle` | 161 |
| 2 | `tests/test_flows.py::TestFlows._add_func` | 158 |
| 3 | `crates/dagayn-graph/src/tests.rs::stores_flows_and_reads_flow_inputs` | 147 |
| 4 | `dagayn/incremental.py::incremental_update` | 124 |
| 5 | `dagayn/refactor/dead_code.py::find_dead_code` | 121 |
| 6 | `dagayn/search.py::hybrid_search` | 115 |
| 7 | `docs/RUST-CORE-MIGRATION-WIP.md::phase-1-rust-graph-engine` | 114 |
| 8 | `tests/test_integration_v2.py::TestV2Integration.test_full_pipeline` | 111 |
| 9 | `dagayn/sap.py::compute_sap_metrics` | 107 |
| 10 | `dagayn/skills.py::install_platform_configs` | 107 |

Top bridges:

| Rank | Node | Betweenness |
|---:|---|---:|
| 1 | `tests/test_main.py::TestLongRunningToolsAreAsync.test_regression_guard_does_not_depend_on_fastmcp_internals` | 0.009445 |
| 2 | `tests/test_integration_v2.py::TestV2Integration.test_full_pipeline` | 0.007415 |
| 3 | `dagayn-vscode/test/sqlite.test.ts::it:isValid() returns false after close()@L509` | 0.004581 |
| 4 | `dagayn/main.py::_tool` | 0.003581 |
| 5 | `dagayn/search.py::hybrid_search` | 0.003055 |
| 6 | `tests/test_communities.py::TestCommunities.test_detected_cohesions_match_direct_computation` | 0.002411 |
| 7 | `dagayn/incremental.py::incremental_update` | 0.002310 |
| 8 | `tests/test_skills.py::TestInstallCodexHooks.test_reinstall_pi_hooks_deduplicates_dagayn_entries` | 0.002144 |
| 9 | `tests/test_cli_serve.py::test_serve_infers_local_embedding_from_existing_graph` | 0.002102 |
| 10 | `tests/test_daemon.py::TestWatchDaemon.test_status_from_state_reports_alive` | 0.002053 |

Bridge centrality is now read from persisted scores when available, so these values are useful as leads rather than query-time performance problems. A lot of bridge signal still lands on tests. That is not automatically bad: integration tests genuinely connect many subsystems. It does mean bridge rankings should not be read as "production chokepoints only" unless filtered.

## 6. Knowledge Gaps

`architecture_analysis_tool(mode="knowledge_gaps")` returned 2,609 raw gaps and `truncated=true`.

Raw counts:

| Category | Raw Count |
|---|---:|
| isolated nodes | 2,220 |
| thin communities | 88 |
| untested hotspots | 75 |
| single-file communities | 226 |

Thresholds:

| Threshold | Value |
|---|---:|
| isolated max degree | 1 |
| thin community min size | 3 |
| single-file min size | 3 |
| untested hotspot percentile | 0.95 |
| untested hotspot min degree | 37 |
| positive-degree candidate count | 2,234 |

Top untested-hotspot evidence is the most actionable part of this section. The graph reports no direct `TESTED_BY` edge for these high-degree code nodes:

| Node | Degree | Evidence |
|---|---:|---|
| `dagayn/refactor/dead_code.py::find_dead_code` | 121 | p95+ degree, no direct `TESTED_BY` edge |
| `crates/dagayn-graph/src/lib.rs::GraphStore` | 106 | p95+ degree, no direct `TESTED_BY` edge |
| `crates/dagayn-parser/src/core.rs::RustOwnedParser.parse_file_in_repo` | 103 | p95+ degree, no direct `TESTED_BY` edge |
| `dagayn/tools/query.py::query_graph` | 96 | p95+ degree, no direct `TESTED_BY` edge |
| `crates/dagayn-graph/src/lib.rs::GraphStore.analyze_changes_json` | 92 | p95+ degree, no direct `TESTED_BY` edge |

This is evidence about graph-visible direct coverage, not proof that behavior is untested. For example, `query_graph_tool(pattern="tests_for")` finds heuristic tests for `find_dead_code` and `query_graph`; it finds direct high-confidence tests for `dagayn/cli/commands/build.py::handle`; it finds zero tests for `parse_file_in_repo` and `analyze_changes_json`.

## 7. Flow Signals

Top flows by criticality:

| Rank | Flow | Criticality | Nodes |
|---:|---|---:|---:|
| 1 | activate | 0.6650 | 20 |
| 2 | embed | 0.6100 | 2 |
| 3 | embed_query | 0.6100 | 2 |
| 4 | benchmark_review_workflow | 0.6100 | 2 |
| 5 | benchmark_architecture_workflow | 0.6100 | 2 |
| 6 | benchmark_debug_workflow | 0.6100 | 2 |
| 7 | benchmark_onboard_workflow | 0.6100 | 2 |
| 8 | benchmark_pre_merge_workflow | 0.6100 | 2 |
| 9 | query_graph | 0.6100 | 4 |
| 10 | main | 0.4967 | 6 |

The flow list supports the same priority pattern as hubs and bridges: activation, embeddings, query graph, MCP dispatch, and benchmark paths are the interactive surfaces users feel first.

## 8. What Is Already Done

These should not be treated as fresh improvement candidates unless a regression appears:

- hub / bridge score persistence through `hub_scores` and `bridge_scores`
- Rust-backed Markdown artifact reference resolution
- `parse_diff_ranges` LRU cache
- normalized `edges.target_name` and index-backed lookup
- `mtime_ns` incremental skip and migration support
- embedding search matrix cache and BLAS fast path when numpy is available
- process-level `EmbeddingStore` cache
- local per-MCP-tool latency benchmark machinery
- docs-scope ADP / SDP cleanup

The remaining performance work is therefore narrower: batch embedding generation writes, reduce remaining row-level write fallback paths, batch DFS traversal where it still uses size-1 lookups, and turn latency baselines into an actionable gate.

## 9. Improvement Proposals

### Priority 1: Give Rust parser core direct graph-visible tests

Target: `crates/dagayn-parser/src/core.rs::RustOwnedParser.parse_file_in_repo`

Evidence:

- degree 103, above p95 hotspot threshold 37
- `query_graph_tool(pattern="tests_for")` returned 0 direct/heuristic tests
- refactor suggestions flag it as a large function with many collaborators
- parser behavior is product-critical across all supported languages

Recommended first step: add or adjust Rust tests so graph extraction creates direct `TESTED_BY` evidence for `parse_file_in_repo`, then split one responsibility only if the tests make the behavior boundary clear.

Acceptance:

- focused Rust parser tests pass
- `tests_for` no longer returns 0 for the target
- graph stats do not show new parser-side ADP/SDP regressions

### Priority 2: Make Rust GraphStore API ownership smaller and more test-visible

Targets:

- `crates/dagayn-graph/src/lib.rs::GraphStore`
- `crates/dagayn-graph/src/lib.rs::GraphStore.analyze_changes_json`

Evidence:

- Rust GraphStore class is 2,936 lines
- `analyze_changes_json` degree 92, above p95 threshold
- direct `tests_for` returned 0 for `analyze_changes_json`
- refactor suggestion split pressure for Rust GraphStore is 65.72

Recommended first step: do not split the whole class in one move. Start by carving out one internally cohesive area, such as analysis JSON assembly or flow/community read serialization, behind a private module while keeping the PyO3-facing method names stable.

Acceptance:

- parity tests still pass
- Python and Rust graph stores expose the same caller-facing behavior
- the extracted module has focused tests or graph-visible coverage evidence

### Priority 3: Decompose `find_dead_code`

Target: `dagayn/refactor/dead_code.py::find_dead_code`

Evidence:

- degree 121
- line count 281
- branch count 83
- graph reports no direct `TESTED_BY` edge, while `tests_for` finds heuristic tests in `tests/test_refactor.py`

Why this is a good next implementation: it is important, but smaller and safer than GraphStore. It also has an existing test neighborhood, so extraction can be done without inventing a new harness.

Recommended split:

- candidate collection
- dynamic/public API exclusions
- call/reference resolution
- result explanation and confidence

Acceptance:

- existing `tests/test_refactor.py` passes
- direct test evidence improves if practical
- output JSON remains compatible

### Priority 4: Split `query_graph` dispatcher paths

Target: `dagayn/tools/query.py::query_graph`

Evidence:

- degree 96
- line count 244
- branch count 66
- multiple existing tests in `tests/test_tools.py`

Recommended first step: extract pattern handlers for `docs_for`, `implementations_of`, `tests_for`, caller/callee/import patterns, and file summary into private helpers. Keep the public tool response schema unchanged.

Acceptance:

- `tests/test_tools.py` focused query tests pass
- `query_graph_tool` output remains stable for common patterns
- helper boundaries make future docs/code artifact behavior easier to test

### Priority 5: Split CLI build orchestration after preserving behavior

Target: `dagayn/cli/commands/build.py::handle`

Evidence:

- top hub degree 161
- line count 419
- branch count 68
- direct high-confidence tests already exist

This is high impact but not the first move because direct coverage evidence exists. The safe path is to extract option normalization, build/update selection, postprocess/embed handling, and output rendering separately while leaving command behavior unchanged.

### Priority 6: Improve SAP signal filtering

Targets: SAP metric/reporting code, not necessarily product architecture.

Evidence:

- code SAP returns 7 violations
- some top SAP entries are tests, fixtures, no-eligible-type packages, or VS Code UI/test scopes
- `dagayn/parser/_base`, `dagayn/graph`, and `dagayn/parser` look more actionable than `dagayn-vscode/test`

Recommended first step: add explicit scope filters or explanation buckets for tests, fixtures, isolated packages, and no-eligible-type packages. The goal is not to hide data; it is to keep SAP from recommending refactors where the metric is not semantically meaningful.

### Priority 7: Turn latency baseline into a regression story

Target: `dagayn/eval/benchmarks/mcp_latency.py` and `recent_changes_effects`.

Evidence:

- local benchmark machinery exists
- there is not yet a committed reference baseline and tolerance policy

Recommended first step: save local baseline JSON for the current repo, then compare repeated runs to establish variance before CI gating. Gate only after p50/p95 variance is known.

## 10. Non-Priorities

These are not good next tasks unless a specific bug appears:

- Reworking docs-scope ADP/SDP: current count is 0/0.
- Moving bridge centrality to persisted scores: already implemented.
- Replacing embedding search cosine loop with vectorized cache: already implemented for numpy.
- Adding `parse_diff_ranges` cache: already implemented.
- Adding `target_name` column for suffix lookup: already implemented.
- Adding `mtime_ns` incremental skip: already implemented.

## 11. Recommended Next Move

If the goal is the highest product value with the least thrash, start with Priority 3: decompose `dagayn/refactor/dead_code.py::find_dead_code`.

Reason: it is a true high-degree production function, graph-visible as a hotspot, already surrounded by tests, and small enough to improve in one controlled change. It will also exercise the refactor-suggestion surface itself, which is one of dagayn's core agent-facing workflows.

If the goal is to reduce hidden core risk instead, start with Priority 1: direct graph-visible tests for `parse_file_in_repo`. That is more foundational, but the Rust parser boundary is a little sharper-edged and deserves a test-first pass.

## 12. Verification Notes

Key commands used:

```bash
dagayn tool list_graph_stats_tool --format json
dagayn tool architecture_analysis_tool --arg mode='"overview"' --arg detail_level='"minimal"' --arg top_n=10 --format json
dagayn tool architecture_analysis_tool --arg mode='"knowledge_gaps"' --arg detail_level='"minimal"' --arg top_n=10 --format json
dagayn tool architecture_analysis_tool --arg mode='"adp_violations"' --arg artifact_scope='"code"' --format json
dagayn tool architecture_analysis_tool --arg mode='"sdp_violations"' --arg artifact_scope='"code"' --format json
dagayn tool architecture_analysis_tool --arg mode='"sap_violations"' --arg artifact_scope='"code"' --arg top_n=12 --format json
dagayn tool architecture_analysis_tool --arg mode='"adp_violations"' --arg artifact_scope='"docs"' --format json
dagayn tool architecture_analysis_tool --arg mode='"sdp_violations"' --arg artifact_scope='"docs"' --format json
dagayn tool flow_tool --arg mode='"list"' --arg sort_by='"criticality"' --arg limit=10 --format json
dagayn tool refactor_tool --arg mode='"suggest"' --arg limit=20 --format json
dagayn tool find_large_functions_tool --arg min_lines=80 --arg limit=15 --format json
```

Important truncation states:

- architecture overview: `truncated=true`; communities kept 1 of 368, coupling kept 1 of 5.
- knowledge gaps: `truncated=true`; raw gap count 2,609, returned examples 10 per category.
- refactor suggestions: 766 total, first 20 shown; counts are split 134, document 49, remove 583.

The conclusions above treat graph output as ranked evidence, not as automatic approval to refactor.
