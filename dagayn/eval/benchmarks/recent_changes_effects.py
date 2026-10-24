"""Measure effects of the recent high/medium-priority graph improvements.

This benchmark compares current optimized paths against small legacy-equivalent
implementations on the same machine and dataset. It is intended for local
baseline capture, not CI pass/fail gating.
"""

from __future__ import annotations

import importlib
import logging
import tempfile
import time
from collections.abc import Callable
from pathlib import Path
from typing import Any

from dagayn.graph import GraphStore
from dagayn.parser import EdgeInfo, NodeInfo

logger = logging.getLogger(__name__)

type BenchmarkValue = Any
type BenchmarkPayload = dict[str, BenchmarkValue]


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


def _scenario_centrality(store: Any, config: BenchmarkPayload) -> BenchmarkPayload:
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


def _seed_remove_store(db_path: Path, file_count: int) -> GraphStore:
    store = GraphStore(db_path)
    store.store_file_batch(_make_file_batch(file_count))
    return store


def _make_file_batch(file_count: int) -> list[tuple[str, list[NodeInfo], list[EdgeInfo], str, int]]:
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
    return batch


def _legacy_store_file_batch(
    store: GraphStore,
    batch: list[tuple[str, list[NodeInfo], list[EdgeInfo], str, int]],
) -> None:
    for item in batch:
        store.store_file_batch([item])


def _scenario_batch_remove(config: BenchmarkPayload) -> BenchmarkPayload:
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


def _scenario_store_file_batch(config: BenchmarkPayload) -> BenchmarkPayload:
    file_count = int(config.get("effect_store_files", 100))
    batch = _make_file_batch(file_count)
    with tempfile.TemporaryDirectory() as tmpdir:
        tmp = Path(tmpdir)
        legacy_store = GraphStore(tmp / "legacy_store_batch.db")
        batch_store = GraphStore(tmp / "store_batch.db")
        try:
            legacy_ms, _ = _measure_ms(lambda: _legacy_store_file_batch(legacy_store, batch))
            batch_ms, _ = _measure_ms(lambda: batch_store.store_file_batch(batch))
        finally:
            legacy_store.close()
            batch_store.close()
    return {
        "scenario": "store_file_batch_bulk_replace",
        "before_ms": round(legacy_ms, 3),
        "after_ms": round(batch_ms, 3),
        "speedup": _speedup(legacy_ms, batch_ms),
        "file_count": file_count,
        "node_count": file_count * 2,
        "edge_count": file_count,
    }


def _scenario_mcp_latency(
    repo_path: Path, store: Any, config: BenchmarkPayload
) -> list[BenchmarkPayload]:
    mcp_latency = importlib.import_module("dagayn.eval.benchmarks.mcp_latency")

    rows = mcp_latency.run(
        repo_path,
        store,
        {**config, "latency_repeat": int(config.get("latency_repeat", 1))},
    )
    out: list[BenchmarkPayload] = []
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


def run(repo_path: Path, store: Any, config: BenchmarkPayload) -> list[BenchmarkPayload]:
    """Run all recent-change effect measurements."""
    results: list[BenchmarkPayload] = []

    def _centrality() -> BenchmarkPayload | list[BenchmarkPayload]:
        return _scenario_centrality(store, config)

    def _batch_remove() -> BenchmarkPayload | list[BenchmarkPayload]:
        return _scenario_batch_remove(config)

    def _store_file_batch() -> BenchmarkPayload | list[BenchmarkPayload]:
        return _scenario_store_file_batch(config)

    def _mcp_latency() -> BenchmarkPayload | list[BenchmarkPayload]:
        return _scenario_mcp_latency(repo_path, store, config)

    # Keep scenario names stable even when a helper raises before returning.
    _centrality.__name__ = "bridge_centrality_persisted_read"
    _batch_remove.__name__ = "remove_files_data_batch"
    _store_file_batch.__name__ = "store_file_batch_bulk_replace"
    _mcp_latency.__name__ = "mcp_latency"

    scenarios: list[Callable[[], BenchmarkPayload | list[BenchmarkPayload]]] = [
        _centrality,
        _batch_remove,
        _store_file_batch,
        _mcp_latency,
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
