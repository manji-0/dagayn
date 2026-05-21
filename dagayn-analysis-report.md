<!-- constrained-by ./AGENTS.md -->
<!-- constrained-by ./docs/USAGE.md -->
<!-- constrained-by ./docs/COMMANDS.md -->
<!-- derived-from ./README.md -->
<!-- derived-from ./docs/ARCHITECTURE.md -->
<!-- derived-from ./docs/FEATURES.md -->
<!-- derived-from ./docs/SCHEMA.md -->
<!-- derived-from ./docs/PERFORMANCE-IMPROVEMENTS-WIP.md -->
<!-- derived-from ./docs/RUST-CORE-MIGRATION-WIP.md -->
<!-- derived-from ./docs/LOCAL-EMBEDDINGS.md -->
<!-- derived-from ./docs/DAEMON-CONFIG.md -->
<!-- derived-from ./docs/plans/ANALYSIS-TOOL-STRATEGY.md -->
<!-- derived-from ./skills/writing-markdown-document/SKILL.md -->

# dagayn 総合分析レポート

> 作成日: 2026-05-21
> 対象リポジトリ: `/Users/manji0/src/dagayn`
> 対象バージョン: `pyproject.toml` 上の `dagayn` 4.0.2
> グラフ根拠: `list_graph_stats_tool` / `architecture_analysis_tool` / `flow_tool` / `review_tool` / `dagayn tool find_large_functions_tool` / `uv tool install --editable`

この文書は、dagayn の思想、提供機能、実装構造、性能上の設計と課題、そして dagayn 自身を dagayn で分析した結果を1つにまとめた総合レポートである。単なる機能一覧ではなく、なぜその設計になっているのか、どのコード・ドキュメント・グラフ指標が根拠になっているのか、今後どこを改善すべきかまでを一貫して説明する。

## 1. エグゼクティブサマリー

dagayn は「DAG is All You Need」を掲げる、コードレビューと影響分析のためのローカル知識グラフ基盤である。リポジトリを SQLite ベースの構造グラフに変換し、ファイル、シンボル、呼び出し、インポート、テスト、Markdown 依存、Terraform 参照、実行フロー、コミュニティ、検索インデックスを同じデータモデル上で扱う。

中心思想は明確である。AI エージェントが毎回リポジトリ全体を読むのではなく、構造化されたグラフを先に問い合わせ、必要な箇所だけを読む。これにより、レビュー対象の絞り込み、変更影響の推定、テスト候補の提示、アーキテクチャ上の危険箇所の発見、ドキュメントとコードの対応付けを、トークン効率よく行える。

この fork は upstream の `code-review-graph` から派生しているが、現行ドキュメントでは upstream prose を canonical とは扱わない。dagayn 自体の特長は、Terraform と Markdown を first-class に扱うこと、Rust バックエンドへ移行していること、MCP 3.0 の小さな dispatcher surface を採用していること、Codex / Claude / OpenCode などのエージェント設定を自動化すること、そして mixed-language monorepo を主対象にしていることである。

2026-05-21 の incremental refresh 後の自己分析では、グラフは 8,114 ノード、47,431 エッジ、388 ファイル、29 言語、7,726 embedding を持つ。ノード種別は `Function` 2,908、`Test` 1,766、`DocBody` 1,823、`DocSection` 773、`Class` 456、`File` 388。エッジ種別は `CALLS` 28,849、`CONTAINS` 7,796、`TESTED_BY` 6,725、`IMPORTS_FROM` 1,867、`CROSS_ARTIFACT` 1,840、`DEPENDS_ON` 168、`REFERENCES` 139、`INHERITS` 43、`IMPLEMENTS` 4 である。

アーキテクチャ分析の主な発見は次の通りである。

- コミュニティは全体で 369。最大は `docs-tool` 896 ノード、次に `tests-nodes` 337、`tests-detect` 313、`tests-files` 260、`docs-returns` 260。
- `architecture_analysis_tool(mode="overview", detail_level="minimal")` は 2 件の high coupling warning を返した。最大表示は `tests-detect` と `tests-nodes` の 12 エッジ結合で、もう1件は `docs-tool` と `tests-nodes` の 11 エッジ結合。出力は coupling 5 件中 1 件のみ保持され、`truncated=true` だった。
- ハブ上位は `dagayn/cli/commands/build.py::handle` degree 161、`tests/test_flows.py::TestFlows._add_func` degree 158、`crates/dagayn-graph/src/tests.rs::stores_flows_and_reads_flow_inputs` degree 147。
- ブリッジ上位は `tests/test_main.py::TestLongRunningToolsAreAsync.test_regression_guard_does_not_depend_on_fastmcp_internals` betweenness 0.009462、`tests/test_integration_v2.py::TestV2Integration.test_full_pipeline` 0.005896、`dagayn/search.py::hybrid_search` 0.002908。
- 知識ギャップは raw count 2,637。内訳は isolated nodes 2,284、thin communities 163、untested hotspots 75、single-file communities 115。未テスト hotspot の p95 degree 閾値は 37。
- ADP / SDP / SAP は `artifact_scope="code"` を既定にして再計算された。code scope の ADP 違反は 1 件、SDP 違反は 0 件、SAP 違反は 7 件である。
- docs scope では ADP 違反が 3 件、SDP 違反が 1 件残る。これはコード設計の問題ではなく、Markdown dependency directive による文書構造の signal として扱うべきである。

## 2. dagayn の思想

<!-- constrained-by #1-エグゼクティブサマリー -->

### 2.1 グラフを読むことを最初の行動にする

dagayn の思想は「コードを読む前に、構造を読む」である。`AGENTS.md` でも、広範なテキスト検索の前に `get_minimal_context_tool` を呼ぶことが明示されている。これは単なる作法ではなく、AI エージェントの制約に対する設計方針である。

AI エージェントは大きなコードベースを一度に完全には読めない。全文検索だけに頼ると、検索語に依存した断片的な把握になりやすい。dagayn は、呼び出し関係、インポート、テスト、ドキュメント依存、コミュニティ、フローを先に抽出しておくことで、エージェントが「どこを読むべきか」を構造的に判断できるようにする。

この方針は次の挙動に現れている。

- `get_minimal_context_tool` はタスク説明から workflow、risk、key entities、top communities / flows、次に呼ぶべきツールを返す。
- `review_tool(mode="changes")` は変更ファイルからリスク、理由コード、推奨テスト、影響フロー、ドキュメント更新候補をまとめる。
- `architecture_analysis_tool(mode="overview")` は概要に留め、必要なら `hubs`、`bridges`、`knowledge_gaps`、`adp_violations` などへ drill down する。
- `query_graph_tool` は callers / callees / imports / tests / docs / implementations などの定型関係に限定し、探索を小さく保つ。

つまり dagayn は「便利な可視化ツール」ではなく、エージェントの探索順序を制御するコンテキスト圧縮装置である。

### 2.2 ローカル性と再現性

dagayn はローカル SQLite を中核にする。外部データベースは不要で、`.dagayn/graph.db` と必要に応じて `.dagayn/embeddings.db` を使う。これにより、レビューや探索は基本的にローカルで完結する。

ローカル性には3つの意味がある。

1. セキュリティ: ソースコードを外部サービスへ送らずに構造分析できる。
2. 再現性: 同じコミット、同じ graph build なら、同じ nodes / edges / metrics を参照できる。
3. エージェント適合性: MCP server がローカルプロセスとして起動し、Codex などのクライアントから低コストで問い合わせられる。

embedding は例外的にローカルまたはリモートを選べる。`dagayn install --mode fts` は embedding なしの FTS-only、`--mode local` は managed Qwen GGUF sidecar、`--mode remote` は OpenAI-compatible / Google / MiniMax provider を使う。重要なのは、検索の基盤が FTS5 によって常に成立し、embedding は任意の品質向上層として設計されている点である。

### 2.3 コード、ドキュメント、インフラを同じグラフに入れる

dagayn は一般的なソースコードだけでなく、Markdown、Jupyter / Databricks notebook、Terraform を graph source として扱う。これは mixed-language monorepo を主対象にする fork-specific な設計である。

この思想の実践例は Markdown policy に最もよく表れている。Markdown の見出しは `DocSection` ノードになり、相対リンクは `IMPORTS_FROM` / `REFERENCES`、directive comment は `DEPENDS_ON`、backtick symbol や `dagayn:` directive は `CROSS_ARTIFACT` になる。ドキュメントはコードの外にある説明ではなく、コードと同じグラフで影響分析される artifact である。

Terraform についても同じである。`resource`、`data`、`module`、`variable`、`local`、`output`、`provider`、`check` などがノードになり、`var.x`、`local.x`、`module.x`、provider source、module source、`depends_on` 相当の制約が edge になる。アプリケーションコードとインフラコードが同じ影響分析に入ることが、この fork の実用上の価値である。

### 2.4 分析結果は「証拠付きのリード」であって、自動承認ではない

dagayn の設計は、グラフ分析を絶対視しない。`AGENTS.md` は、threshold、count、reason code、truncation state を明示し、結果は review lead として扱うよう求めている。これは実装上も重要である。

たとえば `knowledge_gaps` の untested hotspot は、p95 degree 以上で `TESTED_BY` edge がないノードを検出する。これは重要な候補を浮かび上がらせるが、動的 dispatch、外部 entry point、fixture、生成コード、CLI entry などを完全に理解するわけではない。したがって、修正前には実際の public API、テスト、動的呼び出しを確認する必要がある。

dagayn は「正解を出すツール」ではなく、「正解に近づくための構造的な足場」である。

## 3. 提供機能

<!-- derived-from #2-dagayn-の思想 -->

### 3.1 グラフライフサイクル

基本コマンドは `dagayn build`、`dagayn update`、`dagayn postprocess`、`dagayn watch`、`dagayn status` である。

`build` は初回または full rebuild 用で、`--force-full-build` / `--force` により既存 graph database と SQLite sidecar を削除してから再構築する。`update` は変更差分に基づく incremental refresh を行う。`watch` は開発中のファイル変更を監視する。`postprocess` は既存グラフに対して flows、communities、FTS などを再計算する。

`build` と `update` は `--local-embedding low|high|none` を受け取り、graph refresh 後に local Qwen embedding を生成できる。local embedding server の readiness timeout、request timeout、batch size はそれぞれ別パラメータで制御される。

### 3.2 MCP 3.0 の compact dispatcher surface

dagayn v3 は MCP の公開面を小さく保つ。デフォルトで露出する主要ツールは次の7つである。

| ツール | 役割 |
|---|---|
| `get_minimal_context_tool` | タスクの初期方針、risk、key entities、次ツール候補 |
| `review_tool` | `changes` / `context` / `affected_flows` / `impact` のレビュー dispatcher |
| `flow_tool` | `list` / `get` の実行フロー dispatcher |
| `architecture_analysis_tool` | architecture overview / hubs / bridges / gaps / ADP / SDP / SAP dispatcher |
| `refactor_tool` | rename preview、dead code、refactor suggestion |
| `query_graph_tool` | callers、callees、imports、tests、docs、implementations、file summary |
| `semantic_search_nodes_tool` | FTS / embedding / hybrid semantic search |

高度な maintenance tools、wiki generation、embedding build、refactor application、cross-repo search は、`dagayn serve --tools all` または明示的な allow-list で公開する。CLI では `dagayn tool <tool-name>` が同じ実装を JSON で呼び出すため、MCP server の allow-list が狭くても shell 経由で補える。

この設計は、エージェントに大量のツール名を見せないためのものでもある。ユーザーやエージェントは、最初に小さな dispatcher を呼び、必要な mode だけを選べばよい。

### 3.3 変更レビューと影響分析

`review_tool(mode="changes")` は dagayn の中核的なレビュー面である。変更ファイルを検出し、変更ノード、risk score、reason codes、affected flows、recommended tests、documentation candidates、hotspot proximity、architecture risks をまとめる。

`review_tool(mode="impact")` は指定ファイルの blast radius を返す。今回、`dagayn-analysis-report.md` に対して実行した結果は、直接変更 48 ノード、2 hop 以内の impact 14 ノード、追加影響ファイル 4、risk medium、`truncated=false` だった。これは Markdown 文書の更新が周辺ドキュメントと一部 artifact edge に波及していることを示す。

`review_tool(mode="affected_flows")` は変更が実行フローへどう波及するかを見る。`mode="context"` は変更周辺の source context を取得する。これらは同一 dispatcher にまとまっているため、レビュー手順は「changes で全体を見る、必要なら impact / affected_flows / context へ drill down」という形になる。

### 3.4 アーキテクチャ分析

`architecture_analysis_tool` は dagayn の構造分析 dispatcher である。mode は `overview`、`communities`、`community`、`hubs`、`bridges`、`knowledge_gaps`、`surprising_connections`、`adp_violations`、`sdp_metrics`、`sdp_violations`、`sap_metrics`、`sap_violations` を持つ。

`overview` は `architecture_health` を含む。今回の出力では、signals は `community_coupling`、`hub_nodes`、`bridge_nodes`、`knowledge_gaps`、`surprising_connections`、`adp`、`sdp`、`sap`。reason codes は `high_cross_community_coupling`、`hub_nodes`、`bridge_nodes`、`knowledge_gaps`、`surprising_connections`、`adp_violations`、`sap_violations` だった。ADP/SDP/SAP は既定で `artifact_scope="code"` になり、Markdown dependency は docs scope に分離される。

各分析は数量と閾値を返す。たとえば `knowledge_gaps` は p95 degree 37 を untested hotspot 閾値として使い、test nodes、test-like file paths、Markdown documentation sections を除外する。このように、結果の根拠と限界が response に含まれる。

### 3.5 検索

`semantic_search_nodes_tool` は hybrid search を提供する。設計は2本の retrieval arm を Reciprocal Rank Fusion でマージする形である。

1. FTS5 BM25: `nodes_fts` virtual table を使い、symbol name、qualified name、path、signature、identifier token、docstring、Markdown body などを検索する。
2. Cosine similarity: embedding store が構築済みなら vector search を行う。

RRF の k は 10。一般的な 60 より小さく、score の差が 0.05 から 0.2 程度に広がるように調整されている。結果には `search_mode` と `source` が付き、hybrid / fts_only / embedding_only / keyword_fallback、fts / embedding / both / keyword を区別できる。

rerank には kind boost、context-file boost、intent rerank、test deboost がある。PascalCase は class/type、snake_case は function、dotted path は qualified name を強める。自然言語クエリでは documentation intent と code/test intent を軽く分類し、Markdown とコードの順位を調整する。

### 3.6 Refactor support

`refactor_tool` は `rename`、`dead_code`、`suggest` を提供する。rename は refactor preview を返し、`apply_refactor_tool` に渡せる refactor id を生成する。dead code は caller、test、importer、entry point を見て未参照候補を出す。suggest は remove、move、split、document などの候補と execution plan、required tests、rollback guidance、defer conditions を返す。

重要なのは、refactor suggestion は自動実行の指示ではない点である。dynamic dispatch、public API、generated entry point、test artifact を確認してから実施する設計になっている。

### 3.7 Docs、wiki、visualization、export

`dagayn visualize` は HTML、GraphML、Mermaid C4、SVG、Cypher、Obsidian export を出力する。HTML mode は auto / full / community / file。Jupyter / Databricks notebook は input であり report output ではない。Graphviz / DOT は built-in export target ではない。

`dagayn wiki` は `.dagayn/wiki/` に community-based Markdown wiki を生成する。各 community page は members、execution flows、cross-community dependencies、code-scoped package-level ADP / SDP / SAP metrics を含む。

Markdown は単に可視化対象ではなく、graph extraction 対象である。heading、relative link、reference-style link、HTML dependency directive、`dagayn:` documentation directive、backtick symbol が graph edge になる。

### 3.8 Integration、hooks、multi-repo

`dagayn install` は Codex、Claude、OpenCode などの AI coding platform を検出し、MCP configuration、skills、hooks、instructions を書く。Codex の場合は `~/.codex/hooks.json` と `~/.codex/config.toml` の hooks feature flag を更新する。Claude は `~/.claude/settings.json` に hooks を書く。

hooks は session start、編集後、commit 前後の graph refresh を担う。典型的には編集後や commit-time checks 前に `dagayn update --skip-flows`、post-commit で full `dagayn update` を走らせる。

multi-repo では `dagayn register`、`dagayn unregister`、`dagayn repos`、`dagayn daemon` がある。registry は `~/.dagayn/registry.json` に保存され、daemon は複数 repo の watch / refresh を管理する。

## 4. 実装構造

<!-- derived-from #3-提供機能 -->

### 4.1 全体パイプライン

`docs/ARCHITECTURE.md` は dagayn の pipeline を5段階で説明している。

1. file discovery と language detection
2. parser extraction による nodes / edges 生成
3. SQLite persistence
4. optional post-processing: flows、communities、search indexes
5. query-time analysis: review、search、refactor

この pipeline は、CLI と MCP の両方から同じ実装を呼ぶ。CLI は `dagayn/cli/commands/*`、MCP は `dagayn/main.py` と `dagayn/tools/*` が入口である。

### 4.2 Python frontend と Rust backend

現行 dagayn は Rust backend が default である。Python 側の `dagayn/parser/core.py::CodeParser` は compatibility wrapper で、実際の parsing は native extension `dagayn._core.parse_rust_owned_file_compact_json` に渡す。source checkout に native extension がない場合は、削除済みの Python parser へ fallback せず、明確に失敗する。

Rust crates は主に次の役割を持つ。

| crate | 役割 |
|---|---|
| `crates/dagayn-parser` | Rust-owned parser。言語別 parser、Markdown、Terraform、notebook 周辺を含む |
| `crates/dagayn-graph` | Rust-backed graph store と analysis JSON surface |
| `crates/dagayn-py` | PyO3 binding。Python package から Rust core を呼ぶ入口 |
| `crates/dagayn-core` | core 型・共有基盤 |
| `crates/dagayn-grammars` | Tree-sitter grammar provisioning |
| `crates/dagayn-postproc` | post-processing 用 Rust crate |

`pyproject.toml` は maturin build backend を使い、Python package に Rust extension を組み込む。Python 要件は 3.12 以上で、runtime dependency は `mcp`、`fastmcp`、`networkx`、`watchdog`、`igraph`。dev dependency には `pytest`、`ruff`、`pyinstrument` などが含まれる。

### 4.3 Parser extraction

Parser は各ファイルから `NodeInfo` と `EdgeInfo` を生成する。ノードは file path、qualified name、line range、language、parent、params、return type、modifier、test flag、extra を持つ。edge は kind、source、target、file path、line、extra を持つ。

Rust-owned parser は compact JSON を返し、Python wrapper が `NodeInfo` / `EdgeInfo` に decode する。`_normalize_path_string` は parser path と display path の差を吸収し、qualified name 内の path も表示側へ変換する。

language detection は `dagayn/parser/dispatch.py` 側に集約され、Markdown、Terraform、Rust、Python / notebook、Bash、Go、Java、Ruby、C#、PHP、Kotlin、Swift、Scala、Solidity、Dart、Lua、Luau、C/C headers、Perl XS、C++、Objective-C、Elixir、GDScript、R、Julia、Perl、Vue、Svelte、Zig、PowerShell、JavaScript / JSX / TypeScript / TSX / Astro などを扱う。

### 4.4 Markdown extraction

Markdown parser は heading を `DocSection` として抽出する。inline link / reference-style link は `IMPORTS_FROM` と `REFERENCES`、HTML directive は `DEPENDS_ON`、`dagayn:` directive と backtick symbol は `CROSS_ARTIFACT` になる。

この文書冒頭の `constrained-by` / `derived-from` コメントも dagayn が読むためのものである。これは装飾ではなく、この総合レポートがどのドキュメントに制約され、どの資料から派生したかを graph に残すための dependency declaration である。

Markdown-sourced `CROSS_ARTIFACT` は post-processing で code graph の symbol name と照合される。ちょうど1つの non-Markdown match があれば high confidence で target が promoted され、0件または複数件なら unresolved のまま low confidence として残る。これにより、将来 graph update で解決可能になる。

### 4.5 Terraform extraction

Terraform parser は `.tf` と `.tfvars` を first-class に扱う。`resource.type.name`、`data.type.name`、`module.name`、`var.name`、`local.key`、`output.name`、`provider.name` などを qualified name にする。

edge は `REFERENCES`、`CALLS`、`IMPORTS_FROM`、`CONTAINS`、`DEPENDS_ON` を生成する。module source が local path なら、呼び出し module から target directory への `IMPORTS_FROM` により module boundary をまたぐ impact analysis が可能になる。`.tfvars` の top-level assignment は `var.name` ノードになり、対応する variable block と `REFERENCES` で結ばれる。

### 4.6 Storage model

Graph data は SQLite に保存される。schema の安定した user-facing model は、nodes、edges、metadata、derived structures である。

Node kind は `File`、`Class`、`Function`、`Type`、`Test`、`DocSection`。Edge kind は `CALLS`、`IMPORTS_FROM`、`REFERENCES`、`CONTAINS`、`INHERITS`、`IMPLEMENTS`、`TESTED_BY`、`DEPENDS_ON`、`CROSS_ARTIFACT`。

`TESTED_BY` は covered production symbol から test symbol へ向く。`CROSS_ARTIFACT` は bridge kind、relationship role、evidence kind、confidence tier を extra に持つ。Markdown 由来の場合は raw backtick span を `extra.original_symbol_name` として残し、postprocess が idempotent に再解決できる。

Derived structures として communities、flow memberships、FTS、embeddings がある。FTS5 の `nodes_fts` は build 後に常に利用可能で、embedding は optional である。

### 4.7 Incremental build

`dagayn/incremental.py` は changed files を git diff などから検出し、変更ファイルと影響ファイルだけを parse / store する。default parser backend は Rust。ignore pattern は `.dagayn/**`、`node_modules/**`、`.git/**`、`target/**`、`vendor/**`、`coverage/**`、lockfile、SQLite sidecar などを除外する。

performance 改善として、worker process ごとに `CodeParser` を singleton 化している。`ProcessPoolExecutor(initializer=_init_worker)` により、各 worker が Tree-sitter grammar を毎ファイル再ロードしない。store batch size は `DAGAYN_STORE_BATCH_SIZE`、Rust parse batch size は `DAGAYN_RUST_PARSE_BATCH_SIZE` で調整可能である。

repo root 解決は `CRG_REPO_ROOT` を最優先し、git / svn root を探索し、最後に cwd へ fallback する。data dir は通常 `<repo>/.dagayn` だが、`CRG_DATA_DIR` で外部化できる。

### 4.8 MCP server implementation

`dagayn/main.py` は FastMCP server entry point である。`_DEFAULT_MCP_TOOL_NAMES` は compact surface を定義し、`dagayn serve --tools` または `CRG_TOOLS` によって公開ツールを制御する。

heavy tools は asyncio event loop を塞がないよう `asyncio.to_thread` で offload される。`build_or_update_graph_tool`、`run_postprocess_tool`、`embed_graph_tool`、`review_tool` などが該当する。これは Windows で `ProcessPoolExecutor` と sync handler が single event loop thread を塞ぐ問題への対策でもある。

`_resolve_repo_root` は explicit `repo_root`、`dagayn serve --repo` の default、None の順で解決する。embedding provider も server default と client override を分ける。remote embedding provider は OpenAI-compatible、Google、MiniMax の必要 env vars がちょうど1 provider 分だけ存在するときに自動推論される。

### 4.9 Tool implementation

MCP tool は `dagayn/tools/*` に分割されている。

| ファイル | 主な責務 |
|---|---|
| `tools/context.py` | minimal context、workflow routing、risk summary |
| `tools/review.py` / `review_dispatcher.py` | change review、impact、affected flows、context |
| `tools/query.py` | graph query、semantic search、traversal |
| `tools/architecture_analysis.py` | architecture dispatcher |
| `tools/analysis_tools.py` | hubs、bridges、knowledge gaps、surprises |
| `tools/community_tools.py` | communities、architecture overview |
| `tools/architecture_tools.py` | ADP / SDP |
| `tools/sap_tools.py` | SAP metrics / violations |
| `tools/refactor_tools.py` | rename / dead code / suggest |
| `tools/build.py` | build/update/postprocess/embed graph tools |
| `tools/_common.py` | store acquisition、output budgeting、common response |

3.0 では v2 の split architecture tools は public surface から取り除かれ、`architecture_analysis_tool(mode=...)` に集約されている。同様に review と flow も dispatcher-based である。

### 4.10 CLI implementation

CLI command files は `dagayn/cli/commands` にあり、`build.py`、`detect_changes.py`、`eval_cmd.py`、`init.py`、`profile.py`、`registry.py`、`serve.py`、`tool.py`、`wiki.py`、`daemon.py` などに分かれる。

CLI は人間と automation の両方を想定する。`dagayn tool` は MCP tool implementation を shell から呼ぶための互換面で、MCP server の公開ツールを変更せずに advanced tool を使う escape hatch になっている。`dagayn profile` は `pyinstrument` により build / search / mcp-tool などを profile し、`.dagayn/profiles/` に HTML を出力する。

## 5. 性能設計と現状

<!-- derived-from #4-実装構造 -->

### 5.1 性能の基本方針

dagayn の性能方針は「parse と postprocess は必要なときにまとめて行い、query time は graph / index を読むだけに寄せる」である。AI エージェントから呼ばれる MCP tool は対話的であるため、query-time latency と token budget が重要になる。

そのために以下の設計がある。

- incremental update により全ファイル再 parse を避ける。
- Rust-owned parser と Rust-backed graph store によって parser / storage のホットパスを移行する。
- FTS5 index を build 後に常備し、semantic search は embedding がない場合も FTS-only で機能する。
- MCP heavy tool は thread offload し、stdio event loop を塞がない。
- `_get_store` は process-level store cache を持ち、GraphStore と NetworkX graph cache を tool call 間で再利用する。
- query / analysis では N+1 を batch query や snapshot injection へ置き換える。

### 5.2 実装済み性能改善

`docs/PERFORMANCE-IMPROVEMENTS-WIP.md` によれば、以下は shipped である。

| 領域 | shipped 内容 |
|---|---|
| `compute_risk_score` | `analyze_changes` が inbound edges、flow criticalities、community IDs、transitive test count などを batch prefetch |
| `get_communities` | all community member qualified names を1クエリで取得 |
| `get_affected_flows` | `_hydrate_flow_rows` 経由で path nodes を bulk fetch |
| `traverse_graph` BFS | depth frontier ごとに nodes / edges を batch fetch |
| `_single_hop_dependents` | `_batch_hop_dependents` へ委譲し file batch ごとに1クエリ |
| `generate_suggested_questions` | `GraphSnapshot` を一度作り、hub / bridge / surprise / gap 分析へ注入 |
| process-level store cache | `_store_cache`、mtime staleness、lease / pinned、`DAGAYN_DISABLE_STORE_CACHE` |
| PRAGMA tuning | `synchronous=NORMAL`、64MB cache、256MB mmap、temp in memory |
| parser singleton | worker process ごとに `CodeParser` を再利用 |
| token estimate | `str(dict)` を避け、qualified name / file / name 長から概算 |
| `idx_nodes_parent_name` | `parent_name, name` index を追加 |

### 5.3 検索性能

dagayn の検索性能は、単純な全文検索の速さだけでなく、AI エージェントが短い待ち時間で「読むべきノード」を見つけられるかを基準に設計されている。検索パスは `semantic_search_nodes_tool` から `dagayn/tools/query.py::semantic_search_nodes`、さらに `dagayn/search.py::hybrid_search` へ入り、FTS5、embedding、RRF merge、boost / deboost、batch node fetch の順に処理される。

検索性能の基礎は FTS5 である。`nodes_fts` は build / postprocess 後に常備され、`GraphStore.fts_query` は SQLite FTS5 の BM25 を使う。BM25 の重みは `name` 8.0、`qualified_name` 6.0、`file_path` 3.0、`signature` 4.0、`identifier_tokens` 5.0、`doc_text` 1.0 で、短い identifier match が本文中の偶然一致より強く出るようにしている。入力は quoted phrase または separator split された AND / OR query に変換され、FTS5 operator injection を避ける。

FTS index には、名前、qualified name、file path、signature だけでなく、camelCase / PascalCase / snake_case / path を分解した `identifier_tokens` と、上限付きの source / Markdown section body も入る。これにより、`LocalEmbeddingProvider` のような PascalCase symbol は `local embedding provider` でも検索でき、Markdown section も検索対象になる。

`hybrid_search` は FTS arm と embedding arm を別々に走らせ、Reciprocal Rank Fusion で統合する。さらに自然言語クエリから identifier-shaped token を抽出し、たとえば "tests for embed_graph" のような文でも `embed_graph` 単体の FTS sub-query を追加する。merge 後は candidate ids をまとめて `get_nodes_by_ids` で batch fetch し、kind boost、qualified-name boost、context-file boost、intent rerank、test deboost を適用する。テストは source と名前や docstring が近くなりやすいため、明示的な test / coverage query でない限り 0.6 倍に deboost される。

embedding 側の性能は、現行実装では2段階に分かれる。`dagayn/search.py` は process-level `EmbeddingStore` cache を持ち、key は `(db_path, provider, model)`、invalidator は database mtime である。これにより、検索ごとに embedding SQLite connection と provider wrapper を作り直すコストは避けられている。

さらに `dagayn/embeddings.py::EmbeddingStore.search` は numpy が利用可能な場合、embedding rows を `(N, D)` の `float32` matrix と row norm として process-level cache に載せ、query vector との類似度を `matrix @ q` の1回の BLAS call で計算する。cache key は `(db_path, provider_name, mtime_ns)` で、同じ database / provider の古い entry は削除される。numpy がない環境では pure-Python loop に fallback するため、機能は維持されるが、大きい embedding store では latency が伸びる。

検索品質の既存測定では、`docs/LOCAL-EMBEDDINGS.md` の 4,597 nodes / 12 query suite で FTS5 only は mean MRR 0.71、Precision@1 0.67、Precision@5 0.75。Qwen3-Embedding-0.6B `low` の hybrid は mean MRR 0.88、Precision@1 0.83、Precision@5 0.92。Qwen3-Embedding-4B `high` は mean MRR 0.82、Precision@1 0.75、Precision@5 0.92 だった。FTS5 は exact name と PascalCase に強く、embedding は conceptual query の穴を埋める。小さい 0.6B preset がこの suite では 4B と同等以上で、メモリは約 1/7 なので、通常は `low` が妥当な既定値である。

検索性能の現状評価は次の通りである。

| 項目 | 現状 | 性能上の意味 |
|---|---|---|
| Exact identifier search | FTS5 BM25 + identifier token | 高速で、embedding なしでも強い |
| Natural-language conceptual search | FTS + embedding RRF | embedding がある場合に recall が上がる |
| Fallback | hybrid -> FTS-only -> embedding-only -> keyword LIKE | index や provider が欠けても検索不能になりにくい |
| EmbeddingStore lifecycle | process-level cache | tool call ごとの connection / provider 初期化を削減 |
| Vector similarity | numpy matrix cache + BLAS fast path | embedding rows 全件に対する Python loop を回避 |
| Result hydration | candidate ids の batch fetch | merge 後の N+1 fetch を避ける |
| Ranking tuning | kind / context / intent / test deboost | source node が test や docs に埋もれにくい |

残る検索性能課題は、品質と latency の両面にある。まず、embedding の生成側では `embed_nodes` の既存 embedding 確認と insert がまだ batch 化余地を持つ。次に、query vector の provider call は `_embed_query_cached` で cache されるが、provider や model の組み合わせ、remote endpoint の latency、失敗時の fallback policy は実運用でさらに測る必要がある。最後に、`semantic_search_nodes` の p95 latency target は WIP plan では 500 ms 未満だが、まだ CI gate ではなく、current repo / larger synthetic repo / remote provider の3条件で baseline を分ける必要がある。

### 5.4 残る性能課題

未実装または partial の項目も明確である。

- `get_flow_by_id` 単体呼び出しは path step ごとに `get_node_by_id` を呼ぶ。
- hub / bridge scores は runtime calculation が残る。特に bridge は NetworkX betweenness を query time に走らせる。
- write side は `upsert_node` / `upsert_edge` の per-row INSERT + SELECT、`store_file_batch` 未使用、communities / flows の insert loops が残る。
- Markdown artifact ref resolver は edge ごとに SELECT + UPDATE / DELETE している。
- embedding search は numpy 利用時の matrix cache + BLAS fast path を持つが、numpy 非導入環境では pure-Python cosine loop に fallback する。
- `embed_nodes` は node ごとに embedding SELECT / INSERT している。
- suffix LIKE は index が効きにくい。
- incremental update は mtime が変わっていないファイルでも bytes read + sha256 を行う。
- DFS traversal は BFS と違い size-1 batch のまま。
- `parse_diff_ranges` は複数 tool call で同じ `git diff` を繰り返し得る。
- provider `embed_query` 結果は `_embed_query_cached` で LRU cache されるが、remote provider ごとの latency baseline と cache hit rate はまだ継続測定が必要である。

これらは性能上の TODO であると同時に、設計の方向性を示している。dagayn は query-time の全表 scan や Python loop を段階的に postprocess / Rust / cache / batch へ移す途中にある。

### 5.5 Benchmark と測定

`dagayn eval` は build performance、flow completeness、impact accuracy、N+1 count、search quality、token efficiency などの benchmark を持つ。`nplusone_count.py` は `sqlite3.set_trace_callback` による SQL statement counter を使い、baseline を超えたら failure にできる。

`dagayn profile` は `pyinstrument` により CPU profiler output を `.dagayn/profiles/profile_<subcommand>_<timestamp>.html` に書く。

未整備のものとして、per-MCP-tool wall-clock latency benchmark と CI regression gates が残る。WIP plan の初期 target は、`semantic_search_nodes` p95 < 500 ms、`detect_changes` < 300 ms、`get_impact_radius` < 100 ms、`get_review_context` < 200 ms、hub / bridge post-Fix B < 50 ms、depth 3 traversal < 150 ms である。ただしこれは hard acceptance criteria ではなく、測定後に調整される calibration target とされている。

## 6. dagayn 自身のグラフ分析

<!-- derived-from #5-性能設計と現状 -->

### 6.1 グラフ統計

`list_graph_stats_tool` の 2026-05-21 出力は以下である。

| 指標 | 値 |
|---|---:|
| total nodes | 8,114 |
| total edges | 47,431 |
| files | 388 |
| languages | 29 |
| embeddings | 7,726 |
| last updated | 2026-05-21T20:15:50 |

ノード種別:

| kind | count |
|---|---:|
| Function | 2,908 |
| Test | 1,766 |
| DocBody | 1,823 |
| DocSection | 773 |
| Class | 456 |
| File | 388 |

エッジ種別:

| kind | count |
|---|---:|
| CALLS | 28,849 |
| CONTAINS | 7,796 |
| TESTED_BY | 6,725 |
| IMPORTS_FROM | 1,867 |
| CROSS_ARTIFACT | 1,840 |
| DEPENDS_ON | 168 |
| REFERENCES | 139 |
| INHERITS | 43 |
| IMPLEMENTS | 4 |

この構成から、dagayn 自身のグラフはコード呼び出しとテスト関係が大きな割合を占めつつ、Markdown / cross artifact edge もかなり多いことが分かる。`CROSS_ARTIFACT` 1,840 は、この repo がドキュメントと実装の対応を積極的に graph 化していることを示す。一方で ADP/SDP/SAP は code scope と docs scope を分けて読む必要がある。

### 6.2 コミュニティ構造

`architecture_analysis_tool(mode="overview", detail_level="minimal")` は 369 communities、5 coupled pairs shown、2 warnings を返した。最大表示された coupling は `tests-detect` から `tests-nodes` への 12 edges coupling で、もう1つの warning は `docs-tool` から `tests-nodes` への 11 edges coupling である。出力は `truncated=true` で、communities は 369 件中 1 件、cross-community coupling は 5 件中 1 件だけ保持されていた。

SQLite の communities table を size 降順に見ると上位は次の通りである。

| community | size | cohesion |
|---|---:|---:|
| docs-tool | 896 | 0.6560 |
| tests-nodes | 337 | 0.6259 |
| tests-detect | 313 | 0.6610 |
| tests-files | 260 | 0.5266 |
| docs-returns | 260 | 0.5633 |
| tests-install | 257 | 0.5463 |
| docs-confidence | 250 | 0.5260 |
| graph-nodes | 189 | 0.7938 |
| src-file | 142 | 0.5142 |
| src-py | 112 | 0.6682 |
| tests-provider | 104 | 0.6068 |
| tests-flows | 102 | 0.2227 |

最大 community が `docs-tool` であることは、この repo の特徴をよく表している。dagayn は tool surface と agent workflow をドキュメントとして厚く持ち、それらが code / tests と `CROSS_ARTIFACT` で結びつく。

### 6.3 ハブノード

`architecture_analysis_tool(mode="hubs", top_n=10)` は最高 degree のノードを返した。

| rank | qualified name | kind | in | out | total |
|---:|---|---|---:|---:|---:|
| 1 | `dagayn/cli/commands/build.py::handle` | Function | 3 | 158 | 161 |
| 2 | `tests/test_flows.py::TestFlows._add_func` | Function | 78 | 80 | 158 |
| 3 | `crates/dagayn-graph/src/tests.rs::stores_flows_and_reads_flow_inputs` | Function | 1 | 146 | 147 |
| 4 | `dagayn/incremental.py::incremental_update` | Function | 17 | 108 | 125 |
| 5 | `dagayn/refactor/dead_code.py::find_dead_code` | Function | 4 | 115 | 119 |
| 6 | `dagayn/search.py::hybrid_search` | Function | 45 | 73 | 118 |
| 7 | `docs/RUST-CORE-MIGRATION-WIP.md::phase-1-rust-graph-engine` | DocSection | 1 | 113 | 114 |
| 8 | `tests/test_integration_v2.py::TestV2Integration.test_full_pipeline` | Test | 56 | 55 | 111 |
| 9 | `dagayn/sap.py::compute_sap_metrics` | Function | 37 | 71 | 108 |
| 10 | `dagayn/skills.py::install_platform_configs` | Function | 33 | 74 | 107 |

ハブは変更時の blast radius が大きい。特に `build.py::handle`、`incremental_update`、`hybrid_search`、`compute_sap_metrics`、`install_platform_configs` は product behavior の中心なので、変更時には `review_tool(mode="impact")` と関連テストの確認が必要である。

### 6.4 ブリッジノード

`architecture_analysis_tool(mode="bridges", top_n=10)` は betweenness centrality 上位を返した。

| rank | qualified name | kind | betweenness |
|---:|---|---|---:|
| 1 | `tests/test_main.py::TestLongRunningToolsAreAsync.test_regression_guard_does_not_depend_on_fastmcp_internals` | Test | 0.009462 |
| 2 | `tests/test_integration_v2.py::TestV2Integration.test_full_pipeline` | Test | 0.005896 |
| 3 | `dagayn/search.py::hybrid_search` | Function | 0.002908 |
| 4 | `dagayn-vscode/test/sqlite.test.ts::it:isValid() returns false after close()@L509` | Test | 0.002477 |
| 5 | `dagayn/sap.py::compute_sap_metrics` | Function | 0.002349 |
| 6 | `dagayn-vscode/test/sqlite.test.ts::describe:SqliteReader@L248` | Test | 0.002256 |
| 7 | `dagayn/main.py::_tool` | Function | 0.002022 |
| 8 | `tests/test_cli_serve.py::test_serve_infers_local_embedding_from_existing_graph` | Test | 0.001952 |
| 9 | `tests/test_daemon.py::TestWatchDaemon.test_status_from_state_reports_alive` | Test | 0.001941 |
| 10 | `dagayn/daemon.py::WatchDaemon` | Class | 0.001891 |

上位に test が多いのは、この repo の test graph が複数領域を横断しているためである。production code では `hybrid_search`、`compute_sap_metrics`、`_tool`、`WatchDaemon` が chokepoint として見える。

### 6.5 知識ギャップ

`architecture_analysis_tool(mode="knowledge_gaps", top_n=10)` は raw total 2,637、`truncated=true` を返した。

| category | raw count | returned | threshold / note |
|---|---:|---:|---|
| isolated_nodes | 2,284 | 10 | degree <= 1 |
| thin_communities | 163 | 10 | size < 3 |
| untested_hotspots | 75 | 10 | non-file degree p95 >= 37 |
| single_file_communities | 115 | 10 | size >= 3 and one file |

Degree distribution は candidate positive degree count 2,234、p95 degree 37。untested hotspot からは test nodes、test-like file paths、Markdown sections が除外される。single-file communities では README、LICENSE、SECURITY、CODE_OF_CONDUCT のような自然な standalone document も noise として分類される。

上位 untested hotspot は次の通りである。

| qualified name | degree | evidence |
|---|---:|---|
| `dagayn/refactor/dead_code.py::find_dead_code` | 119 | p95 以上かつ direct `TESTED_BY` edge なし |
| `crates/dagayn-graph/src/lib.rs::GraphStore` | 106 | p95 以上かつ direct `TESTED_BY` edge なし |
| `crates/dagayn-parser/src/core.rs::RustOwnedParser.parse_file_in_repo` | 100 | p95 以上かつ direct `TESTED_BY` edge なし |
| `dagayn/tools/query.py::query_graph` | 95 | p95 以上かつ direct `TESTED_BY` edge なし |
| `crates/dagayn-graph/src/lib.rs::GraphStore.analyze_changes_json` | 92 | p95 以上かつ direct `TESTED_BY` edge なし |
| `dagayn/graph/core.py::GraphStore` | 89 | p95 以上かつ direct `TESTED_BY` edge なし |
| `diagrams/generate_diagrams.py::TC` | 84 | p95 以上かつ direct `TESTED_BY` edge なし |
| `dagayn/exports.py::export_obsidian_vault` | 83 | p95 以上かつ direct `TESTED_BY` edge なし |
| `crates/dagayn-graph/src/lib.rs::GraphStore.get_edges_by_endpoints` | 82 | p95 以上かつ direct `TESTED_BY` edge なし |
| `crates/dagayn-graph/src/lib.rs::betweenness_centrality` | 82 | p95 以上かつ direct `TESTED_BY` edge なし |

これは「テストが存在しない」と断定するものではない。graph 上の direct `TESTED_BY` edge がない、という構造的な signal である。Rust 側は Rust unit tests や Python parity tests が間接的に守っている可能性があるため、改善時には direct / transitive / heuristic coverage の見直しが必要である。

### 6.6 Surprising connections

`architecture_analysis_tool(mode="surprising_connections", top_n=10)` は cross-community、rare-community-pair、cross-language、degree-imbalance などの reason を持つ接続を返した。

上位例:

| source | target | edge | score | reasons |
|---|---|---|---:|---|
| `dagayn-analysis-report.md::64-ブリッジノード` | `dagayn/sap.py::compute_sap_metrics` | CROSS_ARTIFACT | 0.557 | cross-community, rare-community-pair, cross-language, degree-imbalance |
| `docs/refactoring-priority-report.md::hotspots-and-chokepoints` | `dagayn/sap.py::compute_sap_metrics` | CROSS_ARTIFACT | 0.557 | cross-community, rare-community-pair, cross-language, degree-imbalance |
| `docs/refactoring-priority-report.md::hotspots-and-chokepoints` | `dagayn/embeddings.py::get_provider` | CROSS_ARTIFACT | 0.553 | cross-community, rare-community-pair, cross-language, degree-imbalance |
| `docs/audits/mcp-tool-heuristic-review-2026-05-05.md::low-interface-count-is-high-but-mostly-purposeful` | `dagayn/communities.py::get_architecture_overview` | CROSS_ARTIFACT | 0.552 | cross-community, rare-community-pair, cross-language, degree-imbalance |
| `CHANGELOG.md::235--2026-04-30` | `CHANGELOG.md::performance` | CONTAINS | 0.551 | cross-community, rare-community-pair, peripheral-to-hub, degree-imbalance |

ここで重要なのは、surprising connection の多くが Markdown から code / test symbol への `CROSS_ARTIFACT` である点である。これはドキュメントと実装が結びついているという利点でもあり、cross-community noise の源泉でもある。Markdown レポートを書くときは、読みやすさのための backtick と、graph に残したい code obligation を意識的に分ける必要がある。

### 6.7 ADP / SDP / SAP

ADP / SDP / SAP は 2026-05-21 時点で `artifact_scope` を持つ。既定は `code` で、Markdown dependency directive は code architecture から除外される。従来の mixed graph を確認したい場合は `artifact_scope="all"`、文書構造だけを確認したい場合は `artifact_scope="docs"` を使う。

Code scope の ADP violations は package granularity で 1 件、`truncated=false`。

| cycle | length | weight | severity |
|---|---:|---:|---:|
| `dagayn/visualization -> dagayn` | 2 | 3 | 6 |

Code scope の SDP violations は 0 件、`truncated=false`。以前の `docs -> <root>` のような SDP warning は code design signal ではなく docs scope に分離された。

Docs scope の ADP / SDP は次の通りである。

| scope | finding | count / delta |
|---|---|---:|
| docs ADP | `<root> -> docs/plans -> docs` | severity 66 |
| docs ADP | `<root> -> docs` | severity 64 |
| docs ADP | `docs -> docs/plans` | severity 52 |
| docs SDP | `docs -> <root>` | delta 0.2 |

SAP violations は code scope で 7 件、`min_distance=0.5`、`truncated=false`。

| scope | distance | zone |
|---|---:|---|
| `dagayn-vscode/test` | 1.0 | uselessness |
| `dagayn/parser/_base` | 1.0 | pain |
| `dagayn-vscode/src/webview` | 1.0 | uselessness |
| `dagayn/graph` | 0.9 | pain |
| `dagayn/parser` | 0.8 | pain |
| `dagayn-vscode/src/views` | 0.6667 | pain |
| `dagayn` | 0.6667 | pain |

今回の変更により、ADP / SDP / SAP の解釈は明確になった。code scope の値はコード設計の signal として扱い、docs scope の値は Markdown 文書間の依存構造の signal として別枠で扱う。mixed graph は traceability の確認には有用だが、設計原則メトリクスの既定値にはしない。

### 6.8 実行フロー

`flow_tool(mode="list", detail_level="minimal", limit=10)` の上位は次の通り。

| rank | flow | criticality | node count |
|---:|---|---:|---:|
| 1 | activate | 0.665 | 20 |
| 2 | embed | 0.61 | 2 |
| 3 | embed_query | 0.61 | 2 |
| 4 | benchmark_review_workflow | 0.61 | 2 |
| 5 | benchmark_architecture_workflow | 0.61 | 2 |
| 6 | benchmark_debug_workflow | 0.61 | 2 |
| 7 | benchmark_onboard_workflow | 0.61 | 2 |
| 8 | benchmark_pre_merge_workflow | 0.61 | 2 |
| 9 | query_graph | 0.61 | 4 |
| 10 | main | 0.4967 | 6 |

`activate` は VS Code extension 側の activation flow と見られ、最も criticality が高い。MCP / CLI では `query_graph` と `main` が目立つ。変更レビュー時には affected flows がここに乗るかどうかが重要である。

### 6.9 大規模ファイル・クラス

`dagayn tool find_large_functions_tool --arg min_lines=80 --arg limit=10` は 10 件を返した。

| rank | node | kind | lines |
|---:|---|---|---:|
| 1 | `crates/dagayn-graph/src/lib.rs` | File | 4,095 |
| 2 | `crates/dagayn-graph/src/lib.rs::GraphStore` | Class | 2,936 |
| 3 | `crates/dagayn-parser/src/core_tests.rs` | File | 2,627 |
| 4 | `tests/test_skills.py` | File | 2,429 |
| 5 | `tests/test_tools.py` | File | 2,250 |
| 6 | `tests/test_parser.py` | File | 2,227 |
| 7 | `dagayn/graph/core.py` | File | 2,062 |
| 8 | `dagayn/graph/core.py::GraphStore` | Class | 1,936 |
| 9 | `dagayn/skills.py` | File | 1,936 |
| 10 | `dagayn/incremental.py` | File | 1,905 |

最大の構造課題は Rust `GraphStore` と Python `GraphStore` の二重の大きさである。Rust 移行途中のためある程度は避けられないが、責務境界、query APIs、migration、analysis JSON surface を分ける余地がある。テストファイルも大きいが、これは多言語 fixture と parity coverage の厚さを反映している。

## 7. 総合評価

<!-- derived-from #6-dagayn-自身のグラフ分析 -->

### 7.1 強み

dagayn の最大の強みは、エージェントが実際に使う workflow を中心に設計されている点である。`get_minimal_context_tool` から始め、review / architecture / query / search / refactor へ進む動線が明確で、tool surface も v3 で整理されている。

技術的にも、Rust-owned parser、SQLite graph store、FTS5、optional embedding、MCP dispatcher、CLI fallback、hooks、skills、docs directive が統合されている。単なる parser の集合ではなく、AI coding agent の操作環境として一貫している。

また、Markdown と Terraform を first-class に扱う点は実務上大きい。多くの repo では、仕様、運用、ADR、infra、CI、notebook がコードレビューの重要情報だが、通常のコード解析ツールでは周辺扱いされる。dagayn はそれらを graph のノードとエッジにする。

### 7.2 リスク

主なリスクは3つある。

1. 実装の中心部が大きい。Rust `GraphStore`、Python `GraphStore`、`incremental.py`、`main.py`、`skills.py` は変更時の認知負荷が高い。
2. query-time analysis がまだ重い領域を持つ。bridge centrality、embedding cosine、write-side N+1、flow hydration の一部などは改善余地がある。
3. Markdown cross artifact edge は便利だが、backtick の多用により surprising connection noise を生み得る。文書作成ルールと postprocess の signal quality が重要である。

### 7.3 優先改善提案

優先度高:

1. `dagayn/cli/commands/build.py::handle` の direct coverage evidence を改善する。degree 161 の top hub であるため、CLI build path の unit / integration coverage を graph 上でも見える形にする。
2. Runtime bridge centrality を postprocess score table へ移す。`bridge_scores` と `hub_scores` の永続化は MCP latency を大きく改善する。
3. Rust `GraphStore` と Python `GraphStore` の責務境界を文書化し、移行完了までの API 所有権を明確にする。
4. Embedding search の cosine loop を vectorized cache に置き換える。semantic search は対話的に呼ばれるため効果が大きい。
5. Markdown artifact ref resolver と write-side storage の batch 化を進める。

優先度中:

6. `parse_diff_ranges` の LRU cache を追加し、review sequence 内の repeated git diff を削減する。
7. `get_flow_by_id` と DFS traversal を batch 化する。
8. docs scope の ADP / SDP cycle を整理し、plan docs と root docs の依存方向を明確にする。code scope の ADP / SDP とは別枠で扱う。
9. `dagayn-analysis-report.md` のような分析文書では、意図的な `dagayn:` directive と説明用 backtick を使い分ける authoring guideline を追加する。
10. Per-MCP-tool latency benchmark を実装し、CI ではなくまず手元で baseline JSON を保存する。

優先度低:

11. single-file communities のうち、README 翻訳や security / code of conduct のような自然な孤立は noise として除外または分類する。
12. suffix LIKE の長期的解決として target name 正規化列を検討する。
13. mtime-based incremental skip を migration とともに導入する。

## 8. まとめ

<!-- derived-from #1-エグゼクティブサマリー -->
<!-- derived-from #2-dagayn-の思想 -->
<!-- derived-from #3-提供機能 -->
<!-- derived-from #4-実装構造 -->
<!-- derived-from #5-性能設計と現状 -->
<!-- derived-from #6-dagayn-自身のグラフ分析 -->
<!-- derived-from #7-総合評価 -->

dagayn は、コードベースを単なるファイル集合ではなく、AI エージェントが問い合わせ可能な knowledge graph として扱うプロダクトである。思想は「先に構造を読み、必要な source だけを読む」。提供機能は build / update / postprocess、compact MCP dispatcher、review / architecture / query / semantic search / refactor、Markdown / Terraform / notebook extraction、visualization / wiki / export、hooks / install / multi-repo に広がる。

実装は Python frontend と Rust backend の混合で、Rust-owned parser と Rust-backed graph store へ移行しつつ、Python 側が CLI、MCP、workflow、analysis orchestration を担っている。性能面では batch query、store cache、worker parser singleton、FTS5、optional embedding、thread offload が既に効いている一方、hub / bridge persistence、embedding vectorization、write-side batch、mtime skip、latency benchmark は今後の主要課題である。

dagayn 自身の current graph は 8,114 nodes / 47,431 edges / 388 files で、docs、tests、parser、graph store、MCP tool surface が大きな構造単位として現れている。分析結果は、tests-detect と tests-nodes の coupling、top hub としての build command handler、bridge としての search / SAP / MCP dispatch path、2,637 件の structural knowledge gaps、code scope では ADP 1 件・SDP 0 件、docs scope では ADP 3 件・SDP 1 件、7 件の SAP violations、大規模な Rust / Python GraphStore を示した。

結論として、dagayn はすでに「AI coding agent のための構造的コンテキスト基盤」として一貫した形を持っている。次の成熟段階では、query-time 重処理を postprocess / cache / Rust へ移し、graph signal の noise を authoring rule と resolver quality と artifact-scoped metrics で抑え、主要 hub の coverage evidence を graph 上でも明確にすることが重要である。
