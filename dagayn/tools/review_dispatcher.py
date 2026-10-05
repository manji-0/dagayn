"""Unified review dispatcher."""

from __future__ import annotations

from collections.abc import Callable
from functools import partial
from typing import Any, Literal

from pydantic import ValidationError

from ..contracts.state_types import (
    ReviewMode,
    format_validation_error,
    parse_review_request,
)
from ._common import ToolPayload
from ._dispatch import dispatch_error as _error
from ._dispatch import with_dispatch_metadata
from .query import get_impact_radius
from .review import detect_changes_func, get_review_context
from .review_flows import get_affected_flows_func

_with_dispatch_metadata = partial(
    with_dispatch_metadata, summary_label="Review", hints_tool="review"
)


def review_func(
    mode: ReviewMode = "changes",
    changed_files: list[str] | None = None,
    base: str = "HEAD~1",
    include_source: bool | None = None,
    max_depth: int = 2,
    max_nodes: int = 50,
    max_lines_per_file: int = 200,
    detail_level: Literal["minimal", "standard", "verbose"] = "standard",
    repo_root: str | None = None,
) -> ToolPayload:
    """Run review analysis by dispatching to the requested internal mode."""
    try:
        request = parse_review_request(
            mode=mode,
            changed_files=changed_files,
            base=base,
            include_source=include_source,
            max_depth=max_depth,
            max_nodes=max_nodes,
            max_lines_per_file=max_lines_per_file,
            detail_level=detail_level,
            repo_root=repo_root,
        )
    except ValidationError as exc:
        return _error(format_validation_error(exc), mode=mode, repo_root=repo_root)

    include_source = request.include_source
    subtools: dict[str, tuple[str, Callable[..., ToolPayload], dict[str, Any]]] = {
        "changes": (
            "detect_changes_func",
            detect_changes_func,
            {
                "base": request.base,
                "changed_files": request.changed_files,
                "include_source": include_source if include_source is not None else False,
                "max_depth": request.max_depth,
                "repo_root": request.repo_root,
                "detail_level": request.detail_level,
            },
        ),
        "context": (
            "get_review_context",
            get_review_context,
            {
                "changed_files": request.changed_files,
                "max_depth": request.max_depth,
                "include_source": True if include_source is None else include_source,
                "max_lines_per_file": request.max_lines_per_file,
                "repo_root": request.repo_root,
                "base": request.base,
                "detail_level": request.detail_level,
            },
        ),
        "affected_flows": (
            "get_affected_flows_func",
            get_affected_flows_func,
            {
                "changed_files": request.changed_files,
                "base": request.base,
                "repo_root": request.repo_root,
            },
        ),
    }
    called_subtool, subtool, kwargs = subtools.get(
        request.mode,
        (
            "get_impact_radius",
            get_impact_radius,
            {
                "changed_files": request.changed_files,
                "max_depth": request.max_depth,
                "max_results": request.max_nodes,
                "repo_root": request.repo_root,
                "base": request.base,
                "detail_level": request.detail_level,
            },
        ),
    )
    return _with_dispatch_metadata(
        subtool(**kwargs),
        mode=request.mode,
        called_subtool=called_subtool,
        repo_root=request.repo_root,
    )
