"""Active MCP tool allow-list for the running server.

Prompts, hints, and next-step suggestions must only name tools that are
callable on the surface that named them. See: #107
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


def active_tool_surface() -> frozenset[str] | None:
    """Return the active allow-list, or ``None`` when unrestricted."""
    return _active_allow_list


def tool_is_exposed(name: str) -> bool:
    """Return True when *name* is callable on the active MCP surface."""
    if _active_allow_list is None:
        return True
    return name in _active_allow_list


def filter_tool_names(names: list[str]) -> list[str]:
    """Drop tool names that are not on the active MCP surface."""
    if _active_allow_list is None:
        return names
    return [name for name in names if name in _active_allow_list]


def _suggestion_tool_name(suggestion: str) -> str:
    """Extract a leading MCP tool name from a free-form suggestion string."""
    head, _, _tail = suggestion.partition(" -- ")
    return head.split(" ", 1)[0].split("(", 1)[0]


def suggestion_is_callable(suggestion: str) -> bool:
    """Return True when a next-step suggestion is valid on this surface.

    CLI invocations (``dagayn ...``) are always callable. MCP tool names are
    checked against the active allow-list.
    """
    if _active_allow_list is None:
        return True
    stripped = suggestion.strip()
    if stripped.startswith("Run:"):
        stripped = stripped[4:].strip()
    tool = _suggestion_tool_name(stripped)
    if tool == "dagayn" or not tool.endswith("_tool"):
        return True
    return tool in _active_allow_list


def filter_suggestions(suggestions: list[str]) -> list[str]:
    """Drop free-form suggestions that name a non-exposed MCP tool."""
    return [item for item in suggestions if suggestion_is_callable(item)]
