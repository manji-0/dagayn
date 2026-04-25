# dagayn

`dagayn` は `code-review-graph` のフォークです。このリポジトリでは fork 本体を `dagayn` として扱います。

## 何ができるか

- リポジトリをローカルの知識グラフへ変換
- 変更影響範囲の解析
- MCP 経由で AI ツールに最小限の文脈を渡す
- Terraform、Markdown、Notebook を含むモノレポ解析

## クイックスタート

```bash
pip install git+https://github.com/manji-0/dagayn.git
dagayn install
dagayn build
dagayn status
```

## fork としての重点

- Terraform を第一級サポート
- Markdown 構造と依存コメントの抽出
- repo root 基準の相対 path を前提にした graph 運用
- `ruff` / `ty` ベースの CI

詳細は `README.md` と `docs/` を参照してください。
