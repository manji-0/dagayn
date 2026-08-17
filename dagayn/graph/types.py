"""Data types for the dagayn graph package."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Optional, TypedDict

from ..bridge_types import BridgeMissingnessRecord, BridgeTransitionRecord
from ..state_types import ConfidenceTier


@dataclass
class GraphNode:
    id: int
    kind: str
    name: str
    qualified_name: str
    file_path: str
    line_start: int
    line_end: int
    language: str
    parent_name: Optional[str]
    params: Optional[str]
    return_type: Optional[str]
    is_test: bool
    file_hash: Optional[str]
    extra: dict
    signature: Optional[str] = None


@dataclass
class GraphEdge:
    id: int
    kind: str
    source_qualified: str
    target_qualified: str
    file_path: str
    line: int
    extra: dict
    confidence: float = 1.0
    confidence_tier: ConfidenceTier = "EXTRACTED"


class ImpactRadiusResult(TypedDict):
    changed_nodes: list[GraphNode]
    impacted_nodes: list[GraphNode]
    impacted_files: list[str]
    edges: list[GraphEdge]
    bridge_transitions: list[BridgeTransitionRecord]
    low_confidence_bridges: list[BridgeMissingnessRecord]
    truncated: bool
    total_impacted: int


class SubgraphResult(TypedDict):
    nodes: list[GraphNode]
    edges: list[GraphEdge]


@dataclass
class FlowAdjacency:
    """In-memory adjacency structure for flow tracing.

    Loaded once via :meth:`GraphStore.load_flow_adjacency` and passed to
    ``trace_flows`` / ``compute_criticality`` to avoid per-edge SQLite
    point queries on large graphs.

    ``bridge_edges`` maps ``source -> target -> transition metadata`` for
    reportable ``CROSS_ARTIFACT`` hops included in ``calls_out``.
    """

    calls_out: dict[str, list[str]]
    has_tested_by: set[str]
    nodes_by_qn: dict[str, "GraphNode"]
    nodes_by_id: dict[int, "GraphNode"]
    bridge_edges: dict[str, dict[str, BridgeTransitionRecord]] | None = None


@dataclass(frozen=True)
class FtsQueryResult:
    """FTS5 search hits plus how the query was matched."""

    hits: list[tuple[int, float]]
    match_mode: str  # "and", "or", "phrase", or "none"


@dataclass
class GraphStats:
    total_nodes: int
    total_edges: int
    nodes_by_kind: dict[str, int]
    edges_by_kind: dict[str, int]
    languages: list[str]
    files_count: int
    last_updated: Optional[str]
