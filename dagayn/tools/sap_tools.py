"""MCP tool wrappers for Stable Abstractions Principle (SAP) analysis."""

from __future__ import annotations

from collections.abc import Mapping
from typing import Literal, Optional

from .._scope import ArtifactScope
from ..dependency_profiles import DependencyProfile, validate_dependency_profile
from ..sap import compute_sap_metrics, find_sap_violations
from ._common import ToolPayload, _error_response, _get_store, apply_output_budget, make_response


def _classify_sap_zone(violation: Mapping[str, object]) -> str:
    """Classify a SAP violation into a coarse architectural zone."""
    zone = violation.get("zone")
    if isinstance(zone, str) and zone:
        return zone

    abstractness_value = violation.get("abstractness", 0.0)
    instability_value = violation.get("instability", 0.0)
    abstractness = (
        float(abstractness_value) if isinstance(abstractness_value, (int, float, str)) else 0.0
    )
    instability = (
        float(instability_value) if isinstance(instability_value, (int, float, str)) else 0.0
    )
    if abstractness <= 0.5 and instability <= 0.5:
        return "pain"
    if abstractness >= 0.5 and instability >= 0.5:
        return "uselessness"
    return "off-main-sequence"


def compute_sap_metrics_func(
    repo_root: Optional[str] = None,
    scope_kind: Literal["file", "package", "directory"] = "package",
    unit_filter: Optional[list[str]] = None,
    top_n: int = 30,
    artifact_scope: ArtifactScope = "code",
    detail_level: Literal["minimal", "standard", "verbose"] = "standard",
    dependency_profile: DependencyProfile = "strict_static",
) -> ToolPayload:
    """Compute SAP abstractness, instability, and distance metrics per scope.

    For each scope, measures:
      A (abstractness)  = Na / Nt
      I (instability)   = Ce / (Ca + Ce)
      D (distance)      = |A + I - 1|

    Scopes on the main sequence have D ≈ 0. High D means the scope is
    either too abstract with no dependents (useless abstractions) or
    too concrete with many dependents (fragile, hard to change).

    Dependency edges: IMPORTS_FROM + DEPENDS_ON + INHERITS + IMPLEMENTS (fixed).

    Args:
        repo_root: Repository root (auto-detected if empty).
        scope_kind: "package" (directory-level, default), "file", or
            "directory" (synonym for package).
        unit_filter: Optional list of scope_key prefix strings to restrict
            output to matching scopes.
        top_n: Return the top N entries by distance. Default: 30.
        artifact_scope: "code" (default), "docs", or "all".
        detail_level: ``verbose`` includes SAP-inapplicable scopes in the main
            metrics list.  Default output keeps them in a separate
            ``inapplicable_metrics`` bucket.
        dependency_profile: Dependency edge profile. Default: strict_static.
    """
    try:
        dependency_profile = validate_dependency_profile(dependency_profile)
    except ValueError as exc:
        return _error_response(str(exc), dependency_profile=dependency_profile)
    store, _root = _get_store(repo_root)
    raw_metrics = compute_sap_metrics(
        store,
        scope_kind=scope_kind,
        unit_filter=unit_filter,
        artifact_scope=artifact_scope,
        dependency_profile=dependency_profile,
    )
    applicable_metrics = [m for m in raw_metrics if m.get("sap_applicable", True)]
    inapplicable_metrics = [m for m in raw_metrics if not m.get("sap_applicable", True)]
    visible_metrics = raw_metrics if detail_level == "verbose" else applicable_metrics
    inapplicable_by_reason: dict[str, int] = {}
    for metric in inapplicable_metrics:
        reason = str(metric.get("applicability_reason") or "inapplicable")
        inapplicable_by_reason[reason] = inapplicable_by_reason.get(reason, 0) + 1

    metrics_slice = visible_metrics[:top_n]
    inapplicable_slice = inapplicable_metrics[:top_n]
    truncated = len(visible_metrics) > top_n or len(inapplicable_metrics) > top_n

    return make_response(
        "ok",
        f"Computed SAP metrics for {len(raw_metrics)} {scope_kind}(s) "
        f"(artifact_scope={artifact_scope}, dependency_profile={dependency_profile}); "
        f"{len(applicable_metrics)} applicable "
        f"and {len(inapplicable_metrics)} inapplicable."
        f" Showing top {min(top_n, len(visible_metrics))} by distance."
        + (" Results truncated." if truncated else ""),
        metrics=metrics_slice,
        inapplicable_metrics=inapplicable_slice,
        total=len(raw_metrics),
        visible_total=len(visible_metrics),
        applicable_count=len(applicable_metrics),
        inapplicable_count=len(inapplicable_metrics),
        truncated=truncated,
        inapplicable_by_reason=inapplicable_by_reason,
        inapplicable_visibility=(
            "included_in_metrics" if detail_level == "verbose" else "separate_bucket"
        ),
        scope_kind=scope_kind,
        artifact_scope=artifact_scope,
        dependency_profile=dependency_profile,
        detail_level=detail_level,
        next_tool_suggestions=[
            'architecture_analysis_tool mode="sap_violations" -- find far-from-sequence scopes',
            'architecture_analysis_tool mode="sdp_metrics" -- check raw instability',
            'architecture_analysis_tool mode="community" -- explore the scope as a community',
        ],
    )


def detect_sap_violations_func(
    repo_root: Optional[str] = None,
    scope_kind: Literal["file", "package", "directory"] = "package",
    min_distance: float = 0.5,
    top_n: int = 30,
    artifact_scope: ArtifactScope = "code",
    dependency_profile: DependencyProfile = "strict_static",
) -> ToolPayload:
    """Detect scopes that violate the Stable Abstractions Principle.

    A violation is a scope with D = |A + I - 1| > min_distance, meaning
    it deviates significantly from the main sequence. Two archetypes:
      - Zone of Pain:    A ≈ 0, I ≈ 0 (concrete + stable = fragile)
      - Zone of Uselessness: A ≈ 1, I ≈ 1 (abstract + unstable = no users)

    Dependency edges: IMPORTS_FROM + DEPENDS_ON + INHERITS + IMPLEMENTS (fixed).

    Args:
        repo_root: Repository root (auto-detected if empty).
        scope_kind: "package" (default), "file", or "directory".
        min_distance: Minimum D value to flag (exclusive). Default: 0.5.
        top_n: Return the top N violations by distance. Default: 30.
        artifact_scope: "code" (default), "docs", or "all".
        dependency_profile: Dependency edge profile. Default: strict_static.
    """
    try:
        dependency_profile = validate_dependency_profile(dependency_profile)
    except ValueError as exc:
        return _error_response(str(exc), dependency_profile=dependency_profile)
    store, _root = _get_store(repo_root)
    raw_violations = find_sap_violations(
        store,
        scope_kind=scope_kind,
        artifact_scope=artifact_scope,
        dependency_profile=dependency_profile,
        min_distance=min_distance,
    )
    violations = [
        {
            "scope_key": violation["scope_key"],
            "display_name": violation["display_name"],
            "distance": violation["distance"],
            "zone": _classify_sap_zone(violation),
        }
        for violation in raw_violations
    ]
    total = len(violations)
    truncated = total > top_n
    excluded_scope_categories = ["test-scope", "fixture-scope"]
    exclusion_reason = (
        "test and fixture scopes are retained in sap_metrics notes but omitted from sap_violations"
    )
    payload = make_response(
        "ok",
        f"Found {total} SAP violation(s) at {scope_kind} level "
        f"(artifact_scope={artifact_scope}, dependency_profile={dependency_profile}, "
        f"min_distance={min_distance})."
        + " sap_violations suppresses test and fixture scopes; inspect "
        "sap_metrics notes for raw values."
        + (f" Showing top {top_n} by distance." if truncated else ""),
        violations=violations[:top_n],
        count=total,
        total=total,
        truncated=truncated,
        scope_kind=scope_kind,
        artifact_scope=artifact_scope,
        dependency_profile=dependency_profile,
        min_distance=min_distance,
        excluded_scope_categories=excluded_scope_categories,
        exclusion_reason=exclusion_reason,
        next_tool_suggestions=[
            'architecture_analysis_tool mode="sap_metrics" -- see full A/I/D scores',
            'architecture_analysis_tool mode="adp_violations" -- check cyclic dependencies',
            'review_tool mode="impact" -- check blast radius of a violating scope',
        ],
    )
    apply_output_budget(payload, budget_tokens=5000, list_priorities=["violations"])
    return payload
