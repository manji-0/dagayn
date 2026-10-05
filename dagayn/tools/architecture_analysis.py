"""Unified architecture analysis dispatcher."""

from __future__ import annotations

from typing import Literal

from ..contracts.state_types import ArchitectureAnalysisMode
from ._common import ToolPayload, _get_store
from ._native import native_tool


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
    # Resolves the repository and creates, migrates, or waits for the graph;
    # the Rust tool reads it.
    store, _root = _get_store(repo_root)
    try:
        return native_tool(
            "architecture_analysis_tool",
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
    finally:
        store.close()
