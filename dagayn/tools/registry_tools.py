"""Tools 21, 22: list_repos_func, cross_repo_search_func."""

from __future__ import annotations

import logging
from pathlib import Path
from typing import Any

from ..graph import GraphStore
from ..search import hybrid_search
from ._common import handle_tool_runtime_error, make_response

logger = logging.getLogger(__name__)


# ---------------------------------------------------------------------------
# Tool 21: list_repos  [REGISTRY]
# ---------------------------------------------------------------------------


def list_repos_func() -> dict[str, Any]:
    """List all registered repositories.

    [REGISTRY] Returns the list of repositories registered in the global
    multi-repo registry at ``~/.dagayn/registry.json``.

    Returns:
        List of registered repos with paths and aliases.
    """
    from ..registry import Registry

    try:
        registry = Registry()
        repos = registry.list_repos()
        return make_response(
            "ok",
            f"{len(repos)} registered repository(ies).",
            repos=repos,
            next_tool_suggestions=[
                "cross_repo_search_tool -- search across registered repositories",
                "dagayn register <path> -- add another repository to the registry",
            ],
        )
    except Exception as exc:
        return handle_tool_runtime_error(exc, logger=logger, context="list_repos")


# ---------------------------------------------------------------------------
# Tool 22: cross_repo_search  [REGISTRY]
# ---------------------------------------------------------------------------


def cross_repo_search_func(
    query: str,
    kind: str | None = None,
    limit: int = 20,
    model: str | None = None,
    provider: str | None = None,
) -> dict[str, Any]:
    """Search across all registered repositories.

    [REGISTRY] Runs hybrid_search on each registered repo's graph database
    and merges the results.

    Args:
        query: Search query string.
        kind: Optional node kind filter (e.g. "Function", "Class").
        limit: Maximum results per repo (default: 20).
        model: Embedding model for hybrid search.
        provider: Embedding provider for hybrid search.

    Returns:
        Combined search results from all registered repos.
    """
    from ..registry import Registry

    try:
        registry = Registry()
        repos = registry.list_repos()
        if not repos:
            return make_response(
                "ok",
                "No repositories registered.",
                results=[],
                next_tool_suggestions=[
                    "Run: dagayn register <path> -- add a repository to the registry",
                    "list_repos_tool -- check currently registered repos",
                ],
            )

        all_results: list[dict[str, Any]] = []
        searched_repos: list[str] = []

        for repo_entry in repos:
            repo_path = Path(repo_entry["path"])
            db_path = repo_path / ".dagayn" / "graph.db"
            if not db_path.exists():
                continue

            try:
                store = GraphStore(str(db_path))
                try:
                    hs = hybrid_search(
                        store,
                        query,
                        kind=kind,
                        limit=limit,
                        model=model,
                        provider=provider,
                    )
                    alias = repo_entry.get("alias", repo_path.name)
                    for r in hs["results"]:
                        r["repo"] = alias
                        r["repo_path"] = str(repo_path)
                    all_results.extend(hs["results"])
                    searched_repos.append(alias)
                finally:
                    store.close()
            except Exception as exc:
                logger.warning("Search failed for %s: %s", repo_path, exc)

        # Sort all results by score descending
        all_results.sort(key=lambda r: r.get("score", 0), reverse=True)

        return make_response(
            "ok",
            (
                f"Found {len(all_results)} result(s) across "
                f"{len(searched_repos)} repo(s) for '{query}'."
            ),
            results=all_results[:limit],
            repos_searched=searched_repos,
            next_tool_suggestions=[
                "list_repos_tool -- inspect the registered repositories",
                "semantic_search_nodes_tool -- search within the current repository only",
            ],
        )
    except Exception as exc:
        return handle_tool_runtime_error(exc, logger=logger, context="cross_repo_search")
