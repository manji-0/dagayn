"""Tools 13, 14, 15: community listing, detail, architecture overview."""

from __future__ import annotations

import logging
from typing import Any, cast

from .._scope import ArtifactScope
from ..communities import (
    ArchitectureOverviewResult,
    CommunityRecord,
    get_architecture_overview,
    get_communities,
)
from ..graph import node_to_dict
from ..graph.sqlite_errors import borrowed_sqlite_connection
from ..hints import generate_hints, get_session
from ..stability_policy import component_stability_profiles, stability_policy_summary
from ._common import (
    ToolPayload,
    _get_store,
    apply_output_budget,
    graph_answerability_summary,
    guidance_actions_to_hints,
    handle_tool_runtime_error,
    make_guidance_item,
    missingness_from_answerability,
)

logger = logging.getLogger(__name__)


def _architecture_health_summary(
    store: Any,
    overview: ArchitectureOverviewResult,
    *,
    top_n: int,
    artifact_scope: ArtifactScope,
    snapshot: Any | None = None,
) -> ToolPayload:
    """Compose specialized architecture signals into one bounded report."""
    example_limit = min(max(top_n, 1), 5)
    include_tests = artifact_scope != "code"

    try:
        from ..analysis import (
            KnowledgeGapRecord,
            KnowledgeGapsResult,
            build_graph_snapshot,
            find_bridge_nodes,
            find_hub_nodes,
            find_knowledge_gaps,
            find_surprising_connections,
        )
        from ..architecture import find_adp_violations, find_sdp_violations
        from ..sap import find_sap_violations

        # One snapshot shared by every sub-analysis below. Each find_* helper
        # otherwise re-reads and JSON-parses the full edge table on its own
        # (~0.3 s per call on a 70k-edge graph), which dominated this tool.
        if snapshot is None:
            snapshot = build_graph_snapshot(store)

        hubs = find_hub_nodes(
            store,
            top_n=example_limit,
            artifact_scope=artifact_scope,
            include_tests=include_tests,
            snapshot=snapshot,
        )
        bridges = find_bridge_nodes(
            store,
            top_n=example_limit,
            artifact_scope=artifact_scope,
            include_tests=include_tests,
            snapshot=snapshot,
        )
        gaps: KnowledgeGapsResult = find_knowledge_gaps(
            store,
            top_n=example_limit,
            artifact_scope=artifact_scope,
            include_tests=include_tests,
            snapshot=snapshot,
        )
        surprises = find_surprising_connections(
            store,
            top_n=example_limit,
            artifact_scope=artifact_scope,
            include_tests=include_tests,
            snapshot=snapshot,
        )
        adp = find_adp_violations(
            store,
            granularity="package",
            artifact_scope=artifact_scope,
            snapshot=snapshot,
        )[:example_limit]
        sdp = find_sdp_violations(
            store,
            granularity="package",
            artifact_scope=artifact_scope,
            snapshot=snapshot,
        )[:example_limit]
        sap = find_sap_violations(
            store,
            scope_kind="package",
            artifact_scope=artifact_scope,
            snapshot=snapshot,
        )[:example_limit]
    except Exception as exc:  # pragma: no cover - defensive for backend parity drift
        return {
            "status": "partial",
            "error": str(exc),
            "drill_downs": {
                "hubs": {"tool": "architecture_analysis_tool", "mode": "hubs"},
                "bridges": {"tool": "architecture_analysis_tool", "mode": "bridges"},
                "knowledge_gaps": {
                    "tool": "architecture_analysis_tool",
                    "mode": "knowledge_gaps",
                },
                "surprising_connections": {
                    "tool": "architecture_analysis_tool",
                    "mode": "surprising_connections",
                },
                "adp": {
                    "tool": "architecture_analysis_tool",
                    "mode": "adp_violations",
                    "artifact_scope": artifact_scope,
                },
                "sdp": {
                    "tool": "architecture_analysis_tool",
                    "mode": "sdp_violations",
                    "artifact_scope": artifact_scope,
                },
                "sap": {
                    "tool": "architecture_analysis_tool",
                    "mode": "sap_violations",
                    "artifact_scope": artifact_scope,
                },
            },
        }

    gap_keys = (
        "untested_hotspots",
        "single_file_communities",
        "isolated_nodes",
        "thin_communities",
    )
    gap_lists: dict[str, list[KnowledgeGapRecord]] = {
        "untested_hotspots": gaps["untested_hotspots"],
        "single_file_communities": gaps["single_file_communities"],
        "isolated_nodes": gaps["isolated_nodes"],
        "thin_communities": gaps["thin_communities"],
    }
    raw_gap_counts = gaps["_meta"]["raw_counts"]
    gap_counts = {key: int(raw_gap_counts.get(key, len(gap_lists[key]))) for key in gap_keys}

    reason_codes: list[str] = []
    stale_communities = sum(
        1
        for comm in overview.get("communities", [])
        if comm.get("size") != comm.get("assigned_member_count", comm.get("size"))
    )
    if stale_communities:
        reason_codes.append("stale_community_membership")
    if overview.get("warnings"):
        reason_codes.append("high_cross_community_coupling")
    if hubs:
        reason_codes.append("hub_nodes")
    if bridges:
        reason_codes.append("bridge_nodes")
    if sum(gap_counts.values()):
        reason_codes.append("knowledge_gaps")
    if surprises:
        reason_codes.append("surprising_connections")
    if adp:
        reason_codes.append("adp_violations")
    if sdp:
        reason_codes.append("sdp_violations")
    if sap:
        reason_codes.append("sap_violations")

    guidance: list[ToolPayload] = []
    if hubs:
        guidance.append(
            make_guidance_item(
                claim="Hub nodes are review leads because many edges meet there.",
                evidence={"type": "computed", "metric": "degree", "examples": hubs[:3]},
                confidence="medium",
                missingness=[
                    {
                        "reason_code": "hub_score_is_degree_rank",
                        "severity": "low",
                        "claim_effect": "high degree is a lead, not proof of bad design",
                    }
                ],
                action='architecture_analysis_tool mode="hubs" -- inspect high-degree nodes',
                reason_codes=["hub_nodes"],
                counts={"hub_nodes": len(hubs)},
            )
        )
    if bridges:
        guidance.append(
            make_guidance_item(
                claim="Betweenness bridge nodes are interoperability chokepoints.",
                evidence={"type": "computed", "metric": "betweenness", "examples": bridges[:3]},
                confidence="medium",
                missingness=[
                    {
                        "reason_code": "bridge_score_is_betweenness_rank",
                        "severity": "low",
                        "claim_effect": "high betweenness is a lead, not proof of bad design",
                    }
                ],
                action=(
                    'architecture_analysis_tool mode="bridges" -- inspect chokepoints; '
                    'query_graph_tool pattern="docs_for" -- follow nearby contracts'
                ),
                reason_codes=["bridge_nodes"],
                counts={"bridge_nodes": len(bridges)},
            )
        )

    cross_artifact_count = 0
    try:
        stats = store.get_stats()
        edges_by_kind = getattr(stats, "edges_by_kind", None) or {}
        cross_artifact_count = int(edges_by_kind.get("CROSS_ARTIFACT", 0) or 0)
    except Exception:  # pragma: no cover - defensive for backend parity drift
        cross_artifact_count = 0
    if cross_artifact_count:
        reason_codes.append("cross_artifact_edges_present")
        guidance.append(
            make_guidance_item(
                claim=(
                    f"Graph contains {cross_artifact_count} CROSS_ARTIFACT edge(s); "
                    "treat bridges as first-class transitions when reviewing coupling."
                ),
                evidence={
                    "type": "extracted",
                    "cross_artifact_edge_count": cross_artifact_count,
                },
                confidence="medium",
                missingness=[
                    {
                        "reason_code": "cross_artifact_bridge_is_static_evidence",
                        "severity": "low",
                        "claim_effect": (
                            "prefer docs_for / implementations_of / reportable bridges; "
                            "treat low-confidence bridges as caveats"
                        ),
                    }
                ],
                action=(
                    'query_graph_tool pattern="docs_for" -- follow documentation bridges; '
                    'pattern="implementations_of" -- follow implementation bridges'
                ),
                reason_codes=["cross_artifact_edges_present"],
                counts={"cross_artifact_edges": cross_artifact_count},
            )
        )

    if adp or sdp or sap:
        guidance.append(
            make_guidance_item(
                claim="Architecture metric violations should be reviewed as ranked leads.",
                evidence={
                    "type": "computed",
                    "adp_violations": len(adp),
                    "sdp_violations": len(sdp),
                    "sap_violations": len(sap),
                    "artifact_scope": artifact_scope,
                },
                confidence="medium",
                missingness=[
                    {
                        "reason_code": "metric_warning_not_verdict",
                        "severity": "low",
                        "claim_effect": "ADP/SDP/SAP signals need source-level review",
                    }
                ],
                action=(
                    'architecture_analysis_tool mode="sdp_violations" -- drill into metric leads'
                ),
                reason_codes=["adp_violations", "sdp_violations", "sap_violations"],
                counts={
                    "adp_violations": len(adp),
                    "sdp_violations": len(sdp),
                    "sap_violations": len(sap),
                },
            )
        )

    return {
        "status": "ok",
        "scoring_policy": {
            "version": "architecture-health-v1",
            "artifact_scope": artifact_scope,
            "signals": [
                "community_coupling",
                "hub_nodes",
                "bridge_nodes",
                "cross_artifact_edges",
                "knowledge_gaps",
                "surprising_connections",
                "adp",
                "sdp",
                "sap",
            ],
            "bounded_top_n": example_limit,
            "formulas": {
                "adp": "cycles in package dependency graph",
                "sdp": "dependencies should point toward lower instability",
                "sap": "distance from main sequence D=|A+I-1|",
            },
            "thresholds": {
                "sap_violation_distance_min": 0.5,
                "artifact_scope_default": "code",
                "code_scope_includes_tests": False,
            },
        },
        "counts": {
            "communities": len(overview.get("communities", [])),
            "coupled_pairs_shown": len(overview.get("cross_community_coupling", [])),
            "warnings": len(overview.get("warnings", [])),
            "hub_nodes": len(hubs),
            "bridge_nodes": len(bridges),
            "cross_artifact_edges": cross_artifact_count,
            "knowledge_gaps": sum(gap_counts.values()),
            "surprising_connections": len(surprises),
            "adp_violations": len(adp),
            "sdp_violations": len(sdp),
            "sap_violations": len(sap),
        },
        "reason_codes": reason_codes,
        "guidance": guidance,
        "top_examples": {
            "hub_nodes": hubs,
            "bridge_nodes": bridges,
            "knowledge_gaps": {key: gap_lists[key][: min(3, example_limit)] for key in gap_keys},
            "surprising_connections": surprises,
            "adp_violations": adp,
            "sdp_violations": sdp,
            "sap_violations": [
                {
                    "scope_key": violation.get("scope_key"),
                    "display_name": violation.get("display_name"),
                    "distance": violation.get("distance"),
                    "zone": violation.get("zone"),
                }
                for violation in sap
            ],
        },
        "drill_downs": {
            "communities": {"tool": "architecture_analysis_tool", "mode": "communities"},
            "coupling": {"tool": "architecture_analysis_tool", "mode": "overview"},
            "hubs": {"tool": "architecture_analysis_tool", "mode": "hubs"},
            "bridges": {"tool": "architecture_analysis_tool", "mode": "bridges"},
            "knowledge_gaps": {
                "tool": "architecture_analysis_tool",
                "mode": "knowledge_gaps",
            },
            "surprising_connections": {
                "tool": "architecture_analysis_tool",
                "mode": "surprising_connections",
            },
            "adp": {
                "tool": "architecture_analysis_tool",
                "mode": "adp_violations",
                "artifact_scope": artifact_scope,
            },
            "sdp": {
                "tool": "architecture_analysis_tool",
                "mode": "sdp_violations",
                "artifact_scope": artifact_scope,
            },
            "sap": {
                "tool": "architecture_analysis_tool",
                "mode": "sap_violations",
                "artifact_scope": artifact_scope,
            },
        },
    }


# ---------------------------------------------------------------------------
# Tool 13: list_communities  [EXPLORE]
# ---------------------------------------------------------------------------


def list_communities_func(
    repo_root: str | None = None,
    sort_by: str = "size",
    min_size: int = 0,
    detail_level: str = "standard",
    limit: int | None = None,
) -> ToolPayload:
    """List detected code communities in the codebase.

    [EXPLORE] Retrieves stored communities from the knowledge graph.
    Each community represents a cluster of related code entities
    (functions, classes) detected via the Leiden algorithm or
    file-based grouping.

    Args:
        repo_root: Repository root path. Auto-detected if omitted.
        sort_by: Sort column: size, cohesion, or name.
        min_size: Minimum community size to include (default: 0).
        detail_level: "standard" (default) returns full community data;
                      "minimal" returns only name, size, and cohesion
                      per community.
        limit: Optional maximum number of communities to return.

    Returns:
        List of communities with size and cohesion scores.
    """
    store = None
    try:
        store, root = _get_store(repo_root)
        if detail_level == "minimal":
            valid_sorts = {"size", "cohesion", "name"}
            sort = sort_by if sort_by in valid_sorts else "size"
            order = "DESC" if sort in ("size", "cohesion") else "ASC"
            with borrowed_sqlite_connection(store) as conn:
                rows = conn.execute(
                    "SELECT name, size, cohesion FROM communities "
                    f"WHERE size >= ? ORDER BY {sort} {order}",  # nosec B608
                    (min_size,),
                ).fetchall()
            communities = [
                {
                    "name": row["name"],
                    "size": row["size"],
                    "cohesion": row["cohesion"],
                }
                for row in rows
            ]
        else:
            communities = get_communities(store, sort_by=sort_by, min_size=min_size)
        total = len(communities)
        visible_communities = communities[:limit] if limit is not None else communities
        truncated = limit is not None and total > limit
        result: ToolPayload = {
            "status": "ok",
            "summary": f"Found {total} communities"
            + (f". Showing first {limit}." if truncated else ""),
            "communities": visible_communities,
            "total": total,
            "truncated": truncated,
        }
        apply_output_budget(result, budget_tokens=4000, list_priorities=["communities"])
        result["_hints"] = generate_hints("list_communities", result, get_session())
        return result
    except Exception as exc:
        return handle_tool_runtime_error(exc, logger=logger, context="list_communities")
    finally:
        if store is not None:
            store.close()


# ---------------------------------------------------------------------------
# Tool 14: get_community  [EXPLORE]
# ---------------------------------------------------------------------------


def get_community_func(
    community_name: str | None = None,
    community_id: int | None = None,
    include_members: bool = False,
    repo_root: str | None = None,
) -> ToolPayload:
    """Get details of a single code community.

    [EXPLORE] Retrieves a community by its database ID or by name match.
    Optionally includes the full list of member nodes.

    Args:
        community_name: Name to search for (partial match). Ignored if
                        community_id given.
        community_id: Database ID of the community.
        include_members: If True, include full member node details.
        repo_root: Repository root path. Auto-detected if omitted.

    Returns:
        Community details, or not_found status.
    """
    store = None
    try:
        store, root = _get_store(repo_root)
        community: CommunityRecord | None = None
        all_communities = get_communities(store)

        if community_id is not None:
            for c in all_communities:
                if c.get("id") == community_id:
                    community = c
                    break
        elif community_name is not None:
            for c in all_communities:
                if community_name.lower() in c["name"].lower():
                    community = c
                    break

        if community is None:
            return {
                "status": "not_found",
                "summary": ("No community found matching the given criteria."),
            }

        # member_qns is the full list of qualified names — trim when not requested
        if not include_members and "member_qns" in community:
            qns = community.pop("member_qns")
            qns_list = list(qns or [])
            community["total_members"] = len(qns_list)
            community["member_qns_sample"] = qns_list[:5]
        elif include_members:
            cid = community.get("id")
            if cid is not None:
                member_nodes = store.get_nodes_by_community_id(cid)
                members = [node_to_dict(n) for n in member_nodes]
                community["member_details"] = members
                apply_output_budget(
                    cast(ToolPayload, community),
                    budget_tokens=5000,
                    list_priorities=["member_details"],
                )

        result: ToolPayload = {
            "status": "ok",
            "summary": (
                f"Community '{community['name']}': "
                f"{community['size']} nodes, "
                f"cohesion {community['cohesion']:.4f}"
            ),
            "community": community,
        }
        result["_hints"] = cast(ToolPayload, generate_hints("get_community", result, get_session()))
        return result
    except Exception as exc:
        return handle_tool_runtime_error(exc, logger=logger, context="get_community")
    finally:
        if store is not None:
            store.close()


# ---------------------------------------------------------------------------
# Tool 15: get_architecture_overview  [EXPLORE]
# ---------------------------------------------------------------------------


def get_architecture_overview_func(
    repo_root: str | None = None,
    detail_level: str = "standard",
    top_n: int = 20,
    artifact_scope: ArtifactScope = "code",
) -> ToolPayload:
    """Generate an architecture overview based on community structure.

    [EXPLORE] Builds a high-level view of the codebase architecture by
    analyzing community boundaries and cross-community coupling.
    Includes warnings for high coupling between communities.

    detail_level controls output size:
      "minimal"  — name/size/cohesion per community, top-5 coupling pairs, warnings
      "standard" — full community metadata (no member lists), top-N coupling pairs
      "verbose"  — adds member lists and raw per-edge cross_community_edges list

    Args:
        repo_root: Repository root path. Auto-detected if omitted.
        detail_level: Output verbosity: "minimal", "standard" (default), "verbose".
        top_n: Max coupling pairs in standard mode (default 20).
        artifact_scope: Scope for ADP/SDP/SAP health signals: "code" (default),
            "docs", or "all".

    Returns:
        Architecture overview with communities, cross_community_coupling, and warnings.
    """
    store = None
    try:
        store, root = _get_store(repo_root)
        answerability = graph_answerability_summary(store)
        overview = get_architecture_overview(store, detail_level=detail_level, top_n=top_n)
        n_communities = len(overview["communities"])
        n_coupling = len(overview["cross_community_coupling"])
        n_warnings = len(overview["warnings"])
        total_pairs = len(overview["cross_community_coupling"])
        shown_note = f" (top {total_pairs} shown)" if detail_level == "standard" else ""
        result: ToolPayload = {
            "status": "ok",
            "summary": (
                f"Architecture: {n_communities} communities, "
                f"{n_coupling} coupled pairs{shown_note}, "
                f"{n_warnings} warning(s)"
            ),
            "artifact_scope": artifact_scope,
            **overview,
        }
        from ..analysis import build_graph_snapshot

        snapshot = build_graph_snapshot(store)
        result["architecture_health"] = _architecture_health_summary(
            store,
            overview,
            top_n=top_n,
            artifact_scope=artifact_scope,
            snapshot=snapshot,
        )
        result["stable_component_policy"] = stability_policy_summary(
            component_stability_profiles(store, snapshot=snapshot),
            limit=min(max(top_n, 1), 5),
        )
        result["answerability"] = answerability
        result["missingness"] = missingness_from_answerability(answerability)
        apply_output_budget(
            result,
            budget_tokens=4000,
            list_priorities=[
                "architecture_health",
                "warnings",
                "communities",
                "cross_community_coupling",
                "cross_community_edges",
            ],
        )
        architecture_health = result.get("architecture_health")
        guidance = (
            architecture_health.get("guidance", []) if isinstance(architecture_health, dict) else []
        )
        result["_hints"] = guidance_actions_to_hints(guidance)
        if not result["_hints"]["next_steps"]:
            result["_hints"] = generate_hints("get_architecture_overview", result, get_session())
        return result
    except Exception as exc:
        return handle_tool_runtime_error(exc, logger=logger, context="get_architecture_overview")
    finally:
        if store is not None:
            store.close()
