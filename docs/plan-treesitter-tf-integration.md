# tree-sitter-terraform を code-review-graph に統合する計画

## Context

code-review-graph は現在 `.tf` ファイルを完全にスキップしている（`EXTENSION_TO_LANGUAGE` にエントリなし）。`tree-sitter-language-pack` には generic な HCL ベースの `terraform.abi3.so` が同梱されているが、全てが `block` ノードで表現されるため構造解析に不向き。

本プロジェクトの `tree-sitter-terraform` は `resource_block`, `data_block` 等 **13種のTerraform固有ノードタイプ** を持ち、code-review-graph のグラフ構築に最適。これを統合することで、Terraform コードの構造的な変更検知・影響分析が可能になる。

## 方式選定

| 方式 | メリット | デメリット |
|------|---------|-----------|
| A: tslp の terraform.abi3.so を差し替え + parser.py 修正 | 最小の変更量、tslp のローダーをそのまま利用 | uv upgrade で上書きされる |
| B: parser.py に独自ローダー追加 | tslp に依存しない | _get_parser の改変が必要 |
| C: code-review-graph を fork | 自由度が高い | メンテナンスコスト大 |
| D: generic HCL 文法で妥協 | .abi3.so 差し替え不要 | 構造情報が貧弱（本末転倒） |

**推奨: 方式 A** — .abi3.so 差し替え + parser.py への Terraform サポート追加。変更箇所が明確で、メンテナンススクリプトで upgrade 時の再適用も自動化できる。

## code-review-graph のアーキテクチャ

### パーサーローディング

- `tree_sitter_language_pack` (v0.13.0) 経由でネイティブ `.abi3.so` をロード
- `tslp.get_parser(language)` → `get_language()` → `get_binding()` → `importlib.import_module(f".bindings.{language_name}")`
- バインディング: `~/.local/share/uv/tools/code-review-graph/lib/python3.12/site-packages/tree_sitter_language_pack/bindings/`

### 言語登録（5つの辞書, parser.py 内）

1. **`EXTENSION_TO_LANGUAGE`** (line 74) — ファイル拡張子 → 言語名
2. **`_CLASS_TYPES`** (line 124) — クラス的ノードタイプ
3. **`_FUNCTION_TYPES`** (line 169) — 関数的ノードタイプ
4. **`_IMPORT_TYPES`** (line 219) — import的ノードタイプ
5. **`_CALL_TYPES`** (line 259) — 関数呼出ノードタイプ

### 抽出パイプライン

- `_extract_from_tree()` (line 1089) で再帰的にAST走査
- 言語固有エクストラクター: `_extract_elixir_constructs`, `_extract_solidity_constructs` 等
- `tags.scm` は **未使用**（手動AST走査方式）

### グラフスキーマ

- `NodeInfo`: kind = File | Class | Function | Type | Test
- `EdgeInfo`: kind = CALLS | IMPORTS_FROM | INHERITS | IMPLEMENTS | CONTAINS | TESTED_BY | DEPENDS_ON | REFERENCES

## tree-sitter-terraform の構造

### 13の固有ノードタイプ

| ノード | キーワード | フィールド |
|--------|-----------|-----------|
| `resource_block` | `resource` | type (string_lit), name (string_lit), body |
| `data_block` | `data` | type, name, body |
| `variable_block` | `variable` | name, body |
| `output_block` | `output` | name, body |
| `module_block` | `module` | name, body |
| `provider_block` | `provider` | name, body |
| `locals_block` | `locals` | body のみ |
| `terraform_block` | `terraform` | body のみ |
| `moved_block` | `moved` | body のみ |
| `import_block` | `import` | body のみ |
| `check_block` | `check` | name, body |
| `removed_block` | `removed` | body のみ |
| `ephemeral_block` | `ephemeral` | type, name, body |

### その他の重要ノード

- `function_call` — Terraform組込関数 (merge, lookup, toset等)
- `attribute` — key = value
- `block` — ネストブロック (lifecycle, provisioner, dynamic等)

## 実装ステップ

### Step 1: Python バインディングのビルド

`treesitter-tf` に Python 用の `terraform.abi3.so` をビルドする仕組みを追加。

**新規ファイル**: `treesitter-tf/bindings/python/binding.c`
- `tree_sitter_terraform()` を PyCapsule でラップし `PyInit_terraform` をエクスポート
- 既存の tslp バインディングと同一インターフェース

**ビルドコマンド**:
```bash
cc -shared -fPIC -DPy_LIMITED_API=0x03070000 \
  -I ../../src -I ../../src/tree_sitter \
  $(python3-config --includes) \
  -undefined dynamic_lookup \
  binding.c ../../src/parser.c ../../src/scanner.c \
  -o terraform.abi3.so
```

**検証**: Python で `import` して `resource_block` ノードが生成されることを確認。

### Step 2: .abi3.so の差し替え

ビルドした `terraform.abi3.so` を tslp のバインディングディレクトリにコピー:
```
~/.local/share/uv/tools/code-review-graph/lib/python3.12/site-packages/
  tree_sitter_language_pack/bindings/terraform.abi3.so
```

元のファイルは `.orig` としてバックアップ。

### Step 3: parser.py — 拡張子マッピング追加

**ファイル**: `code_review_graph/parser.py` (line ~120)

```python
# EXTENSION_TO_LANGUAGE に追加
".tf": "terraform",
".tfvars": "terraform",
```

### Step 4: parser.py — 型辞書追加

4つの辞書に空リストで登録（全て `_extract_terraform_constructs` で処理）:

```python
_CLASS_TYPES["terraform"] = []
_FUNCTION_TYPES["terraform"] = []
_IMPORT_TYPES["terraform"] = []
_CALL_TYPES["terraform"] = []
```

### Step 5: parser.py — `_extract_from_tree` にディスパッチ追加

line ~1153 付近（Elixir ディスパッチの後）に追加:

```python
# --- Terraform-specific constructs ---
if language == "terraform" and self._extract_terraform_constructs(
    child, node_type, source, language, file_path,
    nodes, edges, enclosing_class, enclosing_func,
    import_map, defined_names, _depth,
):
    continue
```

### Step 6: parser.py — `_extract_terraform_constructs` 実装

Elixir/Solidity のパターンに倣い、ノードタイプごとにグラフノード・エッジを生成:

| grammar ノードタイプ | graph kind | 命名規則 | extra |
|---------------------|-----------|---------|-------|
| `resource_block` | Class | `resource.{type}.{name}` | `terraform_kind: "resource"` |
| `data_block` | Class | `data.{type}.{name}` | `terraform_kind: "data"` |
| `ephemeral_block` | Class | `ephemeral.{type}.{name}` | `terraform_kind: "ephemeral"` |
| `module_block` | Class | `module.{name}` | `terraform_kind: "module"` |
| `provider_block` | Class | `provider.{name}` | `terraform_kind: "provider"` |
| `variable_block` | Function | `var.{name}` | `terraform_kind: "variable"` |
| `output_block` | Function | `output.{name}` | `terraform_kind: "output"` |
| `locals_block` | Function (per attr) | `local.{attr_name}` | `terraform_kind: "local"` |
| `terraform_block` | Class | `terraform` | `terraform_kind: "terraform"` |
| `check_block` | Test | `check.{name}` | `terraform_kind: "check"` |
| `import_block` | (edge only) | — | IMPORTS_FROM edge |
| `moved_block` | (edge only) | — | REFERENCES edge |
| `removed_block` | (edge only) | — | — |
| `function_call` (body内) | — | — | CALLS edge |

**エッジ生成ルール**:
- `module_block`: `source` 属性値から IMPORTS_FROM エッジ
- `function_call`: enclosing block → function_name に CALLS エッジ
- 全ブロック: File → block に CONTAINS エッジ
- `terraform_block` 内 `required_providers`: DEPENDS_ON エッジ

**名前抽出**: `string_lit` ノードの `.text` から引用符を除去するヘルパー `_strip_tf_string` を追加。

### Step 7: parser.py — `_collect_file_scope` にTerraform分岐追加

line ~2894 付近に追加。Terraform ブロックの名前を `defined_names` に収集:

```python
if language == "terraform":
    name = self._get_terraform_block_name(child)
    if name:
        defined_names.add(name)
    continue
```

### Step 8: メンテナンススクリプト

**新規ファイル**: `treesitter-tf/scripts/install-crg-plugin.sh`

1. `terraform.abi3.so` をビルド（未ビルドの場合）
2. tslp バインディングを差し替え
3. `parser.py` にパッチ適用（diff ベース）
4. スモークテスト実行

`uv tool upgrade code-review-graph` 後に再実行するだけで復元可能。

## 対象ファイル

**treesitter-tf 側（新規作成）**:
- `bindings/python/binding.c`
- `bindings/python/build.sh`
- `scripts/install-crg-plugin.sh`

**code-review-graph 側（修正）**:
- `parser.py` — 拡張子マッピング、型辞書、ディスパッチ、エクストラクター追加
- `tree_sitter_language_pack/bindings/terraform.abi3.so` — カスタム文法で差し替え

## 検証方法

1. **単体**: Python で `.tf` をパースし `resource_block` ノードが出ることを確認
2. **統合**: `examples/basic.tf` に対して `code-review-graph build` を実行し、ノード・エッジが生成されることを確認
3. **MCP**: `list_graph_stats_tool` で terraform ファイル数が表示、`semantic_search_nodes` で `resource.aws_instance.web` が検索可能であることを確認

## リスクと対策

| リスク | 対策 |
|-------|------|
| uv upgrade で .abi3.so が上書き | install-crg-plugin.sh で再適用 |
| parser.py の行番号ずれ | diff パッチ + バージョンピン |
| tree-sitter ABI バージョン不一致 | 現在 v15 で一致。更新時は `tree-sitter generate` で再生成 |
| macOS/Linux のクロスプラットフォーム | ビルドスクリプトでプラットフォーム検出 |

## 将来的な改善

- code-review-graph 本体への PR（parser.py の Terraform サポート部分）
- tree-sitter-language-pack への PR（カスタム文法の採用）
- これらが受理されればローカルパッチは不要になる
