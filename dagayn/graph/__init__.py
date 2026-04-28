"""dagayn graph package — public re-exports."""

from __future__ import annotations

import os

from ._protocol import GraphQueryProtocol, GraphStoreProtocol
from .helpers import _sanitize_name, edge_to_dict, node_to_dict
from .types import FlowAdjacency, GraphEdge, GraphNode, GraphStats

_BACKEND = os.environ.get("DAGAYN_BACKEND", "python").strip().lower()

if _BACKEND == "python":
    from .core import GraphStore
elif _BACKEND == "rust":
    try:
        from dagayn._core import GraphStore
    except ImportError as exc:
        raise RuntimeError(
            "DAGAYN_BACKEND=rust was requested, but dagayn._core is not installed. "
            "Build the PyO3 extension with maturin before using the Rust backend."
        ) from exc
else:
    raise RuntimeError(
        f"Unsupported DAGAYN_BACKEND={_BACKEND!r}; expected 'python' or 'rust'."
    )

__all__ = [
    "FlowAdjacency",
    "GraphEdge",
    "GraphNode",
    "GraphQueryProtocol",
    "GraphStats",
    "GraphStore",
    "GraphStoreProtocol",
    "_sanitize_name",
    "edge_to_dict",
    "node_to_dict",
]
