"""Impact query-performance benchmark.

Measures wall time for ``get_impact_radius`` at several depths. Embedding
search is intentionally omitted — use ``embedding_materials``.
"""

from __future__ import annotations

import logging
from collections.abc import Callable
from pathlib import Path
from typing import Any

from dagayn.eval.benchmarks.mcp_latency import _time_call

logger = logging.getLogger(__name__)

type BenchmarkValue = Any
type BenchmarkPayload = dict[str, BenchmarkValue]


def _first_file(store: Any) -> str:
    nodes = store.get_all_nodes(exclude_files=False)
    for node in nodes:
        if node.file_path:
            return str(node.file_path)
    return ""


def run(repo_path: Path, store: Any, config: BenchmarkPayload) -> list[BenchmarkPayload]:
    """Run query-performance scenarios against an already-built graph."""
    repeat = int(config.get("query_repeat", 3))
    first_file = _first_file(store)
    results: list[BenchmarkPayload] = []

    def record(scenario: str, fn: Callable[[], Any], **extra: BenchmarkValue) -> None:
        try:
            best_ms, median_ms, p95_ms = _time_call(fn, repeat)
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

    impact_fn = getattr(store, "get_impact_radius", None)
    if callable(impact_fn) and first_file:
        for depth in (1, 3, 6):
            record(
                f"get_impact_radius_depth_{depth}",
                lambda d=depth: impact_fn([first_file], max_depth=d),
                depth=depth,
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
