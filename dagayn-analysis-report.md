<!-- derived-from ./skills/writing-markdown-document/SKILL.md#stage-0--prerequisites -->
<!-- derived-from ./skills/writing-markdown-document/SKILL.md#dagayn-markdown-reference -->
<!-- derived-from ./skills/writing-markdown-document/SKILL.md#stage-1--outline--sort-sections -->
<!-- derived-from ./skills/writing-markdown-document/SKILL.md#stage-2--draft--verify-edges -->
<!-- derived-from ./skills/writing-markdown-document/SKILL.md#stage-3--polish -->
<!-- derived-from ./skills/writing-markdown-document/SKILL.md#stage-4--summary--conclusion -->
<!-- derived-from ./skills/review-delta/SKILL.md#review-delta -->
<!-- derived-from ./skills/review-delta/SKILL.md#steps -->
<!-- derived-from ./skills/review-delta/SKILL.md#advantages-over-full-repo-review -->
<!-- derived-from ./skills/review-pr/SKILL.md#review-pr -->
<!-- derived-from ./skills/review-pr/SKILL.md#steps -->
<!-- derived-from ./skills/review-pr/SKILL.md#tips -->
<!-- derived-from ./skills/explore-codebase/SKILL.md#explore-codebase -->
<!-- derived-from ./skills/explore-codebase/SKILL.md#steps -->
<!-- derived-from ./skills/explore-codebase/SKILL.md#tips -->
<!-- derived-from ./skills/explore-codebase/SKILL.md#token-efficiency-rules -->

# dagayn コード分析レポート

> 生成日: 2026-05-03 | グラフ: 5,693ノード / 41,172エッジ / 377ファイル / 30言語

---

## 1. グラフ概要

`list_graph_tool` で取得したグラフ全体の統計。`build_or_update_graph_tool` によって構築されたグラフには以下のノード・エッジが存在する。

| 項目 | 値 |
|------|-----|
| ノード数 | 5,693 |
| エッジ数 | 41,172 |
| ファイル数 | 377 |
| 対応言語 | 30言語 |
| テスト数 | 1,526 |
| 最終更新 | 2026-05-03T13:28:00 |

### エッジ内訳

| エッジ種別 | 件数 |
|------------|------|
| CALLS | 27,200 |
| TESTED_BY | 5,432 |
| CONTAINS | 5,322 |
| IMPORTS_FROM | 1,723 |
| CROSS_ARTIFACT | 1,259 |
| DEPENDS_ON | 53 |
| INHERITS | 37 |
| IMPLEMENTS | 4 |
| REFERENCES | 142 |

---

## 2. アーキテクチャ構造

`list_communities_tool` と `get_architecture_overview` で取得したアーキテクチャ構造。352のコミュニティが検出され、そのうちサイズ5以上は203。`get_impact_radius` でハイカップリングも検出される。

- **コミュニティ数**: 352 (サイズ5以上: 203)
- **ハイカップリング警告**: 2件

### トップコミュニティ

| 名前 | サイズ | 協調性 | 主要言語 | 内容 |
|------|--------|--------|----------|------|
| tests-files | 251 | 0.525 | Python | `embeddings`, `incremental`, tests, パフォーマンス/マイグレーションドキュメント |

### ハイカップリング警告

| 出側コミュニティ | 入側コミュニティ | エッジ数 | 種別 |
|-----------------|-----------------|----------|------|
| tests-communities | tests-flows | 17 | CROSS_ARTIFACT: 6, CALLS: 9, CONTAINS: 2 |
| tests-files | tests-flows | 14 | — |

---

## 3. ハブノード（接続数上位15）

`get_hub_nodes_tool` で取得した、高い次数（in + out）を持つノード。変更時の影響範囲が大きい。Fileノードは除外。`review-pr` スキルの `steps` で言及されるように、ハブノードへの変更はアーキテクチャ全体に波及する。

| # | 名前 | 修飾名 | 種別 | 入次数 | 出次数 | 総次数 | ファイル |
|---|------|--------|------|--------|--------|--------|----------|
| 1 | `_add_func` | tests/test_flows.py::TestFlows._add_func | Function | 78 | 80 | 158 | tests/test_flows.py |
| 2 | `parse_rescript` | crates/dagayn-parser/src/rescript_legacy.rs::parse_rescript | Function | 1 | 155 | 156 | crates/dagayn-parser/src/rescript_legacy.rs |
| 3 | `stores_flows_and_reads_flow_inputs` | crates/dagayn-graph/src/tests.rs::stores_flows_and_reads_flow_inputs | Function | 1 | 149 | 150 | crates/dagayn-graph/src/tests.rs |
| 4 | `handle` | dagayn/cli/commands/build.py::handle | Function | 1 | 139 | 140 | dagayn/cli/commands/build.py |
| 5 | `incremental_update` | dagayn/incremental.py::incremental_update | Function | 13 | 114 | 127 | dagayn/incremental.py |
| 6 | `find_dead_code` | dagayn/refactor/dead_code.py::find_dead_code | Function | 3 | 115 | 118 | dagayn/refactor/dead_code.py |
| 7 | `test_full_pipeline` | tests/test_integration_v2.py::TestV2Integration.test_full_pipeline | Test | 56 | 55 | 111 | tests/test_integration_v2.py |
| 8 | `parse` | dagayn/parser/languages/rescript.py::parse | Function | 1 | 104 | 105 | dagayn/parser/languages/rescript.py |
| 9 | `parse_file_in_repo` | crates/dagayn-parser/src/core.rs::RustOwnedParser.parse_file_in_repo | Function | 3 | 100 | 103 | crates/dagayn-parser/src/core.rs |
| 10 | `GraphStore` | crates/dagayn-graph/src/lib.rs::GraphStore | Class | 2 | 100 | 102 | crates/dagayn-graph/src/lib.rs |
| 11 | `buildGraph` | dagayn-vscode/src/webview/graph.ts::buildGraph | Function | 6 | 90 | 96 | dagayn-vscode/src/webview/graph.ts |
| 12 | `compute_sap_metrics` | dagayn/sap.py::compute_sap_metrics | Function | 30 | 66 | 96 | dagayn/sap.py |
| 13 | `computes_summary_tables` | crates/dagayn-graph/src/tests.rs::computes_summary_tables | Function | 1 | 94 | 95 | crates/dagayn-graph/src/tests.rs |
| 14 | `analyze_changes_json` | crates/dagayn-graph/src/lib.rs::GraphStore.analyze_changes_json | Function | 1 | 91 | 92 | crates/dagayn-graph/src/lib.rs |
| 15 | `OpenAIEmbeddingProvider` | dagayn/embeddings.py::OpenAIEmbeddingProvider | Class | 43 | 49 | 92 | dagayn/embeddings.py |

---

## 4. ブリッジノード（betweenness centrality 上位15）

`get_bridge_nodes_tool` で取得した、多くのノードペア間の最短パス上に位置するノード。切断されると複数のコード領域の接続性が失われる。`review-delta` スキルの `advantages-over-full-repo-review` で言及されるように、ブリッジノードの特定は変更影響範囲の理解に不可欠。

| # | 名前 | 修飾名 | 種別 | Betweenness | ファイル |
|---|------|--------|------|-------------|----------|
| 1 | `test_full_pipeline` | tests/test_integration_v2.py::TestV2Integration.test_full_pipeline | Test | 0.011432 | tests/test_integration_v2.py |
| 2 | `test_regression_guard_does_not_depend_on_fastmcp_internals` | tests/test_main.py::TestLongRunningToolsAreAsync.test_regression_guard_does_not_depend_on_fastmcp_internals | Test | 0.011022 | tests/test_main.py |
| 3 | `it:isValid() returns false after close()@L483` | dagayn-vscode/test/sqlite.test.ts::it:isValid() returns false after close()@L483 | Test | 0.005768 | dagayn-vscode/test/sqlite.test.ts |
| 4 | `_bridges` | tests/test_parser.py::TestBridgeExpansion._bridges | Function | 0.004045 | tests/test_parser.py |
| 5 | `full_build` | dagayn/incremental.py::full_build | Function | 0.003753 | dagayn/incremental.py |
| 6 | `test_status_from_state_reports_alive` | tests/test_daemon.py::TestWatchDaemon.test_status_from_state_reports_alive | Test | 0.003380 | tests/test_daemon.py |
| 7 | `incremental_update` | dagayn/incremental.py::incremental_update | Function | 0.003320 | dagayn/incremental.py |
| 8 | `WatchDaemon` | dagayn/daemon.py::WatchDaemon | Class | 0.003284 | dagayn/daemon.py |
| 9 | `EmbeddingStore` | dagayn/embeddings.py::EmbeddingStore | Class | 0.002760 | dagayn/embeddings.py |
| 10 | `run_post_processing` | dagayn/postprocessing.py::run_post_processing | Function | 0.002478 | dagayn/postprocessing.py |
| 11 | `test_detected_cohesions_match_direct_computation` | tests/test_communities.py::TestCommunities.test_detected_cohesions_match_direct_computation | Test | 0.002407 | tests/test_communities.py |
| 12 | `describe:SqliteReader@L243` | dagayn-vscode/test/sqlite.test.ts::describe:SqliteReader@L243 | Test | 0.002365 | dagayn-vscode/test/sqlite.test.ts |
| 13 | `compute_sap_metrics` | dagayn/sap.py::compute_sap_metrics | Function | 0.002306 | dagayn/sap.py |
| 14 | `test_rust_backend_routes_databricks_py_exports` | tests/test_rust_backend_parity.py::test_rust_backend_routes_databricks_py_exports | Test | 0.002293 | tests/test_rust_backend_parity.py |
| 15 | `get_provider` | dagayn/embeddings.py::get_provider | Function | 0.002184 | dagayn/embeddings.py |

---

## 5. 知識ギャップ（349件）

`get_knowledge_gaps_tool` で取得した、構造的な弱点を4カテゴリに分類。`explore-codebase` スキルの `steps` で言及されるように、孤立ノードや未テストハブの特定はコードベースの健全性評価に不可欠。

### 5.1 孤立ノード（50件）

接続数1のノード。CHANGELOG.md, CLAUDE.md, CODE_OF_CONDUCT.md, CONTRIBUTING.md, README.hi-IN.md, README.ja-JP.md の見出しセクションが大半を占める。

代表的な孤立ノード:
- `CHANGELOG.md::features`, `::performance-1`, `::fixes`, `::tests`, `::010--initial-dagayn-fork`, `::unreleased`
- `CLAUDE.md::suggested-local-workflow`, `::verification-commands`, `::good-prompts-to-start-with`, `::fork-specific-emphasis`, `::golden-rule`, `::build--compile-80-90-savings`, `::test-60-99-savings`, `::git-59-80-savings`, `::github-26-87-savings`, `::javascripttypescript-tooling-70-90-savings`, `::files--search-60-75-savings`, `::analysis--debug-70-90-savings`, `::infrastructure-85-savings`, `::network-65-70-savings`, `::meta-commands`, `::token-savings-overview`
- `CODE_OF_CONDUCT.md::expected-behavior`, `::unacceptable-behavior`, `::maintainer-responsibilities`, `::contribution-policy`, `::scope`
- `CONTRIBUTING.md::issues`, `::pull-requests`, `::development-setup-maintainers`

### 5.2 薄コミュニティ（151件）

メンバー数3未満のコミュニティ。`src-markdown` (サイズ0) が代表例。

### 5.3 テスト未カバーハブ（20件）

次数が高くながらテストからの参照がないノード。`review-pr` スキルの `tips` で言及されるように、ハブノードのテストカバレッジは必須のレビュー項目。

| 名前 | 修飾名 | 種別 | 次数 | ファイル |
|------|--------|------|------|----------|
| `parse_rescript` | crates/dagayn-parser/src/rescript_legacy.rs::parse_rescript | Function | 156 | crates/dagayn-parser/src/rescript_legacy.rs |

**重大な懸念:** Rustパーサーの核心関数 `parse_rescript` (degree 156) がテスト未カバー。

### 5.4 単一ファイルコミュニティ（128件）

1ファイルのみで構成されるコミュニティ。`CLAUDE.md` (サイズ5), `skills/reading-markdown-document`, `skills/writing-markdown-document`, `skills/debug-issue`, `skills/review-delta`, `docs/audits`, `dagayn-vscode` など。

---

## 6. 不自然な接続（Surprising Connections）上位15

`get_surprising_connections_tool` で取得した、コミュニティ間・言語間の予期せぬ結合。スコアが高いほどアーキテクチャ上の懸念度が高い。`review-delta` スキルの `steps` で言及されるように、不自然な接続はアーキテクチャの腐敗を示す指標となる。

| # | 出側 | 入側 | エッジ種別 | スコア | 理由 |
|---|------|------|-----------|--------|------|
| 1 | `CHANGELOG.md::performance` | `dagayn/refactor/dead_code.py::find_dead_code` | CROSS_ARTIFACT | 0.557 | cross-community, rare-community-pair, cross-language, degree-imbalance |
| 2 | `docs/CROSS-ARTIFACT-EDGES-WIP.md::bridge-family-2--markdown--code-symbol-references-` | `dagayn/tools/query.py::query_graph` | CROSS_ARTIFACT | 0.555 | cross-community, rare-community-pair, cross-language, degree-imbalance |
| 3 | `docs/PERFORMANCE-IMPROVEMENTS-WIP.md::embeddingstore-re-instantiated-per-search` | `dagayn/search.py::hybrid_search` | CROSS_ARTIFACT | 0.554 | cross-community, rare-community-pair, cross-language, degree-imbalance |
| 4 | `docs/PERFORMANCE-IMPROVEMENTS-WIP.md::42-codeparser-singleton-per-worker-quick-win--shipped` | `dagayn/parser/core.py::CodeParser` | CROSS_ARTIFACT | 0.553 | cross-community, rare-community-pair, cross-language, degree-imbalance |
| 5 | `docs/COMMANDS.md::mcp-tools` | `dagayn/tools/build.py::build_or_update_graph` | CROSS_ARTIFACT | 0.552 | cross-community, rare-community-pair, cross-language, degree-imbalance |
| 6 | `docs/audits/mcp-tool-output-review.md::36-get_review_context_tool-minimal--key_entities-に絶対パス混入` | `dagayn/tools/_common.py::_get_store` | CROSS_ARTIFACT | 0.552 | cross-community, rare-community-pair, cross-language, degree-imbalance |
| 7 | `CHANGELOG.md::performance` | `dagayn/graph/core.py::GraphStore.get_local_subgraph` | CROSS_ARTIFACT | 0.551 | cross-community, rare-community-pair, cross-language, degree-imbalance |
| 8 | `docs/audits/mcp-tool-output-review.md::phase-3--mediumlow-改修-commit-tbd` | `dagayn/analysis.py::find_surprising_connections` | CROSS_ARTIFACT | 0.551 | cross-community, rare-community-pair, cross-language, degree-imbalance |
| 9 | `dagayn/embeddings.py::LocalEmbeddingProvider` | `dagayn/embeddings.py::LocalEmbeddingProvider.embed_query` | CONTAINS | 0.526 | cross-community, rare-community-pair, peripheral-to-hub, degree-imbalance |
| 10 | `docs/COMMANDS.md::mcp-tools` | `dagayn/wiki.py::generate_wiki` | CROSS_ARTIFACT | 0.511 | cross-community, rare-community-pair, cross-language, degree-imbalance |
| 11 | `skills/explore-codebase/SKILL.md::steps` | `dagayn/communities.py::get_architecture_overview` | CROSS_ARTIFACT | 0.511 | cross-community, rare-community-pair, cross-language, degree-imbalance |
| 12 | `AGENTS.md::when-to-use-graph-tools-first` | `dagayn/communities.py::get_architecture_overview` | CROSS_ARTIFACT | 0.511 | cross-community, rare-community-pair, cross-language, degree-imbalance |
| 13 | `AGENTS.md::key-tools` | `dagayn/communities.py::get_architecture_overview` | CROSS_ARTIFACT | 0.511 | cross-community, rare-community-pair, cross-language, degree-imbalance |
| 14 | `GEMINI.md::when-to-use-graph-tools-first` | `dagayn/communities.py::get_architecture_overview` | CROSS_ARTIFACT | 0.511 | cross-community, rare-community-pair, cross-language, degree-imbalance |
| 15 | `GEMINI.md::key-tools` | `dagayn/communities.py::get_architecture_overview` | CROSS_ARTIFACT | 0.511 | cross-community, rare-community-pair, cross-language, degree-imbalance |

---

## 7. ADP違反（循環依存）41件（パッケージレベル）

`detect_adp_violations_tool` で取得した、Acyclic Dependencies Principleに違反する循環依存。重大度（severity）が高い順に上位20件。`review-delta` スキルの `steps` で言及されるように、循環依存はモジュール境界の再設計が必要であることを示す。

| # | 循環パス | 長さ | 重み | 重大度 |
|---|----------|------|------|--------|
| 1 | `dagayn/tools` → `dagayn` → `dagayn/cli` → `dagayn/cli/commands` → `dagayn/eval` → `dagayn/eval/benchmarks` | 6 | 64 | 384 |
| 2 | `dagayn/tools` → `dagayn` → `dagayn/cli` → `dagayn/cli/commands` → `dagayn/eval` | 5 | 71 | 355 |
| 3 | `dagayn/tools` → `dagayn` → `dagayn/cli` → `dagayn/cli/commands` | 4 | 61 | 244 |
| 4 | `dagayn/tools` → `dagayn/refactor` → `dagayn/graph` → `dagayn/parser` → `dagayn` → `dagayn/cli` → `dagayn/cli/commands` → `dagayn/eval` | 8 | 23 | 184 |
| 5 | `dagayn/tools` → `dagayn/graph` → `dagayn/parser` → `dagayn` → `dagayn/cli` → `dagayn/cli/commands` → `dagayn/eval` | 7 | 24 | 168 |
| 6 | `dagayn/tools` → `dagayn/refactor` → `dagayn/graph` → `dagayn` → `dagayn/cli` → `dagayn/cli/commands` → `dagayn/eval` | 7 | 22 | 154 |
| 7 | `dagayn/tools` → `dagayn/graph` → `dagayn/parser` → `dagayn` → `dagayn/cli` → `dagayn/cli/commands` → `dagayn/eval` → `dagayn/eval/benchmarks` | 8 | 16 | 144 |
| 8 | `dagayn/tools` → `dagayn/graph` → `dagayn` → `dagayn/cli` → `dagayn/cli/commands` → `dagayn/eval` | 6 | 23 | 138 |
| 9 | `dagayn/tools` → `dagayn/graph` → `dagayn/parser` → `dagayn` → `dagayn/cli` → `dagayn/cli/commands` → `dagayn/eval` → `dagayn/eval/benchmarks` | 8 | 17 | 136 |
| 10 | `dagayn/tools` → `dagayn/refactor` → `dagayn/graph` → `dagayn` → `dagayn/cli` → `dagayn/cli/commands` → `dagayn/eval` → `dagayn/eval/benchmarks` | 8 | 15 | 120 |
| 11 | `dagayn/tools` → `dagayn` | 2 | 57 | 114 |
| 12 | `dagayn/tools` → `dagayn/graph` → `dagayn` → `dagayn/cli` → `dagayn/cli/commands` → `dagayn/eval` → `dagayn/eval/benchmarks` | 7 | 16 | 112 |
| 13 | `dagayn/tools` → `dagayn/refactor` → `dagayn` → `dagayn/cli` → `dagayn/cli/commands` → `dagayn/eval` | 6 | 17 | 102 |
| 14 | `dagayn/tools` → `dagayn/refactor` → `dagayn/graph` → `dagayn/parser` → `dagayn` → `dagayn/cli` → `dagayn/cli/commands` | 7 | 13 | 91 |
| 15 | `dagayn/cli/commands` → `dagayn` → `dagayn/cli` | 3 | 29 | 87 |
| 16 | `dagayn/tools` → `dagayn/graph` → `dagayn/parser` → `dagayn` → `dagayn/cli` → `dagayn/cli/commands` | 6 | 14 | 84 |
| 17 | `dagayn/tools` → `dagayn/refactor` → `dagayn/graph` → `dagayn` → `dagayn/cli` → `dagayn/cli/commands` | 6 | 12 | 72 |
| 18 | `dagayn/tools` → `dagayn/refactor` → `dagayn` → `dagayn/cli` → `dagayn/cli/commands` → `dagayn/eval` → `dagayn/eval/benchmarks` | 7 | 10 | 70 |
| 19 | `dagayn/tools` → `dagayn/graph` → `dagayn` → `dagayn/cli` → `dagayn/cli/commands` | 5 | 13 | 65 |
| 20 | `dagayn/eval` → `dagayn/eval/benchmarks` → `dagayn` → `dagayn/cli` → `dagayn/cli/commands` | 5 | 13 | 65 |

---

## 8. SAP違反（安定抽象原理）

`compute_sap_metrics_tool` で取得した、60パッケージがZone of Pain（具体かつ安定）またはZone of Uselessness（抽象かつ不安定）に存在。距離1.0（最大リスク）のパッケージ。`review-pr` スキルの `tips` で言及されるように、SAP違反はアーキテクチャの腐敗を示す。

| パッケージ | 抽象度 | 不安定度 | 距離 | メンバー数 | 状態 |
|------------|--------|----------|------|-----------|------|
| hooks | 0.0 | 0.0 | 1.0 | 1 | 孤立 |
| skills/reading-markdown-document | 0.0 | 0.0 | 1.0 | 7 | 孤立 |
| tests/fixtures/parity/notebook | 0.0 | 0.0 | 1.0 | 3 | 孤立 |
| crates/dagayn-core/src | 0.0 | 0.0 | 1.0 | 1 | 孤立 |
| crates/dagayn-py/src | 0.0 | 0.0 | 1.0 | 122 | 孤立 |
| tests/fixtures/parity/terraform_only | 0.0 | 0.0 | 1.0 | 7 | 孤立 |
| dagayn/parser/_base | 0.0 | 0.0 | 1.0 | 10 | 依存先あり（36+11+1） |
| dagayn-vscode/media/walkthrough | 0.0 | 0.0 | 1.0 | 6 | 孤立 |
| dagayn/visualization/templates | 0.0 | 0.0 | 1.0 | 1 | 孤立 |
| tests/fixtures/parity/python_only | 0.0 | 0.0 | 1.0 | 10 | 孤立 |
| dagayn-vscode | 0.0 | 0.0 | 1.0 | 24 | 孤立 |
| skills/writing-markdown-document | 0.0 | 0.0 | 1.0 | 9 | 孤立 |
| skills/debug-issue | 0.0 | 0.0 | 1.0 | 5 | 孤立 |
| skills/review-delta | 0.0 | 0.0 | 1.0 | 4 | 孤立 |
| tests/fixtures/src/lib | 0.0 | 0.0 | 1.0 | 2 | 依存先あり |
| tests/fixtures/java_multipackage/domain | 0.0 | 0.0 | 1.0 | 6 | 孤立 |
| docs/audits | 0.0 | 0.0 | 1.0 | 43 | 孤立 |
| tests/fixtures/parity/mixed | 0.0 | 0.0 | 1.0 | 6 | 孤立 |
| crates/dagayn-py | 0.0 | 0.0 | 1.0 | 2 | 孤立 |
| dagayn-vscode/src/webview | 1.0 | 1.0 | 1.0 | 23 | 抽象かつ不安定 |

---

## 9. 大規模ファイル・クラス（50行超）

`find_large_functions_tool` で取得した、50行以上のファイル・クラスを行数順に列出。`review-delta` スキルの `steps` で言及されるように、大規模な関数・クラスは分解の候補となる。

| # | 名前 | 種別 | 行数 | ファイル |
|---|------|------|------|----------|
| 1 | `crates/dagayn-graph/src/lib.rs` | File | 3,630 | Rust |
| 2 | `GraphStore` | Class | 2,724 | crates/dagayn-graph/src/lib.rs (Rust) |
| 3 | `crates/dagayn-parser/src/core_tests.rs` | File | 2,649 | Rust |
| 4 | `tests/test_parser.py` | File | 2,175 | Python |
| 5 | `tests/test_multilang.py` | File | 2,096 | Python |
| 6 | `dagayn/incremental.py` | File | 1,919 | Python |
| 7 | `dagayn/graph/core.py` | File | 1,911 | Python |
| 8 | `GraphStore` | Class | 1,823 | dagayn/graph/core.py (Python) |
| 9 | `tests/test_tools.py` | File | 1,735 | Python |
| 10 | `tests/test_skills.py` | File | 1,552 | Python |
| 11 | `crates/dagayn-parser/src/python.rs` | File | 1,377 | Rust |
| 12 | `crates/dagayn-py/src/lib.rs` | File | 1,331 | Rust |
| 13 | `dagayn/parser/core.py` | File | 1,266 | Python |
| 14 | `dagayn/main.py` | File | 1,252 | Python |
| 15 | `CodeParser` | Class | 1,228 | dagayn/parser/core.py (Python) |
| 16 | `tests/test_refactor.py` | File | 1,225 | Python |
| 17 | `dagayn/skills.py` | File | 1,174 | Python |
| 18 | `tests/test_embeddings.py` | File | 1,165 | Python |
| 19 | `TestCodeParser` | Class | 990 | tests/test_parser.py (Python) |
| 20 | `dagayn/embeddings.py` | File | 982 | Python |

---

## 10. 主要実行フロー（Criticality 順）

`list_flows_tool` で取得した、クリティカル度順の実行フロー。`explore-codebase` スキルの `steps` で言及されるように、実行フローの理解はコードベースの振る舞いを把握するために不可欠。

| # | フロー名 | エントリ次数 | 深さ | ノード数 | ファイル数 | Criticality |
|---|----------|-------------|------|----------|-----------|-------------|
| 1 | `activate` | 18 | 3 | 18 | 9 | 0.68 |
| 2 | `getImpactRadius` | 9 | 2 | 9 | 1 | 0.62 |
| 3 | `getStats` | 3 | 1 | 3 | 1 | 0.61 |
| 4 | `getNodesBySize` | 3 | 1 | 3 | 1 | 0.61 |
| 5 | `benchmark_review_workflow` | 2 | 1 | 2 | 1 | 0.61 |
| 6 | `benchmark_architecture_workflow` | 2 | 1 | 2 | 1 | 0.61 |
| 7 | `benchmark_debug_workflow` | 2 | 1 | 2 | 1 | 0.61 |
| 8 | `benchmark_onboard_workflow` | 2 | 1 | 2 | 1 | 0.61 |
| 9 | `benchmark_pre_merge_workflow` | 2 | 1 | 2 | 1 | 0.61 |
| 10 | `query_graph` | 2 | 1 | 2 | 1 | 0.61 |
| 11 | `getAllFiles` | 2 | 1 | 2 | 1 | 0.53 |
| 12 | `searchNodes` | 3 | 1 | 3 | 1 | 0.53 |
| 13 | `main` | 6 | 3 | 6 | 2 | 0.497 |
| 14 | `getNodeAtCursor` | 3 | 1 | 3 | 1 | 0.49 |
| 15 | `compute_missing_signatures` | 2 | 1 | 2 | 1 | 0.485 |
| 16 | `computes_missing_signatures` | 2 | 1 | 2 | 1 | 0.485 |
| 17 | `embed_query` (Local) | 2 | 1 | 2 | 1 | 0.485 |
| 18 | `embed_query` (OpenAI) | 2 | 1 | 2 | 1 | 0.485 |
| 19 | `embed_query` (Google) | 2 | 1 | 2 | 1 | 0.485 |
| 20 | `_scenario_traverse_graph` | 2 | 1 | 2 | 1 | 0.485 |

---

## 11. 推奨アクション

### 優先度高

1. **`parse_rescript` のテスト追加** — degree 156 のハブでありながらテスト未カバー。Rustパーサーの正しさを保証するため必須。`get_knowledge_gaps_tool` で検出された未テストハブの最優先対応項目。
2. **ADP違反の解消** — `dagayn/tools` ↔ `dagayn/cli/commands` 間の循環依存（severity 384）が最严重。`detect_adp_violations_tool` で検出された41件の循環依存のうち、`dagayn/tools` を起点とするものが大半を占める。モジュール境界の再設計が必要。
3. **大規模クラスの分割** — Rust `GraphStore` (2,724行), Python `GraphStore` (1,823行), `CodeParser` (1,228行) の分解。`find_large_functions_tool` で検出された大規模クラスのうち、`GraphStore` はRust版とPython版の両方が存在し、`dagayn/graph/_protocol.py` の `get_impact_radius` とも関連する。

### 優先度中

4. **ドキュメントと実装の結合分離** — CHANGELOG.md, COMMANDS.md などのドキュメントが実装コードと直接結合している15件の CROSS_ARTIFACT エッジを再検討。`get_surprising_connections_tool` で検出されたハイスコア接続の大半を占める。
5. **孤立ノードの整理** — 50件の孤立ノード（主にドキュメント見出し）のグラフからの除外または適切な結合。`get_knowledge_gaps_tool` で検出された孤立ノードの整理は、グラフの品質向上に直結する。
6. **単一ファイルコミュニティの統合** — 128件の単一ファイルコミュニティを既存コミュニティに統合。`list_communities_tool` で検出されたコミュニティ構造の再編成。

### 優先度低

7. **SAP違反パッケージの再編** — 60パッケージがZone of Pain/Uselessness に存在。`compute_sap_metrics_tool` で検出された `dagayn-vscode/src/webview` (距離1.0) の抽象化見直し。
8. **ハイカップリングコミュニティの分離** — `tests-communities` ↔ `tests-flows` (17エッジ), `tests-files` ↔ `tests-flows` (14エッジ) の結合度低減。`get_architecture_overview` で検出されたハイカップリング警告の対応。

---

## 12. まとめ

<!-- derived-from #グラフ概要 -->
<!-- derived-from #アーキテクチャ構造 -->
<!-- derived-from #ハブノード接続数上位15 -->
<!-- derived-from #ブリッジノード-betweenness-centrality-上位15 -->
<!-- derived-from #知識ギャップ349件 -->
<!-- derived-from #不自然な接続Surprising-Connections-上位15 -->
<!-- derived-from #ADP違反循環依存41件パッケージレベル -->
<!-- derived-from #SAP違反安定抽象原理 -->
<!-- derived-from #大規模ファイル-クラス50行超 -->
<!-- derived-from #主要実行フローCriticality-順 -->
<!-- derived-from #推奨アクション -->

本レポートは、dagayn 自体のコードベースを `build_or_update_graph_tool` で構築した知識グラフを基に、以下のツールで分析した結果をまとめたもの。

| ツール | 目的 |
|--------|------|
| `build_or_update_graph_tool` | 知識グラフの構築・更新 |
| `list_graph_stats_tool` | グラフ統計の取得 |
| `list_communities_tool` | コミュニティ構造の分析 |
| `get_architecture_overview` | アーキテクチャ概要の取得 |
| `get_hub_nodes_tool` | ハブノード（高接続数）の特定 |
| `get_bridge_nodes_tool` | ブリッジノード（ chokepoint ）の特定 |
| `get_knowledge_gaps_tool` | 知識ギャップ（孤立・未テスト等）の検出 |
| `get_surprising_connections_tool` | 不自然な接続の検出 |
| `detect_adp_violations_tool` | ADP違反（循環依存）の検出 |
| `compute_sap_metrics_tool` | SAP違反（安定抽象原理）の検出 |
| `find_large_functions_tool` | 大規模関数・クラスの検出 |
| `list_flows_tool` | 実行フローの分析 |
| `detect_changes` | 変更影響範囲の分析 |
| `get_impact_radius_tool` | 変更の波及範囲の特定 |
| `query_graph_tool` | 呼び出し関係・インポート関係の追跡 |

### 主要発見

- **`parse_rescript` (degree 156) がテスト未カバー** — Rustパーサー核心部の懸念。`get_knowledge_gaps_tool` で検出された未テストハブの最優先対応。
- **ADP違反41件** — `dagayn/tools` ↔ `dagayn/cli/commands` 間の循環依存（severity 384）が最严重。`detect_adp_violations_tool` で検出。
- **349件の知識ギャップ** — 孤立ノード50, 薄コミュニティ151, 未テストハブ20, 単一ファイルコミュニティ128。
- **60パッケージがSAP違反** — `compute_sap_metrics_tool` でZone of Pain/Uselessness に存在を確認。
- **3,630行のRust GraphStore** — `find_large_functions_tool` で検出された最大ファイル。

<!-- supersedes ./dagayn-analysis-report.md -->
