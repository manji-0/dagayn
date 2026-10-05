"""Which MCP tools a ``dagayn serve`` session exposes.

Kept apart from :mod:`dagayn.server.main` so the stdio front end in
``dagayn._core`` can resolve the surface without importing fastmcp.
"""

from __future__ import annotations

_DEFAULT_MCP_TOOL_NAMES: frozenset[str] = frozenset(
    {
        "get_minimal_context_tool",
        "ensure_graph_tool",
        "review_tool",
        "flow_tool",
        "architecture_analysis_tool",
        "refactor_tool",
        "query_graph_tool",
        "semantic_search_nodes_tool",
        "get_docs_section_tool",
    }
)
_ALL_TOOL_SENTINELS: frozenset[str] = frozenset({"*", "all", "full"})


def _parse_tool_allow_list(raw: str) -> set[str]:
    """Parse a comma-separated MCP tool allow-list."""
    return {tool.strip() for tool in raw.split(",") if tool.strip()}


def _resolve_tool_allow_list(tools: str | None = None) -> set[str] | None:
    """Resolve tool filtering from CLI/env args.

    ``None`` means expose every registered tool.  When no CLI/env value is
    supplied, return the compact default public surface instead of the full
    maintenance/debugging surface.
    """
    import os

    raw = tools if tools is not None else os.environ.get("CRG_TOOLS")
    if raw is None:
        return set(_DEFAULT_MCP_TOOL_NAMES)
    parsed = _parse_tool_allow_list(raw)
    if parsed & _ALL_TOOL_SENTINELS:
        return None
    return parsed or None
