"""Helpers for materializing graph edge rows."""

from __future__ import annotations

import json
from typing import TYPE_CHECKING

from ..state_types import ConfidenceTier, normalize_confidence_tier
from ._sql import _edge_target_name

if TYPE_CHECKING:
    from ..parser._base.types import EdgeInfo


def edge_storage_metadata(edge: EdgeInfo) -> tuple[str, float, ConfidenceTier]:
    """Return serialized edge metadata and normalized confidence fields."""
    extra = edge.extra or {}
    confidence = float(extra.get("confidence", 1.0))
    confidence_tier = normalize_confidence_tier(extra.get("confidence_tier"))
    if edge.target.startswith("<unresolved:") or edge.source.startswith("<unresolved:"):
        confidence = min(confidence, 0.2)
        confidence_tier = "LOW"
    return (
        json.dumps(extra),
        confidence,
        confidence_tier,
    )


def edge_insert_values(edge: EdgeInfo, updated_at: float) -> tuple:
    """Return the SQLite values used by edge INSERT statements."""
    extra, confidence, confidence_tier = edge_storage_metadata(edge)
    return (
        edge.kind,
        edge.source,
        edge.target,
        _edge_target_name(edge.target),
        edge.file_path,
        edge.line,
        extra,
        confidence,
        confidence_tier,
        updated_at,
    )


def edge_update_values(edge: EdgeInfo, updated_at: float, edge_id: int) -> tuple:
    """Return the SQLite values used by id-keyed edge UPDATE statements."""
    extra, confidence, confidence_tier = edge_storage_metadata(edge)
    return (
        _edge_target_name(edge.target),
        edge.line,
        extra,
        confidence,
        confidence_tier,
        updated_at,
        edge_id,
    )


def edge_identity_update_values(edge: EdgeInfo, updated_at: float) -> tuple:
    """Return SET + WHERE values for identity-keyed UPDATE … RETURNING id.

    Identity key: ``(kind, source_qualified, target_qualified, file_path, line)``.
    """
    extra, confidence, confidence_tier = edge_storage_metadata(edge)
    return (
        _edge_target_name(edge.target),
        extra,
        confidence,
        confidence_tier,
        updated_at,
        edge.kind,
        edge.source,
        edge.target,
        edge.file_path,
        edge.line,
    )
