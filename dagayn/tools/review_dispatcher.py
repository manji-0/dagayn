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

_with_dispatch_metadata = partial(with_dispatch_metadata, summary_label="Review")

#: The internal tool each mode answered with, as ``called_subtool`` names it.
_SUBTOOLS: dict[str, str] = {
    "changes": "detect_changes_func",
    "affected_flows": "get_affected_flows_func",
}


def review_func(
    mode: ReviewMode = "changes",
    changed_files: list[str] | None = None,
    base: str | None = None,
    include_source: bool | None = None,
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
