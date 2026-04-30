# MCPツール出力 監査レポート

**監査日**: 2026-04-30  
**対象グラフ**: dagayn本体 (4,246 nodes / 25,797 edges / 328 files)  
**コミット**: d8f4672 (main)

---

## エグゼクティブサマリー

dagaynの35 MCPツールを実際に実行して出力を観測した。**4ツールがMCPフレームワークの出力上限を超過して使用不能**になっており、さらに数ツールが大規模リポジトリで同様の問題を引き起こすリスクがある。envelope構造の不統一（`status`フィールド欠落、`_hints`vs`next_tool_suggestions`混在）も全体的に見られる。

| 重大度 | 件数 | 内容 |
|--------|------|------|
| **Critical** | 4 | MCP上限超過で実際に使用不能 |
| **High** | 6 | 大規模リポで上限超過の現実的リスク |
| **Medium** | 9 | 出力過多・envelope不統一・設計上の問題 |
| **Low** | 7 | 小規模改善点 |

---

## 1. Critical: MCP出力上限超過（即時使用不能）

これらのツールはdagayn本体（中規模Python/TSプロジェクト）に対してさえ出力上限に達し、MCPフレームワークがエラーを返す。実際の呼び出しは失敗する。

### 1.1 `get_architecture_overview_tool` — 268,150文字

**症状**: stdandard呼び出しで `Error: result (268,150 characters) exceeds maximum allowed tokens`

**根本原因**:
- 280コミュニティのすべてのメタデータ（`members[]`含む）と全クロスコミュニティエッジを返す
- `detail_level`/`top_n` パラメータを実装コードには追加したが、MCPスキーマ（`@mcp.tool()`デコレータのシグネチャ）に公開していないため、MCP クライアントは `detail_level="minimal"` を渡す手段がない

**実測**: 268,150文字 ≈ 67,000トークン

**確認場所**:
- `dagayn/main.py:578-603` — MCPラッパー（`detail_level`, `top_n`引数あり）
- ToolSearch返却スキーマ: `{"repo_root"}` のみ（`detail_level`/`top_n`が欠落）

**改修案**: `@mcp.tool()` シグネチャに `detail_level: str = "standard"` と `top_n: int = 20` を追加し、fastmcpがパラメータをスキーマに含めるようにする。minimal/standard呼び出しは機能するはず。

---

### 1.2 `get_impact_radius_tool` (standard) — 297,975文字

**症状**: `Error: result (297,975 characters) exceeds maximum allowed tokens`

**根本原因**:
- `_common.py` 1ファイルの変更だけで452個の被影響ノード（各12フィールド）＋対応するエッジ（各8フィールド）をすべて返す
- `max_results=500` キャップはノード数を制限するが、452ノード×フルオブジェクトでも上限超過

**実測**: 297,975文字 ≈ 74,500トークン

**minimal動作**: `detail_level="minimal"` は正常（≈100トークン）で設計通り  
**改修案**: standard モードに `max_nodes: int = 50` 上限を追加し `truncated=True` フラグを立てる。または standard のデフォルトを minimal に近い投影に変更。

---

### 1.3 `refactor_tool(mode="dead_code")` — 141,701バイト

**症状**: `Error: result (139,689 characters) exceeds maximum allowed tokens`

**根本原因**: デッドコード候補の全ノードをフルオブジェクト（12フィールド）で返す。`limit`/`top_n`パラメータなし。

**実測**: 141,701バイト ≈ 35,000トークン

**改修案**: `limit: int = 50` パラメータを追加し、`total` と `truncated` フラグを返す。

---

### 1.4 `refactor_tool(mode="suggest")` — 194,282バイト

**症状**: `Error: result (192,270 characters) exceeds maximum allowed tokens`

**根本原因**: コミュニティドリブンのリファクタ提案をすべての候補について返す。`limit`/`top_n`なし。

**実測**: 194,282バイト ≈ 48,500トークン

**改修案**: dead_code同様 `limit: int = 50` を追加。

---

## 2. High: 大規模リポで上限超過リスク

dagayn本体でも大きく、本番規模リポでは確実に上限超過する。

### 2.1 `get_knowledge_gaps_tool` — ≈25,000文字（無制限）

**観測**: `total_gaps: 297` を4種のリストで全返却
- `isolated_nodes`: 50件
- `thin_communities`: 109件（各 `community_id, name, size`）
- `untested_hotspots`: 20件（各6フィールド）
- `single_file_communities`: 118件（各4フィールド）

`top_n` / `limit` パラメータなし。`status`フィールドなし。  
**改修案**: 各リスト個別に `top_n: int = 20` を追加、または全体`limit`を設ける。

---

### 2.2 `detect_sap_violations_tool` — ≈20,000文字（全スコープ返却）

**観測**: 33件の violations を `compute_sap_metrics_tool` と同一のフル構造体で返す。`top_n` なし。  
`count: 33` は中規模リポの数値。本番規模では数百件になりうる。  
`status`フィールドなし。  
**改修案**: `top_n: int = 30` を追加（`compute_sap_metrics_tool` と同じデフォルト）。

---

### 2.3 `get_community_tool(include_members=True)` — 68.1KB

**観測**: 229ノードのフルオブジェクト（12フィールド）を返す。  
`include_members=False` でも `members[]` として229件の文字列リストを返す（≈8KB）。つまり `include_members` が制御するのは「文字列リスト」か「フルオブジェクト」かの差のみで、メンバー一覧自体は常に返る。

**改修案**: `include_members=False` 時は `members`フィールドを省略（`member_count`のみ）。フルオブジェクトが必要な場合のみ `include_members=True` を使用。

---

### 2.4 `list_communities_tool` (minimal) — ≈14,000文字（280件）

**観測**: minimal モードでも 280コミュニティ × 3フィールドを全列挙。  
`sort_by` と `min_size` フィルタはあるが全コミュニティを返す上限なし。  
**改修案**: `limit: int = 50` パラメータを追加。

---

### 2.5 `get_flow_tool(include_source=True)` — 大（未計測）

**観測**: `activate` フロー（18ノード）の全ソースコードを含む。`registerNavigationCommands` だけで330行。本ケースの出力は数万文字規模と推定。  
`include_source=True` がデフォルト`False`なのは適切だが、大きなフロー（`parse_file`: 28ノード）では上限超過のリスクがある。  
**改修案**: `include_source=True` 時に `max_lines_per_step: int = 50` を追加。

---

### 2.6 `detect_changes_tool` (standard) — スケール依存

**観測**: 27ファイル変更で`changed_functions[]`33件+`test_gaps[]`24件+`review_priorities[]`10件をフルオブジェクトで返す（≈8,000文字）。100ファイル変更のPRでは≈30,000文字になる。  
**改修案**: `changed_functions`, `test_gaps`, `review_priorities` それぞれに上限（例: top 20）を設け、`truncated` フラグを付ける。

---

## 3. Medium: 出力過多・構造的問題

### 3.1 `get_architecture_overview_tool` スキーマ欠落（detail_level未公開）

`dagayn/main.py:578` の `get_architecture_overview_tool` は `detail_level: str = "standard"` と `top_n: int = 20` を引数に持つが、MCP ToolSearch返却スキーマには `repo_root` のみが含まれる。fastmcp が Optional でない引数のみをスキーマに含めている可能性がある。  
**影響**: MCPクライアントは minimal モードを指定できず、常に268,150文字の出力を受け取る。

---

### 3.2 `find_large_functions_tool` — 絶対パス返却・detail_level未対応

**観測**: `results[].name` が `/Users/wataru_manji/src/dagayn/tests/test_parser.py` の絶対パスを返す（`qualified_name` の相対パスと重複）。  
`summary` は上位10件に truncate されるが `results[]` は `limit=50` 分をフル返却（各12フィールド）。`detail_level` なし。  
**改修案**: `name` フィールドをファイルの場合は `qualified_name`（相対パス）に統一、または `detail_level` を追加して minimal では5フィールドに削減。

---

### 3.3 `detect_adp_violations_tool` — top_n なし（41件全返却）

**観測**: 41件のサイクルをすべて返す。`top_n` なし。各 violation は `nodes[], length, edge_weight, severity`（コンパクト）。  
本番コードでは数百サイクルになりうる。`status`フィールドなし。  
**改修案**: `top_n: int = 30` を追加。

---

### 3.4 envelope 不統一: `status` フィールド欠落

以下のツールは `status: "ok"/"error"` が返却辞書に含まれない:

| ツール | 代替の区別方法 |
|--------|--------------|
| `get_hub_nodes_tool` | `count` フィールド存在で成功判断 |
| `get_bridge_nodes_tool` | 同上 |
| `get_surprising_connections_tool` | 同上 |
| `get_suggested_questions_tool` | 同上 |
| `compute_sap_metrics_tool` | `total` フィールド |
| `compute_sdp_metrics_tool` | `total` フィールド |
| `detect_adp_violations_tool` | `count` フィールド |
| `detect_sdp_violations_tool` | `count` フィールド |
| `detect_sap_violations_tool` | `count` フィールド |
| `get_knowledge_gaps_tool` | `total_gaps` フィールド |

クライアントコードがエラーハンドリングに `status == "error"` を使う場合、これらのツールではエラーが検出されない。

---

### 3.5 envelope 不統一: `_hints` vs `next_tool_suggestions` 混在

`_hints: {next_steps, related, warnings}` 構造を使うツール（多数派）と、トップレベルの `next_tool_suggestions: []` だけを使うツール（少数派）が混在する:

**`next_tool_suggestions` のみ（`_hints`なし）**:
- `traverse_graph_tool`, `get_hub_nodes_tool`, `get_bridge_nodes_tool`
- `get_surprising_connections_tool`, `get_suggested_questions_tool`
- `compute_sap_metrics_tool`, `compute_sdp_metrics_tool`
- `detect_adp_violations_tool`, `detect_sdp_violations_tool`, `detect_sap_violations_tool`

**`_hints` のみ（`next_tool_suggestions`なし）**:
- `semantic_search_nodes_tool`, `get_affected_flows_tool`, `detect_changes_tool`
- `get_community_tool`, `list_flows_tool`, `list_communities_tool` 他多数

**両方なし**:
- `list_graph_stats_tool`, `list_repos_tool`

---

### 3.6 `get_review_context_tool` (minimal) — key_entities に絶対パス混入

**観測**: `key_entities` フィールドに絶対パス（`/Users/wataru_manji/src/dagayn/dagayn/tools/_common.py`）と関数名（`_get_store` など）が混在。絶対パスはリポジトリ外部への情報漏洩リスクと可読性低下を招く。  
**改修案**: `key_entities` に含めるのは `qualified_name`（相対パス形式）のみに統一。

---

### 3.7 `get_suggested_questions_tool` — 全件返却・limit なし

**観測**: 12件返却（`by_priority: {high:5, medium:5, low:2}`）。`status`なし。  
dagaynは小規模だが、大規模リポでは数十〜数百件になりうる。  
**改修案**: `top_n: int = 15` 追加。

---

### 3.8 `compute_sap_metrics_tool` / `detect_sap_violations_tool` の重複

`detect_sap_violations_tool` の返却構造が `compute_sap_metrics_tool` と同一（violation もフルメトリクスオブジェクト）。違反スコープのみを知りたい場合でも `compute_sap_metrics_tool` と同サイズの出力になる。  
**改修案**: `detect_sap_violations_tool` は violations の `scope_key, display_name, distance, zone` のみを返し、詳細は `compute_sap_metrics_tool` に委譲する設計に変更。

---

### 3.9 `list_graph_stats_tool` — summary と raw データの重複

**観測**: `summary` フィールドに全統計をテキストで含み、かつ `nodes_by_kind`, `edges_by_kind`, `languages[]` のネスト辞書も raw で返す（冗長）。  
`status`, `_hints`, `next_tool_suggestions` すべてなし。  
出力サイズ: ≈1,100バイト（許容範囲だが整合性欠如）。

---

## 4. Low: 小規模改善点

### 4.1 `get_docs_section_tool` — max_chars なし

セクションが長い場合（`languages` など）にも無制限で返す。現在の `usage` セクションは3文で小さいが、設計上のキャップがない。`max_chars: int = 2000` を追加推奨。

### 4.2 `get_wiki_page_tool` — 生成前は not_found のみ

wiki 未生成時のエラーメッセージが `"No wiki page found"` だけで、`generate_wiki_tool` を先に実行する必要があることが不明。`hints.next_steps` に `generate_wiki_tool` を含めると良い。

### 4.3 `cross_repo_search_tool` — レジストリ未登録時の案内が薄い

`"No repositories registered."` のみで `register` コマンドの使い方案内がない。`_hints` または `summary` に `dagayn register <path>` の案内を追加推奨。

### 4.4 `traverse_graph_tool` — `start_node` に絶対パス形式の qualified_name

`"start_node":"dagayn/graph/_protocol.py::GraphQueryProtocol.get_impact_radius"` — これは正しい形式だが、ノードが見つからない場合のエラーメッセージが不明。エラーケースの異常系テストが必要。

### 4.5 `refactor_tool(rename)` — `apply_refactor_tool` に dry_run フィードバックなし

dry_run の出力（diff）は `refactor_tool` の `_hints.next_steps` に案内がない。`apply_refactor_tool` に `dry_run: True` を推奨する hint を追加すると UX が改善。

### 4.6 `detect_sdp_violations_tool` — 小規模コードベースでは件数少ない

3件のみ（dagayn本体）。問題なく機能しているが、大規模リポでの件数爆発リスクは存在する（`top_n`なし）。

### 4.7 `get_surprising_connections_tool` — surprise_score が全件0.5

15件すべての `surprise_score: 0.5`。スコアの差別化がなく、上位/下位の判断ができない。スコアリングロジックの見直しを推奨。

---

## 5. 正常系の正しさ — 問題なしと確認したツール

以下のツールは正常系で意図通りの出力を確認:

| ツール | 出力サイズ（標準） | 備考 |
|--------|------------------|------|
| `list_graph_stats_tool` | ≈275トークン | 構造冗長だがサイズは許容 |
| `list_repos_tool` | ≈20トークン | 完璧に小さい |
| `get_minimal_context_tool` | ≈115トークン | 設計通りの超コンパクト |
| `get_impact_radius_tool` (minimal) | ≈100トークン | `truncated`フラグ正常 |
| `query_graph_tool` (callers_of, minimal) | ≈60トークン | projectionが正確 |
| `query_graph_tool` (callers_of, standard) | ≈115トークン | edges含みで適切 |
| `traverse_graph_tool` | ≈375トークン | `token_budget`が最も丁寧に実装 |
| `semantic_search_nodes_tool` | 0件（FTS fallback） | keyword fallbackが正常動作 |
| `detect_changes_tool` (minimal) | ≈150トークン | review_priorities[:3]が正常 |
| `get_affected_flows_tool` | ≈75トークン | 0件でも正常構造 |
| `list_flows_tool` (minimal) | ≈650トークン | limit=50で50件 |
| `get_flow_tool` (include_source=False) | ≈750トークン | 18ノードで適切 |
| `get_hub_nodes_tool` | ≈300トークン | top_n=10で正常 |
| `get_bridge_nodes_tool` | ≈300トークン | betweenness値も正常 |
| `get_suggested_questions_tool` | ≈450トークン | by_priority分類が機能 |
| `compute_sdp_metrics_tool` | ≈500トークン | top_n=30スライス正常 |
| `detect_sdp_violations_tool` | ≈125トークン | 3件で小さい |
| `detect_adp_violations_tool` | ≈1,250トークン | 41件は許容（小〜中規模） |
| `compute_sap_metrics_tool` | ≈3,750トークン | top_n=30スライス正常 |
| `refactor_tool` (rename) | ≈175トークン | editsが2件で正常 |
| `apply_refactor_tool` (dry_run) | ≈250トークン | diff出力が正確 |
| `get_docs_section_tool` | ≈75トークン | `usage`セクションで小さい |
| `get_wiki_page_tool` | not_found | wiki未生成で正常エラー |
| `cross_repo_search_tool` | ≈20トークン | 未登録で正常エラー |
| `get_review_context_tool` (minimal) | ≈115トークン | 適切にコンパクト |
| `get_surprising_connections_tool` | ≈1,000トークン | top_n=15で正常（スコア問題はあり） |

---

## 6. 後続タスク候補

優先度順:

| # | 対象ツール | 改修内容 | 優先度 |
|---|-----------|---------|--------|
| 1 | `get_architecture_overview_tool` | MCPスキーマに `detail_level`/`top_n` を公開 | Critical |
| 2 | `get_impact_radius_tool` | standard モードに `max_nodes: int = 50` 上限を追加 | Critical |
| 3 | `refactor_tool(dead_code/suggest)` | `limit: int = 50` パラメータ追加、`truncated` フラグ | Critical |
| 4 | `get_community_tool` | `include_members=False` 時は `members[]`フィールド省略 | High |
| 5 | `get_knowledge_gaps_tool` | 各リストに `top_n: int = 20` 追加 | High |
| 6 | `detect_sap_violations_tool` | 軽量返却構造に変更（フルメトリクス省略） | High |
| 7 | `list_communities_tool` | `limit: int = 50` 追加 | High |
| 8 | `detect_changes_tool` | `changed_functions`/`test_gaps` に上限追加 | High |
| 9 | `get_flow_tool` | `include_source=True` 時に `max_lines_per_step: int = 50` 追加 | High |
| 10 | envelope統一 | `status` フィールドを全10ツールに追加 | Medium |
| 11 | envelope統一 | `_hints` か `next_tool_suggestions` に一本化 | Medium |
| 12 | `find_large_functions_tool` | `name`フィールドの絶対パス問題修正 + `detail_level`追加 | Medium |
| 13 | `get_review_context_tool` | `key_entities`の絶対パス除去 | Medium |
| 14 | `detect_adp_violations_tool` | `top_n: int = 30` 追加 | Medium |
| 15 | `get_suggested_questions_tool` | `top_n: int = 15` 追加 | Medium |
| 16 | `get_surprising_connections_tool` | スコアリング差別化を改善 | Low |
| 17 | `get_docs_section_tool` | `max_chars: int = 2000` 追加 | Low |
| 18 | `get_wiki_page_tool` | エラー時に `generate_wiki_tool` を `hints`に追加 | Low |

---

## 付録: ツール別観測データ

### 出力サイズ実測表

| ツール | モード | 観測サイズ | MCP上限超過 |
|--------|--------|-----------|------------|
| `list_graph_stats_tool` | — | ≈1,100 B | なし |
| `list_repos_tool` | — | ≈75 B | なし |
| `find_large_functions_tool` | limit=50 | ≈10,000 B | なし |
| `query_graph_tool` | standard, 1件 | ≈450 B | なし |
| `query_graph_tool` | minimal, 1件 | ≈230 B | なし |
| `traverse_graph_tool` | depth=3, budget=2000 | ≈1,500 B | なし |
| `semantic_search_nodes_tool` | standard, 0件 | ≈300 B | なし |
| `get_impact_radius_tool` | **standard** | **297,975 B** | **YES** |
| `get_impact_radius_tool` | minimal | ≈400 B | なし |
| `detect_changes_tool` | standard, 27files | ≈8,000 B | なし |
| `detect_changes_tool` | minimal | ≈600 B | なし |
| `get_review_context_tool` | minimal | ≈450 B | なし |
| `get_affected_flows_tool` | — | ≈300 B | なし |
| `get_minimal_context_tool` | — | ≈450 B | なし |
| `get_architecture_overview_tool` | **standard** | **268,150 B** | **YES** |
| `list_communities_tool` | minimal, 280件 | ≈14,000 B | なし |
| `get_community_tool` | include_members=False | ≈8,000 B | なし |
| `get_community_tool` | **include_members=True** | **68,100 B** | **境界** |
| `get_hub_nodes_tool` | top_n=10 | ≈1,200 B | なし |
| `get_bridge_nodes_tool` | top_n=10 | ≈1,200 B | なし |
| `get_knowledge_gaps_tool` | — | ≈25,000 B (推定) | 大規模でYES |
| `get_surprising_connections_tool` | top_n=15 | ≈4,000 B | なし |
| `get_suggested_questions_tool` | — | ≈1,800 B | なし |
| `list_flows_tool` | minimal, 50件 | ≈2,600 B | なし |
| `get_flow_tool` | include_source=False, 18ノード | ≈3,000 B | なし |
| `get_flow_tool` | include_source=True, 18ノード | ≈数万B | 大規模でYES |
| `compute_sap_metrics_tool` | top_n=30 | ≈15,000 B | なし |
| `detect_sap_violations_tool` | min_distance=0.5 | ≈20,000 B | 大規模でYES |
| `compute_sdp_metrics_tool` | top_n=30 | ≈2,000 B | なし |
| `detect_sdp_violations_tool` | — | ≈500 B | なし |
| `detect_adp_violations_tool` | — | ≈5,000 B | 大規模でYES |
| `refactor_tool` | **dead_code** | **141,701 B** | **YES** |
| `refactor_tool` | **suggest** | **194,282 B** | **YES** |
| `refactor_tool` | rename, 2編集 | ≈700 B | なし |
| `apply_refactor_tool` | dry_run, 2ファイル | ≈1,000 B | なし |
| `get_docs_section_tool` | usage | ≈300 B | なし |
| `get_wiki_page_tool` | not_found | ≈100 B | なし |
| `cross_repo_search_tool` | no registry | ≈75 B | なし |

### `traverse_graph_tool` — ベストプラクティス実装

`traverse_graph_tool` は `token_budget` パラメータで逐次トークン積算を行い `truncated: bool` を返す。他のツールが参考にすべき設計:

```python
# dagayn/tools/query.py:610-614
approx_tokens = 0
token_budget = 2000
...
# 超過したら打切り + truncated=True
```

---

*このレポートはdagayn本体グラフ（4,246 nodes）に対して35ツールをすべて実行した観測結果に基づく。実装コードの引用行番号は監査時点のものであり、リファクタリング後は変化する可能性がある。*

---

## 改修ログ

### Phase 1 — Critical hotfixes (commit `9b73a9f`)

| ツール | 修正内容 |
|---|---|
| `get_impact_radius_tool` | `max_nodes: int = 50` を追加。デフォルト50ノードでカット、`truncated`/`total_impacted` フラグを返す |
| `refactor_tool(dead_code)` | `limit: int = 50` を追加、`total`・`truncated` フラグを返す |
| `refactor_tool(suggest)` | 同上 |
| `test_integration_v2` | `cross_community_edges` → `cross_community_coupling` テスト修正 |

`get_architecture_overview_tool` の `detail_level`/`top_n` は fastmcp サーバー側では正しく公開されており、Claude Code の ToolSearch キャッシュが古いだけ。Claude Code の再起動で解決。

### Phase 2 — 共通インフラ + High 適用 (commit `1ee6ed1`)

**新規ヘルパー** (`dagayn/tools/_common.py`):
- `make_response()` — status/summary 必須の envelope ビルダー
- `apply_output_budget()` — リストをバイナリハーフィングでトークン上限内に収める
- `projection_for_detail_level()` — detail_level に応じたフィールド投影

**High 6件への apply_output_budget 適用**:

| ツール | 適用内容 | budget |
|---|---|---|
| `get_knowledge_gaps` | 4リストを優先度順にカット | 4000 tokens |
| `detect_sap_violations` | violations リストをカット | 5000 tokens |
| `list_communities` | communities リストをカット | 4000 tokens |
| `detect_changes` (standard) | test_gaps/affected_flows/review_priorities をカット | 8000 tokens |
| `get_flow` (include_source) | ステップソースを 2000 文字上限、steps リストをカット | 8000 tokens |
| `get_community` | include_members=False 時: member_qns→sample(5件)+total_members | 5000 tokens |

### Phase 2 続き — envelope 統一 (commit `TBD`)

`status`/`summary` を欠いていた 5 ツールへ追加:

| ツール | 対応 |
|---|---|
| `get_hub_nodes` | `make_response("ok", "Found N hub nodes...")` に変更 |
| `get_bridge_nodes` | `make_response("ok", "Found N bridge nodes...")` に変更 |
| `compute_sap_metrics` | `status`/`summary` フィールド追加 |
| `compute_sdp_metrics` | `status`/`summary` フィールド追加 |
| `detect_sdp_violations` | `status`/`summary` フィールド追加 |

### Phase 3 — Medium/Low 改修 (commit `TBD`)

| 重大度 | ツール | 修正内容 |
|---|---|---|
| M-3.5 | `make_response` / 各ツール envelope | `next_tool_suggestions` だけを返していたレスポンスでも `_hints.next_steps` を自動補完し、`list_graph_stats_tool` / `list_repos_tool` も標準 envelope に統一 |
| M-3.6 | `get_review_context_tool` | minimal の `key_entities` を repo-relative な `qualified_name` 表示へ統一 |
| H-2.2 / M-3.4 / M-3.8 | `detect_sap_violations_tool` | `top_n: int = 30` を追加、`status`/`summary`/`truncated` を追加、返却を `scope_key, display_name, distance, zone` に簡素化 |
| M-3.3 | `detect_adp_violations_tool` | `top_n: int = 30` 追加、`status`/`summary`/`truncated` 追加 |
| M-3.7 | `get_suggested_questions_tool` | `top_n: int = 15` 追加、高優先度先出し、`status`/`summary`/`truncated` |
| M-3.9 | `list_graph_stats_tool` | 冗長な multi-line summary を簡潔化し、`next_tool_suggestions` / `_hints` を追加 |
| M-3.4 | `get_surprising_connections_tool` | `make_response()` で `status`/`summary` 追加 |
| M-3.2 | `find_large_functions_tool` | File ノードの `name` 絶対パス → 相対パスに統一 |
| L-4.1 | `get_docs_section_tool` | `max_chars: int = 4000` 追加、コンテンツ末尾に `... (truncated)` |
| L-4.2 | `get_wiki_page_tool` | not_found 時に `generate_wiki_tool` を次アクション hint に追加 |
| L-4.3 | `cross_repo_search_tool` | 未登録時の案内に `dagayn register` と `list_repos_tool` を追加 |
| L-4.4 | `traverse_graph_tool` | `not_found` 時も `status`/`summary`/`_hints` 付きの標準 envelope を返すよう修正し、異常系テストを追加 |
| L-4.5 | `refactor_tool(rename)` | `next_tool_suggestions` に `apply_refactor_tool(dry_run=True)` を追加 |
| L-4.6 | `detect_sdp_violations_tool` | `top_n: int = 30` と `truncated` を追加し、大規模リポでも上限を制御 |
| L-4.7 | `get_surprising_connections_tool` / `find_surprising_connections` | rare-community-pair / degree-imbalance の加点で `surprise_score` が単一値に潰れにくいよう改善 |

**2026-04-30 再確認**:
- Phase 1 の critical hotfixes は現行コードでも実装済み
- Phase 2 の共通ヘルパー導入と High 向け出力制御は実装済み
- Phase 3 相当の未達だった `detect_sap_violations_tool` と 3.5〜4.7 の残項目もこの差分で完了し、監査レポートの phase 1〜3 範囲は現行実装と整合した
