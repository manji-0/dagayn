"""Unified architecture analysis dispatcher."""

from __future__ import annotations

from collections.abc import Callable
from functools import partial
from typing import Any, Literal, cast

from pydantic import ValidationError

from ..contracts.dependency_profiles import DependencyProfile
from ..contracts.state_types import (
    ArchitectureAnalysisMode,
    format_validation_error,
    parse_architecture_analysis_request,
)
from ._common import ToolPayload
from ._dispatch import dispatch_error as _error
from ._dispatch import with_dispatch_metadata
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

_with_dispatch_metadata = partial(
    with_dispatch_metadata,
    summary_label="Architecture analysis",
    hints_tool="architecture_analysis",
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
) -> ToolPayload:
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

    # Mode -> (subtool name, subtool, keyword names). Values come from ``computed`` or the
    # request; request models are mode-specific, so read fields lazily.
    computed: dict[str, Any] = {
        "include_tests": include_tests,
        "dependency_profile": dependency_profile_value,
        "limit": request.top_n,
    }
    scoped = ("repo_root", "top_n", "artifact_scope", "include_tests")
    layered = ("repo_root", "granularity", "artifact_scope", "dependency_profile")
    subtools: dict[str, tuple[str, Callable[..., ToolPayload], tuple[str, ...]]] = {
        "overview": (
            "get_architecture_overview_func",
            get_architecture_overview_func,
            ("repo_root", "detail_level", "top_n", "artifact_scope"),
        ),
        "communities": (
            "list_communities_func",
            list_communities_func,
            ("repo_root", "sort_by", "min_size", "detail_level", "limit"),
        ),
        "community": (
            "get_community_func",
            get_community_func,
            ("repo_root", "community_name", "community_id", "include_members"),
        ),
        "hubs": ("get_hub_nodes_func", get_hub_nodes_func, scoped),
        "bridges": ("get_bridge_nodes_func", get_bridge_nodes_func, scoped),
        "knowledge_gaps": ("get_knowledge_gaps_func", get_knowledge_gaps_func, scoped),
        "surprising_connections": (
            "get_surprising_connections_func",
            get_surprising_connections_func,
            scoped,
        ),
        "adp_violations": (
            "detect_adp_violations_func",
            detect_adp_violations_func,
            (*layered, "min_cycle_size", "max_cycle_length", "top_n"),
        ),
        "sdp_metrics": ("compute_sdp_metrics_func", compute_sdp_metrics_func, (*layered, "top_n")),
        "sdp_violations": (
            "detect_sdp_violations_func",
            detect_sdp_violations_func,
            (*layered, "min_delta", "top_n"),
        ),
        "sap_metrics": (
            "compute_sap_metrics_func",
            compute_sap_metrics_func,
            (
                "repo_root",
                "scope_kind",
                "unit_filter",
                "artifact_scope",
                "top_n",
                "detail_level",
                "dependency_profile",
            ),
        ),
    }
    called_subtool, subtool, fields = subtools.get(
        request.mode,
        (
            "detect_sap_violations_func",
            detect_sap_violations_func,
            (
                "repo_root",
                "scope_kind",
                "artifact_scope",
                "dependency_profile",
                "min_distance",
                "top_n",
            ),
        ),
    )
    kwargs = {
        name: computed[name] if name in computed else getattr(request, name) for name in fields
    }
    return _with_dispatch_metadata(
        subtool(**kwargs),
        mode=request.mode,
        called_subtool=called_subtool,
        repo_root=request.repo_root,
    )
