"""Tools 21, 22: list_repos_func, cross_repo_search_func."""

from __future__ import annotations

import logging
from pathlib import Path
from typing import Any, cast

from ..graph import GraphStore
from ..paths import db_path_for
from ..search import hybrid_search
from ..state_types import seal_missingness_item
from ..write_lock import graph_read_lock
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
        # A repo that could not be searched used to vanish from the response
        # entirely: repos_searched listed successes only and no warning was
        # emitted, so reduced recall looked like an exhaustive answer.
        skipped_repos: list[dict[str, str]] = []
        repo_modes: dict[str, dict[str, Any]] = {}

        for repo_entry in repos:
            repo_path = Path(repo_entry["path"])
            alias = repo_entry.get("alias", repo_path.name)
            if not repo_path.is_dir():
                skipped_repos.append({"repo": alias, "reason": "stale_registry_entry"})
                continue
            db_path = db_path_for(repo_path)
            if not db_path.exists():
                skipped_repos.append({"repo": alias, "reason": "no_graph"})
                continue

            try:
                with graph_read_lock(db_path):
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
                        for r in hs["results"]:
                            r["repo"] = alias
                            r["repo_path"] = str(repo_path)
                        all_results.extend(hs["results"])
                        searched_repos.append(alias)
                        # Per-repo mode matters for interpreting merged scores: a
                        # keyword-only fallback and a full hybrid arm are not on the
                        # same scale.
                        repo_modes[alias] = {
                            "mode": hs.get("mode"),
                            "embedding_health": hs.get("embedding_health") or {},
                        }
                    finally:
                        store.close()
            except Exception as exc:
                logger.warning("Search failed for %s: %s", repo_path, exc)
                skipped_repos.append(
                    {"repo": alias, "reason": f"search_failed: {type(exc).__name__}"}
                )

        # Sort all results by score descending
        all_results.sort(key=lambda r: r.get("score", 0), reverse=True)

        summary = (
            f"Found {len(all_results)} result(s) across "
            f"{len(searched_repos)} repo(s) for '{query}'."
        )
        missingness: list[dict[str, Any]] = []
        if skipped_repos:
            summary += (
                f" {len(skipped_repos)} registered repo(s) could not be searched:"
                " recall is reduced, not exhaustive."
            )
            missingness.append(
                cast(
                    dict[str, Any],
                    seal_missingness_item(
                        {
                            "reason_code": "registered_repos_not_searched",
                            "severity": "high",
                            "claim_effect": (
                                "results omit these repositories entirely, so absence is not"
                                " evidence the symbol does not exist there"
                            ),
                            "details": {"skipped_repos": skipped_repos[:20]},
                        }
                    ),
                )
            )
        mixed_modes = {info.get("mode") for info in repo_modes.values()}
        if len(mixed_modes) > 1:
            missingness.append(
                cast(
                    dict[str, Any],
                    seal_missingness_item(
                        {
                            "reason_code": "mixed_search_modes_across_repos",
                            "severity": "medium",
                            "claim_effect": (
                                "scores come from different search arms and are not directly"
                                " comparable across repositories"
                            ),
                            "details": {"modes": sorted(str(m) for m in mixed_modes)},
                        }
                    ),
                )
            )

        return make_response(
            "ok",
            summary,
            results=all_results[:limit],
            repos_searched=searched_repos,
            repos_skipped=skipped_repos,
            repo_search_modes=repo_modes,
            missingness=missingness,
            next_tool_suggestions=[
                "list_repos_tool -- inspect the registered repositories",
                "semantic_search_nodes_tool -- search within the current repository only",
            ],
        )
    except Exception as exc:
        return handle_tool_runtime_error(exc, logger=logger, context="cross_repo_search")
