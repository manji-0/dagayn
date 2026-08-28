//! CROSS_ARTIFACT bridge classification.
//!
//! Port of `dagayn.cross_artifact`: reportable bridges expand impact as hard
//! structural claims, low-confidence bridges are surfaced as caveats instead.

use crate::*;

/// Tiers safe to treat as hard structural claims in impact/flow traversal.
const REPORTABLE_CONFIDENCE_TIERS: &[&str] = &["EXACT", "HIGH", "EXTRACTED"];

pub(crate) fn is_cross_artifact(edge: &GraphEdge) -> bool {
    edge.kind == "CROSS_ARTIFACT"
}

pub(crate) fn cross_artifact_role(edge: &GraphEdge) -> Option<&str> {
    if !is_cross_artifact(edge) {
        return None;
    }
    edge.extra.get("relationship_role").and_then(Value::as_str)
}

/// The effective tier of a loaded edge.
///
/// Python's `confidence_tier_of` falls back to `extra.confidence_tier`, but a
/// row-loaded edge never needs it: both backends normalize a NULL column to
/// `EXTRACTED` while materializing the row, so the column is always set.
pub(crate) fn confidence_tier_of(edge: &GraphEdge) -> &'static str {
    edge.confidence_tier.as_str()
}

pub(crate) fn is_unresolved_target(edge: &GraphEdge) -> bool {
    edge.target_qualified.starts_with("<unresolved:")
}

fn extra_str<'a>(edge: &'a GraphEdge, key: &str) -> &'a str {
    edge.extra.get(key).and_then(Value::as_str).unwrap_or("")
}

/// Noisy unresolved Markdown code-span bridges.
fn is_low_confidence_unresolved_markdown_code_span(edge: &GraphEdge) -> bool {
    is_cross_artifact(edge)
        && extra_str(edge, "relationship_role") == "describes_symbol"
        && is_unresolved_target(edge)
        && confidence_tier_of(edge) == "LOW"
        && matches!(extra_str(edge, "evidence_kind"), "markdown_code_span" | "")
}

/// Resolved implicit Markdown code-span bridges capped at MEDIUM.
fn is_low_confidence_resolved_implicit_markdown_code_span(edge: &GraphEdge) -> bool {
    is_cross_artifact(edge)
        && !is_unresolved_target(edge)
        && extra_str(edge, "relationship_role") == "describes_symbol"
        && extra_str(edge, "evidence_kind") == "markdown_code_span"
        && extra_str(edge, "evidence_source") == "code_span"
        && confidence_tier_of(edge) == "MEDIUM"
}

/// True when a CROSS_ARTIFACT edge must not be treated as a hard claim.
pub(crate) fn is_low_confidence_bridge(edge: &GraphEdge) -> bool {
    if !is_cross_artifact(edge) {
        return false;
    }
    if is_unresolved_target(edge)
        || is_low_confidence_unresolved_markdown_code_span(edge)
        || is_low_confidence_resolved_implicit_markdown_code_span(edge)
    {
        return true;
    }
    confidence_tier_of(edge) == "LOW"
}

/// True when a CROSS_ARTIFACT edge may expand impact/flows as a hard claim.
pub(crate) fn is_reportable_bridge(edge: &GraphEdge) -> bool {
    if !is_cross_artifact(edge) || is_unresolved_target(edge) || is_low_confidence_bridge(edge) {
        return false;
    }
    REPORTABLE_CONFIDENCE_TIERS.contains(&confidence_tier_of(edge))
}

fn optional_str(value: &str) -> Value {
    if value.is_empty() {
        Value::Null
    } else {
        Value::String(value.to_string())
    }
}

/// Explainable path payload for a CROSS_ARTIFACT hop.
pub(crate) fn bridge_transition_value(edge: &GraphEdge) -> Value {
    json!({
        "kind": "CROSS_ARTIFACT",
        "source": edge.source_qualified.clone(),
        "target": edge.target_qualified.clone(),
        "relationship_role": cross_artifact_role(edge).map(str::to_string),
        "bridge_kind": optional_str(extra_str(edge, "bridge_kind")),
        "evidence_kind": optional_str(extra_str(edge, "evidence_kind")),
        "evidence_source": optional_str(extra_str(edge, "evidence_source")),
        "confidence": edge.confidence,
        "confidence_tier": optional_str(confidence_tier_of(edge)),
        "file_path": edge.file_path.clone(),
        "line": edge.line,
        "claim_strength": if is_reportable_bridge(edge) { "hard" } else { "caveat" },
    })
}

/// Missingness item for a low-confidence bridge (caveat, not hard claim).
pub(crate) fn low_confidence_bridge_missingness(edge: &GraphEdge) -> Value {
    let meta = bridge_transition_value(edge);
    json!({
        "reason_code": "low_confidence_cross_artifact_bridge",
        "severity": "medium",
        "claim_effect":
            "bridge is visible as a caveat only; do not treat the other side as confirmed impact",
        "bridge": {
            "source": meta.get("source").cloned().unwrap_or(Value::Null),
            "target": meta.get("target").cloned().unwrap_or(Value::Null),
            "relationship_role": meta.get("relationship_role").cloned().unwrap_or(Value::Null),
            "bridge_kind": meta.get("bridge_kind").cloned().unwrap_or(Value::Null),
            "confidence_tier": meta.get("confidence_tier").cloned().unwrap_or(Value::Null),
        },
    })
}

/// Split edges into reportable transitions and low-confidence caveats.
pub(crate) fn collect_bridge_transitions(edges: &[GraphEdge]) -> (Vec<Value>, Vec<Value>) {
    let mut transitions = Vec::new();
    let mut caveats = Vec::new();
    for edge in edges {
        if !is_cross_artifact(edge) {
            continue;
        }
        if is_reportable_bridge(edge) {
            transitions.push(bridge_transition_value(edge));
        } else if is_low_confidence_bridge(edge) {
            caveats.push(low_confidence_bridge_missingness(edge));
        }
    }
    (transitions, caveats)
}
