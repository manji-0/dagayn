# Graph construction efficiency plan

<!-- supersedes ./PERFORMANCE-IMPROVEMENTS-WIP.md#3-benchmark-infrastructure-and-measurement-baselines -->
<!-- constrained-by ./RUST-CORE-MIGRATION-WIP.md -->
<!-- constrained-by ./ARCHITECTURE.md -->

> **Status:** Active. Graph construction (parse, SQLite write, derived
> postprocess) is measured and optimized separately from embedding / HNSW / ANN.

This plan is the next efficiency wave after the shipped N+1 and batch-write
work in [`PERFORMANCE-IMPROVEMENTS-WIP.md`](./PERFORMANCE-IMPROVEMENTS-WIP.md)
and the Rust writer in [`RUST-CORE-MIGRATION-WIP.md`](./RUST-CORE-MIGRATION-WIP.md).
It does not reopen those items. The remaining cost is in three places:
unmeasured axes, Python materialization of huge `GraphNode` lists, and
file-centric derived-graph incremental updates.

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
Taken from the construction half of PR #151; FTS Lindera/wakati and hybrid
search from that PR stay out of this wave. Reverse-CALLS stays in
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

## Out of scope

- Embedding generation, HNSW, and ANN (keep `embedding_materials` separate)
- New per-item PyO3 APIs
- Making full postprocess the incremental default
- 1M-node CI (manual / nightly via `DAGAYN_SCALE_1M`)
