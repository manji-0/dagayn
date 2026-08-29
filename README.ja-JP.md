# dagayn

> **DAG is All You Need** — 知識グラフを中心としたコードレビューとインパクト分析のアプローチ。

`dagayn` は `code-review-graph` のフォークです。ポリグロットリポジトリ、特にインフラ比重の高いコードベースを対象とした実践的な AI 支援レビューに特化しています。

上流のグラフ中心レビューモデルを継承しつつ、独自プロダクトとしてドキュメント・メンテナンスを行っています。主な差別化点は、Terraform の第一級サポート、フォーク固有のパーシングのためのコミット固定グラマー取得、より広範なプラットフォームインストールフロー、そしてアプリコード・ドキュメント・インフラが混在するモノレポへの対応強化です。

## 何ができるか

`dagayn` はリポジトリをローカル SQLite 知識グラフへと変換します。ファイル・シンボル・参照・呼び出しエッジ・インポート・テストリンク・コミュニティ・実行フローを記録します。AI エージェントはタスクのたびにリポジトリ全体を再読みするのではなく、このグラフに問い合わせることができます。

実用上のメリット:

- レビューコンテキストウィンドウの縮小
- 変更影響範囲の高速解析
- より安全なリファクタ
- 大規模リポジトリでのナビゲーション向上
- コード・ドキュメント・ノートブック・Terraform を一元的に扱えるワークフロー

## フォークとしての位置づけ

`dagayn` は明示的に `code-review-graph` のフォークです。

上流のドキュメントを正典とは扱いません。このリポジトリのガイド・例・コマンド説明はすべて `dagayn` 自身のために記述されています。

上流への帰属・原著者情報については [NOTICE](NOTICE) を参照してください。

## 主な特徴

- `.tf` および `.tfvars` の第一級 Terraform パーシング
- Markdown 構造と依存コメント、および `dagayn:` ドキュメントリンクの抽出
- `.ipynb` ノートブックパーシング
- ネイティブ日本語 FTS（Lindera IPADIC 形態素 + CJK bigram）。活用形クエリでも AND マッチする
- インクリメンタルグラフ更新、ウォッチモード、worktree sync、session prepare
- AI コーディングツール向け MCP サーバー
- インパクト半径・レビューコンテキスト・コミュニティ・フロー・リファクタのグラフクエリ
- ネイティブ Rust グラフストア、パーサ、FTS、フロー、後処理（`dagayn._core`）
- マルチリポジトリレジストリとデーモンワークフロー
- GraphML / Mermaid C4 / SVG / Cypher / Obsidian グラフエクスポート

## 対応言語・ファイル種別

主要アプリケーション言語に加え、リポジトリ付随フォーマットをカバーします。

主なもの:

- Python, JavaScript, TypeScript, TSX, Go, Rust, Java, C, C++, C#, Ruby, PHP, Kotlin, Swift, Scala, Solidity, Dart, Lua, Luau, Objective-C, Bash, Elixir, Zig, PowerShell, Julia, Perl, R, GDScript, Vue, Svelte, Astro
- Markdown
- Jupyter ノートブックと Databricks ノートブックソース/エクスポートをグラフ入力として解析
- Terraform

現在のカバレッジ一覧は `docs/FEATURES.md` と `docs/LLM-OPTIMIZED-REFERENCE.md` を参照してください。

## Terraform サポート

`dagayn` はアプリケーションコードと同等の第一級言語として Terraform を扱います。`.tf` と `.tfvars` の両ファイルを専用の Tree-sitter グラマーで解析します。

### 解析対象ブロック

| ブロック | 修飾名パターン | グラフ種別 |
|---|---|---|
| `resource "type" "name"` | `resource.type.name` | Class |
| `data "type" "name"` | `data.type.name` | Class |
| `variable "name"` | `var.name` | Function |
| `locals { key = … }` | `local.key`（属性ごと） | Function |
| `output "name"` | `output.name` | Function |
| `module "name"` | `module.name` | Class |
| `provider "name"` | `provider.name` | Class |
| `terraform {}` | `terraform` | Class |
| `check "name"` | `check.name` | Test |
| `ephemeral "type" "name"` | `ephemeral.type.name` | Class |
| `import {}` | エッジのみ | — |
| `moved {}` | エッジのみ | — |
| `removed {}` | エッジのみ | — |

### 生成されるエッジ種別

- **REFERENCES** — ブロック本体内の `var.x`, `local.x`, `module.x`, `output.x`, `provider.x`, `data.type.name`, `resource_type.name` 式。専用正規表現で抽出し、Terraform 組み込みプレフィックス（`count`, `each`, `path`, `self`, `terraform`）はスキップ。
- **CALLS** — `merge(…)` や `length(…)` などの組み込み関数呼び出し。
- **IMPORTS_FROM** — `module` ブロックと `terraform required_providers` の `source` 属性、および `import` ブロックのターゲット。
- **CONTAINS** — ファイルとそのファイル内で定義された各ブロックの包含関係。
- **DEPENDS_ON** — `terraform` ブロック内の `required_providers` バージョン制約。

### クロスモジュール解析

`module` ブロックの `source` がローカルパスを参照する場合、呼び出し元モジュールから対象ディレクトリへ `IMPORTS_FROM` エッジが記録されます。これにより、インパクト半径クエリがモジュール境界を越えることができます。

### `.tfvars` ファイル

変数値ファイル（`.tfvars`）は Terraform として解析されます。トップレベルの属性代入は `var.name` ノードとなり、`.tf` ファイル内の対応する `variable` ブロックへ REFERENCES エッジで接続されます。これにより変数データフローの完全な図がグラフに現れます。

## Markdown サポート

`dagayn` はソースコードと並行して Markdown ドキュメントからグラフノードとエッジを抽出します。散文アーキテクチャ決定と、それが説明するコードが同一グラフに現れます。

### 解析対象ノード種別

| 要素 | 修飾名パターン | グラフ種別 |
|---|---|---|
| ドキュメント | ファイルパス | File |
| `# 見出し` ～ `###### 見出し` | `file::slug` | DocSection |
| Setext H1 / H2（アンダーライン形式） | `file::slug` | DocSection |
| 見出し配下の段落・リスト・表・コード本文 | `file::slug--body-N` | DocBody |

見出しスラッグは GitHub Markdown 規約に従います: 小文字化、スペースとハイフンを `-` に統一、英数字以外を除去。同一ファイル内に重複する見出しがある場合は数値サフィックスが付きます（`slug-1`, `slug-2`, …）。

### 生成されるエッジ種別

- **CONTAINS** — 見出し階層。レベル 1 見出しの下に現れるレベル 2 見出しはその子として記録されます。
- **REFERENCES** — セクション間のインラインまたは参照スタイルリンク: `[text](./other.md#heading)` や `[text](#local-heading)`。ソースは含むセクション、ターゲットは `file::slug` 形式に解決されます。
- **IMPORTS_FROM** — クロスファイルリンク。リンクまたはディレクティブが別の Markdown ファイルを指す場合、現在のファイルからターゲットへ `IMPORTS_FROM` エッジが追加されます。
- **DEPENDS_ON** — ディレクティブコメント（下記参照）。

### ディレクティブコメント

ディレクティブコメントは、文書間依存関係を機械可読な形式で表現する構造化 HTML コメントです:

```markdown
<!-- constrained-by ./decisions/adr-001.md#context -->
<!-- blocked-by ./specs/open-issue.md -->
<!-- supersedes ./old-api.md#endpoint-design -->
<!-- derived-from ./research/background.md#findings -->
```

対応ディレクティブ種別:

| ディレクティブ | 意味 |
|---|---|
| `constrained-by` | このセクションの設計は参照先ドキュメント/セクションに制約される |
| `blocked-by` | 参照先の対応待ちで実装がブロックされている |
| `supersedes` | このドキュメントは参照先の内容を置き換える |
| `derived-from` | このセクションは参照先ソースから派生している |

各ディレクティブは **DEPENDS_ON** エッジになります。エッジ属性 `markdown_directive_kind` に具体的なディレクティブ種別が記録されます。

### ドキュメントディレクティブ（`dagayn:`）

<!-- derived-from ./docs/MARKDOWN-AUTHORING.md -->

`<!-- dagayn: implemented-by path::symbol -->` 形式の HTML コメントは、Markdown セクションからコード（または他の成果物）へ `CROSS_ARTIFACT` エッジを作ります。対応種別には `implemented-by`、`discusses-artifact`、`raises-issue-for` があります。コード側からは `# dagayn: implements docs/spec.md#Section` のような行コメントで逆方向を指せます。

契約の全体は [`docs/MARKDOWN-AUTHORING.md`](docs/MARKDOWN-AUTHORING.md) を参照してください。

### リンク解決

パーサが処理するリンク形式:

- `[text](./relative/path.md#section)` — ソースファイルからの相対パスで解決
- `[text](#local-section)` — 同じファイルのセクションに解決
- `[ref]: path` — 参照定義スタイル
- 外部 URL（`http://`, `https://`, `mailto:`）は無視

## インストール

```bash
pip install dagayn
```

永続的な分離 CLI 環境には `uv tool install` も使えます:

```bash
uv tool install dagayn
```

一回限りの分離 CLI なら `uvx` が便利です:

```bash
uvx --from dagayn dagayn --help
```

公開ホイールには対応ターゲット向けのコンパイル済み拡張が含まれるため、通常の PyPI インストールでは Git リポジトリからのビルドは不要です。

分離ツールインストールを好む場合は `pipx` も使えます。

## クイックスタート

```bash
dagayn install
dagayn build
dagayn status
```

`install` は対応 AI コーディングプラットフォームを自動検出し、適切な場所に MCP 設定を書き込みます。引数なしで TTY から実行すると埋め込みモードの選択を求められます（下記参照）。`-y` または非 TTY の stdin ではモードを明示する必要があります。

`build` で初期グラフを作成します。

既存のグラフデータベースを消してゼロから作り直すときは `dagayn build --force-full-build`（または `--force`）を使います。

`status` でグラフの存在確認と基本カウントが確認できます。

### インストールモードの選択

`dagayn install` は次の埋め込み戦略を第一級オプションとしてサポートします:

```bash
# 1. FTS のみ — 埋め込みなし、最速、モデルダウンロードなし。
dagayn install --mode fts-only

# 2. ローカル — 管理された BGE-M3 llama.cpp GGUF サイドカー。
dagayn install --mode local-embedding

# 3. 管理された Qwen3 llama.cpp GGUF サイドカー。
dagayn install --mode local-embedding-llama --preset low    # Qwen3-Embedding-0.6B (~1 GB)

# 4. リモート — OpenAI 互換 / Google / MiniMax クラウド埋め込み。
dagayn install --mode remote-embedding --provider openai
dagayn install --mode remote-embedding --provider google
dagayn install --mode remote-embedding --provider minimax
```

`--mode remote-embedding` では、AI コーディングツールを起動するシェルにプロバイダの環境変数を設定します（`openai` なら `CRG_OPENAI_API_KEY`、`CRG_OPENAI_BASE_URL`、`CRG_OPENAI_MODEL`）。MCP サーバーは起動時にそれらを継承し、生成される `dagayn serve --remote-embedding <provider>` エントリが MCP 検索にそのプロバイダを使います。正確な環境変数一覧はインストール時に表示されます。旧ショートカット（`--mode fts`、`--mode local`、`--mode local --preset low`、`--mode llama-qwen3`、`--mode remote`、`--local-embedding low`）は新しい明示的なモード名のエイリアスとして残っています。

### ネイティブグラフストア

<!-- derived-from ./docs/USAGE.md#native-graph-store -->

グラフストア、パーサ、FTS、フロー、後処理はネイティブ Rust 拡張（`dagayn._core`）で動きます。フォールバックする Python グラフエンジンはありません。`DAGAYN_BACKEND=python` は拒否されます。ハイブリッド検索のランキングと manifest-bridge 抽出は Python に残っています。

パーサ対象は Markdown、Terraform、Rust、Python/ノートブック、Bash、Go、Java、Ruby、C#、PHP、Kotlin、Swift、Scala、Solidity、Dart、Lua、Luau、C / C ヘッダ / Perl XS、C++、Objective-C、Elixir、GDScript、R、Julia、Perl、Vue、Svelte、Zig、PowerShell、対応スクリプト言語の shebang 付き拡張子なしファイル、および中核の JavaScript / JSX / TypeScript / TSX / Astro です:

```bash
dagayn build
dagayn update
```

ネイティブ拡張のないソースチェックアウトは明確に失敗します。

## よく使う CLI フロー

```bash
dagayn build
dagayn update
dagayn watch
dagayn worktree sync
dagayn detect-changes --base HEAD~1
dagayn visualize --format graphml
dagayn serve
```

### MCP ツールサーフェス

<!-- derived-from ./docs/COMMANDS.md#mcp-tool-surface -->

`dagayn serve` はコンパクトなデフォルトワークフローサーフェスを公開します。主要ツールに加え `review_tool`、`flow_tool`、`architecture_analysis_tool` などのディスパッチャがあるため、日常セッションで名前付きサーバープロファイルは不要です。

```bash
dagayn serve
dagayn serve --tools query_graph_tool,semantic_search_nodes_tool
```

`--tools` は公開ツールの一部を隠したいデプロイ向けの、カンマ区切りの正確な許可リストです。永続サーバー設定では同じ制御に `CRG_TOOLS` を使えます。

ツール応答は校正済みの guidance 契約を使います。互換フィールド（`status`、`summary`、`_hints`、`next_tool_suggestions`）は残り、レビュー・アーキテクチャ・フロー・リファクタ・検索・クエリ応答には `guidance`、`answerability`、`missingness` も含まれます。guidance 項目は `claim`、`evidence`、`confidence`、`missingness`、`action`、`reason_codes`、`counts` を持ち、エージェントはグラフ出力を判決ではなく証拠順位付きの手がかりとして扱えます。上位の推奨には `detail_level="minimal"`、裏付けセクション全体には `detail_level="standard"` を使います。`query_graph_tool` のゼロ件・未検出応答には `zero_result_reason`、`next_action`、`result_count`、`results`、`answerability`、`missingness` が含まれます。不在はソースやテストで確認するまでグラフ限定として扱ってください。ドキュメントブリッジ結果は証拠を `authored`、`extracted`、`heuristic_reachable` とラベル付けし、Markdown 追跡可能性を検証済み契約と混同しないようにします。

## レポートとエクスポート

`dagayn visualize` は静的なグラフ成果物を出力します。

- `--format` は必須で、`graphml`, `mermaid-c4`, `svg`, `cypher`, `obsidian` をサポート
- `mermaid-c4` は Mermaid の `C4Component` コードを出力し、ファイルをコンポーネント、クロスファイル依存を関係として集約します
- `svg` は matplotlib を使うため、必要なら eval extra を入れます: `pip install "dagayn[eval]"`
- Jupyter / Databricks ノートブックはレポート出力形式ではなく、グラフ入力として扱います

## AI プラットフォーム連携

`dagayn install` が MCP を設定できるターゲット:

- Codex
- Claude / Claude Code
- Cursor
- Windsurf
- Zed
- Continue
- OpenCode
- Antigravity
- Qwen Code
- Kiro
- Qoder
- Pi
- Hermes Agent

`--platform <name>` で特定プラットフォームのみに限定できます。
Codex ではグローバル `~/.codex/hooks.json` を作り、`~/.codex/config.toml` で hooks を有効にしてセッション中にグラフを更新します。Claude hooks はグローバル `~/.claude/settings.json` に書かれます。インストールされる git hooks はコミット前チェックで `dagayn update --skip-flows` を、コミット後に完全な `dagayn update` を実行します。ローカル埋め込みインストールモードを選んだ場合、生成される AI ツール更新 hooks も同じローカル埋め込みサイドカー引数を渡し、編集時の更新でベクトルを維持します。

プラットフォーム固有の指示ファイルも必要に応じてインストールされます:

- Claude は `~/.claude/CLAUDE.md`
- Codex は `~/.codex/AGENTS.md`
- OpenCode は `~/.config/opencode/AGENTS.md`
- Qoder は `QODER.md`
- `--platform qcoder` は `qoder` のエイリアスとして受け付けます

## グラフの使い方

典型的なレビューループ:

1. グラフをビルドまたは更新
2. 最小コンテキストまたは変更レビューを依頼
3. 影響を受けるファイルとシンボルのみを確認
4. 必要に応じてコミュニティ・フロー・クロスファイル参照を辿る
5. 編集後にインクリメンタルで更新

グラフはデフォルトで `.dagayn/` 以下にローカル保存されます。外部データベースは不要です。

## セマンティック検索と埋め込み

<!-- derived-from ./docs/ARCHITECTURE.md#hybrid-search -->

`semantic_search_nodes` は埋め込みがあるときは exact/name 検索と埋め込みベースの曖昧検索を組み合わせ、ないときは FTS のみにフォールバックします。どの検索経路が寄与したかは `search_mode` と結果ごとの `source` で報告します。ネイティブ FTS は日本語を Lindera IPADIC 形態素（辞書の基本形を含む）と重なり合う CJK bigram で分割するため、`検索する` のような活用形クエリでも `検索を行う` に AND マッチします。

FTS 索引、RRF マージ、リランキング、テキストモード、プロバイダ設定などの実装詳細は
[`docs/ARCHITECTURE.md#hybrid-search`](docs/ARCHITECTURE.md#hybrid-search) と
[`docs/LOCAL-EMBEDDINGS.md`](docs/LOCAL-EMBEDDINGS.md) を参照してください。

### 埋め込みモードとプロバイダ

| モード/プロバイダ | 実行場所 | 追加インストール | 必要な環境変数 |
|---|---|---|---|
| `--local-embedding` | 管理された localhost llama-server GGUF サイドカー | — | — |
| `openai` | クラウドまたはセルフホストゲートウェイ | — | `CRG_OPENAI_API_KEY`, `CRG_OPENAI_BASE_URL`, `CRG_OPENAI_MODEL` |
| `google` | Google Cloud | `dagayn[google-embeddings]` | `GOOGLE_API_KEY` |
| `minimax` | MiniMax Cloud | — | `MINIMAX_API_KEY` |

`openai` プロバイダは標準の `/v1/embeddings` スキーマを話すため、本物の OpenAI、Azure OpenAI、LiteLLM、vLLM、LocalAI、Ollama（OpenAI モード）などと同様のゲートウェイで動きます。`CRG_OPENAI_BASE_URL` が localhost を指すときはクラウド送信警告は自動で抑制されます。

ベクトル検索はデフォルトで Rust ネイティブのコサイン類似度バックエンドを使います。アーキテクチャ固有の SIMD（aarch64 では NEON、x86_64 では AVX と SSE フォールバック、それ以外はスカラー）でドット積を計算するため、外部 BLAS や Accelerate は不要です。ネイティブ検索が使えないときの Python 経路へのフォールバックは `DAGAYN_EMBEDDING_SEARCH_BACKEND=auto`、A/B テストには `DAGAYN_EMBEDDING_SEARCH_BACKEND=python` を設定します。Python 経路は numpy が入っていれば BLAS matmul（`pip install "dagayn[numpy]"`）、なければ純粋 Python のコサインループです。numpy は必須のハード依存ではありません。
`dagayn serve --local-embedding` は管理された llama.cpp GGUF サイドカーで BGE-M3 を動かし、加速を Python プロセスの外に置きます。古い sentence-transformers/PyTorch の `provider="local"` モードは削除済みです。ローカル埋め込みは管理された llama-server サイドカー、または別の localhost 上の OpenAI 互換エンドポイントを指します。

### 埋め込みの実行

MCP 経由で `embed_graph_tool` を呼ぶか、エージェントに `build_or_update_graph_tool` の後で呼ばせます。完全ローカルなら `dagayn build --local-embedding`、`dagayn update --local-embedding`、`dagayn serve --local-embedding` を優先します。これらは llama-server を管理し、内部で OpenAI 互換の localhost エンドポイントを使います。既に設定済みのプロバイダを使うときだけ `provider` と任意で `model` を渡します。

```
dagayn build --local-embedding
embed_graph_tool(provider="openai")   # 環境の CRG_OPENAI_* を読む
embed_graph_tool(provider="google")   # 環境の GOOGLE_API_KEY を読む
embed_graph_tool(provider="minimax")  # 環境の MINIMAX_API_KEY を読む
```

埋め込みは `.dagayn/graph.db` 内の `embeddings` テーブルに保存されます。プロバイダ、モデル、または `DAGAYN_EMBEDDING_TEXT_MODE` を切り替えるとキャッシュが分割され、次の呼び出しでその組の再埋め込みが走ります。

### 検索品質

現行の検索ベンチマークは 20 クエリです。exact/name と目的ベースの検索向けに 12 の標準クエリ、関数の振る舞いに関する目的・プロセスパターン散文向けに 8 の構造クエリがあります。

| 検索モード | クエリセット | MRR | Hit@5 | Hit@20 |
|---|---|---:|---:|---:|
| `material` テキスト | 全件 (20) | 0.5528 | 14/20 | 18/20 |
| `narrative` テキスト | 全件 (20) | 0.6671 | 18/20 | 19/20 |
| intent-routed | 全件 (20) | **0.6725** | **18/20** | **19/20** |

8 件の構造クエリでは、`narrative` は `material` に対して MRR が 0.2881 から 0.5875、Hit@5 が 3/8 から 7/8 に改善します。詳細なベンチマーク表、検索モードの注記、ローカルモデル比較は
[`docs/LOCAL-EMBEDDINGS.md#search-quality`](docs/LOCAL-EMBEDDINGS.md#search-quality)
を参照してください。

### プライバシーとクラウド送信

クラウドプロバイダへデータを送る前に、`dagayn` は stderr へ警告を出し、送信内容（関数名、docstring、ファイルパス）を列挙します。一度承認して以降の警告を抑えるには:

```bash
export CRG_ACCEPT_CLOUD_EMBEDDINGS=1
```

完全オフラインにするには `--local-embedding` を使い、dagayn に localhost の llama-server を管理させます。Python ML スタックや PyTorch 依存は不要です。

## ドキュメントマップ

- `docs/USAGE.md` — インストールと日常ワークフロー
- `docs/RECIPES.md` — watch、レジストリ/デーモン、埋め込みのコピーペーストレシピ
- `docs/COMMANDS.md` — CLI・MCP ツール・プロンプト・エクスポートアーティファクト
- `docs/FEATURES.md` — フォークの重点と上流との相違点
- `docs/ARCHITECTURE.md` — パーサ・ストレージ・後処理パイプライン
- `docs/SCHEMA.md` — ノード・エッジ・メタデータモデル
- `docs/MARKDOWN-AUTHORING.md` — グラフ対応 Markdown ディレクティブと `dagayn:` リンク
- `docs/SESSION-GRAPH-FRESHNESS.md` — session prepare、worktree、MCP 最初のツールの準備状態
- `docs/EVALUATION-SEMANTICS.md` — メトリクスの役割、プロファイル要約、ゲート、コスト、セマンティックレポート出力
- `docs/LOCAL-EMBEDDINGS.md` — 管理サイドカーとローカル埋め込みのセットアップ
- `docs/DAEMON-CONFIG.md` — レジストリとウォッチデーモンのファイル形式
- `docs/TROUBLESHOOTING.md` — 実践的な修正方法
- `docs/LLM-OPTIMIZED-REFERENCE.md` — 機械向けリファレンスセクション

## 現在の開発方針

このフォークが現在重視しているもの:

- インフラ対応レビュー、特に Terraform
- 混合言語モノレポ
- repo root 基準の安定した相対パスグラフ登録
- ターミナルおよびエディタエージェント向け MCP ファーストワークフロー
- ホステッドサービスなしの再現可能なローカル解析

## セキュリティとプライバシー

`dagayn` はローカルグラフストレージを前提に設計されています。一部のオプションの埋め込みプロバイダーはリモート API を呼び出す場合がありますが、それらはオプトイン形式で別途ドキュメント化されています。

詳細は `SECURITY.md` と `docs/LEGAL.md` を参照してください。

## コントリビュート

開発セットアップ・検証コマンド・コントリビュートルールは `CONTRIBUTING.md` を参照してください。

## ライセンス

MIT。`LICENSE` を参照してください。
