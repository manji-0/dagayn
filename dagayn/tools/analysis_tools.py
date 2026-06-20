"""MCP tool wrappers for graph analysis features."""

from __future__ import annotations

from typing import Any, Optional

from .._scope import ArtifactScope
from ..analysis import (
    find_bridge_nodes,
    find_hub_nodes,
    find_knowledge_gaps,
    find_surprising_connections,
    generate_suggested_questions,
)
from ._common import (
    _get_store,
    apply_output_budget,
    graph_answerability_summary,
    guidance_actions_to_hints,
    make_guidance_item,
    make_response,
    missingness_from_answerability,
)


def get_hub_nodes_func(
    repo_root: Optional[str] = None,
    top_n: int = 10,
    artifact_scope: ArtifactScope = "all",
    include_tests: bool = True,
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
    try:
        answerability = graph_answerability_summary(store)
        hubs = find_hub_nodes(
            store,
            top_n=top_n,
            artifact_scope=artifact_scope,
            include_tests=include_tests,
        )
        guidance = [
            make_guidance_item(
                claim="Hub nodes are review leads because many edges meet there.",
                evidence={"type": "computed", "metric": "degree", "examples": hubs[:3]},
                confidence="medium" if hubs else "low",
                missingness=[
                    {
                        "reason_code": "hub_score_is_degree_rank",
                        "severity": "low",
                        "claim_effect": "high degree is a lead, not proof of bad design",
                    }
                ],
                action='review_tool mode="impact" -- check blast radius of a hub',
                reason_codes=["hub_nodes"],
                counts={"hub_nodes": len(hubs)},
            )
        ]
        payload = make_response(
            "ok",
            f"Found {len(hubs)} hub node(s) with highest connectivity.",
            hub_nodes=hubs,
            count=len(hubs),
            artifact_scope=artifact_scope,
            include_tests=include_tests,
            answerability=answerability,
            missingness=missingness_from_answerability(answerability),
            guidance=guidance,
            next_tool_suggestions=[
                'review_tool mode="impact" -- check blast radius of a hub',
                "query_graph_tool callers_of -- see what calls a hub",
                'architecture_analysis_tool mode="bridges" -- find architectural chokepoints',
            ],
        )
        payload["_hints"] = guidance_actions_to_hints(guidance)
        return payload
    finally:
        store.close()


def get_bridge_nodes_func(
    repo_root: Optional[str] = None,
    top_n: int = 10,
    artifact_scope: ArtifactScope = "all",
    include_tests: bool = True,
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
    try:
        answerability = graph_answerability_summary(store)
        bridges = find_bridge_nodes(
            store,
            top_n=top_n,
            artifact_scope=artifact_scope,
            include_tests=include_tests,
        )
        guidance = [
            make_guidance_item(
                claim="Bridge nodes are architectural chokepoints on many shortest paths.",
                evidence={
                    "type": "computed",
                    "metric": "betweenness",
                    "examples": bridges[:3],
                },
                confidence="medium" if bridges else "low",
                missingness=[
                    {
                        "reason_code": "betweenness_is_heuristic_lead",
                        "severity": "low",
                        "claim_effect": "betweenness ranks review priority, not runtime failure",
                    }
                ],
                action='architecture_analysis_tool mode="hubs" -- compare with high-degree nodes',
                reason_codes=["bridge_nodes"],
                counts={"bridge_nodes": len(bridges)},
            )
        ]
        payload = make_response(
            "ok",
            f"Found {len(bridges)} bridge node(s) (high betweenness centrality).",
            bridge_nodes=bridges,
            count=len(bridges),
            artifact_scope=artifact_scope,
            include_tests=include_tests,
            answerability=answerability,
            missingness=missingness_from_answerability(answerability),
            guidance=guidance,
            next_tool_suggestions=[
                'architecture_analysis_tool mode="hubs" -- find most connected nodes',
                'review_tool mode="impact" -- check blast radius',
                'review_tool mode="changes" -- see if bridges are affected',
            ],
        )
        payload["_hints"] = guidance_actions_to_hints(guidance)
        return payload
    finally:
        store.close()


def get_knowledge_gaps_func(
    repo_root: Optional[str] = None,
    top_n: int = 20,
    artifact_scope: ArtifactScope = "all",
    include_tests: bool = True,
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
    try:
        answerability = graph_answerability_summary(store)
        gaps = find_knowledge_gaps(
            store,
            top_n=top_n,
            artifact_scope=artifact_scope,
            include_tests=include_tests,
        )
        category_keys = (
            "untested_hotspots",
            "single_file_communities",
            "isolated_nodes",
            "thin_communities",
        )
        meta = gaps.get("_meta", {})
        raw_counts = meta.get("raw_counts", {})
        total = sum(int(raw_counts.get(key, len(gaps[key]))) for key in category_keys)
        guidance = [
            make_guidance_item(
                claim=f"Found {total} knowledge-gap signal(s) across four structural categories.",
                evidence={
                    "type": "computed",
                    "gap_counts": {key: len(gaps[key]) for key in category_keys},
                    "thresholds": meta.get("thresholds", {}),
                },
                confidence="medium" if total else "low",
                missingness=[
                    {
                        "reason_code": "knowledge_gap_is_review_lead",
                        "severity": "low",
                        "claim_effect": "gaps highlight review targets, not automatic defects",
                    }
                ],
                action='refactor_tool mode="dead_code" -- cross-check unused symbols',
                reason_codes=["knowledge_gaps"],
                counts={"total_gaps": total},
            )
        ]
        payload = make_response(
            "ok",
            f"Found {total} knowledge gaps across 4 categories.",
            gaps=gaps,
            total_gaps=total,
            gap_counts={key: len(gaps[key]) for key in category_keys},
            raw_gap_counts={key: int(raw_counts.get(key, len(gaps[key]))) for key in category_keys},
            thresholds=meta.get("thresholds", {}),
            degree_distribution=meta.get("degree_distribution", {}),
            artifact_scope=artifact_scope,
            include_tests=include_tests,
            scoped_counts=meta.get("scoped_counts", {}),
            truncated=bool(meta.get("truncated", False)),
            answerability=answerability,
            missingness=missingness_from_answerability(answerability),
            guidance=guidance,
            next_tool_suggestions=[
                "refactor dead_code -- find unused symbols",
                'architecture_analysis_tool mode="hubs" -- find high-impact nodes',
                "get_suggested_questions -- review prompts",
            ],
        )
        payload["_hints"] = guidance_actions_to_hints(guidance)
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
    finally:
        store.close()


def get_surprising_connections_func(
    repo_root: Optional[str] = None,
    top_n: int = 15,
    artifact_scope: ArtifactScope = "all",
    include_tests: bool = True,
) -> dict[str, Any]:
    """Find unexpected architectural coupling in the codebase.

    Scores edges by surprise factors: cross-community,
    cross-language, peripheral-to-hub, cross-test-boundary.

    Args:
        repo_root: Repository root (auto-detected if empty).
        top_n: Number of top surprises to return (default 15).
    """
    store, _root = _get_store(repo_root)
    try:
        answerability = graph_answerability_summary(store)
        surprises = find_surprising_connections(
            store,
            top_n=top_n,
            artifact_scope=artifact_scope,
            include_tests=include_tests,
        )
        guidance = [
            make_guidance_item(
                claim="Surprising connections are ranked coupling leads, not verdicts.",
                evidence={
                    "type": "computed",
                    "examples": surprises[:3],
                    "count": len(surprises),
                },
                confidence="medium" if surprises else "low",
                missingness=[
                    {
                        "reason_code": "surprise_score_is_heuristic",
                        "severity": "low",
                        "claim_effect": "scores prioritize review, not proof of bad design",
                    }
                ],
                action='architecture_analysis_tool mode="overview" -- inspect community structure',
                reason_codes=["surprising_connections"],
                counts={"surprising_connections": len(surprises)},
            )
        ]
        payload = make_response(
            "ok",
            f"Found {len(surprises)} surprising connection(s).",
            surprising_connections=surprises,
            count=len(surprises),
            artifact_scope=artifact_scope,
            include_tests=include_tests,
            answerability=answerability,
            missingness=missingness_from_answerability(answerability),
            guidance=guidance,
            next_tool_suggestions=[
                'architecture_analysis_tool mode="overview" -- community structure',
                "query_graph_tool callers_of -- trace the coupling",
                'architecture_analysis_tool mode="bridges" -- find chokepoints',
            ],
        )
        payload["_hints"] = guidance_actions_to_hints(guidance)
        return payload
    finally:
        store.close()


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
    try:
        answerability = graph_answerability_summary(store)
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

        ordered = by_priority["high"] + by_priority["medium"] + by_priority["low"]
        total = len(ordered)
        truncated = total > top_n
        returned = ordered[:top_n]
        guidance = [
            make_guidance_item(
                claim=f"Generated {total} review question(s) from graph signals.",
                evidence={
                    "type": "computed",
                    "by_priority": {k: len(v) for k, v in by_priority.items()},
                    "returned": len(returned),
                },
                confidence="medium" if returned else "low",
                action='review_tool mode="changes" -- apply questions to current changes',
                reason_codes=["suggested_questions"],
                counts={"total_questions": total, "returned_questions": len(returned)},
            )
        ]
        payload = make_response(
            "ok",
            f"Generated {total} review question(s)."
            + (f" Showing top {top_n} (high priority first)." if truncated else ""),
            questions=returned,
            total=total,
            truncated=truncated,
            by_priority={k: len(v) for k, v in by_priority.items()},
            answerability=answerability,
            missingness=missingness_from_answerability(answerability),
            guidance=guidance,
            next_tool_suggestions=[
                'architecture_analysis_tool mode="knowledge_gaps" -- structural weaknesses',
                'review_tool mode="changes" -- risk-scored review',
                'architecture_analysis_tool mode="overview" -- community map',
            ],
        )
        payload["_hints"] = guidance_actions_to_hints(guidance)
        return payload
    finally:
        store.close()
