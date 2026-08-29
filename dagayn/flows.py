"""Thin flow API. Algorithms live in ``dagayn.legacy_py.flows``.

Rust-backed stores keep rebuild/query on ``dagayn._core``. The Python
implementation is imported only when a native method is missing or fails.
"""

from __future__ import annotations

import json
import logging
from typing import Any, Callable, Optional, cast

from .graph import GraphNode, GraphStore
from .state_types import AffectedFlowsResult, ChangeFlowRecord

logger = logging.getLogger(__name__)

DEFAULT_FLOW_MAX_DEPTH = 15
DEFAULT_FLOW_MAX_NODES = 512
FLOW_KIND_REACHABLE_SET = "reachable_set"


def _legacy() -> Any:
    from dagayn.legacy_py import flows as impl

    return impl


def detect_entry_points(
    store: GraphStore,
    include_tests: bool = False,
) -> list[GraphNode]:
    """Find functions that are entry points in the graph."""
    native = getattr(store, "detect_entry_points_json", None)
    if callable(native):
        try:
            rows = json.loads(cast(Callable[[bool], str], native)(include_tests))
            ids = [int(row["id"]) for row in rows if isinstance(row, dict) and "id" in row]
            nodes_by_id = store.get_nodes_by_ids(ids)
            return [nodes_by_id[node_id] for node_id in ids if node_id in nodes_by_id]
        except Exception:  # noqa: BLE001 — native acceleration must be optional
            logger.debug("Native entry-point detection failed; falling back", exc_info=True)
    return _legacy().detect_entry_points(store, include_tests=include_tests)


def rebuild_stored_flows(
    store: GraphStore,
    *,
    max_depth: int = DEFAULT_FLOW_MAX_DEPTH,
    include_tests: bool = False,
) -> int:
    """Rebuild stored flows, keeping reachable-set tracing inside Rust when possible."""
    native = getattr(store, "rebuild_flows_json", None)
    if callable(native):
        try:
            payload = json.loads(cast(Callable[[int, bool], str], native)(max_depth, include_tests))
            return int(payload.get("count") or 0)
        except Exception:  # noqa: BLE001 — native acceleration must be optional
            logger.debug("Native flow rebuild failed; falling back", exc_info=True)
    return _legacy().rebuild_stored_flows(store, max_depth=max_depth, include_tests=include_tests)


def incremental_trace_flows(
    store: GraphStore,
    changed_files: list[str],
    max_depth: int = 15,
) -> int:
    """Re-trace flows whose reachable sets are affected by *changed_files*."""
    if not changed_files:
        return 0
    native = getattr(store, "incremental_trace_flows_json", None)
    if callable(native):
        try:
            payload = json.loads(
                cast(Callable[[list[str], int], str], native)(changed_files, max_depth)
            )
            return int(payload.get("count") or 0)
        except Exception:  # noqa: BLE001
            logger.debug("Native incremental flow trace failed; falling back", exc_info=True)
    return _legacy().incremental_trace_flows(store, changed_files, max_depth=max_depth)


def get_flows(
    store: GraphStore,
    sort_by: str = "criticality",
    limit: int = 50,
) -> list[Any]:
    """Retrieve stored flows from the database."""
    allowed_sort = {"criticality", "depth", "node_count", "file_count", "name"}
    if sort_by not in allowed_sort:
        sort_by = "criticality"
    rust_get = getattr(store, "get_flows_json", None)
    if callable(rust_get):
        rows_json = cast(Callable[[str, int], str], rust_get)(sort_by, limit)
        return _legacy()._annotate_flow_rows_liveness(store, json.loads(rows_json))
    return _legacy().get_flows(store, sort_by=sort_by, limit=limit)


def get_flow_by_id(store: GraphStore, flow_id: int) -> Optional[Any]:
    """Retrieve a single flow with reachable-set membership details."""
    rust_get = getattr(store, "get_flow_by_id_json", None)
    if callable(rust_get):
        raw = cast(Callable[[int], str | None], rust_get)(flow_id)
        if not raw:
            return None
        return _legacy()._annotate_flow_dict_bridges(store, json.loads(raw))
    return _legacy().get_flow_by_id(store, flow_id)


def get_affected_flows(
    store: GraphStore,
    changed_files: list[str],
) -> AffectedFlowsResult:
    """Find flows that include nodes from the given changed files."""
    if not changed_files:
        return {"affected_flows": [], "total": 0}
    rust_get = getattr(store, "get_affected_flows_json", None)
    if callable(rust_get):
        affected_json = cast(Callable[[list[str]], str], rust_get)(changed_files)
        impl = _legacy()
        affected = cast(
            list[ChangeFlowRecord],
            [impl._annotate_flow_dict_bridges(store, flow) for flow in json.loads(affected_json)],
        )
        return {"affected_flows": affected, "total": len(affected)}
    return _legacy().get_affected_flows(store, changed_files)


def __getattr__(name: str) -> Any:
    value = getattr(_legacy(), name)
    globals()[name] = value
    return value


__all__ = [
    "DEFAULT_FLOW_MAX_DEPTH",
    "DEFAULT_FLOW_MAX_NODES",
    "FLOW_KIND_REACHABLE_SET",
    "detect_entry_points",
    "get_affected_flows",
    "get_flow_by_id",
    "get_flows",
    "incremental_trace_flows",
    "rebuild_stored_flows",
]
