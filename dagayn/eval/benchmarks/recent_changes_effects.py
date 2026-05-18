"""Measure effects of the recent high/medium-priority graph improvements.

This benchmark compares current optimized paths against small legacy-equivalent
implementations on the same machine and dataset. It is intended for local
baseline capture, not CI pass/fail gating.
"""

from __future__ import annotations

import logging
import subprocess
import tempfile
import time
from collections.abc import Callable
from pathlib import Path
from typing import Any, NamedTuple

from dagayn.graph import GraphStore
from dagayn.parser import EdgeInfo, NodeInfo

logger = logging.getLogger(__name__)


class _LegacyDfsResult(NamedTuple):
    nodes: list[str]
    sql_call_count: int


def _measure_ms(fn: Callable[[], Any], repeat: int = 1) -> tuple[float, Any]:
    timings: list[float] = []
    result: Any = None
    for _ in range(max(1, repeat)):
        start = time.perf_counter()
        result = fn()
        timings.append((time.perf_counter() - start) * 1000.0)
    timings.sort()
    return timings[len(timings) // 2], result


def _speedup(before_ms: float, after_ms: float) -> float:
    if after_ms <= 0:
        return 0.0
    return round(before_ms / after_ms, 2)


def _scenario_diff_cache(config: dict) -> dict:
    from dagayn import changes

    repeat = int(config.get("effect_repeat", 1))
    with tempfile.TemporaryDirectory() as tmpdir:
        repo = Path(tmpdir)
        subprocess.run(["git", "init"], cwd=repo, capture_output=True, check=True)
        subprocess.run(
            ["git", "config", "user.email", "bench@example.com"],
            cwd=repo,
            capture_output=True,
            check=True,
        )
        subprocess.run(
            ["git", "config", "user.name", "Bench"],
            cwd=repo,
            capture_output=True,
            check=True,
        )
        target = repo / "app.py"
        target.write_text("def main():\n    return 1\n", encoding="utf-8")
        subprocess.run(["git", "add", "."], cwd=repo, capture_output=True, check=True)
        subprocess.run(
            ["git", "commit", "-m", "initial"],
            cwd=repo,
            capture_output=True,
            check=True,
        )
        target.write_text("def main():\n    return 2\n", encoding="utf-8")

        uncached_ms, _ = _measure_ms(
            lambda: (
                changes.parse_git_diff_ranges(str(repo), "HEAD"),
                changes.parse_git_diff_ranges(str(repo), "HEAD"),
            ),
            repeat=repeat,
        )
        changes._parse_diff_ranges_cached.cache_clear()
        cached_ms, cached = _measure_ms(
            lambda: (
                changes.parse_diff_ranges(str(repo), "HEAD"),
                changes.parse_diff_ranges(str(repo), "HEAD"),
            ),
            repeat=repeat,
        )
        changes._parse_diff_ranges_cached.cache_clear()

    return {
        "scenario": "parse_diff_ranges_cache",
        "before_ms": round(uncached_ms, 3),
        "after_ms": round(cached_ms, 3),
        "speedup": _speedup(uncached_ms, cached_ms),
        "before_git_diff_calls": 2,
        "after_git_diff_calls": 1,
        "changed_file_count": len(cached[0]),
    }


def _scenario_centrality(store: Any, config: dict) -> dict:
    from dagayn.analysis import find_bridge_nodes, persist_centrality_scores

    repeat = int(config.get("centrality_repeat", 1))
    runtime_ms, runtime = _measure_ms(
        lambda: find_bridge_nodes(store, top_n=10, use_persisted=False),
        repeat=repeat,
    )
    persist_centrality_scores(store)
    persisted_ms, persisted = _measure_ms(
        lambda: find_bridge_nodes(store, top_n=10),
        repeat=max(1, int(config.get("effect_repeat", 3))),
    )
    return {
        "scenario": "bridge_centrality_persisted_read",
        "before_ms": round(runtime_ms, 3),
        "after_ms": round(persisted_ms, 3),
        "speedup": _speedup(runtime_ms, persisted_ms),
        "runtime_result_count": len(runtime),
        "persisted_result_count": len(persisted),
    }


def _legacy_dfs(store: Any, start_qn: str, max_depth: int) -> _LegacyDfsResult:
    visited: set[str] = set()
    order: list[str] = []
    stack: list[tuple[str, int]] = [(start_qn, 0)]
    sql_call_count = 0
    while stack:
        qn, depth = stack.pop()
        if qn in visited or depth > max_depth:
            continue
        sql_call_count += 1
        if store.get_node(qn) is None:
            continue
        visited.add(qn)
        order.append(qn)
        if depth + 1 > max_depth:
            continue
        sql_call_count += 2
        neighbors = [edge.target_qualified for edge in store.get_edges_by_source(qn)]
        neighbors.extend(edge.source_qualified for edge in store.get_edges_by_target(qn))
        for neighbor in reversed(neighbors):
            if neighbor not in visited:
                stack.append((neighbor, depth + 1))
    return _LegacyDfsResult(order, sql_call_count)


def _highest_degree_non_file_qn(store: Any) -> tuple[str | None, int]:
    non_file_qns = {node.qualified_name for node in store.get_all_nodes(exclude_files=True)}
    degree: dict[str, int] = {}
    for edge in store.get_all_edges():
        if edge.source_qualified in non_file_qns:
            degree[edge.source_qualified] = degree.get(edge.source_qualified, 0) + 1
        if edge.target_qualified in non_file_qns:
            degree[edge.target_qualified] = degree.get(edge.target_qualified, 0) + 1
    if not degree:
        return (next(iter(non_file_qns), None), 0)
    target, count = max(degree.items(), key=lambda item: item[1])
    return target, count


def _scenario_dfs(store: Any, config: dict) -> dict:
    depth = int(config.get("effect_dfs_depth", 3))
    target, degree = _highest_degree_non_file_qn(store)
    if not target:
        return {
            "scenario": "dfs_local_subgraph_batch",
            "status": "skipped",
            "reason": "no non-file nodes",
        }

    legacy_ms, legacy = _measure_ms(lambda: _legacy_dfs(store, target, depth))
    batched_ms, batched = _measure_ms(lambda: store.get_local_subgraph(target, depth))
    nodes_map, _adj = batched
    return {
        "scenario": "dfs_local_subgraph_batch",
        "before_ms": round(legacy_ms, 3),
        "after_ms": round(batched_ms, 3),
        "speedup": _speedup(legacy_ms, batched_ms),
        "legacy_nodes": len(legacy.nodes),
        "batched_nodes": len(nodes_map),
        "legacy_sql_call_count": legacy.sql_call_count,
        "batched_sql_call_count": 3,
        "start_degree": degree,
        "depth": depth,
    }


def _seed_remove_store(db_path: Path, file_count: int) -> GraphStore:
    store = GraphStore(db_path)
    batch = []
    for idx in range(file_count):
        path = f"src/file_{idx}.py"
        nodes = [
            NodeInfo("File", path, path, 1, 3, "python"),
            NodeInfo("Function", f"func_{idx}", path, 1, 3, "python"),
        ]
        edges = [
            EdgeInfo("CONTAINS", path, f"{path}::func_{idx}", path, 1),
        ]
        batch.append((path, nodes, edges, f"hash-{idx}", 0))
    store.store_file_batch(batch)
    return store


def _scenario_batch_remove(config: dict) -> dict:
    file_count = int(config.get("effect_remove_files", 100))
    with tempfile.TemporaryDirectory() as tmpdir:
        tmp = Path(tmpdir)
        legacy_store = _seed_remove_store(tmp / "legacy.db", file_count)
        batch_store = _seed_remove_store(tmp / "batch.db", file_count)
        files = [f"src/file_{idx}.py" for idx in range(file_count)]
        try:
            legacy_ms, _ = _measure_ms(
                lambda: [legacy_store.remove_file_data(path) for path in files]
            )
            batch_ms, _ = _measure_ms(lambda: batch_store.remove_files_data(files))
        finally:
            legacy_store.close()
            batch_store.close()
    return {
        "scenario": "remove_files_data_batch",
        "before_ms": round(legacy_ms, 3),
        "after_ms": round(batch_ms, 3),
        "speedup": _speedup(legacy_ms, batch_ms),
        "file_count": file_count,
    }


def _scenario_mcp_latency(repo_path: Path, store: Any, config: dict) -> list[dict]:
    from dagayn.eval.benchmarks import mcp_latency

    rows = mcp_latency.run(
        repo_path,
        store,
        {**config, "latency_repeat": int(config.get("latency_repeat", 1))},
    )
    out = []
    for row in rows:
        out.append(
            {
                "scenario": f"mcp_latency:{row.get('scenario')}",
                "after_ms": row.get("median_ms"),
                "status": row.get("status"),
                "repeat": row.get("repeat"),
            }
        )
    return out


def run(repo_path: Path, store: Any, config: dict) -> list[dict]:
    """Run all recent-change effect measurements."""
    results: list[dict] = []
    scenarios: list[Callable[[], dict | list[dict]]] = [
        lambda: _scenario_diff_cache(config),
        lambda: _scenario_centrality(store, config),
        lambda: _scenario_dfs(store, config),
        lambda: _scenario_batch_remove(config),
        lambda: _scenario_mcp_latency(repo_path, store, config),
    ]
    for scenario in scenarios:
        try:
            result = scenario()
        except Exception as exc:  # noqa: BLE001
            logger.warning("Recent-change effect scenario failed: %s", exc)
            results.append(
                {
                    "benchmark": "recent_changes_effects",
                    "scenario": getattr(scenario, "__name__", "unknown"),
                    "status": "error",
                    "error": str(exc),
                }
            )
            continue
        rows = result if isinstance(result, list) else [result]
        for row in rows:
            row.setdefault("benchmark", "recent_changes_effects")
            row.setdefault("status", "ok")
            results.append(row)
    return results
