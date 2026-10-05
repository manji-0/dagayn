"""MCP tool wrappers for graph analysis features."""

from __future__ import annotations

from typing import Optional

from ._common import ToolPayload, _get_store
from ._native import native_tool


def get_suggested_questions_func(
    repo_root: Optional[str] = None,
    top_n: int = 15,
) -> ToolPayload:
    """Auto-generate review questions from graph analysis.

    Produces questions about: bridge nodes, untested hubs,
    surprising connections, thin communities, and untested
    hotspots.

    Args:
        repo_root: Repository root (auto-detected if empty).
        top_n: Maximum questions to return. High-priority first. Default: 15.
    """
    store, _root = _get_store(repo_root)
    try:
        return native_tool("get_suggested_questions_tool", repo_root=repo_root, top_n=top_n)
    finally:
        store.close()
