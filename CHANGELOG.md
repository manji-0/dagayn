# Changelog

`dagayn` の変更履歴です。

## 0.1.0 — Initial dagayn fork

- `code-review-graph` (作者: Tirth Kanani, https://github.com/tirth8205/code-review-graph) をフォークし、`dagayn` として独立運用を開始
- 第一級の Terraform 解析サポート
- Markdown 構造解析と directive ベースの依存抽出
- グラフ登録パスをリポジトリルート相対に統一
- パッケージ名・CLI・ストレージディレクトリを `dagayn` に統一
- 原作者表記は [NOTICE](NOTICE) を参照

## 2.3.5 — 2026-04-30

### Performance

- `get_impact_radius_sql`: replace `get_edges_among` with temp table JOIN (B5)
- `find_dependents`: batch frontier lookups, reducing N+1 to 3 queries/hop
- `store_file_batch`: batch processing in `full_build` and `incremental_update` (A2)
- Add mtime-based incremental skip (migration v11, `mtime_ns` column)
- `find_dead_code`: batch-preload edges, reducing O(N) SQL to O(1) lookups
- Batch postprocessing `CROSS_ARTIFACT` ref resolution (N+1 → 3 queries)
- `get_local_subgraph`: batch DFS traversal via recursive CTE
- Vectorize embedding search with numpy BLAS and process-level matrix cache
- Batch flow INSERT/DELETE with `executemany` and `IN (?,…)`
- Batch community INSERT/UPDATE in `_clear_and_store_communities`
- migration v10: add `idx_nodes_parent_name(parent_name, name)` index
- Cache `parse_diff_ranges` and `embed_query` results with `lru_cache`
- Pin `EmbeddingStore` per-process; batch `embed_nodes` hash lookups
- Bulk-insert nodes/edges with `executemany`, use `RETURNING id` in upsert
- SQLite PRAGMA tuning, parser worker singleton, token-estimate fix

## 2.3.4 — 2026-04-28

### Features

- Add `writing/reading-markdown-document` skills with global install support (#1)

### Performance

- Pin sqlite connection, batch node lookups, add profiler scaffold (#2)

### Refactoring

- Split `parser/core.py` (3476 → 1269 lines) into focused modules
- Split `extension.ts::registerCommands` into feature modules (Phase 3-1)
- Extract `graphWebview` HTML/CSS to static assets (Phase 3-2)
- Add Protocol classes to lift SAP abstractness (Phase 4)

### Fixes

- Apply ruff-format to `parser/_protocol.py`

## Unreleased
