# 日本語検索フィクスチャ

<!-- constrained-by ../../../docs/ARCHITECTURE.md#japanese-fts-quality-gates -->
<!-- derived-from ../../../docs/ARCHITECTURE.md#hybrid-search -->

dagayn のネイティブ FTS が対象にする混合モノレポを、CI で再現できるように
小さく切ったもの。Wikipedia 系の日本語 IR（MIRACL / JaQuAD / mMARCO-ja）や
ニュースコーパスは文書検索であり、DocSection / Function / Terraform ブロック
という dagayn のノードモデルを持たない。このリポジトリの `README.ja-JP.md`
は本物の日本語ドキュメントだが、検索語の衝突・活用形・CJK 識別子・
Terraform コメントを同時に持たない。

このディレクトリは次を同時に含む。

- 共有語 `検索` が NLP・UI・運用・インストール・インフラに散る Markdown
- 本文は `検索を行う`、クエリは `検索する` という活用のずれ
- 英日混在（`GraphStore`）と CJK 関数名（`ユーザー取得`）
- Python docstring 経由の `トークン検証` / `課金バッチ`
- Terraform リソースに日本語コメント
