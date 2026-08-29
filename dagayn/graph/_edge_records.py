"""Helpers for serializing graph edge metadata."""

from __future__ import annotations

import json
from typing import TYPE_CHECKING

from dagayn.state_types import ConfidenceTier, normalize_confidence_tier

if TYPE_CHECKING:
    from dagayn.parser._base.types import EdgeInfo

type EdgeStorageMetadata = tuple[str, float, ConfidenceTier]


def edge_storage_metadata(edge: EdgeInfo) -> EdgeStorageMetadata:
    """Return serialized edge metadata and normalized confidence fields."""
    extra = edge.extra or {}
    confidence = float(extra.get("confidence", 1.0))
    explicit_tier = str(extra.get("confidence_tier") or "").upper()
    confidence_tier = normalize_confidence_tier(extra.get("confidence_tier"))
    if (edge.target.startswith("<unresolved:") or edge.source.startswith("<unresolved:")) and (
        not explicit_tier or explicit_tier in {"EXTRACTED", "UNKNOWN"}
    ):
        confidence = min(confidence, 0.2)
        confidence_tier = "LOW"
    return (
        json.dumps(extra),
        confidence,
        confidence_tier,
    )
