<!-- constrained-by ../AGENTS.md -->
<!-- derived-from ./ARCHITECTURE.md -->
<!-- derived-from ./COMMANDS.md -->
<!-- derived-from ./SCHEMA.md -->
<!-- derived-from ./MARKDOWN-AUTHORING.md -->

# dagayn 最新分析レポート

作成日: 2026-05-21

対象: dagayn 4.0.2 / branch `main` / local commit `47ae2fd`

根拠: `get_minimal_context_tool`, `list_graph_stats_tool`, `architecture_analysis_tool`, `flow_tool`, `refactor_tool`, `query_graph_tool`, `find_large_functions_tool`, targeted source inspection。

## 1. 要約

dagayn は、AI coding agent が repo 全体を読み直す代わりに、先に構造グラフへ問い合わせるためのローカル知識グラフ基盤である。現時点の強みはかなり明確で、Rust parser / SQLite graph / FTS5 / optional embeddings / MCP dispatcher / review and architecture tools が一つの運用面としてまとまっている。

今回の再分析で一番重要なのは、改善候補の重心が「未実装の高速化」から「大きくなった中核責務をどう小さく安全に扱うか」へ移っている点である。hub / bridge persistence、embedding search vectorization、Markdown artifact ref resolver batching、`parse_diff_ranges` cache、`target_name` index、`mtime_ns` incremental skip、local MCP latency benchmark は実装済みとして扱うべきで、次の主戦場ではない。

最新 graph では docs scope の ADP / SDP は 0 件になった。残る code scope の ADP は `dagayn` と `dagayn/visualization` の軽い 2-cycle 1 件のみで、severity は 6。したがって、今すぐ大規模な architecture cleanup を狙うより、parser / graph store / high-degree tool functions の直接テスト証拠、責務分割、metric noise の制御を優先した方がよい。

## 2. グラフスナップショット

`list_graph_stats_tool` の最新値:

| 指標 | 値 |
|---|---:|
| ノード数 | 8,022 |
| エッジ数 | 47,133 |
| ファイル数 | 388 |
| 言語数 | 29 |
| embedding 数 | 7,634 |
| 最終更新 | 2026-05-22T00:05:56 |

ノード種別:

| 種別 | 件数 |
|---|---:|
| Function | 2,908 |
| Test | 1,766 |
| DocBody | 1,759 |
| DocSection | 745 |
| Class | 456 |
| File | 388 |

エッジ種別:

| 種別 | 件数 |
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

解釈: production behavior は主に call edge と test edge で表現されているが、Markdown も依然として graph の主要な構成要素である。architecture result を読むときは、今後も code scope と docs scope を分ける必要がある。

## 3. 現在の構造

保存サイズ順の上位 community:

| 順位 | Community | サイズ | 凝集度 |
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

`architecture_analysis_tool(mode="overview")` は community 369 件を返し、coupled pair は 5 件表示、warning は 0 件、`truncated=true` だった。表示された coupling example は `docs-tool -> tests-files` で、edge は 9 件、その大半は `CROSS_ARTIFACT` だった。

最も大きな構造上の事実は、依存ルール違反ではない。少数の実装面が非常に大きく、中心性も高いことである。

| 単位 | シグナル |
|---|---|
| `crates/dagayn-graph/src/lib.rs` | 4,095 行の file。Rust GraphStore class は 2,936 行 |
| `dagayn/graph/core.py` | 2,062 行の file。Python GraphStore class は 1,936 行 |
| `dagayn/cli/commands/build.py::handle` | top hub、degree 161 |
| `dagayn/refactor/dead_code.py::find_dead_code` | degree 121、branch pressure が高い |
| `dagayn/tools/query.py::query_graph` | degree 96、dispatcher complexity |
| `dagayn/incremental.py::incremental_update` | degree 125、大きな workflow coordinator |

## 4. アーキテクチャシグナル

ADP / SDP / SAP は、scope と一緒に読む場合にだけ有用である。

| Scope | 結果 |
|---|---|
| code ADP | violation 1 件: `dagayn <-> dagayn/visualization`, severity 6 |
| code SDP | `min_delta=0.1` で violation 0 件 |
| docs ADP | violation 0 件 |
| docs SDP | violation 0 件 |
| code SAP | `min_distance=0.5` で violation 7 件 |

ADP result は小さい。visualization cycle は実在するが、edge weight は低く、次の project として最適ではない。

SAP result はより興味深いが、noise も多い。violation には、plausible design signal と言える `dagayn/parser/_base`、`dagayn/graph`、`dagayn/parser`、`dagayn` が含まれる。一方で、`dagayn-vscode/test` や `dagayn-vscode/src/webview` も含まれており、ここでは abstractness/instability が Python/Rust core と同じ意味を持つとは限らない。次の SAP 改善では、大きな refactor を導く前に scope filtering と explanation を改善するのがよい。

## 5. ハブとブリッジ

上位 hub:

| 順位 | ノード | Degree |
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

上位 bridge:

| 順位 | ノード | 媒介中心性 |
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

bridge centrality は、利用できる場合は永続化済み score から読まれるようになっている。そのため、これらの値は query-time performance problem ではなく、有力な手がかりとして使える。bridge signal の多くはまだ tests に集中している。これは自動的に悪いわけではない。integration test は実際に多くの subsystem を接続する。ただし filter しない限り、bridge ranking を「production chokepoint だけ」と読んではいけない。

## 6. ナレッジギャップ

`architecture_analysis_tool(mode="knowledge_gaps")` は raw gap 2,609 件を返し、`truncated=true` だった。

raw count:

| カテゴリ | raw count |
|---|---:|
| isolated nodes | 2,220 |
| thin communities | 88 |
| untested hotspots | 75 |
| single-file communities | 226 |

threshold:

| threshold | 値 |
|---|---:|
| isolated max degree | 1 |
| thin community min size | 3 |
| single-file min size | 3 |
| untested hotspot percentile | 0.95 |
| untested hotspot min degree | 37 |
| positive-degree candidate count | 2,234 |

top untested-hotspot evidence は、この section で最も actionable な部分である。graph は、次の high-degree code node に direct `TESTED_BY` edge がないと報告している。

| ノード | Degree | 根拠 |
|---|---:|---|
| `dagayn/refactor/dead_code.py::find_dead_code` | 121 | p95+ degree、direct `TESTED_BY` edge なし |
| `crates/dagayn-graph/src/lib.rs::GraphStore` | 106 | p95+ degree、direct `TESTED_BY` edge なし |
| `crates/dagayn-parser/src/core.rs::RustOwnedParser.parse_file_in_repo` | 103 | p95+ degree、direct `TESTED_BY` edge なし |
| `dagayn/tools/query.py::query_graph` | 96 | p95+ degree、direct `TESTED_BY` edge なし |
| `crates/dagayn-graph/src/lib.rs::GraphStore.analyze_changes_json` | 92 | p95+ degree、direct `TESTED_BY` edge なし |

これは graph-visible な direct coverage に関する根拠であり、behavior が未テストであることの証明ではない。たとえば `query_graph_tool(pattern="tests_for")` は `find_dead_code` と `query_graph` に heuristic tests を見つける。`dagayn/cli/commands/build.py::handle` には direct high-confidence tests を見つける。一方で、`parse_file_in_repo` と `analyze_changes_json` には test を 0 件と返す。

## 7. フローシグナル

criticality 順の上位 flow:

| 順位 | Flow | Criticality | ノード数 |
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

flow list も hubs and bridges と同じ priority pattern を支持している。activation、embeddings、query graph、MCP dispatch、benchmark path は、user が最初に体感する interactive surface である。

## 8. すでに完了していること

regression が現れない限り、これらを新しい improvement candidate として扱うべきではない。

- `hub_scores` と `bridge_scores` による hub / bridge score persistence
- Rust-backed Markdown artifact reference resolution
- `parse_diff_ranges` LRU cache
- normalized `edges.target_name` と index-backed lookup
- `mtime_ns` incremental skip と migration support
- numpy が利用可能な場合の embedding search matrix cache と BLAS fast path
- process-level `EmbeddingStore` cache
- local per-MCP-tool latency benchmark machinery
- docs-scope ADP / SDP cleanup

したがって、残る performance work はより狭い。embedding generation writes の batch 化、残っている row-level write fallback path の削減、まだ size-1 lookup を使っている DFS traversal の batch 化、latency baseline を actionable gate に変えることである。

## 9. 改善提案

### 優先度 1: Rust parser core に graph-visible な direct test を追加する

対象: `crates/dagayn-parser/src/core.rs::RustOwnedParser.parse_file_in_repo`

根拠:

- degree 103 で、p95 hotspot threshold 37 を上回る
- `query_graph_tool(pattern="tests_for")` が direct/heuristic tests を 0 件と返した
- refactor suggestions が、多くの collaborator を持つ large function として flag している
- parser behavior は、対応している全言語にまたがって product-critical である

推奨する最初の一手: graph extraction が `parse_file_in_repo` に対する direct `TESTED_BY` evidence を作れるように Rust tests を追加または調整する。そのうえで、tests によって behavior boundary が明確になった場合だけ、責務を 1 つ分割する。

受け入れ条件:

- focused Rust parser tests が通る
- `tests_for` が対象に対して 0 を返さなくなる
- graph stats に新しい parser-side ADP/SDP regression が出ない

### 優先度 2: Rust GraphStore API の所有範囲を小さくし、test-visible にする

対象:

- `crates/dagayn-graph/src/lib.rs::GraphStore`
- `crates/dagayn-graph/src/lib.rs::GraphStore.analyze_changes_json`

根拠:

- Rust GraphStore class は 2,936 行
- `analyze_changes_json` は degree 92 で、p95 threshold を上回る
- direct `tests_for` は `analyze_changes_json` に 0 を返した
- Rust GraphStore の refactor suggestion split pressure は 65.72

推奨する最初の一手: class 全体を一度に分割しない。PyO3-facing method name を安定させたまま、analysis JSON assembly や flow/community read serialization のような内部的に凝集した領域を 1 つ private module に切り出す。

受け入れ条件:

- parity tests が引き続き通る
- Python と Rust の graph store が、caller-facing behavior として同じものを公開する
- 抽出した module に focused tests または graph-visible coverage evidence がある

### 優先度 3: `find_dead_code` を分解する

**済（2026-08-16）**: `dagayn/refactor/dead_code.py` で `_DeadCodeLookups` / `_collect_dead_code_context`（candidate collection + 全プリロード）/ `_node_dead_code_evidence`（call/reference resolution）を抽出し、`find_dead_code` を薄いオーケストレータ化。挙動は保存（`tests/test_refactor.py` 79件 + 関連223件がパス、dead-code 出力 516件で一致）。

根拠:

- degree 121
- line count 281
- branch count 83
- graph は direct `TESTED_BY` edge がないと報告する一方、`tests_for` は `tests/test_refactor.py` に heuristic tests を見つける

これが次の実装として適している理由: 重要だが、GraphStore より小さく安全である。既存の test neighborhood もあるため、新しい harness を発明せずに抽出できる。

推奨する分割:

- candidate collection
- dynamic/public API exclusions
- call/reference resolution
- result explanation and confidence

受け入れ条件:

- 既存の `tests/test_refactor.py` が通る
- 現実的であれば direct test evidence が改善する
- output JSON の互換性が維持される

### 優先度 4: `query_graph` dispatcher path を分割する

対象: `dagayn/tools/query.py::query_graph`

根拠:

- degree 96
- line count 244
- branch count 66
- `tests/test_tools.py` に複数の既存 test がある

推奨する最初の一手: `docs_for`、`implementations_of`、`tests_for`、caller/callee/import pattern、file summary の pattern handler を private helper に抽出する。public tool response schema は変更しない。

受け入れ条件:

- `tests/test_tools.py` の focused query tests が通る
- common pattern に対する `query_graph_tool` output が安定したままである
- helper boundary により、今後の docs/code artifact behavior を test しやすくなる

### 優先度 5: behavior を維持したうえで CLI build orchestration を分割する

対象: `dagayn/cli/commands/build.py::handle`

根拠:

- top hub degree 161
- line count 419
- branch count 68
- direct high-confidence tests がすでに存在する

これは high impact だが、direct coverage evidence が存在するため最初の一手ではない。安全な進め方は、command behavior を変えずに、option normalization、build/update selection、postprocess/embed handling、output rendering を個別に抽出することである。

### 優先度 6: SAP signal filtering を改善する

対象: SAP metric/reporting code。必ずしも product architecture そのものではない。

根拠:

- code SAP は 7 violations を返す
- 一部の top SAP entry は tests、fixtures、no-eligible-type packages、または VS Code UI/test scopes である
- `dagayn/parser/_base`、`dagayn/graph`、`dagayn/parser` は `dagayn-vscode/test` より actionable に見える

推奨する最初の一手: tests、fixtures、isolated packages、no-eligible-type packages に対する明示的な scope filter または explanation bucket を追加する。目的は data を隠すことではない。metric が意味論的に有効ではない場所で SAP が refactor を勧めないようにすることである。

### 優先度 7: latency baseline を regression story に変える

対象: `dagayn/eval/benchmarks/mcp_latency.py` と `recent_changes_effects`。

根拠:

- local benchmark machinery は存在する
- committed reference baseline と tolerance policy はまだ存在しない

推奨する最初の一手: 現在の repo に対する local baseline JSON を保存し、CI gating の前に repeated runs を比較して variance を把握する。gate は p50/p95 variance が分かってからにする。

## 10. 優先しないこと

具体的な bug が出ない限り、これらは次の task として適していない。

- docs-scope ADP/SDP の作り直し: 現在の count は 0/0。
- bridge centrality の persisted scores 化: 実装済み。
- embedding search cosine loop の vectorized cache 置換: numpy 向けには実装済み。
- `parse_diff_ranges` cache の追加: 実装済み。
- suffix lookup 用の `target_name` column 追加: 実装済み。
- `mtime_ns` incremental skip の追加: 実装済み。

## 11. 推奨される次の一手

最小の混乱で最大の product value を狙うなら、優先度 3、つまり `dagayn/refactor/dead_code.py::find_dead_code` の分解から始める。

理由: これは本物の high-degree production function であり、graph-visible な hotspot であり、すでに tests に囲まれており、1 つの controlled change で改善できる程度に小さい。また、dagayn の core agent-facing workflow の 1 つである refactor-suggestion surface 自体も exercise できる。

hidden core risk を減らすことが目的なら、優先度 1、つまり `parse_file_in_repo` に対する direct graph-visible tests から始める。こちらの方がより foundational だが、Rust parser boundary は少し扱いが難しいため、test-first pass が必要である。

## 12. 検証メモ

使用した主なコマンド:

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

重要な truncation state:

- architecture overview: `truncated=true`; communities は 368 件中 1 件、coupling は 5 件中 1 件を保持。
- knowledge gaps: `truncated=true`; raw gap count は 2,609、各カテゴリの returned examples は 10 件。
- refactor suggestions: 合計 766 件、最初の 20 件を表示。内訳は split 134、document 49、remove 583。

上記の結論は graph output を ranked evidence として扱っており、refactor の自動承認としては扱っていない。
