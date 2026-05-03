"""Interactive D3.js graph visualization public API.

Exports graph data to JSON and generates a self-contained HTML file with
a force-directed D3.js visualization. Dark theme, zoomable, draggable,
with collapsible file clusters, tooltips, legend, and stats bar.

Supports multiple rendering modes for large graphs:
- ``full``  — render every node (default, current behavior)
- ``community`` — aggregate by community; double-click to drill down
- ``file``  — aggregate by file; each file is a node
- ``auto``  — choose community mode when node count exceeds threshold
"""

from __future__ import annotations

from typing import Any

_LAZY_EXPORTS = {
    "_aggregate_community": (".aggregate", "_aggregate_community"),
    "_aggregate_file": (".aggregate", "_aggregate_file"),
    "export_graph_data": (".data", "export_graph_data"),
    "generate_html": (".render", "generate_html"),
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
    "_aggregate_community",
    "_aggregate_file",
    "export_graph_data",
    "generate_html",
]
