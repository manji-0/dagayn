"""Typed state contracts for graph lifecycle and tool responses."""

from __future__ import annotations

from typing import Any, Literal, NotRequired, TypeAlias, TypedDict, cast

ConfidenceTier: TypeAlias = Literal["EXACT", "EXTRACTED", "HIGH", "MEDIUM", "LOW", "UNKNOWN"]
CrossArtifactRole: TypeAlias = Literal[
    "implemented_by",
    "implements_contract",
    "describes_symbol",
    "discusses_artifact",
    "raises_issue_for",
    "explained_by",
    "has_runbook",
    "problem_described_by",
    "discussed_by",
]

MarkdownArtifactResolutionState: TypeAlias = Literal[
    "resolved",
    "dropped",
    "re_resolved",
    "still_unresolved",
]

EmbeddingStatusCode: TypeAlias = Literal[
    "not_indexed",
    "unavailable",
    "empty",
    "unknown",
    "stale",
    "partial",
    "complete",
]

LocalEmbeddingProbeStatus: TypeAlias = Literal[
    "ready",
    "unreachable",
    "not_ready",
    "incompatible",
]

TraversalMode: TypeAlias = Literal["bfs", "dfs"]
ReachabilityState: TypeAlias = Literal["not_found", "complete", "truncated"]
RefactorMode: TypeAlias = Literal["rename", "dead_code", "suggest"]


class MarkdownArtifactResolution(TypedDict):
    state: MarkdownArtifactResolutionState
    edge_id: int
    target_qualified: NotRequired[str]
    target_language: NotRequired[str]
    confidence: NotRequired[float]
    confidence_tier: NotRequired[ConfidenceTier]
    extra: NotRequired[dict[str, Any]]


class EmbeddingStatus(TypedDict, total=False):
    status: EmbeddingStatusCode
    total_embeddings: int
    provider_counts: dict[str, int]
    embeddable_nodes: int
    indexed_embeddings: int
    missing_embeddings: int
    orphan_embeddings: int
    error: str


class TraversalEntry(TypedDict):
    name: str
    qualified_name: str
    kind: str
    file: str
    depth: int


class ReachabilityInfo(TypedDict):
    state: ReachabilityState
    truncated: bool
    max_depth: int
    nodes_visited: int


def normalize_confidence_tier(value: Any, default: ConfidenceTier = "EXTRACTED") -> ConfidenceTier:
    """Return a known confidence tier, preserving type-level state invariants."""
    tier = str(value or default).upper()
    if tier in {"EXACT", "EXTRACTED", "HIGH", "MEDIUM", "LOW", "UNKNOWN"}:
        return cast(ConfidenceTier, tier)
    return default
