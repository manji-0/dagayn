"""Unified architecture analysis dispatcher."""

from __future__ import annotations

from typing import Any, Literal, cast

from pydantic import ValidationError

from ..dependency_profiles import DependencyProfile
from ..hints import generate_hints, get_session
from ..state_types import (
    ArchitectureAnalysisMode,
    format_validation_error,
    parse_architecture_analysis_request,
    seal_dispatcher_error,
    seal_dispatcher_ok,
)
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
    if payload.get("status") == "error":
        payload.setdefault("error", payload["summary"])
        return seal_dispatcher_error(payload)
    payload.setdefault(
        "_hints",
        generate_hints("architecture_analysis", payload, get_session()),
    )
    return seal_dispatcher_ok(payload)


def _error(message: str, *, mode: str, repo_root: str | None) -> dict[str, Any]:
    return seal_dispatcher_error(
        attach_answerability(
            {
                "status": "error",
                "summary": message,
                "error": message,
                "mode": mode,
                "called_subtool": None,
            },
            repo_root,
        )
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
    artifact_scope: Literal["code", "docs", "all"] = "code",
    dependency_profile: Literal[
        "strict_static",
        "implementation",
        "infra_dataflow",
        "artifact_trace",
    ] = "strict_static",
) -> dict[str, Any]:
    """Run architecture analysis by dispatching to the requested internal mode."""
    try:
        request = parse_architecture_analysis_request(
            mode=mode,
            detail_level=detail_level,
            top_n=top_n,
            sort_by=sort_by,
            min_size=min_size,
            community_name=community_name,
            community_id=community_id,
            include_members=include_members,
            granularity=granularity,
            scope_kind=scope_kind,
            unit_filter=unit_filter,
            min_cycle_size=min_cycle_size,
            max_cycle_length=max_cycle_length,
            min_delta=min_delta,
            min_distance=min_distance,
            repo_root=repo_root,
            artifact_scope=artifact_scope,
            dependency_profile=dependency_profile,
        )
    except ValidationError as exc:
        return _error(format_validation_error(exc), mode=mode, repo_root=repo_root)

    include_tests = request.artifact_scope != "code"
    dependency_profile_value = cast(
        DependencyProfile,
        getattr(request, "dependency_profile", "strict_static"),
    )

    if request.mode == "overview":
        return _with_dispatch_metadata(
            get_architecture_overview_func(
                repo_root=request.repo_root,
                detail_level=request.detail_level,
                top_n=request.top_n,
                artifact_scope=request.artifact_scope,
            ),
            mode=request.mode,
            called_subtool="get_architecture_overview_func",
            repo_root=request.repo_root,
        )
    if request.mode == "communities":
        return _with_dispatch_metadata(
            list_communities_func(
                repo_root=request.repo_root,
                sort_by=request.sort_by,
                min_size=request.min_size,
                detail_level=request.detail_level,
                limit=request.top_n,
            ),
            mode=request.mode,
            called_subtool="list_communities_func",
            repo_root=request.repo_root,
        )
    if request.mode == "community":
        return _with_dispatch_metadata(
            get_community_func(
                repo_root=request.repo_root,
                community_name=request.community_name,
                community_id=request.community_id,
                include_members=request.include_members,
            ),
            mode=request.mode,
            called_subtool="get_community_func",
            repo_root=request.repo_root,
        )
    if request.mode == "hubs":
        return _with_dispatch_metadata(
            get_hub_nodes_func(
                repo_root=request.repo_root,
                top_n=request.top_n,
                artifact_scope=request.artifact_scope,
                include_tests=include_tests,
            ),
            mode=request.mode,
            called_subtool="get_hub_nodes_func",
            repo_root=request.repo_root,
        )
    if request.mode == "bridges":
        return _with_dispatch_metadata(
            get_bridge_nodes_func(
                repo_root=request.repo_root,
                top_n=request.top_n,
                artifact_scope=request.artifact_scope,
                include_tests=include_tests,
            ),
            mode=request.mode,
            called_subtool="get_bridge_nodes_func",
            repo_root=request.repo_root,
        )
    if request.mode == "knowledge_gaps":
        return _with_dispatch_metadata(
            get_knowledge_gaps_func(
                repo_root=request.repo_root,
                top_n=request.top_n,
                artifact_scope=request.artifact_scope,
                include_tests=include_tests,
            ),
            mode=request.mode,
            called_subtool="get_knowledge_gaps_func",
            repo_root=request.repo_root,
        )
    if request.mode == "surprising_connections":
        return _with_dispatch_metadata(
            get_surprising_connections_func(
                repo_root=request.repo_root,
                top_n=request.top_n,
                artifact_scope=request.artifact_scope,
                include_tests=include_tests,
            ),
            mode=request.mode,
            called_subtool="get_surprising_connections_func",
            repo_root=request.repo_root,
        )
    if request.mode == "adp_violations":
        return _with_dispatch_metadata(
            detect_adp_violations_func(
                repo_root=request.repo_root,
                granularity=request.granularity,
                artifact_scope=request.artifact_scope,
                dependency_profile=dependency_profile_value,
                min_cycle_size=request.min_cycle_size,
                max_cycle_length=request.max_cycle_length,
                top_n=request.top_n,
            ),
            mode=request.mode,
            called_subtool="detect_adp_violations_func",
            repo_root=request.repo_root,
        )
    if request.mode == "sdp_metrics":
        return _with_dispatch_metadata(
            compute_sdp_metrics_func(
                repo_root=request.repo_root,
                granularity=request.granularity,
                artifact_scope=request.artifact_scope,
                dependency_profile=dependency_profile_value,
                top_n=request.top_n,
            ),
            mode=request.mode,
            called_subtool="compute_sdp_metrics_func",
            repo_root=request.repo_root,
        )
    if request.mode == "sdp_violations":
        return _with_dispatch_metadata(
            detect_sdp_violations_func(
                repo_root=request.repo_root,
                granularity=request.granularity,
                artifact_scope=request.artifact_scope,
                dependency_profile=dependency_profile_value,
                min_delta=request.min_delta,
                top_n=request.top_n,
            ),
            mode=request.mode,
            called_subtool="detect_sdp_violations_func",
            repo_root=request.repo_root,
        )
    if request.mode == "sap_metrics":
        return _with_dispatch_metadata(
            compute_sap_metrics_func(
                repo_root=request.repo_root,
                scope_kind=request.scope_kind,
                unit_filter=request.unit_filter,
                artifact_scope=request.artifact_scope,
                top_n=request.top_n,
                detail_level=request.detail_level,
                dependency_profile=dependency_profile_value,
            ),
            mode=request.mode,
            called_subtool="compute_sap_metrics_func",
            repo_root=request.repo_root,
        )
    return _with_dispatch_metadata(
        detect_sap_violations_func(
            repo_root=request.repo_root,
            scope_kind=request.scope_kind,
            artifact_scope=request.artifact_scope,
            dependency_profile=dependency_profile_value,
            min_distance=request.min_distance,
            top_n=request.top_n,
        ),
        mode=request.mode,
        called_subtool="detect_sap_violations_func",
        repo_root=request.repo_root,
    )
