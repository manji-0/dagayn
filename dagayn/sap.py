"""Stable Abstractions Principle (SAP) analysis.

Computes per-scope metrics A (abstractness), I (instability), and
D (distance from the main sequence) for any dagayn GraphStore.

Reference formulas
------------------
  Na  = number of abstract or contract-like top-level types in scope
  Nt  = number of eligible top-level types in scope
  Ce  = number of distinct outgoing dependent scopes
  Ca  = number of distinct incoming dependent scopes

  A = Na / Nt          (0 if Nt = 0)
  I = Ce / (Ca + Ce)   (0 if Ca + Ce = 0)
  D = |A + I - 1|

Default edges: IMPORTS_FROM + DEPENDS_ON + INHERITS + IMPLEMENTS (fixed)
INHERITS/IMPLEMENTS targets are resolved first by qualified name, then by
bare name when exactly one in-repo node has that name (stdlib names drop out
naturally when they have no matching node).
"""

from __future__ import annotations

import logging
from collections import defaultdict
from typing import Iterable, Literal, Optional

from ._scope import build_node_scope_maps
from .graph import GraphStore

logger = logging.getLogger(__name__)

_SAP_EDGE_KINDS: frozenset[str] = frozenset(
    {"IMPORTS_FROM", "DEPENDS_ON", "INHERITS", "IMPLEMENTS"}
)

_ELIGIBLE_ROLES: frozenset[str] = frozenset(
    {"class", "abstract_class", "interface", "protocol", "trait", "abstract_type", "mixin"}
)

_ABSTRACT_ROLES: frozenset[str] = frozenset(
    {"abstract_class", "interface", "protocol", "trait", "abstract_type"}
)


def compute_sap_metrics(
    store: GraphStore,
    scope_kind: Literal["file", "package", "directory"] = "package",
    unit_filter: Optional[Iterable[str]] = None,
) -> list[dict]:
    """Compute SAP metrics for each scope.

    Returns list of dicts sorted by distance descending. Each dict contains:
    scope_kind, scope_key, display_name, na, nt, ca, ce,
    abstractness, instability, distance, member_count,
    top_incoming_dependencies, top_outgoing_dependencies.
    Scopes with no eligible types or zero couplings include a ``notes`` key.

    Args:
        store: GraphStore instance.
        scope_kind: Aggregation granularity — "file", "package" (default),
            or "directory" (synonym for package in this implementation).
        unit_filter: Optional sequence of scope_key prefix strings to
            restrict output to matching scopes.
    """
    filter_prefixes = list(unit_filter) if unit_filter else None

    qualified_to_scope, name_to_scope = build_node_scope_maps(store, scope_kind)

    scope_na: dict[str, int] = defaultdict(int)
    scope_nt: dict[str, int] = defaultdict(int)
    scope_member_count: dict[str, int] = defaultdict(int)

    for node in store.get_all_nodes(exclude_files=False):
        sk = qualified_to_scope.get(node.qualified_name)
        if sk is None:
            continue
        scope_member_count[sk] += 1

        if node.kind == "Class" and node.parent_name is None:
            extra = node.extra or {}
            role = extra.get("type_role", "class")
            if role in _ELIGIBLE_ROLES:
                scope_nt[sk] += 1
                is_abstract = extra.get("is_abstract", False)
                is_contract = extra.get("is_contract", False)
                if is_abstract or is_contract or role in _ABSTRACT_ROLES:
                    scope_na[sk] += 1

    # Build scope-to-scope dependency counts for Ce/Ca
    dep_graph: dict[str, dict[str, int]] = defaultdict(lambda: defaultdict(int))
    all_scopes: set[str] = set(scope_member_count.keys())

    for edge in store.get_all_edges():
        if edge.kind not in _SAP_EDGE_KINDS:
            continue
        src_scope = qualified_to_scope.get(edge.source_qualified)
        if src_scope is None:
            continue
        tgt_scope = qualified_to_scope.get(edge.target_qualified)
        if tgt_scope is None:
            tgt_scope = name_to_scope.get(edge.target_qualified)
        if tgt_scope is None or src_scope == tgt_scope:
            continue
        dep_graph[src_scope][tgt_scope] += 1
        all_scopes.add(src_scope)
        all_scopes.add(tgt_scope)

    results = []
    for sk in all_scopes:
        if filter_prefixes and not any(sk.startswith(p) for p in filter_prefixes):
            continue

        nt = scope_nt.get(sk, 0)
        na = scope_na.get(sk, 0)
        outgoing = dep_graph.get(sk, {})
        ce = len(outgoing)
        incoming = {s: dep_graph[s][sk] for s in dep_graph if sk in dep_graph[s]}
        ca = len(incoming)

        notes: list[str] = []
        abstractness = na / nt if nt > 0 else 0.0
        if nt == 0:
            notes.append("no-eligible-types")

        total = ca + ce
        instability = ce / total if total > 0 else 0.0
        if total == 0:
            notes.append("isolated")

        distance = abs(abstractness + instability - 1.0)

        top_out = sorted(outgoing.items(), key=lambda x: x[1], reverse=True)[:5]
        top_in = sorted(incoming.items(), key=lambda x: x[1], reverse=True)[:5]

        entry: dict = {
            "scope_kind": scope_kind,
            "scope_key": sk,
            "display_name": sk,
            "na": na,
            "nt": nt,
            "ca": ca,
            "ce": ce,
            "abstractness": round(abstractness, 4),
            "instability": round(instability, 4),
            "distance": round(distance, 4),
            "member_count": scope_member_count.get(sk, 0),
            "top_incoming_dependencies": [{"scope": s, "count": c} for s, c in top_in],
            "top_outgoing_dependencies": [{"scope": s, "count": c} for s, c in top_out],
        }
        if notes:
            entry["notes"] = notes
        results.append(entry)

    results.sort(key=lambda x: x["distance"], reverse=True)
    return results


def find_sap_violations(
    store: GraphStore,
    scope_kind: Literal["file", "package", "directory"] = "package",
    min_distance: float = 0.5,
) -> list[dict]:
    """Find scopes whose distance from the main sequence exceeds min_distance.

    Args:
        store: GraphStore instance.
        scope_kind: Aggregation granularity.
        min_distance: Minimum D value to flag (exclusive). Default: 0.5.
    """
    metrics = compute_sap_metrics(
        store,
        scope_kind=scope_kind,
    )
    violations = [m for m in metrics if m["distance"] > min_distance and m["ca"] + m["ce"] > 0]
    violations.sort(key=lambda x: x["distance"], reverse=True)
    return violations
