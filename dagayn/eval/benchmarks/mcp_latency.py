"""Per-MCP-tool wall-clock latency benchmark.

Calls representative tool functions directly, without MCP transport, and emits
baseline-friendly JSON/CSV rows. The benchmark is intentionally observational:
it records local timings for calibration instead of failing CI on a fixed
threshold.
"""

from __future__ import annotations

import logging
import time
from collections.abc import Callable
from pathlib import Path
from typing import Any

logger = logging.getLogger(__name__)


def _time_call(fn: Callable[[], Any], repeat: int) -> tuple[float, float, float]:
    timings: list[float] = []
    for _ in range(max(1, repeat)):
        start = time.perf_counter()
        fn()
        timings.append((time.perf_counter() - start) * 1000.0)
    timings.sort()
    return timings[0], timings[len(timings) // 2], timings[-1]


def _first_query(config: dict, default: str = "graph") -> str:
    queries = config.get("search_queries") or []
    if queries:
        return str(queries[0].get("query") or default)
    return default


def _first_node_target(store: Any) -> str:
    nodes = store.get_all_nodes(exclude_files=False)
    for node in nodes:
        if node.kind != "File":
            return node.qualified_name
    return nodes[0].qualified_name if nodes else ""


def _scenarios(repo_path: Path, store: Any, config: dict) -> dict[str, Callable[[], Any]]:
    from dagayn.tools.architecture_analysis import architecture_analysis_func
    from dagayn.tools.context import get_minimal_context
    from dagayn.tools.query import query_graph, semantic_search_nodes, traverse_graph_func
    from dagayn.tools.review_dispatcher import review_func

    repo_root = str(repo_path)
    query = _first_query(config)
    target = _first_node_target(store)
    first_file = next(
        (node.file_path for node in store.get_all_nodes(exclude_files=False) if node.file_path),
        None,
    )
    changed_files = [first_file] if first_file else []

    return {
        "get_minimal_context": lambda: get_minimal_context(
            task="latency benchmark",
            changed_files=changed_files,
            repo_root=repo_root,
        ),
        "query_graph_file_summary": lambda: query_graph(
            pattern="file_summary",
            target=target.split("::", 1)[0] if target else "",
            repo_root=repo_root,
            detail_level="minimal",
        ),
        "semantic_search_nodes": lambda: semantic_search_nodes(
            query=query,
            limit=5,
            repo_root=repo_root,
        ),
        "traverse_graph_depth_3": lambda: traverse_graph_func(
            query=query,
            mode="bfs",
            depth=3,
            repo_root=repo_root,
        ),
        "architecture_overview": lambda: architecture_analysis_func(
            mode="overview",
            detail_level="minimal",
            top_n=3,
            repo_root=repo_root,
        ),
        "review_changes": lambda: review_func(
            mode="changes",
            changed_files=changed_files,
            repo_root=repo_root,
            detail_level="minimal",
        ),
    }


def run(repo_path: Path, store: Any, config: dict) -> list[dict]:
    """Run local per-tool latency measurements.

    Config keys:
      ``latency_repeat``: number of repeats per scenario (default 3).
    """
    repeat = int(config.get("latency_repeat", 3))
    results: list[dict] = []
    for name, fn in _scenarios(repo_path, store, config).items():
        try:
            best_ms, median_ms, worst_ms = _time_call(fn, repeat)
        except Exception as exc:  # noqa: BLE001
            logger.warning("Latency scenario %s failed: %s", name, exc)
            results.append(
                {
                    "benchmark": "mcp_latency",
                    "scenario": name,
                    "repeat": repeat,
                    "status": "error",
                    "error": str(exc),
                }
            )
            continue
        results.append(
            {
                "benchmark": "mcp_latency",
                "scenario": name,
                "repeat": repeat,
                "best_ms": round(best_ms, 3),
                "median_ms": round(median_ms, 3),
                "worst_ms": round(worst_ms, 3),
                "status": "baseline",
            }
        )
    return results
