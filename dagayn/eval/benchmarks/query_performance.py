"""Traversal and impact query-performance benchmark.

Measures wall time for ``traverse_graph``, ``get_impact_radius``, and
``get_affected_flows`` at several depths / changed-file counts. Embedding
search is intentionally omitted — use ``embedding_materials``.
"""

from __future__ import annotations

import logging
import time
from collections.abc import Callable
from pathlib import Path
from typing import Any

logger = logging.getLogger(__name__)

type BenchmarkValue = Any
type BenchmarkPayload = dict[str, BenchmarkValue]


def _p95(samples: list[float]) -> float:
    if not samples:
        return 0.0
    ordered = sorted(samples)
    return ordered[min(len(ordered) - 1, int(len(ordered) * 0.95))]


def _time_repeat(fn: Callable[[], Any], repeat: int) -> tuple[float, float, float]:
    samples: list[float] = []
    for _ in range(max(1, repeat)):
        started = time.perf_counter()
        fn()
        samples.append((time.perf_counter() - started) * 1000.0)
    samples.sort()
    return samples[0], samples[len(samples) // 2], _p95(samples)


def _first_file(store: Any) -> str:
    nodes = store.get_all_nodes(exclude_files=False)
    for node in nodes:
        if node.file_path:
            return str(node.file_path)
    return ""


def _first_query(config: BenchmarkPayload) -> str:
    queries = config.get("search_queries") or []
    if queries:
        return str(queries[0].get("query") or "graph")
    return "graph"


def run(repo_path: Path, store: Any, config: BenchmarkPayload) -> list[BenchmarkPayload]:
    """Run query-performance scenarios against an already-built graph."""
    from dagayn.flows import get_affected_flows
    from dagayn.tools.query import traverse_graph_func

    repeat = int(config.get("query_repeat", 3))
    repo_root = str(repo_path)
    query = _first_query(config)
    first_file = _first_file(store)
    results: list[BenchmarkPayload] = []

    def record(scenario: str, fn: Callable[[], Any], **extra: BenchmarkValue) -> None:
        try:
            best_ms, median_ms, p95_ms = _time_repeat(fn, repeat)
        except Exception as exc:  # noqa: BLE001
            logger.warning("query_performance %s failed: %s", scenario, exc)
            results.append(
                {
                    "benchmark": "query_performance",
                    "scenario": scenario,
                    "status": "error",
                    "error": str(exc),
                    **extra,
                }
            )
            return
        results.append(
            {
                "benchmark": "query_performance",
                "scenario": scenario,
                "status": "ok",
                "repeat": repeat,
                "best_ms": round(best_ms, 3),
                "median_ms": round(median_ms, 3),
                "p95_ms": round(p95_ms, 3),
                **extra,
            }
        )

    for depth in (1, 3, 6):
        record(
            f"traverse_graph_depth_{depth}",
            lambda d=depth: traverse_graph_func(
                query=query,
                mode="bfs",
                depth=d,
                repo_root=repo_root,
            ),
            depth=depth,
        )

    impact_fn = getattr(store, "get_impact_radius", None)
    if callable(impact_fn) and first_file:
        for depth in (1, 3, 6):
            record(
                f"get_impact_radius_depth_{depth}",
                lambda d=depth: impact_fn([first_file], max_depth=d),
                depth=depth,
            )

    if first_file:
        files = [first_file]
        for count in (1, 5, 20):
            changed = files * count
            record(
                f"get_affected_flows_{count}_files",
                lambda paths=changed: get_affected_flows(store, paths),
                changed_file_count=count,
            )

    results.append(
        {
            "benchmark": "query_performance",
            "scenario": "embedding",
            "status": "skipped",
            "note": "embedding measured by embedding_materials / embedding_text_modes",
        }
    )
    return results
