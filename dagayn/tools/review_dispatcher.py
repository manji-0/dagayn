"""Unified review dispatcher."""

from __future__ import annotations

import logging
from functools import partial
from typing import Literal, cast

from pydantic import ValidationError

from ..contracts.state_types import (
    ReviewMode,
    format_validation_error,
    parse_review_request,
)
from ._common import ToolPayload, ToolStoreScope, _get_store
from ._dispatch import dispatch_error as _error
from ._dispatch import with_dispatch_metadata
from ._native import native_tool

logger = logging.getLogger(__name__)

_with_dispatch_metadata = partial(
    with_dispatch_metadata, summary_label="Review", hints_tool="review"
)

#: The internal tool each mode answered with, as ``called_subtool`` names it.
_SUBTOOLS: dict[str, str] = {
    "changes": "detect_changes_func",
    "context": "get_review_context",
    "affected_flows": "get_affected_flows_func",
    "impact": "get_impact_radius",
}


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

    # The validated (coerced) request goes to Rust, which declines arguments
    # pydantic would have coerced or rejected.
    with ToolStoreScope(logger=logger, context=_SUBTOOLS[request.mode]) as scope:
        # Resolves the repository and creates, migrates, or waits for the
        # graph; the Rust tool reads it.
        scope.track(_get_store(request.repo_root))
        return cast(
            ToolPayload,
            native_tool(
                "review_tool",
                mode=request.mode,
                base=request.base,
                changed_files=request.changed_files,
                include_source=request.include_source,
                max_depth=request.max_depth,
                max_nodes=request.max_nodes,
                max_lines_per_file=request.max_lines_per_file,
                detail_level=request.detail_level,
                repo_root=request.repo_root,
            ),
        )
    return _with_dispatch_metadata(
        scope.error,
        mode=request.mode,
        called_subtool=_SUBTOOLS[request.mode],
        repo_root=request.repo_root,
    )
