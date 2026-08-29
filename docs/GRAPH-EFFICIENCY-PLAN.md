# Graph construction efficiency plan

<!-- supersedes ./PERFORMANCE-IMPROVEMENTS-WIP.md#3-benchmark-infrastructure-and-measurement-baselines -->
<!-- constrained-by ./RUST-CORE-MIGRATION-WIP.md -->
<!-- constrained-by ./ARCHITECTURE.md -->

> **Status:** Active. Graph construction (parse, SQLite write, derived
> postprocess) is measured and optimized separately from embedding / HNSW / ANN.

This plan is the next efficiency wave after the shipped N+1 and batch-write
work in [`PERFORMANCE-IMPROVEMENTS-WIP.md`](./PERFORMANCE-IMPROVEMENTS-WIP.md)
and the Rust writer in [`RUST-CORE-MIGRATION-WIP.md`](./RUST-CORE-MIGRATION-WIP.md).
It does not reopen those items. Waves 0–3 moved postprocess into Rust. The
remaining cost is **derived-data recomputation strategy**: full FTS rebuilds,
full Brandes, Leiden oversized splits that rescanned every edge, and
file-centric incremental updates that still refreshed whole derived tables.

## Already shipped (do not re-propose)

<!-- constrained-by ./PERFORMANCE-IMPROVEMENTS-WIP.md -->

- N+1 batching on risk, communities, BFS, dependents, and suggested questions
- Write batching: `store_file_batch` / `store_file_batch_json` / `begin_bulk_load`
- Rust-owned parse that does not return node/edge objects to Python
- `incremental_trace_flows` and `incremental_detect_communities` (now reverse-CALLS)
- Coarse JSON analysis (`analyze_changes_json`, `generate_suggested_questions_json`)
- Existing benches: `build_performance`, `nplusone_count`, `mcp_latency`

Writer-only Rust was slower when PyO3 conversion dominated. Coarse JSON
round-trips (`analyze_changes_json`) are the pattern this plan extends.

## Wave 0 — four-axis measurement

<!-- derived-from #already-shipped-do-not-re-propose -->

Benches live under `dagayn/eval/benchmarks/` and register in
`BENCHMARK_REGISTRY`:

| Benchmark | What it measures |
| --- | --- |
| `scale_performance` | Synthetic 10k (default) / 100k (`DAGAYN_SCALE_LARGE=1`) / 1M (`DAGAYN_SCALE_1M=1`, nightly only) |
| `query_performance` | `traverse_graph`, `get_impact_radius`, `get_affected_flows` p95 |

Axes recorded on `scale_performance` rows:

- operations: cold `full_build`, incremental 1-file update, query, MCP
- phase timers: `parse_write_ms`, `postprocess_ms` (embedding is a separate
  `status=skipped` row, never mixed into graph-construction throughput)
- `nodes_per_second`, `edges_per_second`, `sqlite_write_per_second`,
  `peak_rss_mb`, `changed_node_per_second`, query/MCP `p95_ms`

CI smoke is a tiny `scale_node_targets` unit test plus the existing
`nplusone_count` gate. A p95 20% regression comment is a follow-up, not a
blocker for this wave.

```
dagayn eval --benchmark scale_performance
dagayn eval --benchmark query_performance
DAGAYN_SCALE_LARGE=1 dagayn eval --benchmark scale_performance
```

## Wave 1 — compute in Rust, return JSON

<!-- constrained-by ./RUST-CORE-MIGRATION-WIP.md#frozen-decisions -->

Hot path to avoid:

```text
Rust GraphStore -> PyO3 -> list[GraphNode] / list[GraphEdge]
```

Coarse JSON APIs on the native store:

- `detect_entry_points_json` — ids and names only; Python hydrates those ids
- `rebuild_flows_json` — traces and stores flows in-process; Python gets `{count}`
- `incremental_trace_flows_json` — same for the incremental path
- `persist_centrality_scores` — used whenever the bound method exists

`rebuild_stored_flows` is the Python entry that prefers `rebuild_flows_json`
and falls back to `trace_flows` + `store_flows`. Remaining Python-owned
parsers keep the compact tuple-array `store_file_batch_json` wire format.

Full postprocess on a Rust store prefers `run_post_processing_json`
(`dagayn-postproc`): signatures, FTS, bridges, `rebuild_flows_json`, Leiden
communities, then centrality — without shipping flow lists back to Python.
Taken from the construction half of PR #151; hybrid search from that PR stays
out of this wave. Native FTS Lindera/wakati landed separately on the graph
store (`crates/dagayn-graph/src/japanese_fts.rs`) rather than in postproc.
Reverse-CALLS stays in
`crates/dagayn-graph/src/flow_trace.rs`, not a second BFS in postproc.

## Wave 2 — reverse-CALLS incremental flows

<!-- derived-from #wave-1--compute-in-rust-return-json -->

Affected entry points are the union of:

1. reverse `CALLS` from changed nodes to stored flow entries
2. new entry points discovered only in the changed files
3. file-membership / stale-path fallbacks from `get_affected_flow_ids`

Full-graph `detect_entry_points` is not used on the incremental path.
`refresh_flow_criticality` takes an optional `flow_ids` set: retraced flows
plus flows whose members gained `TESTED_BY` edges from the changed files.

Correctness fixture: an unchanged entry in `a.py` already on a flow; `b.py`
gains a new callee that `a.py` already calls. The new node is not in
`flow_memberships`, so file-centric incremental used to skip the flow.
Reverse CALLS retraces it.

## Wave 3 — bulk-load on large incremental writes

<!-- derived-from #wave-0--four-axis-measurement -->

Cold `full_build` already wraps `_StoreBulkLoad` (`begin_bulk_load` /
`finish_bulk_load`). Incremental parsing uses the same wrapper when the parse
batch is at least `BULK_LOAD_FILE_THRESHOLD` (64 files, matching the Rust
index-suspend threshold). Thicken this path further only if Wave 0 still shows
write as the dominant phase.

## Wave 4 — dirty-set derived data

<!-- derived-from #wave-1--compute-in-rust-return-json -->
<!-- derived-from #wave-2--reverse-calls-incremental-flows -->

Rustification of the pipeline is largely done. The next wins are algorithmic
and dirty-set driven, not more Python-to-Rust ports.

`run_post_processing_json(..., changed_files=None)` and `_run_postprocess`
now split FTS:

```text
full build / empty changed_files
    → rebuild_fts_index()          # DROP + scan all nodes

incremental (changed_files set)
    → sync_fts_for_file_paths()    # delete+reinsert those files only
```

If `nodes_fts` is missing, incremental sync falls back to a full rebuild so
the rest of the corpus is not left unindexed. The native full pipeline is
still used only when `full_rebuild` is true; 1-file updates keep reverse-CALLS
flows and incremental communities rather than re-entering the full Leiden +
Brandes pipeline.

Shipped in this wave (do-now items):

| Item | Change |
| --- | --- |
| FTS | Incremental postprocess is `O(nodes in changed files)`, not `O(N)` |
| Centrality N+1 | One `get_community_ids_by_node_ids` batch instead of per-node SQL |
| Brandes | `DenseGraph` (`Vec<Vec<usize>>`); generation-stamped `seen`; sample size `min(500, max(64, ceil(5√V)))` when `V > 5000` |
| Incremental centrality | Recompute Brandes on changed community + neighbors from a SQLite incident-edge subgraph; file deletes drop scores by `file_path` only |
| Shared snapshot | Full pipeline loads nodes/edges once for Leiden + centrality. Incremental postprocess does not take this snapshot |
| Leiden `split_oversized` | One edge scan into `edges_by_community` (`O(E)` instead of `O(K×E)`) |
| Incremental Leiden | Region detect + `replace_communities` when the dirty region is ≤ 50% of nodes. Nodes and induced edges come from `community_id IN (...)` SQL, not `get_all_nodes_filtered` + `get_all_edges` |
| Incremental flows | Reverse/forward CALLS (and reportable CROSS_ARTIFACT) hops expand a dirty qualified-name set in SQL, then only those nodes/edges are materialized. Falls back to the full trace graph above 50% of non-file nodes. Criticality refresh reuses that subgraph |
| Flow criticality | `SELECT ... FROM flows WHERE id IN (...)` when a dirty flow-id set is present |
| Bare-name | One `name → qualified_name` index instead of a SQL lookup per unresolved edge |
| Manifest bridges | Skip the repo-wide manifest walk when no `pyproject.toml` / `package.json` / `openapitools.json` changed |

Sample policy for `V > 5000` is `min(500, max(64, ceil(5√V)))`. Leiden resolution stays `1/log10(N)` (floor 0.05); oversized split remains the quality safety net.

Not in this wave:

- Dynamic exact betweenness (community-local Brandes is the approximation)
- Leiden `1/log10(N)` retune beyond the existing oversized-split safety net
- 1M-node CI (manual / nightly via `DAGAYN_SCALE_1M`)

## Out of scope

- Embedding generation, HNSW, and ANN (keep `embedding_materials` separate)
- New per-item PyO3 APIs
- Making full postprocess the incremental default
- 1M-node CI (manual / nightly via `DAGAYN_SCALE_1M`)
