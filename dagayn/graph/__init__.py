"""dagayn graph package public API.

Storage and query live in ``dagayn._core.GraphStore``. This package keeps
dataclasses, protocols, and serialization helpers for the Python CLI/MCP
layer.
"""

from __future__ import annotations

from typing import Any

_LAZY_EXPORTS = {
    "FlowAdjacency": (".types", "FlowAdjacency"),
    "GraphEdge": (".types", "GraphEdge"),
    "GraphNode": (".types", "GraphNode"),
    "GraphQueryProtocol": ("._protocol", "GraphQueryProtocol"),
    "GraphStats": (".types", "GraphStats"),
    "GraphStoreProtocol": ("._protocol", "GraphStoreProtocol"),
    "_sanitize_name": (".helpers", "_sanitize_name"),
    "edge_to_dict": (".helpers", "edge_to_dict"),
    "node_to_dict": (".helpers", "node_to_dict"),
}


def __getattr__(name: str) -> Any:
    if name == "GraphStore":
        from dagayn._core import GraphStore as RustGraphStore

        globals()[name] = RustGraphStore
        return RustGraphStore
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
]
