"""Build performance benchmark: measures timing of graph operations."""

from __future__ import annotations

import logging
import statistics
import time
from pathlib import Path
from typing import Any

logger = logging.getLogger(__name__)

type BenchmarkValue = Any
type BenchmarkPayload = dict[str, BenchmarkValue]


def run(repo_path: Path, store: Any, config: BenchmarkPayload) -> list[BenchmarkPayload]:
    """Run build performance benchmark."""
    del store
    from dagayn.graph import GraphStore
    from dagayn.incremental import full_build

    repeats = max(1, int(config.get("build_performance_repeat", 1)))
    rows: list[BenchmarkPayload] = []
    timings: list[float] = []
    repo_name = str(config["name"])
    for idx in range(repeats):
        db_path = repo_path / ".dagayn" / f"eval-build-performance-{idx}.db"
        db_path.parent.mkdir(parents=True, exist_ok=True)
        db_path.unlink(missing_ok=True)
        build_store = GraphStore(db_path)
        try:
            started = time.perf_counter()
            build_result = full_build(repo_path, build_store)
            total_ms = (time.perf_counter() - started) * 1000.0
            stats = build_store.get_stats()
            timings.append(total_ms)
            files_parsed = build_result.files_parsed or stats.files_count or 0
            errors_count = len(build_result.errors or [])
            rows.append(
                {
                    "benchmark": "build_performance",
                    "repo": repo_name,
                    "status": "ok",
                    "repeat_index": idx + 1,
                    "build_total_ms": round(total_ms, 3),
                    "files_parsed": files_parsed,
                    "nodes": stats.total_nodes,
                    "edges": stats.total_edges,
                    "file_count": files_parsed,
                    "node_count": stats.total_nodes,
                    "edge_count": stats.total_edges,
                    "errors_count": errors_count,
                    "files_per_second": round(files_parsed / max(total_ms / 1000.0, 0.001), 3),
                    "nodes_per_second": round(stats.total_nodes / max(total_ms / 1000.0, 0.001), 3),
                }
            )
        except Exception as exc:
            logger.warning("Full build timing failed: %s", exc)
            rows.append(
                {
                    "benchmark": "build_performance",
                    "repo": repo_name,
                    "status": "error",
                    "repeat_index": idx + 1,
                    "error": str(exc),
                }
            )
        finally:
            build_store.close()
            db_path.unlink(missing_ok=True)

    if len(timings) > 1:
        rows.append(
            {
                "benchmark": "build_performance",
                "repo": repo_name,
                "status": "aggregate",
                "repeat": repeats,
                "median_build_total_ms": round(statistics.median(timings), 3),
                "best_build_total_ms": round(min(timings), 3),
            }
        )
    return rows
