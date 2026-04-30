"""MCP tool wrappers for Stable Abstractions Principle (SAP) analysis."""

from __future__ import annotations

from typing import Any, Literal, Optional

from ..sap import compute_sap_metrics, find_sap_violations
from ._common import _get_store, apply_output_budget, make_response


def compute_sap_metrics_func(
    repo_root: Optional[str] = None,
    scope_kind: Literal["file", "package", "directory"] = "package",
    unit_filter: Optional[list[str]] = None,
    top_n: int = 30,
) -> dict[str, Any]:
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
    """
    store, _root = _get_store(repo_root)
    metrics = compute_sap_metrics(
        store,
        scope_kind=scope_kind,
        unit_filter=unit_filter,
    )
    return make_response(
        "ok",
        f"Computed SAP metrics for {len(metrics)} {scope_kind}(s)."
        f" Showing top {min(top_n, len(metrics))} by distance.",
        metrics=metrics[:top_n],
        total=len(metrics),
        scope_kind=scope_kind,
        next_tool_suggestions=[
            "detect_sap_violations -- find scopes far from the main sequence",
            "compute_sdp_metrics -- check raw instability without abstractness",
            "get_community -- explore the scope as a community",
        ],
    )


def detect_sap_violations_func(
    repo_root: Optional[str] = None,
    scope_kind: Literal["file", "package", "directory"] = "package",
    min_distance: float = 0.5,
    top_n: int = 30,
) -> dict[str, Any]:
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
    """
    store, _root = _get_store(repo_root)
    raw_violations = find_sap_violations(
        store,
        scope_kind=scope_kind,
        min_distance=min_distance,
    )
    violations = [
        {
            "scope_key": violation["scope_key"],
            "display_name": violation["display_name"],
            "distance": violation["distance"],
            "zone": violation["zone"],
        }
        for violation in raw_violations
    ]
    total = len(violations)
    truncated = total > top_n
    payload = make_response(
        "ok",
        f"Found {total} SAP violation(s) at {scope_kind} level (min_distance={min_distance})."
        + (f" Showing top {top_n} by distance." if truncated else ""),
        violations=violations[:top_n],
        count=total,
        total=total,
        truncated=truncated,
        scope_kind=scope_kind,
        min_distance=min_distance,
        next_tool_suggestions=[
            "compute_sap_metrics -- see full A/I/D scores",
            "detect_adp_violations -- check for cyclic dependencies",
            "get_impact_radius -- check blast radius of a violating scope",
        ],
    )
    apply_output_budget(payload, budget_tokens=5000, list_priorities=["violations"])
    return payload
