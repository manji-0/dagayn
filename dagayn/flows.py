"""Entry-point detection backed by ``dagayn._core``."""

from __future__ import annotations

import json

from .graph import GraphNode, GraphStore


def detect_entry_points(
    store: GraphStore,
    include_tests: bool = False,
) -> list[GraphNode]:
    """Find functions that are entry points in the graph."""
    rows = json.loads(store.detect_entry_points_json(include_tests))
    ids = [int(row["id"]) for row in rows if isinstance(row, dict) and "id" in row]
    nodes_by_id = store.get_nodes_by_ids(ids)
    return [nodes_by_id[node_id] for node_id in ids if node_id in nodes_by_id]


__all__ = ["detect_entry_points"]
