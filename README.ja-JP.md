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
- Markdown 構造と依存コメントを含む依存関係抽出
- `.ipynb` ノートブックパーシング
- インクリメンタルグラフ更新とウォッチモード
- AI コーディングツール向け MCP サーバー
- インパクト半径・レビューコンテキスト・コミュニティ・フロー・リファクタのグラフクエリ
- マルチリポジトリレジストリとデーモンワークフロー
- インタラクティブ可視化と GraphML / Mermaid C4 / SVG / Cypher / Obsidian エクスポート

## 対応言語・ファイル種別

主要アプリケーション言語に加え、リポジトリ付随フォーマットをカバーします。

主なもの:

- Python, JavaScript, TypeScript, TSX, Go, Rust, Java, C#, Ruby, PHP, Kotlin, Swift, Scala, Solidity, Dart, Lua, Luau, Objective-C, Bash, Elixir, Zig, PowerShell, Julia, GDScript, Vue, Svelte, Astro, ReScript
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
| `# 見出し` ～ `###### 見出し` | `file::slug` | Class |
| Setext H1 / H2（アンダーライン形式） | `file::slug` | Class |

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

### リンク解決

パーサが処理するリンク形式:

- `[text](./relative/path.md#section)` — ソースファイルからの相対パスで解決
- `[text](#local-section)` — 同じファイルのセクションに解決
- `[ref]: path` — 参照定義スタイル
- 外部 URL（`http://`, `https://`, `mailto:`）は無視

## インストール

```bash
pip install git+https://github.com/manji-0/dagayn.git
```

分離ツールインストールを好む場合は `pipx` も使えます。

## クイックスタート

```bash
dagayn install
dagayn build
dagayn status
```

`install` は対応 AI コーディングプラットフォームを自動検出し、適切な場所に MCP 設定を書き込みます。

`build` で初期グラフを作成します。

`status` でグラフの存在確認と基本カウントが確認できます。

## よく使う CLI フロー

```bash
dagayn build
dagayn update
dagayn watch
dagayn detect-changes --base HEAD~1
dagayn visualize --serve
dagayn serve
```

## レポート / エクスポート出力

`dagayn visualize` が現行のグラフ用レポート / エクスポート面です。

- 既定出力は `.dagayn/graph.html` のインタラクティブ HTML レポート
- HTML 表示は `--mode auto|full|community|file` をサポート
- `--format` は `html`, `graphml`, `mermaid-c4`, `svg`, `cypher`, `obsidian` をサポート
- `mermaid-c4` は Mermaid の `C4Component` コードを出力し、ファイルをコンポーネント、クロスファイル依存を関係として集約します
- `svg` は matplotlib を使うため、必要なら eval extra を入れます: `pip install "dagayn[eval] @ git+https://github.com/manji-0/dagayn.git"`
- この fork に Graphviz / DOT の組み込み出力はありません
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

## グラフの使い方

典型的なレビューループ:

1. グラフをビルドまたは更新
2. 最小コンテキストまたは変更レビューを依頼
3. 影響を受けるファイルとシンボルのみを確認
4. 必要に応じてコミュニティ・フロー・クロスファイル参照を辿る
5. 編集後にインクリメンタルで更新

グラフはデフォルトで `.dagayn/` 以下にローカル保存されます。外部データベースは不要です。

## ドキュメントマップ

- `docs/USAGE.md` — インストールと日常ワークフロー
- `docs/COMMANDS.md` — CLI・MCP ツール・プロンプト・エクスポートアーティファクト
- `docs/FEATURES.md` — フォークの重点と上流との相違点
- `docs/architecture.md` — パーサ・ストレージ・後処理パイプライン
- `docs/schema.md` — ノード・エッジ・メタデータモデル
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
