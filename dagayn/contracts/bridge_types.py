"""Typed payloads shared by cross-artifact bridge analysis."""

from __future__ import annotations

from typing import Literal, TypedDict


class BridgeTransitionRecord(TypedDict, total=False):
    kind: Literal["CROSS_ARTIFACT"]
    source: str | None
    target: str | None
    relationship_role: str | None
    bridge_kind: str | None
    evidence_kind: str | None
    evidence_source: str | None
    confidence: float | None
    confidence_tier: str | None
    file_path: str | None
    line: int | None
    claim_strength: Literal["hard", "caveat"]


class BridgeMissingnessRecord(TypedDict, total=False):
    reason_code: str
    severity: str
    claim_effect: str
    bridge: BridgeTransitionRecord
