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

    dead = find_dead_code(store)
    for d in dead:
        suggestions.append(
            {
                "type": "remove",
                "description": f"Remove unused {d['kind'].lower()} '{d['name']}'",
                "symbols": [d["qualified_name"]],
                "rationale": "No callers, no test references, no importers, not an entry point.",
            }
        )

    community_rows = store.get_communities_list()

    if community_rows:
        node_community: dict[str, int] = {}
        for crow in community_rows:
            cid = crow["id"]
            member_qns = store.get_community_member_qns(cid)
            for qn in member_qns:
                node_community[qn] = cid

        community_names: dict[int, str] = {r["id"]: r["name"] for r in community_rows}

        all_funcs = store.get_nodes_by_kind(["Function"])

        for fnode in all_funcs:
            f_community = node_community.get(fnode.qualified_name)
            if f_community is None:
                continue

            incoming_calls = [
                e for e in store.get_edges_by_target(fnode.qualified_name) if e.kind == "CALLS"
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
                        }
                    )

    logger.info("suggest_refactorings: produced %d suggestions", len(suggestions))
    return suggestions
