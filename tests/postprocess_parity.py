"""Helpers for Phase 2 post-processing parity (flows and communities)."""

from __future__ import annotations

import json
from typing import TYPE_CHECKING, Any

from dagayn.communities import detect_communities
from dagayn.flows import DEFAULT_FLOW_MAX_DEPTH, DEFAULT_FLOW_MAX_NODES, trace_flows
from dagayn.graph import GraphStore as PythonGraphStore

if TYPE_CHECKING:
    from dagayn._core import GraphStore as RustGraphStore

# Phase 2 acceptance in docs/RUST-CORE-MIGRATION-WIP.md
PARITY_RELATIVE_TOLERANCE = 0.02
PARITY_BOUNDARY_AGREEMENT_MIN = 1.0 - PARITY_RELATIVE_TOLERANCE
DEFAULT_MIN_COMMUNITY_SIZE = 2


def relative_count_delta(python_count: int, rust_count: int) -> float:
    """Return the relative absolute delta between two non-negative counts."""
    baseline = max(python_count, 1)
    return abs(rust_count - python_count) / baseline


def community_boundary_agreement(
    python_communities: list[dict[str, Any]],
    rust_communities: list[dict[str, Any]],
) -> float:
    """Pairwise agreement between two community partitions over member QNs."""
    python_map = _community_membership_map(python_communities)
    rust_map = _community_membership_map(rust_communities)
    nodes = sorted(set(python_map) | set(rust_map))
    if len(nodes) < 2:
        return 1.0

    agree = 0
    total = 0
    for left_index, left in enumerate(nodes):
        for right in nodes[left_index + 1 :]:
            total += 1
            same_python = python_map.get(left) == python_map.get(right)
            same_rust = rust_map.get(left) == rust_map.get(right)
            if same_python == same_rust:
                agree += 1
    return agree / total


def detect_communities_python(
    store: PythonGraphStore,
    min_size: int = DEFAULT_MIN_COMMUNITY_SIZE,
) -> list[dict[str, Any]]:
    return detect_communities(store, min_size=min_size)


def detect_communities_rust(
    store: RustGraphStore,
    min_size: int = DEFAULT_MIN_COMMUNITY_SIZE,
) -> list[dict[str, Any]]:
    return json.loads(store.detect_communities_json(min_size))


def trace_flows_python(
    store: PythonGraphStore,
    max_depth: int = DEFAULT_FLOW_MAX_DEPTH,
    include_tests: bool = False,
    max_nodes: int = DEFAULT_FLOW_MAX_NODES,
) -> list[dict[str, Any]]:
    return trace_flows(
        store,
        max_depth=max_depth,
        include_tests=include_tests,
        max_nodes=max_nodes,
    )


def trace_flows_rust(
    store: RustGraphStore,
    max_depth: int = DEFAULT_FLOW_MAX_DEPTH,
    include_tests: bool = False,
    max_nodes: int = DEFAULT_FLOW_MAX_NODES,
) -> list[dict[str, Any]]:
    return json.loads(
        store.trace_flows_json(max_depth, include_tests, max_nodes),
    )


def _community_membership_map(
    communities: list[dict[str, Any]],
) -> dict[str, int]:
    mapping: dict[str, int] = {}
    for index, community in enumerate(communities):
        members = community.get("members")
        if members is None:
            members = community.get("member_qns", [])
        for qualified_name in members:
            mapping[str(qualified_name)] = index
    return mapping
