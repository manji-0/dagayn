"""Shared helpers for CROSS_ARTIFACT bridge analysis.

Phase 4 analysis integration treats reportable bridges as first-class
transitions for impact, flows, review, and architecture guidance, while
keeping low-confidence bridges as missingness/caveats rather than hard claims.
"""

from __future__ import annotations

from typing import Any

# Tiers safe to treat as hard structural claims in impact/flow traversal.
REPORTABLE_CONFIDENCE_TIERS: frozenset[str] = frozenset({"EXACT", "HIGH", "EXTRACTED"})


def edge_extra(edge: Any) -> dict[str, Any]:
    extra = getattr(edge, "extra", None)
    return extra if isinstance(extra, dict) else {}


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
    return target.startswith("<unresolved:")


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


def is_low_confidence_bridge(edge: Any) -> bool:
    """True when a CROSS_ARTIFACT edge must not be treated as a hard claim."""
    if not is_cross_artifact(edge):
        return False
    if is_unresolved_target(edge):
        return True
    if is_low_confidence_unresolved_markdown_code_span(edge):
        return True
    tier = confidence_tier_of(edge)
    if tier == "LOW" or not tier:
        return True
    return False


def is_reportable_bridge(edge: Any) -> bool:
    """True when a CROSS_ARTIFACT edge may expand impact/flows as a hard claim."""
    if not is_cross_artifact(edge):
        return False
    if is_unresolved_target(edge):
        return False
    if is_low_confidence_bridge(edge):
        return False
    return confidence_tier_of(edge) in REPORTABLE_CONFIDENCE_TIERS


def bridge_transition_dict(edge: Any) -> dict[str, Any]:
    """Explainable path payload for a CROSS_ARTIFACT hop."""
    extra = edge_extra(edge)
    return {
        "kind": "CROSS_ARTIFACT",
        "source": getattr(edge, "source_qualified", None),
        "target": getattr(edge, "target_qualified", None),
        "relationship_role": cross_artifact_role(edge),
        "bridge_kind": extra.get("bridge_kind"),
        "evidence_kind": extra.get("evidence_kind"),
        "evidence_source": extra.get("evidence_source"),
        "confidence": getattr(edge, "confidence", None),
        "confidence_tier": confidence_tier_of(edge) or None,
        "file_path": getattr(edge, "file_path", None),
        "line": getattr(edge, "line", None),
        "claim_strength": "hard" if is_reportable_bridge(edge) else "caveat",
    }


def low_confidence_bridge_missingness(edge: Any) -> dict[str, Any]:
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


def annotate_flow_steps_with_bridges(
    steps: list[dict[str, Any]],
    edges: list[Any],
) -> list[dict[str, Any]]:
    """Mark flow steps that arrive via CROSS_ARTIFACT edges among path nodes.

    The stored flow path is BFS discovery order, so consecutive steps are not
    necessarily parent/child. Arrival is inferred from reportable
    CROSS_ARTIFACT edges whose endpoints are both in the path.
    """
    if not steps:
        return steps

    path_qns = {
        str(step.get("qualified_name"))
        for step in steps
        if isinstance(step.get("qualified_name"), str)
    }
    arrivals: dict[str, dict[str, Any]] = {}
    for edge in edges:
        if not is_reportable_bridge(edge):
            continue
        src = str(getattr(edge, "source_qualified", ""))
        tgt = str(getattr(edge, "target_qualified", ""))
        if src not in path_qns or tgt not in path_qns:
            continue
        arrivals[tgt] = bridge_transition_dict(edge)

    annotated: list[dict[str, Any]] = []
    for index, step in enumerate(steps):
        item = dict(step)
        qn = str(item.get("qualified_name") or "")
        if index == 0:
            item.setdefault("step_kind", "entry")
        elif qn in arrivals:
            item["step_kind"] = "bridge"
            item["transition"] = arrivals[qn]
            item["is_bridge_step"] = True
        else:
            item.setdefault("step_kind", "call")
            item.setdefault("is_bridge_step", False)
        annotated.append(item)
    return annotated


def collect_bridge_transitions(
    edges: list[Any],
    *,
    include_low_confidence: bool = False,
) -> tuple[list[dict[str, Any]], list[dict[str, Any]]]:
    """Split edges into reportable bridge transitions and low-confidence caveats."""
    transitions: list[dict[str, Any]] = []
    caveats: list[dict[str, Any]] = []
    for edge in edges:
        if not is_cross_artifact(edge):
            continue
        if is_reportable_bridge(edge):
            transitions.append(bridge_transition_dict(edge))
        elif include_low_confidence or is_low_confidence_bridge(edge):
            caveats.append(low_confidence_bridge_missingness(edge))
    return transitions, caveats
