"""Flow API backed by ``dagayn._core``.

Reachable-set tracing and persistence live in the native store. This module
shapes those JSON payloads for CLI/MCP callers.
"""

from __future__ import annotations

import json
import logging
from collections.abc import Mapping, Sequence
from typing import Any, Callable, Optional, TypedDict, cast

from .graph import GraphEdge, GraphNode, GraphStore
from .state_types import AffectedFlowsResult, ChangeFlowRecord


class FlowStepRecord(TypedDict, total=False):
    id: int
    qualified_name: str
    name: str
    file_path: str
    kind: str
    is_bridge_step: bool


class FlowRecord(TypedDict, total=False):
    """Reachable-set flow payload shared by tracing and query helpers."""

    id: int
    name: str
    entry_point: str
    entry_point_id: int
    kind: str
    path: list[int]
    members: list[int]
    depth: int
    node_count: int
    file_count: int
    files: list[str]
    truncated: bool
    truncation_reason: str | None
    criticality: float
    steps: list[FlowStepRecord]
    resolved_step_count: int
    missing_step_count: int
    bridge_step_count: int
    resolved_node_count: int
    missing_node_count: int
    created_at: str | None
    updated_at: str | None


logger = logging.getLogger(__name__)

DEFAULT_FLOW_MAX_DEPTH = 15
DEFAULT_FLOW_MAX_NODES = 512
FLOW_KIND_REACHABLE_SET = "reachable_set"


def _require_native(store: GraphStore, name: str) -> Any:
    method = getattr(store, name, None)
    if not callable(method):
        raise RuntimeError(f"GraphStore.{name} is required (Rust GraphStore).")
    return method


def detect_entry_points(
    store: GraphStore,
    include_tests: bool = False,
) -> list[GraphNode]:
    """Find functions that are entry points in the graph."""
    native = _require_native(store, "detect_entry_points_json")
    rows = json.loads(cast(Callable[[bool], str], native)(include_tests))
    ids = [int(row["id"]) for row in rows if isinstance(row, dict) and "id" in row]
    nodes_by_id = store.get_nodes_by_ids(ids)
    return [nodes_by_id[node_id] for node_id in ids if node_id in nodes_by_id]


def rebuild_stored_flows(
    store: GraphStore,
    *,
    max_depth: int = DEFAULT_FLOW_MAX_DEPTH,
    include_tests: bool = False,
) -> int:
    """Rebuild stored flows in the native store."""
    native = _require_native(store, "rebuild_flows_json")
    payload = json.loads(cast(Callable[[int, bool], str], native)(max_depth, include_tests))
    return int(payload.get("count") or 0)


def trace_flows(
    store: GraphStore,
    max_depth: int = DEFAULT_FLOW_MAX_DEPTH,
    include_tests: bool = False,
    max_nodes: int = DEFAULT_FLOW_MAX_NODES,
) -> list[Any]:
    """Trace reachable-set flows and persist them, then return the stored rows."""
    del max_nodes  # native rebuild uses the store's own node cap
    rebuild_stored_flows(store, max_depth=max_depth, include_tests=include_tests)
    return get_flows(store, limit=10**9)


def _hydrate_flow_rows(store: GraphStore, rows: list[Any]) -> list[Any]:
    """Annotate stored flow rows with live steps and bridge markers."""
    payloads: list[Any] = []
    for row in rows:
        if isinstance(row, dict):
            payload = dict(row)
        elif hasattr(row, "keys"):
            payload = {key: row[key] for key in row.keys()}
        else:
            payloads.append(row)
            continue
        if "path" not in payload and payload.get("path_json") is not None:
            raw_path = payload["path_json"]
            payload["path"] = json.loads(raw_path) if isinstance(raw_path, str) else raw_path
        if not payload.get("steps"):
            path_ids = [
                node_id for node_id in (payload.get("path") or []) if isinstance(node_id, int)
            ]
            try:
                nodes_by_id = store.get_nodes_by_ids(path_ids) if path_ids else {}
            except Exception:  # noqa: BLE001 — annotation must never break a listing
                nodes_by_id = {}
            payload["steps"] = [
                {
                    "id": node.id,
                    "qualified_name": node.qualified_name,
                    "name": node.name,
                    "file_path": node.file_path,
                    "kind": node.kind,
                }
                for node_id in path_ids
                if (node := nodes_by_id.get(node_id)) is not None
            ]
        payloads.append(payload)
    return [_annotate_flow_dict_bridges(store, row) for row in payloads]


def incremental_trace_flows(
    store: GraphStore,
    changed_files: list[str],
    max_depth: int = 15,
) -> int:
    """Re-trace flows whose reachable sets are affected by *changed_files*."""
    if not changed_files:
        return 0
    native = _require_native(store, "incremental_trace_flows_json")
    payload = json.loads(cast(Callable[[list[str], int], str], native)(changed_files, max_depth))
    return int(payload.get("count") or 0)


def store_flows(store: GraphStore, flows: Sequence[Mapping[str, object]]) -> int:
    """Persist traced flow dicts through the native store."""
    native = _require_native(store, "store_flows_json")
    return int(cast(Callable[[str], int], native)(json.dumps(list(flows))))


def get_flows(
    store: GraphStore,
    sort_by: str = "criticality",
    limit: int = 50,
) -> list[Any]:
    """Retrieve stored flows from the database."""
    allowed_sort = {"criticality", "depth", "node_count", "file_count", "name"}
    if sort_by not in allowed_sort:
        sort_by = "criticality"
    rust_get = _require_native(store, "get_flows_json")
    rows_json = cast(Callable[[str, int], str], rust_get)(sort_by, limit)
    return _annotate_flow_rows_liveness(store, json.loads(rows_json))


def get_flow_by_id(store: GraphStore, flow_id: int) -> Optional[Any]:
    """Retrieve a single flow with reachable-set membership details."""
    rust_get = _require_native(store, "get_flow_by_id_json")
    raw = cast(Callable[[int], str | None], rust_get)(flow_id)
    if not raw:
        return None
    return _annotate_flow_dict_bridges(store, json.loads(raw))


def get_affected_flows(
    store: GraphStore,
    changed_files: list[str],
) -> AffectedFlowsResult:
    """Find flows that include nodes from the given changed files."""
    if not changed_files:
        return {"affected_flows": [], "total": 0}
    rust_get = _require_native(store, "get_affected_flows_json")
    affected_json = cast(Callable[[list[str]], str], rust_get)(changed_files)
    affected = cast(
        list[ChangeFlowRecord],
        [_annotate_flow_dict_bridges(store, flow) for flow in json.loads(affected_json)],
    )
    return {"affected_flows": affected, "total": len(affected)}


def _annotate_flow_rows_liveness(store: GraphStore, flows: list[Any]) -> list[Any]:
    """Add entry_point names and resolved/missing node counts to listed flows."""
    all_ids = {
        node_id
        for flow in flows
        for node_id in (*(flow.get("path") or []), flow.get("entry_point_id"))
        if isinstance(node_id, int)
    }
    if not all_ids:
        return flows
    try:
        nodes_by_id = store.get_nodes_by_ids(sorted(all_ids))
    except Exception:  # noqa: BLE001 — annotation must never break a listing
        logger.debug("Could not resolve flow node liveness", exc_info=True)
        return flows
    live_ids = set(nodes_by_id.keys())
    for flow in flows:
        entry_id = flow.get("entry_point_id")
        if "entry_point" not in flow and isinstance(entry_id, int):
            node = nodes_by_id.get(entry_id)
            if node is not None:
                flow["entry_point"] = node.qualified_name
        path_ids = [node_id for node_id in (flow.get("path") or []) if isinstance(node_id, int)]
        resolved = sum(1 for node_id in path_ids if node_id in live_ids)
        flow["resolved_node_count"] = resolved
        flow["missing_node_count"] = len(path_ids) - resolved
        if "files" not in flow:
            files: list[str] = []
            seen: set[str] = set()
            for node_id in path_ids:
                node = nodes_by_id.get(node_id)
                if node is None or not node.file_path or node.file_path in seen:
                    continue
                seen.add(node.file_path)
                files.append(node.file_path)
            flow["files"] = files
    return flows


def _collect_cross_artifact_edges_among(
    store: GraphStore,
    path_qns: set[str],
) -> list[GraphEdge]:
    """Fetch CROSS_ARTIFACT edges whose endpoints are both in ``path_qns``."""
    if not path_qns:
        return []
    try:
        get_among = getattr(store, "get_edges_among", None)
        if callable(get_among):
            edges = cast(Callable[[set[str]], list[GraphEdge]], get_among)(path_qns)
            return [edge for edge in edges if getattr(edge, "kind", None) == "CROSS_ARTIFACT"]
        outgoing, incoming = store.get_edges_by_endpoints(list(path_qns))
        bridge_edges: list[GraphEdge] = []
        seen: set[int] = set()
        for edge_list in (*outgoing.values(), *incoming.values()):
            for edge in edge_list:
                edge_id = getattr(edge, "id", None)
                if edge_id in seen:
                    continue
                if edge_id is not None:
                    seen.add(edge_id)
                if getattr(edge, "kind", None) == "CROSS_ARTIFACT":
                    src = str(getattr(edge, "source_qualified", "") or "")
                    tgt = str(getattr(edge, "target_qualified", "") or "")
                    if src in path_qns and tgt in path_qns:
                        bridge_edges.append(edge)
        return bridge_edges
    except Exception:  # pragma: no cover - backend parity drift
        return []


def _annotate_flow_step_resolution(flow: Any) -> Any:
    """Add resolved/missing step counts for stored flow paths."""
    path_ids = flow.get("path") or []
    steps = flow.get("steps") or []
    resolved_step_count = len(steps)
    if path_ids:
        missing_step_count = len(path_ids) - resolved_step_count
    else:
        stored_node_count = int(flow.get("node_count") or resolved_step_count)
        missing_step_count = max(0, stored_node_count - resolved_step_count)
    annotated = dict(flow)
    annotated["resolved_step_count"] = resolved_step_count
    annotated["missing_step_count"] = missing_step_count
    return annotated


def _annotate_flow_dict_bridges(store: GraphStore, flow: Any) -> Any:
    """Mark bridge arrivals on a flow dict returned by the native store."""
    from .cross_artifact import annotate_flow_steps_with_bridges

    steps = list(flow.get("steps") or [])
    path_qns = {
        str(step.get("qualified_name"))
        for step in steps
        if isinstance(step.get("qualified_name"), str)
    }
    bridge_edges = _collect_cross_artifact_edges_among(store, path_qns)
    annotated = dict(flow)
    annotated["steps"] = annotate_flow_steps_with_bridges(steps, bridge_edges)
    annotated["bridge_step_count"] = sum(
        1 for step in annotated["steps"] if step.get("is_bridge_step")
    )
    return _annotate_flow_step_resolution(annotated)


__all__ = [
    "DEFAULT_FLOW_MAX_DEPTH",
    "DEFAULT_FLOW_MAX_NODES",
    "FLOW_KIND_REACHABLE_SET",
    "FlowRecord",
    "FlowStepRecord",
    "detect_entry_points",
    "get_affected_flows",
    "get_flow_by_id",
    "get_flows",
    "incremental_trace_flows",
    "rebuild_stored_flows",
    "store_flows",
    "trace_flows",
]
