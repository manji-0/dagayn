"""Community API backed by ``dagayn._core``."""

from __future__ import annotations

import json
import re
from collections import Counter, defaultdict
from typing import Any, Callable, TypedDict, cast

from .graph import GraphStore, _sanitize_name


class CommunityRecord(TypedDict, total=False):
    id: int
    name: str
    level: int
    size: int
    cohesion: float
    dominant_language: str
    description: str
    members: list[str]
    member_qns: set[str] | list[str]
    assigned_member_count: int
    parent_id: int
    _cohesion_unmeasured: bool
    total_members: int
    member_qns_sample: list[str]
    member_details: list[dict[str, object]]


class CommunityMetricsPayload(TypedDict):
    internal_edges: int
    external_edges: int
    external_degree: int
    cohesion: float
    external_edge_ratio: float


class CrossCommunityEdgeRecord(TypedDict):
    source_community: int
    target_community: int
    edge_kind: str
    source: str
    target: str


class CommunityCouplingRecord(TypedDict):
    source_community_id: int
    source_community_name: str
    target_community_id: int
    target_community_name: str
    edge_count: int
    edge_kinds: dict[str, int]


class ArchitectureOverviewResult(TypedDict, total=False):
    communities: list[CommunityRecord]
    cross_community_coupling: list[CommunityCouplingRecord]
    warnings: list[str]
    cross_community_edges: list[CrossCommunityEdgeRecord]


def _require_native(store: GraphStore, name: str) -> Any:
    method = getattr(store, name, None)
    if not callable(method):
        raise RuntimeError(f"GraphStore.{name} is required (Rust GraphStore).")
    return method


def detect_communities(store: GraphStore, min_size: int = 2) -> list[Any]:
    """Detect communities in the code graph."""
    native = _require_native(store, "detect_communities_json")
    payload = json.loads(cast(str, native(min_size)))
    results: list[Any] = []
    for item in payload:
        if not isinstance(item, dict):
            continue
        results.append(
            {
                "name": str(item.get("name") or "community"),
                "level": int(item.get("level") or 0),
                "size": int(item.get("size") or 0),
                "cohesion": float(item.get("cohesion") or 0.0),
                "dominant_language": str(item.get("dominant_language") or ""),
                "description": str(item.get("description") or ""),
                "members": [str(member) for member in (item.get("members") or [])],
            }
        )
    return results


def count_affected_communities(store: GraphStore, changed_files: list[str]) -> int:
    """Return how many communities are affected by *changed_files*."""
    if not changed_files:
        return 0
    rust_count = _require_native(store, "count_affected_communities")
    return cast(Callable[[list[str]], int], rust_count)(changed_files)


def incremental_detect_communities(
    store: GraphStore,
    changed_files: list[str],
    min_size: int = 2,
    pre_affected_count: int | None = None,
) -> int:
    """Re-detect communities only if changed files affect existing communities."""
    if not changed_files:
        return 0
    native = _require_native(store, "incremental_detect_communities")
    return int(cast(int, native(changed_files, min_size, pre_affected_count)))


def store_communities(store: GraphStore, communities: list[Any]) -> int:
    """Store detected communities in the database."""
    rust_store = _require_native(store, "store_communities_json")
    payload = [
        {
            "name": comm["name"],
            "level": comm.get("level", 0),
            "cohesion": comm.get("cohesion", 0.0),
            "size": comm["size"],
            "dominant_language": comm.get("dominant_language", ""),
            "description": comm.get("description", ""),
            "members": list(comm.get("members", [])),
        }
        for comm in communities
    ]
    return cast(Callable[[str], int], rust_store)(json.dumps(payload))


def get_communities(store: GraphStore, sort_by: str = "size", min_size: int = 0) -> list[Any]:
    """Retrieve stored communities from the database."""
    valid_sorts = {"size", "cohesion", "name"}
    if sort_by not in valid_sorts:
        sort_by = "size"
    rust_get = _require_native(store, "get_communities_json")
    return json.loads(cast(Callable[[str, int], str], rust_get)(sort_by, min_size))


def refresh_community_stats(store: GraphStore) -> dict[str, int]:
    """Recompute community size/cohesion from live node assignments."""
    native = _require_native(store, "refresh_community_stats_json")
    return json.loads(cast(Callable[[], str], native)())


_TEST_COMMUNITY_RE = re.compile(
    r"(^test[-/]|[-/]test([:/]|$)|it:should|describe:|spec[-/]|[-/]spec$)",
    re.IGNORECASE,
)


def _is_test_community(name: str) -> bool:
    """Return True if a community name indicates it is test-dominated."""
    return bool(_TEST_COMMUNITY_RE.search(name))


def get_architecture_overview(
    store: GraphStore,
    detail_level: str = "standard",
    top_n: int = 20,
) -> ArchitectureOverviewResult:
    """Generate an architecture overview based on community structure."""
    communities = get_communities(store)
    node_to_community: dict[str, int] = {}
    for comm in communities:
        comm_id = comm.get("id", 0)
        for qn in comm.get("members", []):
            node_to_community[qn] = comm_id

    all_edges = store.get_all_edges()
    cross_counts: Counter[tuple[int, int]] = Counter()
    kind_counts: dict[tuple[int, int], Counter[str]] = defaultdict(Counter)
    cross_edges: list[dict[str, Any]] = []

    for edge in all_edges:
        if edge.kind == "TESTED_BY":
            continue
        src_comm = node_to_community.get(edge.source_qualified)
        tgt_comm = node_to_community.get(edge.target_qualified)
        if src_comm is not None and tgt_comm is not None and src_comm != tgt_comm:
            pair = (min(src_comm, tgt_comm), max(src_comm, tgt_comm))
            cross_counts[pair] += 1
            kind_counts[pair][edge.kind] += 1
            if detail_level == "verbose":
                cross_edges.append(
                    {
                        "source_community": src_comm,
                        "target_community": tgt_comm,
                        "edge_kind": edge.kind,
                        "source": _sanitize_name(edge.source_qualified),
                        "target": _sanitize_name(edge.target_qualified),
                    }
                )

    warnings: list[str] = []
    for comm in communities:
        stored_size = comm.get("size", 0)
        assigned = comm.get("assigned_member_count", len(comm.get("members", [])))
        if stored_size != assigned and not _is_test_community(comm["name"]):
            warnings.append(
                f"Community '{comm['name']}' stored size ({stored_size}) "
                f"differs from assigned members ({assigned}); "
                "run a full community refresh"
            )
    comm_name_map = {c.get("id", 0): c["name"] for c in communities}
    for (c1, c2), count in cross_counts.most_common():
        if count > 10:
            name1 = comm_name_map.get(c1, f"community-{c1}")
            name2 = comm_name_map.get(c2, f"community-{c2}")
            if _is_test_community(name1) or _is_test_community(name2):
                continue
            warnings.append(f"High coupling ({count} edges) between '{name1}' and '{name2}'")

    pair_limit = None if detail_level == "verbose" else (5 if detail_level == "minimal" else top_n)
    sorted_pairs = cross_counts.most_common(pair_limit)
    cross_community_coupling = [
        {
            "source_community_id": c1,
            "source_community_name": comm_name_map.get(c1, f"community-{c1}"),
            "target_community_id": c2,
            "target_community_name": comm_name_map.get(c2, f"community-{c2}"),
            "edge_count": count,
            "edge_kinds": dict(kind_counts[(c1, c2)]),
        }
        for (c1, c2), count in sorted_pairs
    ]

    if detail_level == "minimal":
        out_communities = [
            {
                "name": c["name"],
                "size": c["size"],
                "assigned_member_count": c.get("assigned_member_count", len(c.get("members", []))),
                "cohesion": c["cohesion"],
            }
            for c in communities
        ]
    elif detail_level == "verbose":
        out_communities = communities
    else:
        out_communities = [{k: v for k, v in c.items() if k != "members"} for c in communities]

    result: ArchitectureOverviewResult = {
        "communities": cast(list[CommunityRecord], out_communities),
        "cross_community_coupling": cast(list[CommunityCouplingRecord], cross_community_coupling),
        "warnings": warnings,
    }
    if detail_level == "verbose":
        result["cross_community_edges"] = cast(list[CrossCommunityEdgeRecord], cross_edges)
    return result


__all__ = [
    "ArchitectureOverviewResult",
    "CommunityCouplingRecord",
    "CommunityMetricsPayload",
    "CommunityRecord",
    "CrossCommunityEdgeRecord",
    "count_affected_communities",
    "detect_communities",
    "get_architecture_overview",
    "get_communities",
    "incremental_detect_communities",
    "refresh_community_stats",
    "store_communities",
]
