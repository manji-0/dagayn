"""Community API backed by ``dagayn._core``."""

from __future__ import annotations

import json
from typing import Any, TypedDict

from .graph import GraphStore


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


def detect_communities(store: GraphStore, min_size: int = 2) -> list[Any]:
    """Detect communities in the code graph."""
    payload = json.loads(store.detect_communities_json(min_size))
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
    return store.count_affected_communities(changed_files)


def incremental_detect_communities(
    store: GraphStore,
    changed_files: list[str],
    min_size: int = 2,
    pre_affected_count: int | None = None,
) -> int:
    """Re-detect communities only if changed files affect existing communities."""
    if not changed_files:
        return 0
    return int(store.incremental_detect_communities(changed_files, min_size, pre_affected_count))


def store_communities(store: GraphStore, communities: list[Any]) -> int:
    """Store detected communities in the database."""
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
    return store.store_communities_json(json.dumps(payload))


def get_communities(store: GraphStore, sort_by: str = "size", min_size: int = 0) -> list[Any]:
    """Retrieve stored communities from the database."""
    valid_sorts = {"size", "cohesion", "name"}
    if sort_by not in valid_sorts:
        sort_by = "size"
    return json.loads(store.get_communities_json(sort_by, min_size))


def refresh_community_stats(store: GraphStore) -> dict[str, int]:
    """Recompute community size/cohesion from live node assignments."""
    return json.loads(store.refresh_community_stats_json())


__all__ = [
    "ArchitectureOverviewResult",
    "CommunityCouplingRecord",
    "CommunityMetricsPayload",
    "CommunityRecord",
    "CrossCommunityEdgeRecord",
    "count_affected_communities",
    "detect_communities",
    "get_communities",
    "incremental_detect_communities",
    "refresh_community_stats",
    "store_communities",
]
