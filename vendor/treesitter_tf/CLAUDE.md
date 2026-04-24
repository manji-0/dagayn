# tree-sitter-terraform

Terraform (HCL) 用の tree-sitter 文法。

## 開発コマンド

```bash
# 文法からパーサーを生成
devbox run generate
# または
devbox run -- tree-sitter generate

# テスト実行
devbox run test
# または
devbox run -- tree-sitter test

# ファイルのパース確認
devbox run -- tree-sitter parse examples/basic.tf

# クエリ実行
devbox run -- tree-sitter query queries/highlights.scm examples/basic.tf
```

## プロジェクト構造

- `grammar.js` — 文法定義 (中核ファイル)
- `src/scanner.c` — 外部スキャナー (文字列補間・ヒアドキュメント)
- `queries/` — Scheme クエリファイル
- `test/corpus/` — テストコーパス

## 設計方針

既存の `tree-sitter-hcl` と異なり、`resource_block`, `data_block` 等の
**Terraform固有のノードタイプ** を持つ。これにより code-review-graph での
構造解析が容易になる。
