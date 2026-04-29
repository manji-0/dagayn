"""MCP tool wrappers for package design principle analysis (ADP, SDP)."""

from __future__ import annotations

from typing import Any, Literal, Optional

from ..architecture import compute_sdp_metrics, find_adp_violations, find_sdp_violations
from ._common import _get_store


def detect_adp_violations_func(
    repo_root: Optional[str] = None,
    granularity: Literal["file", "package"] = "package",
    min_cycle_size: int = 2,
    max_cycle_length: int = 10,
    top_n: int = 30,
) -> dict[str, Any]:
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
    """
    store, _root = _get_store(repo_root)
    violations = find_adp_violations(
        store,
        granularity=granularity,
        min_cycle_size=min_cycle_size,
        max_cycle_length=max_cycle_length,
    )
    total = len(violations)
    truncated = total > top_n
    return {
        "status": "ok",
        "summary": (
            f"Found {total} ADP violation(s) at {granularity} level."
            + (f" Showing top {top_n} by severity." if truncated else "")
        ),
        "violations": violations[:top_n],
        "count": total,
        "truncated": truncated,
        "granularity": granularity,
        "next_tool_suggestions": [
            "get_impact_radius -- check blast radius of a cyclic module",
            "query_graph imports_of -- trace what a module imports",
            "detect_sdp_violations -- check stability-direction violations",
        ],
    }


def compute_sdp_metrics_func(
    repo_root: Optional[str] = None,
    granularity: Literal["file", "package"] = "package",
    top_n: int = 30,
) -> dict[str, Any]:
    """Compute SDP instability metrics for each module/package.

    Instability I = Ce / (Ca + Ce), where Ca = afferent couplings
    (in-degree) and Ce = efferent couplings (out-degree). I = 0 means
    maximally stable, I = 1 means maximally unstable.

    Args:
        repo_root: Repository root (auto-detected if empty).
        granularity: "package" (directory-level) or "file" (file-level).
        top_n: Return the top N most unstable entries. Default: 30.
    """
    store, _root = _get_store(repo_root)
    metrics = compute_sdp_metrics(store, granularity=granularity)
    return {
        "status": "ok",
        "summary": (
            f"Computed SDP instability for {len(metrics)} {granularity}(s)."
            f" Showing top {min(top_n, len(metrics))} most unstable."
        ),
        "metrics": metrics[:top_n],
        "total": len(metrics),
        "granularity": granularity,
        "next_tool_suggestions": [
            "detect_sdp_violations -- find stability-direction violations",
            "detect_adp_violations -- find cyclic dependencies",
            "get_hub_nodes -- find most connected nodes",
        ],
    }


def detect_sdp_violations_func(
    repo_root: Optional[str] = None,
    granularity: Literal["file", "package"] = "package",
    min_delta: float = 0.1,
) -> dict[str, Any]:
    """Detect SDP violations: dependencies pointing toward instability.

    An edge A -> B violates SDP when I(A) < I(B) - min_delta, meaning
    a more-stable module depends on a less-stable one.

    Args:
        repo_root: Repository root (auto-detected if empty).
        granularity: "package" (directory-level) or "file" (file-level).
        min_delta: Minimum instability difference to flag. Default: 0.1.
    """
    store, _root = _get_store(repo_root)
    violations = find_sdp_violations(
        store,
        granularity=granularity,
        min_delta=min_delta,
    )
    return {
        "status": "ok",
        "summary": (
            f"Found {len(violations)} SDP violation(s) at {granularity} level"
            f" (min_delta={min_delta})."
        ),
        "violations": violations,
        "count": len(violations),
        "granularity": granularity,
        "next_tool_suggestions": [
            "compute_sdp_metrics -- see full instability scores",
            "detect_adp_violations -- check for cyclic dependencies",
            "get_impact_radius -- check blast radius of a violating module",
        ],
    }
