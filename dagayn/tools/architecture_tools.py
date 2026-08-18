"""MCP tool wrappers for package design principle analysis (ADP, SDP)."""

from __future__ import annotations

from typing import Literal, Optional

from .._scope import ArtifactScope
from ..architecture import compute_sdp_metrics, find_adp_violations, find_sdp_violations
from ..dependency_profiles import DependencyProfile, validate_dependency_profile
from ._common import ToolPayload, _error_response, _get_store, make_response


def detect_adp_violations_func(
    repo_root: Optional[str] = None,
    granularity: Literal["file", "package"] = "package",
    min_cycle_size: int = 2,
    max_cycle_length: int = 10,
    top_n: int = 30,
    artifact_scope: ArtifactScope = "code",
    dependency_profile: DependencyProfile = "strict_static",
) -> ToolPayload:
    """Detect cyclic dependencies (ADP violations).

    Finds cycles in the IMPORTS_FROM / DEPENDS_ON dependency graph.
    Each result includes the nodes in the cycle, its length, and a
    severity score (length × edge_weight).

    Args:
        repo_root: Repository root (auto-detected if empty).
        granularity: "package" (directory-level) or "file" (file-level).
        min_cycle_size: Minimum cycle length to report. Default: 2.
        max_cycle_length: Maximum cycle length to search. Default: 10.
        top_n: Maximum violations to return, ordered by severity. Default: 30.
        artifact_scope: "code" (default), "docs", or "all".
        dependency_profile: Dependency edge profile. Default: strict_static.
    """
    try:
        dependency_profile = validate_dependency_profile(dependency_profile)
    except ValueError as exc:
        return _error_response(str(exc), dependency_profile=dependency_profile)
    store, _root = _get_store(repo_root)
    violations = find_adp_violations(
        store,
        granularity=granularity,
        artifact_scope=artifact_scope,
        dependency_profile=dependency_profile,
        min_cycle_size=min_cycle_size,
        max_cycle_length=max_cycle_length,
    )
    total = len(violations)
    truncated = total > top_n
    return make_response(
        "ok",
        f"Found {total} ADP violation(s) at {granularity} level "
        f"(artifact_scope={artifact_scope}, dependency_profile={dependency_profile})."
        + (f" Showing top {top_n} by severity." if truncated else ""),
        violations=violations[:top_n],
        count=total,
        truncated=truncated,
        granularity=granularity,
        artifact_scope=artifact_scope,
        dependency_profile=dependency_profile,
        next_tool_suggestions=[
            'review_tool mode="impact" -- check blast radius of a cyclic module',
            "query_graph_tool imports_of -- trace what a module imports",
            'architecture_analysis_tool mode="sdp_violations" -- check stability direction',
        ],
    )


def compute_sdp_metrics_func(
    repo_root: Optional[str] = None,
    granularity: Literal["file", "package"] = "package",
    top_n: int = 30,
    artifact_scope: ArtifactScope = "code",
    dependency_profile: DependencyProfile = "strict_static",
) -> ToolPayload:
    """Compute SDP instability metrics for each module/package.

    Instability I = Ce / (Ca + Ce), where Ca = afferent couplings
    (in-degree) and Ce = efferent couplings (out-degree). I = 0 means
    maximally stable, I = 1 means maximally unstable.

    Args:
        repo_root: Repository root (auto-detected if empty).
        granularity: "package" (directory-level) or "file" (file-level).
        top_n: Return the top N most unstable entries. Default: 30.
        artifact_scope: "code" (default), "docs", or "all".
        dependency_profile: Dependency edge profile. Default: strict_static.
    """
    try:
        dependency_profile = validate_dependency_profile(dependency_profile)
    except ValueError as exc:
        return _error_response(str(exc), dependency_profile=dependency_profile)
    store, _root = _get_store(repo_root)
    metrics = compute_sdp_metrics(
        store,
        granularity=granularity,
        artifact_scope=artifact_scope,
        dependency_profile=dependency_profile,
    )
    return make_response(
        "ok",
        f"Computed SDP instability for {len(metrics)} {granularity}(s) "
        f"(artifact_scope={artifact_scope}, dependency_profile={dependency_profile})."
        f" Showing top {min(top_n, len(metrics))} most unstable.",
        metrics=metrics[:top_n],
        total=len(metrics),
        granularity=granularity,
        artifact_scope=artifact_scope,
        dependency_profile=dependency_profile,
        next_tool_suggestions=[
            'architecture_analysis_tool mode="sdp_violations" -- find stability violations',
            'architecture_analysis_tool mode="adp_violations" -- find cyclic dependencies',
            'architecture_analysis_tool mode="hubs" -- find most connected nodes',
        ],
    )


def detect_sdp_violations_func(
    repo_root: Optional[str] = None,
    granularity: Literal["file", "package"] = "package",
    min_delta: float = 0.1,
    top_n: int = 30,
    artifact_scope: ArtifactScope = "code",
    dependency_profile: DependencyProfile = "strict_static",
) -> ToolPayload:
    """Detect SDP violations: dependencies pointing toward instability.

    An edge A -> B violates SDP when I(A) < I(B) - min_delta, meaning
    a more-stable module depends on a less-stable one.

    Args:
        repo_root: Repository root (auto-detected if empty).
        granularity: "package" (directory-level) or "file" (file-level).
        min_delta: Minimum instability difference to flag. Default: 0.1.
        top_n: Maximum violations to return, ordered by instability gap. Default: 30.
        artifact_scope: "code" (default), "docs", or "all".
        dependency_profile: Dependency edge profile. Default: strict_static.
    """
    try:
        dependency_profile = validate_dependency_profile(dependency_profile)
    except ValueError as exc:
        return _error_response(str(exc), dependency_profile=dependency_profile)
    store, _root = _get_store(repo_root)
    violations = find_sdp_violations(
        store,
        granularity=granularity,
        artifact_scope=artifact_scope,
        dependency_profile=dependency_profile,
        min_delta=min_delta,
    )
    total = len(violations)
    truncated = total > top_n
    return make_response(
        "ok",
        f"Found {total} SDP violation(s) at {granularity} level "
        f"(artifact_scope={artifact_scope}, dependency_profile={dependency_profile}, "
        f"min_delta={min_delta})."
        + (f" Showing top {top_n} by instability gap." if truncated else ""),
        violations=violations[:top_n],
        count=total,
        total=total,
        truncated=truncated,
        granularity=granularity,
        artifact_scope=artifact_scope,
        dependency_profile=dependency_profile,
        next_tool_suggestions=[
            'architecture_analysis_tool mode="sdp_metrics" -- see instability scores',
            'architecture_analysis_tool mode="adp_violations" -- check cyclic dependencies',
            'review_tool mode="impact" -- check blast radius of a violating module',
        ],
    )
