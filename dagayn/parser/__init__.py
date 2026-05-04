"""dagayn parser package public API."""

from __future__ import annotations

from typing import Any

from ._base.types import BridgePattern, CellInfo, EdgeInfo, NodeInfo

_LAZY_EXPORTS = {
    "CodeParser": (".core", "CodeParser"),
    "CodeParserProtocol": ("._protocol", "CodeParserProtocol"),
    "EXTENSION_TO_LANGUAGE": (".dispatch", "EXTENSION_TO_LANGUAGE"),
    "SHEBANG_INTERPRETER_TO_LANGUAGE": (".dispatch", "SHEBANG_INTERPRETER_TO_LANGUAGE"),
    "_SQL_TABLE_RE": (".core", "_SQL_TABLE_RE"),
    "file_hash": (".core", "file_hash"),
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
    "BridgePattern",
    "CellInfo",
    "CodeParser",
    "CodeParserProtocol",
    "EdgeInfo",
    "EXTENSION_TO_LANGUAGE",
    "NodeInfo",
    "SHEBANG_INTERPRETER_TO_LANGUAGE",
    "_SQL_TABLE_RE",
    "file_hash",
]
