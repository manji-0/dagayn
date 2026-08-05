"""Tool: ensure_graph — safe graph bootstrap for the default MCP surface."""

from __future__ import annotations

from typing import Any

from ._common import (
    _evict_store_cache,
    _get_store,
    graph_answerability_summary,
    make_response,
)
from .build import build_or_update_graph

_ENSURE_POSTPROCESS = "minimal"
_ENSURE_LOCAL_EMBEDDING = "none"


def _graph_needs_full_build(stats: Any) -> bool:
    """True when the graph is empty and needs a first-time full parse."""
    return (
        int(getattr(stats, "total_nodes", 0) or 0) == 0
        or int(getattr(stats, "files_count", 0) or 0) == 0
    )


def _graph_is_ready(stats: Any) -> bool:
    """True when the graph already has parse data and a recorded update time."""
    return not _graph_needs_full_build(stats) and bool(getattr(stats, "last_updated", None))


def ensure_graph(
    repo_root: str | None = None,
    force: bool = False,
) -> dict[str, Any]:
    """Ensure a usable code knowledge graph exists for analysis tools.

    Safe defaults for the compact MCP surface:
    - never inherits a serve-time local embedding preset
    - uses ``postprocess="minimal"`` (signatures + FTS only)
    - full rebuild only when the graph is empty
    - ``force=True`` runs an incremental refresh when the graph already exists

    Args:
        repo_root: Repository root path. Auto-detected if omitted.
        force: If True and the graph already has nodes, run an incremental
            update. Empty graphs still take the full-build path.
    """
    _evict_store_cache()
    store, root = _get_store(repo_root, cached=False, use_backend_default=True)
    try:
        stats = store.get_stats()
        needs_full = _graph_needs_full_build(stats)
        ready = _graph_is_ready(stats)
        if ready and not force:
            health = graph_answerability_summary(store, stats)
            return make_response(
                status="ok",
                summary=(
                    f"Graph already present: {stats.total_nodes} nodes, "
                    f"{stats.total_edges} edges across {stats.files_count} files."
                ),
                action="noop",
                reason="graph_ready",
                total_nodes=stats.total_nodes,
                total_edges=stats.total_edges,
                files_count=stats.files_count,
                last_updated=stats.last_updated,
                graph_health=health,
                next_tool_suggestions=[
                    "get_minimal_context_tool",
                    "review_tool",
                    "query_graph_tool",
                ],
            )
    finally:
        store.close()

    if needs_full:
        action = "full"
        reason = "empty_graph"
        build_result = build_or_update_graph(
            full_rebuild=True,
            repo_root=str(root),
            postprocess=_ENSURE_POSTPROCESS,
            local_embedding=_ENSURE_LOCAL_EMBEDDING,
        )
    else:
        action = "incremental"
        reason = "forced_refresh" if force else "missing_last_updated"
        build_result = build_or_update_graph(
            full_rebuild=False,
            repo_root=str(root),
            postprocess=_ENSURE_POSTPROCESS,
            local_embedding=_ENSURE_LOCAL_EMBEDDING,
        )

    # Re-read health after the build so callers see post-ensure answerability.
    store, _ = _get_store(str(root), cached=False, use_backend_default=True)
    try:
        stats = store.get_stats()
        health = graph_answerability_summary(store, stats)
    finally:
        store.close()

    status = build_result.get("status", "ok")
    summary = build_result.get("summary") or (
        f"Ensured graph via {action} build ({stats.total_nodes} nodes, {stats.total_edges} edges)."
    )
    response = make_response(
        status=status,
        summary=summary,
        action=action,
        reason=reason,
        total_nodes=stats.total_nodes,
        total_edges=stats.total_edges,
        files_count=stats.files_count,
        last_updated=stats.last_updated,
        graph_health=health,
        build=build_result,
        next_tool_suggestions=[
            "get_minimal_context_tool",
            "review_tool",
            "query_graph_tool",
        ],
    )
    if build_result.get("warnings"):
        response["warnings"] = build_result["warnings"]
    if build_result.get("errors"):
        response["errors"] = build_result["errors"]
    return response
