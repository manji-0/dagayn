"""Shared helpers for CROSS_ARTIFACT bridge analysis.

Phase 4 analysis integration treats reportable bridges as first-class
transitions for impact, flows, review, and architecture guidance, while
keeping low-confidence bridges as missingness/caveats rather than hard claims.
"""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from typing import Any, cast

from .bridge_types import BridgeMissingnessRecord, BridgeTransitionRecord

# Tiers safe to treat as hard structural claims in impact/flow traversal.
REPORTABLE_CONFIDENCE_TIERS: frozenset[str] = frozenset({"EXACT", "HIGH", "EXTRACTED"})


def edge_extra(edge: Any) -> dict[str, object]:
    extra = getattr(edge, "extra", None)
    return cast(dict[str, object], extra) if isinstance(extra, dict) else {}


def is_cross_artifact(edge: Any) -> bool:
    return getattr(edge, "kind", None) == "CROSS_ARTIFACT"


def cross_artifact_role(edge: Any) -> str | None:
    if not is_cross_artifact(edge):
        return None
    role = edge_extra(edge).get("relationship_role")
    return role if isinstance(role, str) else None


def confidence_tier_of(edge: Any) -> str:
    tier = getattr(edge, "confidence_tier", None) or edge_extra(edge).get("confidence_tier")
    return str(tier or "").upper()


def is_unresolved_target(edge: Any) -> bool:
    target = str(getattr(edge, "target_qualified", "") or "")
    return is_unresolvable_qualified_name(target)


def is_unresolvable_qualified_name(
    qualified_name: str,
    *,
    nodes_by_qn: Mapping[str, Any] | None = None,
) -> bool:
    """True when an edge endpoint cannot be resolved to a graph node row."""
    if qualified_name.startswith("<unresolved:"):
        return True
    if nodes_by_qn is not None:
        return qualified_name not in nodes_by_qn
    return False


def is_low_confidence_unresolved_markdown_code_span(edge: Any) -> bool:
    """True for noisy unresolved Markdown code-span bridges."""
    if not is_cross_artifact(edge):
        return False
    extra = edge_extra(edge)
    role = extra.get("relationship_role")
    evidence = str(extra.get("evidence_kind") or "")
    return (
        role == "describes_symbol"
        and is_unresolved_target(edge)
        and confidence_tier_of(edge) == "LOW"
        and evidence in {"markdown_code_span", ""}
    )


def is_low_confidence_resolved_implicit_markdown_code_span(edge: Any) -> bool:
    """True for resolved implicit Markdown code-span bridges capped at MEDIUM."""
    if not is_cross_artifact(edge):
        return False
    if is_unresolved_target(edge):
        return False
    extra = edge_extra(edge)
    if extra.get("relationship_role") != "describes_symbol":
        return False
    if extra.get("evidence_kind") != "markdown_code_span":
        return False
    if extra.get("evidence_source") != "code_span":
        return False
    return confidence_tier_of(edge) == "MEDIUM"


def is_low_confidence_bridge(edge: Any) -> bool:
    """True when a CROSS_ARTIFACT edge must not be treated as a hard claim.

    This is the exact complement of :func:`is_reportable_bridge` for
    CROSS_ARTIFACT edges: every bridge outside the reportable tiers (``LOW``,
    ``MEDIUM``, ``UNKNOWN``, missing) is surfaced as a caveat, so none
    disappears from both the claim and the caveat output.
    """
    if not is_cross_artifact(edge):
        return False
    if is_unresolved_target(edge):
        return True
    if is_low_confidence_unresolved_markdown_code_span(edge):
        return True
    if is_low_confidence_resolved_implicit_markdown_code_span(edge):
        return True
    return confidence_tier_of(edge) not in REPORTABLE_CONFIDENCE_TIERS


def is_reportable_bridge(edge: Any) -> bool:
    """True when a CROSS_ARTIFACT edge may expand impact/flows as a hard claim."""
    if not is_cross_artifact(edge):
        return False
    if is_unresolved_target(edge):
        return False
    if is_low_confidence_bridge(edge):
        return False
    return confidence_tier_of(edge) in REPORTABLE_CONFIDENCE_TIERS


def bridge_transition_dict(edge: Any) -> BridgeTransitionRecord:
    """Explainable path payload for a CROSS_ARTIFACT hop."""
    extra = edge_extra(edge)

    def _string(value: object) -> str | None:
        return value if isinstance(value, str) else None

    return {
        "kind": "CROSS_ARTIFACT",
        "source": _string(getattr(edge, "source_qualified", None)),
        "target": _string(getattr(edge, "target_qualified", None)),
        "relationship_role": cross_artifact_role(edge),
        "bridge_kind": _string(extra.get("bridge_kind")),
        "evidence_kind": _string(extra.get("evidence_kind")),
        "evidence_source": _string(extra.get("evidence_source")),
        "confidence": getattr(edge, "confidence", None),
        "confidence_tier": confidence_tier_of(edge) or None,
        "file_path": _string(getattr(edge, "file_path", None)),
        "line": getattr(edge, "line", None),
        "claim_strength": "hard" if is_reportable_bridge(edge) else "caveat",
    }


def low_confidence_bridge_missingness(edge: Any) -> BridgeMissingnessRecord:
    """Missingness item for a low-confidence bridge (caveat, not hard claim)."""
    meta = bridge_transition_dict(edge)
    return {
        "reason_code": "low_confidence_cross_artifact_bridge",
        "severity": "medium",
        "claim_effect": (
            "bridge is visible as a caveat only; do not treat the other side as confirmed impact"
        ),
        "bridge": {
            "source": meta.get("source"),
            "target": meta.get("target"),
            "relationship_role": meta.get("relationship_role"),
            "bridge_kind": meta.get("bridge_kind"),
            "confidence_tier": meta.get("confidence_tier"),
        },
    }


def collect_bridge_transitions(
    edges: Sequence[object],
    *,
    include_low_confidence: bool = False,
) -> tuple[list[BridgeTransitionRecord], list[BridgeMissingnessRecord]]:
    """Split edges into reportable bridge transitions and low-confidence caveats."""
    transitions: list[BridgeTransitionRecord] = []
    caveats: list[BridgeMissingnessRecord] = []
    for edge in edges:
        if not is_cross_artifact(edge):
            continue
        if is_reportable_bridge(edge):
            transitions.append(bridge_transition_dict(edge))
        elif include_low_confidence or is_low_confidence_bridge(edge):
            caveats.append(low_confidence_bridge_missingness(edge))
    return transitions, caveats
