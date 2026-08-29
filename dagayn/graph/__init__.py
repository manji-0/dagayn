"""dagayn graph package public API.

The Python ``GraphStore`` implementation lives in ``dagayn.legacy_py.graph``.
Production MCP/CLI callers select the native store through
``dagayn.tools._common._selected_graph_store``.
"""

from __future__ import annotations

from typing import Any

_LAZY_EXPORTS = {
    "FlowAdjacency": (".types", "FlowAdjacency"),
    "GraphEdge": (".types", "GraphEdge"),
    "GraphNode": (".types", "GraphNode"),
    "GraphQueryProtocol": ("._protocol", "GraphQueryProtocol"),
    "GraphStats": (".types", "GraphStats"),
    "GraphStore": (".core", "GraphStore"),
    "GraphStoreProtocol": ("._protocol", "GraphStoreProtocol"),
    "_sanitize_name": (".helpers", "_sanitize_name"),
    "edge_to_dict": (".helpers", "edge_to_dict"),
    "node_to_dict": (".helpers", "node_to_dict"),
    "store_write_transaction": (".helpers", "store_write_transaction"),
}


def __getattr__(name: str) -> Any:
    if name not in _LAZY_EXPORTS:
        raise AttributeError(f"module {__name__!r} has no attribute {name!r}")
    module_name, attr_name = _LAZY_EXPORTS[name]
    from importlib import import_module

    value = getattr(import_module(module_name, __name__), attr_name)
    globals()[name] = value
    return value


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
    "store_write_transaction",
]
