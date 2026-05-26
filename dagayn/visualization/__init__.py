"""Graph export data helpers used by static export formats."""

from __future__ import annotations

from typing import Any

_LAZY_EXPORTS = {
    "_aggregate_community": (".aggregate", "_aggregate_community"),
    "_aggregate_file": (".aggregate", "_aggregate_file"),
    "export_graph_data": (".data", "export_graph_data"),
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
]
