"""SQL statement counter for detecting N+1 regressions.

Wraps a :class:`sqlite3.Connection` with ``set_trace_callback`` and counts
the number of statements executed during a callable's run. Used both as
context-managed instrument (:class:`SQLCounter`) and as a benchmark
runner that exercises a few representative MCP-tool entry points and
asserts that the per-call SQL count stays within a baseline.

The baseline numbers are intentionally generous — they exist so that an
*accidental* N+1 regression is loud, not so that small refactors must
ratchet a magic number every time.
"""

from __future__ import annotations

import logging
import sqlite3
from collections.abc import Callable
from pathlib import Path
from typing import Any

logger = logging.getLogger(__name__)

type BenchmarkValue = Any
type BenchmarkPayload = dict[str, BenchmarkValue]


class SQLCounter:
    """Counts SQL statements executed against a sqlite3.Connection.

    Usage::

        with SQLCounter(store._conn) as counter:  # rust stores have no _conn
            do_work()
        print(counter.count)
    """

    def __init__(self, conn: sqlite3.Connection) -> None:
        self._conn = conn
        self._previous: Callable[[str], None] | None = None
        self.count = 0
        self.statements: list[str] = []
        self._record_text = False

    def __enter__(self) -> "SQLCounter":
        self._conn.set_trace_callback(self._on_statement)
        return self

    def __exit__(self, *exc: object) -> None:
        self._conn.set_trace_callback(None)

    def reset(self) -> None:
        self.count = 0
        self.statements.clear()

    def _on_statement(self, statement: str) -> None:
        self.count += 1
        if self._record_text:
            self.statements.append(statement)


# ---------------------------------------------------------------------------
# Benchmark scenarios
# ---------------------------------------------------------------------------


# Per-tool baselines. Each value is the *maximum* allowed SQL statement
# count for that scenario on the reference graph.  These are deliberately
# loose — a regression that adds an N+1 loop will blow well past them.
_BASELINES: dict[str, int] = {
    "list_communities": 5,
    "single_hop_dependents": 10,
}


def _scenario_list_communities(store: Any, _config: BenchmarkPayload) -> int:
    from dagayn.communities import get_communities

    with SQLCounter(store._conn) as c:
        get_communities(store)
    return c.count


def _scenario_single_hop_dependents(store: Any, _config: BenchmarkPayload) -> int:
    from dagayn.incremental_build import _single_hop_dependents

    nodes = store.get_all_nodes(exclude_files=False)
    file_path = next((n.file_path for n in nodes if n.file_path), None)
    if file_path is None:
        return 0
    with SQLCounter(store._conn) as c:
        _single_hop_dependents(store, file_path)
    return c.count


_SCENARIOS: dict[str, Callable[[Any, BenchmarkPayload], int]] = {
    "list_communities": _scenario_list_communities,
    "single_hop_dependents": _scenario_single_hop_dependents,
}


def run(repo_path: Path, store: Any, config: BenchmarkPayload) -> list[BenchmarkPayload]:
    """Run each scenario and report SQL counts vs. baseline."""
    config = {**config, "repo_path": repo_path}
    stats = store.get_stats()
    results: list[BenchmarkPayload] = []
    if not hasattr(store, "_conn"):
        for name in _SCENARIOS:
            results.append(
                {
                    "scenario": name,
                    "sql_count": None,
                    "baseline": _BASELINES[name],
                    "status": "skipped",
                    "error": "SQL tracing requires a Python sqlite3 connection",
                    "node_count": stats.total_nodes,
                    "edge_count": stats.total_edges,
                    "file_count": stats.files_count,
                }
            )
        return results
    for name, fn in _SCENARIOS.items():
        try:
            count = fn(store, config)
        except Exception as exc:  # noqa: BLE001
            logger.warning("Scenario %s failed: %s", name, exc)
            results.append(
                {
                    "scenario": name,
                    "sql_count": None,
                    "baseline": _BASELINES[name],
                    "status": "error",
                    "error": str(exc),
                    "node_count": stats.total_nodes,
                    "edge_count": stats.total_edges,
                    "file_count": stats.files_count,
                }
            )
            continue
        baseline = _BASELINES[name]
        results.append(
            {
                "scenario": name,
                "sql_count": count,
                "baseline": baseline,
                "node_count": stats.total_nodes,
                "edge_count": stats.total_edges,
                "file_count": stats.files_count,
                "sql_per_1k_nodes": round(count / max(stats.total_nodes, 1) * 1000, 3),
                "status": "ok" if count <= baseline else "regression",
            }
        )
    return results
