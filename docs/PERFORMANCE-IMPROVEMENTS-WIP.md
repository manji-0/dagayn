# Performance improvements — WIP plan

> **Status:** Work in progress — problem areas identified as of 2026-04-29. No implementation started.
>
> **Related:** `RUST-CORE-MIGRATION-WIP.md`, `DAEMON-CONFIG.md`, `ROADMAP.md`

## Purpose

This document describes three independent improvement areas found during an algorithmic audit of the dagayn codebase:

1. **N+1 query patterns** — several code paths issue one SQL statement per graph node rather than batching.
2. **Connection management** — the MCP server opens and closes a `GraphStore` instance on every tool call, preventing in-process caching.
3. **Benchmark and profiling infrastructure** — no CPU profiler or per-tool latency baseline exists today, making it impossible to measure the effect of any improvement.

These are **Python-layer improvements** that can be shipped independently of and in parallel with the Rust core migration described in `RUST-CORE-MIGRATION-WIP.md`. Where a hotspot is already scheduled for Rust replacement, the Python-layer fix is noted as short-term scaffolding.

## Non-goals

- Full rewrites of any module
- HNSW/FAISS-based ANN index (tracked separately as a stretch goal)
- CI performance regression gates (noted as a future possibility, not required here)
- Changes to the graph schema or MCP tool API surface

---

## 1. N+1 query patterns

### 1.1 What is the problem

An N+1 pattern arises when code fetches a list of N items and then issues one additional database query **per item** in a loop instead of fetching all needed data in a single batched query. In dagayn every additional query is a round-trip through the SQLite connection layer, even though SQLite is in-process. The cost is not network latency but function call overhead, row-by-row deserialization, and the inability for the query planner to optimise across items.

All line numbers refer to the codebase state at 2026-04-29.

### 1.2 Known occurrences

#### `compute_risk_score` — `dagayn/changes.py:217`

`compute_risk_score` is called once per changed node inside `analyze_changes`. For each node it issues at minimum four independent SQL statements:

| Call | Statement count |
|---|---|
| `store.get_flow_criticalities_for_node(node.id)` | 1 |
| `store.count_flow_memberships(node.id)` | 1 |
| `store.get_edges_by_target(node.qualified_name)` | 1 |
| `store.get_node_community_id(node.id)` | 1 |
| `store.get_transitive_tests(node.qualified_name)` | 1+ (recursive CTE or per-hop) |

For a diff touching 20 nodes that is 80–100 SQL statements. The `get_edges_by_target` call is made a second time at `changes.py:335` for the test-gap check, duplicating at least one query per node.

**Fix direction:** Pre-fetch all required data for the full set of changed nodes before entering the per-node scoring loop.

```
node_ids  = [n.id  for n in changed_nodes]
node_qns  = [n.qualified_name for n in changed_nodes]

flow_data   = store.get_flow_criticalities_for_nodes(node_ids)       # batch
membership  = store.count_flow_memberships_for_nodes(node_ids)       # batch
inbound     = store.get_edges_by_targets(node_qns)                   # batch WHERE target IN (...)
community   = store.get_community_ids_for_nodes(node_ids)            # batch
test_data   = store.get_transitive_tests_for_nodes(node_qns)         # batch or CTE
```

`compute_risk_score` then receives pre-fetched dicts and performs no additional I/O.

---

#### `get_communities` — `dagayn/communities.py:742–772`

After fetching all community rows in one query, the function calls `store.get_community_member_qns(row["id"])` inside a Python `for` loop — one SELECT per community.

```python
for row in rows:
    member_qns = [_sanitize_name(qn) for qn in store.get_community_member_qns(row["id"])]
```

With K communities this is K+1 total queries.

**Fix direction:** Replace the inner call with a single query that returns all members at once, grouped in Python.

```sql
SELECT community_id, qualified_name
FROM nodes
WHERE community_id IS NOT NULL
ORDER BY community_id
```

Group by `community_id` in Python using `itertools.groupby` or a `defaultdict`. This collapses K queries into 1.

---

#### `get_affected_flows` / `get_flow_by_id` — `dagayn/flows.py:654, 609`

`get_affected_flows` resolves a set of flow IDs, then calls `get_flow_by_id` once per ID (`:683`). Inside `get_flow_by_id`, each path step calls `store.get_node_by_id` (`:625`), making this quadratic in the number of flows × steps.

**Fix direction:** Replace the `get_flow_by_id` loop with a single `WHERE id IN (?,...)` query for flow rows, then a single `WHERE qualified_name IN (?,...)` query for all node data needed across all flows.

---

#### `traverse_graph_func` — `dagayn/tools/query.py:579–624`

The BFS/DFS loop issues three SQL statements per visited node:

```python
node      = store.get_node(current_qn)           # 1 query
out_edges = store.get_edges_by_source(current_qn) # 1 query
in_edges  = store.get_edges_by_target(current_qn) # 1 query
```

With depth 3 on a graph where each node has an average fan-out of 5, this can visit hundreds of nodes and issue hundreds of SQL statements.

There is also a secondary issue: `queue.pop(0)` on a plain `list` is O(N) per pop. For BFS the queue should be a `collections.deque` with `popleft()`.

**Fix direction — deque:** One-line fix, `queue: deque[tuple[str, int]]` with `popleft()`.

**Fix direction — batch edge fetch:** Collect the entire next-depth frontier before visiting it, then fetch all edges for the frontier in one query:

```sql
SELECT * FROM edges WHERE source IN (?,…) OR target IN (?,…)
```

This pattern is already used in the SQL-based `get_impact_radius` (`graph/core.py:667`) and can be reused here.

---

#### `_single_hop_dependents` — `dagayn/incremental.py:710`

`_single_hop_dependents` fetches the file's edges once (`:715`), then calls `store.get_edges_by_target(node.qualified_name)` inside a node-level loop (`:720`). With F files each containing N nodes, the incremental update path issues up to F×N queries.

**Fix direction:** Replace the inner loop with a single `WHERE target IN (?,...)` query over all qualified names in the file, matching the pattern already used in `get_impact_radius_sql`.

---

#### `generate_suggested_questions` — `dagayn/analysis.py:314–346`

This function calls four analysis helpers in sequence:

```python
bridge_nodes    = find_bridge_nodes(store, ...)    # get_all_edges + NetworkX build
hub_nodes       = find_hub_nodes(store, ...)       # get_all_edges + get_all_nodes
connections     = find_surprising_connections(...) # get_all_edges
gaps            = find_knowledge_gaps(store, ...)  # get_all_edges + get_all_nodes
```

Each of `find_hub_nodes`, `find_bridge_nodes`, `find_surprising_connections`, and `find_knowledge_gaps` independently calls `store.get_all_edges()` and `store.get_all_nodes()`. On a 25 000-edge graph this is five full table scans where one would do.

`analysis.py:346` then calls `store.get_all_edges()` a fifth time to build a `tested` set.

**Fix direction:** Compute a shared snapshot `(edges, nodes, community_map, degree_map)` once at the top of `generate_suggested_questions` and inject it into each helper as an optional parameter. Each helper retains its existing signature for callers that do not pass a snapshot.

---

### 1.3 Shared fix patterns

Three patterns cover all cases above.

**Batch IN query**

```python
qmarks = ",".join("?" * len(ids))
rows = conn.execute(f"SELECT ... WHERE id IN ({qmarks})", ids).fetchall()
```

The existing codebase already uses `IN (?,...)` batches with a chunk size of 450 to stay within SQLite's variable limit. New batch calls should follow the same convention.

**Snapshot injection**

For analysis functions that are always called together (e.g., inside `generate_suggested_questions`), add an optional `snapshot` parameter:

```python
def find_hub_nodes(
    store: GraphStore,
    top_n: int = 10,
    *,
    snapshot: GraphSnapshot | None = None,
) -> list[dict]: ...
```

`GraphSnapshot` is a lightweight named tuple or dataclass:

```python
@dataclasses.dataclass(frozen=True)
class GraphSnapshot:
    edges: list[GraphEdge]
    nodes: list[GraphNode]
    community_map: dict[str, int | None]
    in_degree: Counter[str]
    out_degree: Counter[str]
```

**SQLite Recursive CTE for transitive queries**

`get_transitive_tests` can be expressed as a single recursive CTE, mirroring the existing `get_impact_radius_sql` (`graph/core.py:667`). A batch variant takes a set of seed qualified names and returns all reachable test nodes in one query.

### 1.4 Verification

For each fix:

1. Run `uv run pytest --tb=short -q` and confirm no test regressions.
2. Add an assertion in `eval/benchmarks/nplusone_count.py` (see section 3) that the per-tool SQL count does not exceed the newly established baseline.
3. For `compute_risk_score`, add a unit test that constructs a mock `GraphStore` counting calls and asserts that scoring 10 nodes issues at most 10 SQL statements (one batch per query type).

---

## 2. Connection management

### 2.1 Current behaviour

Every MCP tool handler calls `_get_store()` (`dagayn/tools/_common.py:214`), which opens a new `GraphStore` (and therefore a new `sqlite3.Connection`) and closes it in a `finally` block:

```python
def _get_store(repo_root: str | None = None) -> tuple[GraphStore, Path]:
    root = _validate_repo_root(...) if repo_root else find_project_root()
    db_path = get_db_path(root)
    return GraphStore(db_path), root
```

The `GraphStore` constructor runs `_init_schema()` (all `CREATE TABLE IF NOT EXISTS` + index statements) on every instantiation. More importantly, `GraphStore` carries two in-process caches:

- `_nxg_cache: nx.DiGraph | None` (`graph/core.py:128`) — the full NetworkX DiGraph built from all edges.
- `_cache_lock: threading.Lock` — guards `_nxg_cache` during invalidation.

Because `_nxg_cache` is an **instance variable**, it is discarded every time `store.close()` is called. Any tool that calls `find_bridge_nodes` or `_build_networkx_graph` directly will rebuild the full NetworkX graph from scratch, loading all edges into Python memory, even if the underlying database has not changed since the previous request.

### 2.2 Impact

`find_bridge_nodes` (`dagayn/analysis.py:60`) calls `store._build_networkx_graph()` which fetches all edges from SQLite, constructs a `nx.DiGraph`, and then runs `nx.betweenness_centrality`. On a graph with 5 000+ nodes the betweenness calculation uses k=500 sample approximation. This is the most expensive operation in the analysis module, and it runs from scratch on every call to `get_hub_nodes_tool`, `get_bridge_nodes_tool`, or `get_suggested_questions_tool`.

### 2.3 Fix direction A — process-level store cache

Introduce a module-level cache in `dagayn/tools/_common.py` keyed on the resolved `db_path`:

```python
_store_cache: dict[Path, tuple[GraphStore, float]] = {}  # db_path -> (store, mtime)
_store_lock = threading.Lock()

def _get_store(repo_root: str | None = None) -> tuple[GraphStore, Path]:
    root = _validate_repo_root(...) if repo_root else find_project_root()
    db_path = get_db_path(root)
    mtime = db_path.stat().st_mtime
    with _store_lock:
        if db_path in _store_cache:
            cached_store, cached_mtime = _store_cache[db_path]
            if cached_mtime == mtime:
                return cached_store, root
            cached_store.close()
        store = GraphStore(db_path)
        _store_cache[db_path] = (store, mtime)
    return store, root
```

The `finally: store.close()` blocks in all tool handlers must be removed (or made conditional) once the cache is in place.

**Staleness detection:** `st_mtime` of the SQLite file changes on every write commit, which is sufficient. An alternative is `PRAGMA user_version` — the migration layer already increments this — but `st_mtime` is cheaper.

**Thread safety:** The MCP server runs handlers concurrently. The cache dict is protected by `_store_lock`. `sqlite3` connections with `check_same_thread=False` can be shared across threads for read-only access, which covers all MCP query tools. Write tools (build, update) should continue to open their own transient `GraphStore`.

**Open question:** Whether the MCP server process is single-threaded (asyncio event loop) or multi-threaded. If single-threaded, `_store_lock` is unnecessary but harmless.

### 2.4 Fix direction B — persist hub and bridge scores

For `find_bridge_nodes` and `find_hub_nodes` the real fix is to remove the runtime calculation entirely and read pre-computed values from storage, as already done for Leiden community detection.

Proposed schema additions (migration v9):

```sql
CREATE TABLE IF NOT EXISTS hub_scores (
    qualified_name TEXT PRIMARY KEY,
    in_degree      INTEGER NOT NULL,
    out_degree     INTEGER NOT NULL,
    total_degree   INTEGER NOT NULL,
    computed_at    TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS bridge_scores (
    qualified_name TEXT PRIMARY KEY,
    betweenness    REAL NOT NULL,
    computed_at    TEXT NOT NULL
);
```

`dagayn/postprocessing.py` (or a new `postprocessing/centrality.py`) runs `find_hub_nodes` and `find_bridge_nodes` once during the postprocess phase and writes results to these tables. The MCP tools then read from the tables instead of recomputing.

Invalidation: postprocess is re-run on `dagayn build` and `dagayn update --post`. The `computed_at` column allows the tool to warn if scores are stale relative to the graph's last modification time.

**Relation to Rust migration:** `RUST-CORE-MIGRATION-WIP.md` targets postprocessing as Phase 2 of the migration. Pre-computing hub/bridge scores in Python first, then porting the computation to Rust in Phase 2, is the path of least resistance.

### 2.5 Relation to the watch daemon

`DAEMON-CONFIG.md` describes a file-watching daemon that triggers incremental rebuilds. When the daemon completes a build it should invalidate the process-level store cache (Fix A) by touching the db file or incrementing `PRAGMA user_version`. This ensures MCP tools pick up the new graph on the next call without a process restart.

---

## 3. Benchmark infrastructure and measurement baselines

### 3.1 Current state

The only existing performance measurement is `dagayn/eval/benchmarks/build_performance.py`, which times the `build`, `flows`, `communities`, and `search` phases and reports `nodes_per_second`. It is invoked via `dagayn eval`.

There is no:

- CPU or memory profiler integration
- Per-MCP-tool latency measurement
- SQL query count assertion (so N+1 regressions are invisible)
- Established numeric baselines that fail a test when crossed

### 3.2 Proposed additions

#### 3.2.1 CPU profiler CLI (`dagayn profile`)

Add `pyinstrument` as an optional development dependency:

```toml
[tool.uv.optional-dependencies]
dev = [
  "pyinstrument>=4.6",
  ...
]
```

Add a `profile` subcommand to the CLI that wraps any other `dagayn` command with profiling enabled and writes an HTML report:

```
dagayn profile build
dagayn profile search "authentication middleware"
dagayn profile mcp-tool detect_changes
```

Implementation: wrap the target command in a `pyinstrument.Profiler` context manager, write `profile_<subcommand>_<timestamp>.html` to a configurable output directory (default: `.dagayn/profiles/`).

#### 3.2.2 SQL query counter (`eval/benchmarks/nplusone_count.py`)

Use `sqlite3.set_trace_callback` to count the number of SQL statements issued during a single tool call. Establish a baseline count for each tool and assert it does not regress.

Example structure:

```python
class SQLCounter:
    def __init__(self, conn: sqlite3.Connection) -> None:
        self.count = 0
        conn.set_trace_callback(self._on_statement)

    def _on_statement(self, statement: str) -> None:
        self.count += 1
```

Baseline table (to be populated after the N+1 fixes in section 1 are applied):

| Tool | Max SQL statements per call (baseline) |
|---|---|
| `detect_changes` (10 changed nodes) | TBD after fix |
| `get_impact_radius` | TBD after fix |
| `traverse_graph` (depth 3) | TBD after fix |
| `get_hub_nodes` | TBD (should be 1 after Fix B) |
| `get_bridge_nodes` | TBD (should be 1 after Fix B) |
| `list_communities` | TBD after fix |
| `get_affected_flows` | TBD after fix |

Once baselines are set, any future change that increases the count for a tool causes the benchmark to report a regression.

#### 3.2.3 MCP tool latency (`eval/benchmarks/mcp_latency.py`)

A new benchmark that calls each MCP tool function directly (not over the MCP transport) on a representative graph (e.g. the dagayn repository itself) and records wall-clock time with `timeit.repeat`.

Proposed latency targets (rough, to be calibrated once Fix A and Fix B are done):

| Tool | p95 target |
|---|---|
| `semantic_search_nodes` | < 500 ms |
| `detect_changes` | < 300 ms |
| `get_impact_radius` | < 100 ms |
| `get_review_context` | < 200 ms |
| `get_hub_nodes` | < 50 ms (post Fix B) |
| `get_bridge_nodes` | < 50 ms (post Fix B) |
| `traverse_graph` (depth 3) | < 150 ms |

These are not hard acceptance criteria yet; they are a starting point for discussion and will be revised after initial measurement.

#### 3.2.4 Query performance (`eval/benchmarks/query_performance.py`)

Extend the existing `build_performance.py` pattern with traversal-specific benchmarks:

- `traverse_graph` at depth 1, 3, 6 — measure node count and wall time, assert sub-linear growth after batch fix
- `get_impact_radius` at depth 1, 3, 6
- `get_affected_flows` for 1, 5, 20 changed files

### 3.3 Running the benchmarks

All benchmarks should be runnable via:

```
dagayn eval --benchmark all
dagayn eval --benchmark nplusone
dagayn eval --benchmark latency
```

and produce a machine-readable JSON output alongside human-readable console output, so results can be committed as artefacts and diffed across branches.

### 3.4 CI integration (future, not required now)

A scheduled GitHub Actions job could run `dagayn eval --benchmark latency` against a fixed reference graph, compare against a stored baseline JSON, and post a PR comment if any p95 increases by more than 20%. This is not proposed for immediate implementation but is the natural next step once baselines are established.

---

## Open questions

1. **Thread model of the MCP server process.** Is the server single-threaded (asyncio) or does it use a thread pool? This affects whether `_store_lock` in Fix A is necessary and whether `check_same_thread=False` is already set on the connection.

2. **Write-tool isolation.** Build and update commands call `store_file_nodes_edges` which calls `_invalidate_cache`. If the same `GraphStore` instance is held in the process-level cache, cache invalidation during a write needs to be propagated correctly. The safest approach is to not cache store instances that are used for writes.

3. **Snapshot injection API compatibility.** Adding an optional `snapshot` parameter to `find_hub_nodes` / `find_bridge_nodes` / etc. changes their signatures. This is backward-compatible for callers that pass positional arguments, but any callers using keyword arguments need auditing.

4. **Benchmark reference graph.** The latency targets in section 3.2.3 should be calibrated against a specific graph. The dagayn repository itself (316 files, ~4 000 nodes, ~25 000 edges as of 2026-04-29) is the obvious candidate, but a larger synthetic graph may be needed to surface scaling issues.

5. **Interaction with hub/bridge persistence and `_invalidate_cache`.** After Fix B (persist hub/bridge scores), `_invalidate_cache` should also mark the `hub_scores` / `bridge_scores` tables as stale. Whether this is a flag column, a `PRAGMA user_version` bump, or simply relying on `computed_at` vs. graph mtime needs to be decided.

---

## 4. Additional findings (audit 2026-04-30)

Items below were identified in a follow-up audit and are **not yet covered** by sections 1–3 above.

### 4.1 PRAGMA tuning (quick win — shipped)

`GraphStore.__init__` only set `journal_mode=WAL` and `busy_timeout=5000`. Added:

- `synchronous=NORMAL` — WAL does not need `FULL` fsync; reduces write overhead.
- `cache_size=-64000` — 64 MB page cache (was the SQLite default of ~2 MB).
- `mmap_size=268435456` — 256 MB memory-mapped I/O for read-heavy queries.
- `temp_store=MEMORY` — temp tables go to RAM instead of disk.

Same set of PRAGMAs added to `EmbeddingStore` (32 MB / 128 MB).

### 4.2 CodeParser singleton per worker (quick win — shipped)

`_parse_single_file` (`incremental.py`) was constructing a new `CodeParser()` on every
call, re-loading Tree-sitter grammar shared libraries each time inside a worker process.
With `chunksize=20` this meant 20 grammar re-loads per worker per chunk.
Fixed by adding `ProcessPoolExecutor(initializer=_init_worker)` that creates one
`CodeParser` per worker process and reuses it for all files in that worker.

Note: WIP §3 "Out of scope" listed this item as overlapping Rust migration Phase 3.
The Python-only fix is a one-line change with no architectural risk, so it was shipped
independently.

### 4.3 Token-estimate hot loop (quick win — shipped)

`traverse_graph` was calling `len(str(entry)) // 4` on every visited node.
`str(dict)` serialises the entire dict just to count characters.
Replaced with `(len(qualified_name) + len(file) + len(name) + 30) // 4`.

### 4.4 Write-side N+1 (not yet implemented)

#### `upsert_node` / `upsert_edge` — per-row INSERT + SELECT
- `graph/core.py:202-244` (node), `graph/core.py:255-287` (edge)
- Each node causes INSERT/UPSERT + follow-up `SELECT id`. A 200-node file issues 400 statements.
- Fix: `INSERT ... ON CONFLICT DO UPDATE ... RETURNING id` + `executemany`.

#### `store_file_batch` unused by build path
- Defined at `graph/core.py:326-342`, but `full_build` and `incremental_update` call
  `store_file_nodes_edges` per file instead, opening `BEGIN IMMEDIATE` and committing once per file.
- Fix: group 50–100 files and pass them to `store_file_batch`.

#### `_clear_and_store_communities` per-community INSERT loop
- `communities.py:703-738` — inserts one community at a time.
- Fix: `executemany` + single `UPDATE nodes SET community_id` JOIN.

#### `flows.store_flows` / `incremental_trace_flows` INSERT loops
- `flows.py:394-429`, `flows.py:463-556` — flow-by-flow INSERT/DELETE.
- Fix: pre-allocate IDs + `executemany`, delete via `WHERE id IN (?,...)`.

#### `postprocessing._resolve_markdown_artifact_refs`
- `postprocessing.py:83-117` — edge-by-edge SELECT + UPDATE/DELETE.
- Fix: batch-load target symbols, bulk `executemany`.

### 4.5 Embedding search performance (not yet implemented)

#### Pure-Python cosine loop
- `embeddings.py:810-835` — `_cosine_similarity` (`embeddings.py:695-704`) runs a
  Python loop over ~4 k float32 vectors per search request.
- Fix: cache `numpy.ndarray (N, D)` keyed on db mtime; replace loop with
  `vectors @ q / norms` (single BLAS call).

#### `EmbeddingStore` re-instantiated per search
- `search.py:192` — `GraphStore` is pinned across tool calls but `EmbeddingStore`
  opens a fresh sqlite3 connection on every `hybrid_search` call.
- Fix: same process-level cache pattern as `_get_store()` in `tools/_common.py`.

#### `embed_nodes` N+1
- `embeddings.py:778-805` — one `SELECT FROM embeddings WHERE qualified_name = ?` per
  node, then one `INSERT OR REPLACE` per node.
- Fix: batch `WHERE qualified_name IN (?,...)` + `executemany`.

### 4.6 Missing indexes (not yet implemented)

- `nodes(name)` absent — `dead_code.py:328-332` and `postprocessing.py:93-99` run
  `WHERE name = ?` as full scans. Add `idx_nodes_parent_name(parent_name, name)`.
- Suffix LIKE on `edges.target_qualified` (`dead_code.py:276`, `flows.py:749`) cannot
  use any index. Long-term: normalise to equality match or add a `target_name` column.

### 4.7 mtime-based incremental skip (not yet implemented)

`incremental_update` (`incremental.py:963-969`) reads all file bytes and computes sha256
before comparing against the stored `file_hash`, even when `st_mtime` has not changed.

Fix: add `mtime_ns INTEGER` to the `nodes` table (migration v9); skip sha256 when mtime matches.

### 4.8 Other small items (not yet implemented)

- `traverse_graph` DFS path calls `get_nodes_by_qualified_names([qn])` and
  `get_edges_by_endpoints([qn])` with a size-1 list per node. BFS is frontier-batched; DFS is not.
- `get_impact_radius` calls `get_edges_among(all_qns)` after the recursive CTE already
  enumerated the full impacted set. The CTE could return edge rows directly.
- `parse_diff_ranges` shells out to `git diff` on every call; called independently by
  `detect_changes`, `get_impact_radius`, and `get_affected_flows` in sequence.
  Wrap with `functools.lru_cache(maxsize=64)` keyed on `(repo_root, base)`.
- `provider.embed_query` result is not cached; an `lru_cache` on `(provider_name, query_text)`
  makes repeated `semantic_search_nodes` calls free.

---

## Out of scope

- `EmbeddingStore.search` cosine similarity vectorisation (numpy/ANN) — tracked separately
- Parser worker `CodeParser` reuse (`incremental.py:803`) — **shipped in §4.2** as a per-worker singleton via `ProcessPoolExecutor(initializer=_init_worker)`. Full Rust migration remains Phase 3.
- `LIKE '%suffix'` full-scan in `get_files_matching` (`graph/core.py:1029`) — low-frequency path, low priority
- Embedding batch parallelisation (`embeddings.py:507`) — tracked separately
