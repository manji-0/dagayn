"""MCP tool wrappers for graph analysis features."""

from __future__ import annotations

from typing import Any, Optional

from ..analysis import (
    find_bridge_nodes,
    find_hub_nodes,
    find_knowledge_gaps,
    find_surprising_connections,
    generate_suggested_questions,
)
from ._common import _get_store, apply_output_budget, make_response


def get_hub_nodes_func(
    repo_root: Optional[str] = None,
    top_n: int = 10,
) -> dict[str, Any]:
    """Find the most connected nodes in the codebase graph.

    Hub nodes have the highest total degree (in + out edges).
    These are architectural hotspots -- changes to them have
    disproportionate blast radius.

    Args:
        repo_root: Repository root (auto-detected if empty).
        top_n: Number of top hubs to return (default 10).
    """
    store, _root = _get_store(repo_root)
    hubs = find_hub_nodes(store, top_n=top_n)
    return make_response(
        "ok",
        f"Found {len(hubs)} hub node(s) with highest connectivity.",
        hub_nodes=hubs,
        count=len(hubs),
        next_tool_suggestions=[
            'review_tool mode="impact" -- check blast radius of a hub',
            "query_graph callers_of -- see what calls a hub",
            'architecture_analysis_tool mode="bridges" -- find architectural chokepoints',
        ],
    )


def get_bridge_nodes_func(
    repo_root: Optional[str] = None,
    top_n: int = 10,
) -> dict[str, Any]:
    """Find architectural chokepoints via betweenness centrality.

    Bridge nodes sit on the shortest paths between many node
    pairs. If they break, multiple code regions lose
    connectivity.

    Args:
        repo_root: Repository root (auto-detected if empty).
        top_n: Number of top bridges to return (default 10).
    """
    store, _root = _get_store(repo_root)
    bridges = find_bridge_nodes(store, top_n=top_n)
    return make_response(
        "ok",
        f"Found {len(bridges)} bridge node(s) (high betweenness centrality).",
        bridge_nodes=bridges,
        count=len(bridges),
        next_tool_suggestions=[
            'architecture_analysis_tool mode="hubs" -- find most connected nodes',
            'review_tool mode="impact" -- check blast radius',
            'review_tool mode="changes" -- see if bridges are affected',
        ],
    )


def get_knowledge_gaps_func(
    repo_root: Optional[str] = None,
    top_n: int = 20,
) -> dict[str, Any]:
    """Identify structural weaknesses in the codebase.

    Finds: isolated nodes (disconnected), thin communities
    (< 3 members), untested hotspots (high-degree, no tests),
    and single-file communities.

    Args:
        repo_root: Repository root (auto-detected if empty).
        top_n: Maximum items to return per gap category.
    """
    store, _root = _get_store(repo_root)
    gaps = find_knowledge_gaps(store, top_n=top_n)
    category_keys = (
        "isolated_nodes",
        "thin_communities",
        "untested_hotspots",
        "single_file_communities",
    )
    meta = gaps.get("_meta", {})
    raw_counts = meta.get("raw_counts", {})
    total = sum(int(raw_counts.get(key, len(gaps[key]))) for key in category_keys)
    payload = make_response(
        "ok",
        f"Found {total} knowledge gaps across 4 categories.",
        gaps=gaps,
        total_gaps=total,
        gap_counts={key: len(gaps[key]) for key in category_keys},
        raw_gap_counts={key: int(raw_counts.get(key, len(gaps[key]))) for key in category_keys},
        thresholds=meta.get("thresholds", {}),
        degree_distribution=meta.get("degree_distribution", {}),
        truncated=bool(meta.get("truncated", False)),
        next_tool_suggestions=[
            "refactor dead_code -- find unused symbols",
            'architecture_analysis_tool mode="hubs" -- find high-impact nodes',
            "get_suggested_questions -- review prompts",
        ],
    )
    # trim least-important lists first to stay within MCP token limits
    before_budget_counts = {key: len(gaps[key]) for key in category_keys}
    apply_output_budget(
        payload["gaps"],
        budget_tokens=4000,
        list_priorities=[
            "isolated_nodes",
            "single_file_communities",
            "thin_communities",
            "untested_hotspots",
        ],
    )
    after_budget_counts = {key: len(gaps[key]) for key in category_keys}
    if payload["gaps"].get("truncated"):
        payload["truncated"] = True
        payload["budget_truncation"] = payload["gaps"].get("_truncation", {})
        payload["gap_counts"] = after_budget_counts
    elif after_budget_counts != before_budget_counts:
        payload["truncated"] = True
        payload["gap_counts"] = after_budget_counts
    return payload


def get_surprising_connections_func(
    repo_root: Optional[str] = None,
    top_n: int = 15,
) -> dict[str, Any]:
    """Find unexpected architectural coupling in the codebase.

    Scores edges by surprise factors: cross-community,
    cross-language, peripheral-to-hub, cross-test-boundary.

    Args:
        repo_root: Repository root (auto-detected if empty).
        top_n: Number of top surprises to return (default 15).
    """
    store, _root = _get_store(repo_root)
    surprises = find_surprising_connections(store, top_n=top_n)
    return make_response(
        "ok",
        f"Found {len(surprises)} surprising connection(s).",
        surprising_connections=surprises,
        count=len(surprises),
        next_tool_suggestions=[
            'architecture_analysis_tool mode="overview" -- community structure',
            "query_graph callers_of -- trace the coupling",
            'architecture_analysis_tool mode="bridges" -- find chokepoints',
        ],
    )


def get_suggested_questions_func(
    repo_root: Optional[str] = None,
    top_n: int = 15,
) -> dict[str, Any]:
    """Auto-generate review questions from graph analysis.

    Produces questions about: bridge nodes, untested hubs,
    surprising connections, thin communities, and untested
    hotspots.

    Args:
        repo_root: Repository root (auto-detected if empty).
        top_n: Maximum questions to return. High-priority first. Default: 15.
    """
    store, _root = _get_store(repo_root)
    questions = generate_suggested_questions(store)
    by_priority: dict[str, list[dict[str, Any]]] = {
        "high": [],
        "medium": [],
        "low": [],
    }
    for q in questions:
        prio = q.get("priority", "medium")
        if prio in by_priority:
            by_priority[prio].append(q)

    # Return high-priority first, then medium, then low — capped at top_n
    ordered = by_priority["high"] + by_priority["medium"] + by_priority["low"]
    total = len(ordered)
    truncated = total > top_n
    returned = ordered[:top_n]

    return make_response(
        "ok",
        f"Generated {total} review question(s)."
        + (f" Showing top {top_n} (high priority first)." if truncated else ""),
        questions=returned,
        total=total,
        truncated=truncated,
        by_priority={k: len(v) for k, v in by_priority.items()},
        next_tool_suggestions=[
            'architecture_analysis_tool mode="knowledge_gaps" -- structural weaknesses',
            'review_tool mode="changes" -- risk-scored review',
            'architecture_analysis_tool mode="overview" -- community map',
        ],
    )
