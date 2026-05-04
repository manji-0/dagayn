"""Community-driven refactoring suggestions."""

from __future__ import annotations

import logging
from typing import Any

from ..graph import GraphStore, _sanitize_name
from .dead_code import find_dead_code

logger = logging.getLogger(__name__)


def suggest_refactorings(store: GraphStore) -> list[dict[str, Any]]:
    """Produce community-driven refactoring suggestions.

    Currently two categories:
    - **move**: Functions in Community A only called by Community B.
    - **remove**: Dead code (no callers, tests, or importers and not entry points).

    Returns:
        List of suggestion dicts with type, description, symbols, rationale.
    """
    suggestions: list[dict[str, Any]] = []

    community_rows = store.get_communities_list()

    if community_rows:
        node_community: dict[str, int] = {}
        members_by_id = store.get_all_community_member_qns()
        for crow in community_rows:
            cid = crow["id"]
            member_qns = members_by_id.get(cid, [])
            for qn in member_qns:
                node_community[qn] = cid

        community_names: dict[int, str] = {r["id"]: r["name"] for r in community_rows}

        all_funcs = store.get_nodes_by_kind(["Function"])
        func_qns = [fnode.qualified_name for fnode in all_funcs]
        _, incoming_by_qn = store.get_edges_by_endpoints(func_qns)

        for fnode in all_funcs:
            f_community = node_community.get(fnode.qualified_name)
            if f_community is None:
                continue

            incoming_calls = [
                e for e in incoming_by_qn.get(fnode.qualified_name, []) if e.kind == "CALLS"
            ]
            if not incoming_calls:
                continue

            caller_communities = set()
            for edge in incoming_calls:
                c_community = node_community.get(edge.source_qualified)
                if c_community is not None:
                    caller_communities.add(c_community)

            if len(caller_communities) == 1:
                target_community = next(iter(caller_communities))
                if target_community != f_community:
                    src_name = community_names.get(f_community, f"community-{f_community}")
                    tgt_name = community_names.get(
                        target_community, f"community-{target_community}"
                    )
                    suggestions.append(
                        {
                            "type": "move",
                            "description": (
                                f"Move '{_sanitize_name(fnode.name)}' from "
                                f"'{src_name}' to '{tgt_name}'"
                            ),
                            "symbols": [_sanitize_name(fnode.qualified_name)],
                            "rationale": (
                                f"Function is in community '{src_name}' but only "
                                f"called by members of community '{tgt_name}'."
                            ),
                            "priority": "medium",
                            "confidence": "medium",
                            "estimated_risk": "medium",
                            "affected_files": [fnode.file_path],
                            "verification_steps": [
                                "Review imports and call sites before moving the function.",
                                "Run tests for both source and target communities.",
                            ],
                        }
                    )

    dead = find_dead_code(store)
    for d in dead:
        evidence = d.get("evidence", {})
        suggestions.append(
            {
                "type": "remove",
                "description": f"Remove unused {d['kind'].lower()} '{d['name']}'",
                "symbols": [d["qualified_name"]],
                "rationale": (
                    "No callers, test references, importers, references, or subclasses "
                    "were found in the graph."
                ),
                "priority": "low",
                "confidence": d.get("confidence", "medium"),
                "estimated_risk": "medium",
                "affected_files": [d["file"]],
                "reason_codes": d.get("reason_codes", []),
                "evidence": evidence,
                "verification_steps": [
                    "Search for runtime registration or dynamic dispatch before deleting.",
                    "Run the tests that cover the affected file or package.",
                ],
            }
        )

    logger.info("suggest_refactorings: produced %d suggestions", len(suggestions))
    return suggestions
