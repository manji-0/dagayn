"""Unified architecture analysis dispatcher."""

from __future__ import annotations

from typing import Literal

from ..contracts.state_types import ArchitectureAnalysisMode
from ._common import ToolPayload, _get_store
from ._native import native_tool


def architecture_analysis_func(
    mode: ArchitectureAnalysisMode = "overview",
    detail_level: Literal["minimal", "standard", "verbose"] = "minimal",
    scope_kind: Literal["file", "package", "directory"] = "package",
    unit_filter: list[str] | None = None,
    repo_root: str | None = None,
    artifact_scope: Literal["code", "docs", "all"] = "code",
    import_kind: Literal["directive", "link", "lazy_export", "path", "statement"] | None = None,
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
            scope_kind=scope_kind,
            unit_filter=unit_filter,
            repo_root=repo_root,
            artifact_scope=artifact_scope,
            import_kind=import_kind,
        )
    finally:
        store.close()
