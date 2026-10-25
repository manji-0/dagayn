"""Active MCP tool allow-list for the running server.

Prompts and ``next`` calls must only name tools that are callable on the
surface that named them. See: #107
"""

from __future__ import annotations

# ``None`` means unrestricted (CLI ``dagayn tool``, or ``serve --tools all``).
_active_allow_list: frozenset[str] | None = None


def set_active_tool_surface(names: set[str] | frozenset[str] | None) -> None:
    """Record the MCP tools currently exposed by ``dagayn serve``.

    Pass ``None`` when every registered tool is callable (no filter, or an
    ``all`` / ``full`` / ``*`` sentinel).
    """
    global _active_allow_list
    _active_allow_list = frozenset(names) if names is not None else None


def exposed_tool_names() -> frozenset[str] | None:
    """The active allow-list, or ``None`` when every tool is exposed."""
    return _active_allow_list


def tool_is_exposed(name: str) -> bool:
    """Return True when *name* is callable on the active MCP surface."""
    if _active_allow_list is None:
        return True
    return name in _active_allow_list
