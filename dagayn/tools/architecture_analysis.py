"""Unified architecture analysis dispatcher."""

from __future__ import annotations

from typing import Any, Literal

from .._scope import ArtifactScope
from ..hints import generate_hints, get_session
from ._common import attach_answerability
from .analysis_tools import (
    get_bridge_nodes_func,
    get_hub_nodes_func,
    get_knowledge_gaps_func,
    get_surprising_connections_func,
)
from .architecture_tools import (
    compute_sdp_metrics_func,
    detect_adp_violations_func,
    detect_sdp_violations_func,
)
from .community_tools import (
    get_architecture_overview_func,
    get_community_func,
    list_communities_func,
)
from .sap_tools import compute_sap_metrics_func, detect_sap_violations_func

ArchitectureAnalysisMode = Literal[
    "overview",
    "communities",
    "community",
    "hubs",
    "bridges",
    "knowledge_gaps",
    "surprising_connections",
    "adp_violations",
    "sdp_metrics",
    "sdp_violations",
    "sap_metrics",
    "sap_violations",
]


def _with_dispatch_metadata(
    result: dict[str, Any],
    *,
    mode: str,
    called_subtool: str,
    repo_root: str | None,
) -> dict[str, Any]:
    """Add dispatcher metadata without mutating the subtool response."""
    payload = dict(result)
    payload.setdefault("status", "ok")
    payload.setdefault("summary", f"Architecture analysis mode {mode!r} completed.")
    payload["mode"] = mode
    payload["called_subtool"] = called_subtool
    attach_answerability(payload, repo_root)
    payload.setdefault(
        "_hints",
        generate_hints("architecture_analysis", payload, get_session()),
    )
    return payload


def _error(message: str, *, mode: str) -> dict[str, Any]:
    return attach_answerability(
        {
            "status": "error",
            "summary": message,
            "error": message,
            "mode": mode,
            "called_subtool": None,
        }
    )


def architecture_analysis_func(
    mode: ArchitectureAnalysisMode = "overview",
    detail_level: Literal["minimal", "standard", "verbose"] = "minimal",
    top_n: int = 10,
    sort_by: Literal["size", "cohesion", "name"] = "size",
    min_size: int = 0,
    community_name: str | None = None,
    community_id: int | None = None,
    include_members: bool = False,
    granularity: Literal["file", "package"] = "package",
    scope_kind: Literal["file", "package", "directory"] = "package",
    unit_filter: list[str] | None = None,
    min_cycle_size: int = 2,
    max_cycle_length: int = 10,
    min_delta: float = 0.1,
    min_distance: float = 0.5,
    repo_root: str | None = None,
    artifact_scope: ArtifactScope = "code",
) -> dict[str, Any]:
    """Run architecture analysis by dispatching to the requested internal mode."""
    if mode == "overview":
        return _with_dispatch_metadata(
            get_architecture_overview_func(
                repo_root=repo_root,
                detail_level=detail_level,
                top_n=top_n,
                artifact_scope=artifact_scope,
            ),
            mode=mode,
            called_subtool="get_architecture_overview_func",
            repo_root=repo_root,
        )
    if mode == "communities":
        return _with_dispatch_metadata(
            list_communities_func(
                repo_root=repo_root,
                sort_by=sort_by,
                min_size=min_size,
                detail_level=detail_level,
            ),
            mode=mode,
            called_subtool="list_communities_func",
            repo_root=repo_root,
        )
    if mode == "community":
        if community_id is None and not community_name:
            return _error(
                'mode="community" requires community_id or community_name.',
                mode=mode,
            )
        return _with_dispatch_metadata(
            get_community_func(
                repo_root=repo_root,
                community_name=community_name,
                community_id=community_id,
                include_members=include_members,
            ),
            mode=mode,
            called_subtool="get_community_func",
            repo_root=repo_root,
        )
    if mode == "hubs":
        return _with_dispatch_metadata(
            get_hub_nodes_func(repo_root=repo_root, top_n=top_n),
            mode=mode,
            called_subtool="get_hub_nodes_func",
            repo_root=repo_root,
        )
    if mode == "bridges":
        return _with_dispatch_metadata(
            get_bridge_nodes_func(repo_root=repo_root, top_n=top_n),
            mode=mode,
            called_subtool="get_bridge_nodes_func",
            repo_root=repo_root,
        )
    if mode == "knowledge_gaps":
        return _with_dispatch_metadata(
            get_knowledge_gaps_func(repo_root=repo_root, top_n=top_n),
            mode=mode,
            called_subtool="get_knowledge_gaps_func",
            repo_root=repo_root,
        )
    if mode == "surprising_connections":
        return _with_dispatch_metadata(
            get_surprising_connections_func(repo_root=repo_root, top_n=top_n),
            mode=mode,
            called_subtool="get_surprising_connections_func",
            repo_root=repo_root,
        )
    if mode == "adp_violations":
        return _with_dispatch_metadata(
            detect_adp_violations_func(
                repo_root=repo_root,
                granularity=granularity,
                artifact_scope=artifact_scope,
                min_cycle_size=min_cycle_size,
                max_cycle_length=max_cycle_length,
                top_n=top_n,
            ),
            mode=mode,
            called_subtool="detect_adp_violations_func",
            repo_root=repo_root,
        )
    if mode == "sdp_metrics":
        return _with_dispatch_metadata(
            compute_sdp_metrics_func(
                repo_root=repo_root,
                granularity=granularity,
                artifact_scope=artifact_scope,
                top_n=top_n,
            ),
            mode=mode,
            called_subtool="compute_sdp_metrics_func",
            repo_root=repo_root,
        )
    if mode == "sdp_violations":
        return _with_dispatch_metadata(
            detect_sdp_violations_func(
                repo_root=repo_root,
                granularity=granularity,
                artifact_scope=artifact_scope,
                min_delta=min_delta,
                top_n=top_n,
            ),
            mode=mode,
            called_subtool="detect_sdp_violations_func",
            repo_root=repo_root,
        )
    if mode == "sap_metrics":
        return _with_dispatch_metadata(
            compute_sap_metrics_func(
                repo_root=repo_root,
                scope_kind=scope_kind,
                unit_filter=unit_filter,
                artifact_scope=artifact_scope,
                top_n=top_n,
            ),
            mode=mode,
            called_subtool="compute_sap_metrics_func",
            repo_root=repo_root,
        )
    if mode == "sap_violations":
        return _with_dispatch_metadata(
            detect_sap_violations_func(
                repo_root=repo_root,
                scope_kind=scope_kind,
                artifact_scope=artifact_scope,
                min_distance=min_distance,
                top_n=top_n,
            ),
            mode=mode,
            called_subtool="detect_sap_violations_func",
            repo_root=repo_root,
        )
    return _error(f"Unknown architecture analysis mode: {mode!r}.", mode=str(mode))
