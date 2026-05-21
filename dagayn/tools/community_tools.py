"""Tools 13, 14, 15: community listing, detail, architecture overview."""

from __future__ import annotations

from typing import Any

from .._scope import ArtifactScope
from ..communities import get_architecture_overview, get_communities
from ..graph import node_to_dict
from ..hints import generate_hints, get_session
from ._common import _get_store, apply_output_budget


def _architecture_health_summary(
    store: Any,
    overview: dict[str, Any],
    *,
    top_n: int,
    artifact_scope: ArtifactScope,
) -> dict[str, Any]:
    """Compose specialized architecture signals into one bounded report."""
    example_limit = min(max(top_n, 1), 5)

    try:
        from ..analysis import (
            find_bridge_nodes,
            find_hub_nodes,
            find_knowledge_gaps,
            find_surprising_connections,
        )
        from ..architecture import find_adp_violations, find_sdp_violations
        from ..sap import find_sap_violations

        hubs = find_hub_nodes(store, top_n=example_limit)
        bridges = find_bridge_nodes(store, top_n=example_limit)
        gaps = find_knowledge_gaps(store, top_n=example_limit)
        surprises = find_surprising_connections(store, top_n=example_limit)
        adp = find_adp_violations(
            store,
            granularity="package",
            artifact_scope=artifact_scope,
        )[:example_limit]
        sdp = find_sdp_violations(
            store,
            granularity="package",
            artifact_scope=artifact_scope,
        )[:example_limit]
        sap = find_sap_violations(
            store,
            scope_kind="package",
            artifact_scope=artifact_scope,
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
        "isolated_nodes",
        "thin_communities",
        "untested_hotspots",
        "single_file_communities",
    )
    gap_meta = gaps.get("_meta", {})
    raw_gap_counts = gap_meta.get("raw_counts", {})
    gap_counts = {key: int(raw_gap_counts.get(key, len(gaps.get(key, [])))) for key in gap_keys}

    reason_codes: list[str] = []
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

    return {
        "status": "ok",
        "scoring_policy": {
            "version": "architecture-health-v1",
            "artifact_scope": artifact_scope,
            "signals": [
                "community_coupling",
                "hub_nodes",
                "bridge_nodes",
                "knowledge_gaps",
                "surprising_connections",
                "adp",
                "sdp",
                "sap",
            ],
            "bounded_top_n": example_limit,
        },
        "counts": {
            "communities": len(overview.get("communities", [])),
            "coupled_pairs_shown": len(overview.get("cross_community_coupling", [])),
            "warnings": len(overview.get("warnings", [])),
            "hub_nodes": len(hubs),
            "bridge_nodes": len(bridges),
            "knowledge_gaps": sum(gap_counts.values()),
            "surprising_connections": len(surprises),
            "adp_violations": len(adp),
            "sdp_violations": len(sdp),
            "sap_violations": len(sap),
        },
        "reason_codes": reason_codes,
        "top_examples": {
            "hub_nodes": hubs,
            "bridge_nodes": bridges,
            "knowledge_gaps": {key: gaps.get(key, [])[: min(3, example_limit)] for key in gap_keys},
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
) -> dict[str, Any]:
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

    Returns:
        List of communities with size and cohesion scores.
    """
    store, root = _get_store(repo_root)
    try:
        if detail_level == "minimal":
            valid_sorts = {"size", "cohesion", "name"}
            sort = sort_by if sort_by in valid_sorts else "size"
            order = "DESC" if sort in ("size", "cohesion") else "ASC"
            rows = store._conn.execute(
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
        result: dict[str, object] = {
            "status": "ok",
            "summary": f"Found {len(communities)} communities",
            "communities": communities,
        }
        apply_output_budget(result, budget_tokens=4000, list_priorities=["communities"])
        result["_hints"] = generate_hints("list_communities", result, get_session())
        return result
    except Exception as exc:
        return {"status": "error", "error": str(exc)}
    finally:
        store.close()


# ---------------------------------------------------------------------------
# Tool 14: get_community  [EXPLORE]
# ---------------------------------------------------------------------------


def get_community_func(
    community_name: str | None = None,
    community_id: int | None = None,
    include_members: bool = False,
    repo_root: str | None = None,
) -> dict[str, Any]:
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
    store, root = _get_store(repo_root)
    try:
        community: dict | None = None
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
            community["total_members"] = len(qns)
            community["member_qns_sample"] = qns[:5]
        elif include_members:
            cid = community.get("id")
            if cid is not None:
                member_nodes = store.get_nodes_by_community_id(cid)
                members = [node_to_dict(n) for n in member_nodes]
                community["member_details"] = members
                apply_output_budget(
                    community, budget_tokens=5000, list_priorities=["member_details"]
                )

        result = {
            "status": "ok",
            "summary": (
                f"Community '{community['name']}': "
                f"{community['size']} nodes, "
                f"cohesion {community['cohesion']:.4f}"
            ),
            "community": community,
        }
        result["_hints"] = generate_hints("get_community", result, get_session())
        return result
    except Exception as exc:
        return {"status": "error", "error": str(exc)}
    finally:
        store.close()


# ---------------------------------------------------------------------------
# Tool 15: get_architecture_overview  [EXPLORE]
# ---------------------------------------------------------------------------


def get_architecture_overview_func(
    repo_root: str | None = None,
    detail_level: str = "standard",
    top_n: int = 20,
    artifact_scope: ArtifactScope = "code",
) -> dict[str, Any]:
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
    store, root = _get_store(repo_root)
    try:
        overview = get_architecture_overview(store, detail_level=detail_level, top_n=top_n)
        n_communities = len(overview["communities"])
        n_coupling = len(overview["cross_community_coupling"])
        n_warnings = len(overview["warnings"])
        total_pairs = len(overview["cross_community_coupling"])
        shown_note = f" (top {total_pairs} shown)" if detail_level == "standard" else ""
        result = {
            "status": "ok",
            "summary": (
                f"Architecture: {n_communities} communities, "
                f"{n_coupling} coupled pairs{shown_note}, "
                f"{n_warnings} warning(s)"
            ),
            "artifact_scope": artifact_scope,
            **overview,
        }
        result["architecture_health"] = _architecture_health_summary(
            store,
            overview,
            top_n=top_n,
            artifact_scope=artifact_scope,
        )
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
        result["_hints"] = generate_hints("get_architecture_overview", result, get_session())
        return result
    except Exception as exc:
        return {"status": "error", "error": str(exc)}
    finally:
        store.close()
