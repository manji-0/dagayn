"""Unified execution-flow dispatcher."""

from __future__ import annotations

import logging
from functools import partial
from typing import Literal, cast

from pydantic import ValidationError

from ..contracts.state_types import (
    FlowMode,
    format_validation_error,
    parse_flow_request,
)
from ._common import ToolPayload, ToolStoreScope, _get_store
from ._dispatch import dispatch_error as _error
from ._dispatch import with_dispatch_metadata
from ._native import native_tool

logger = logging.getLogger(__name__)

_with_dispatch_metadata = partial(with_dispatch_metadata, summary_label="Flow", hints_tool="flow")


def flow_func(
    mode: FlowMode = "entry_points",
    limit: int | None = None,
    detail_level: Literal["minimal", "standard"] = "standard",
    target: str | None = None,
    repo_root: str | None = None,
) -> ToolPayload:
    """The entry points that reach ``target``, or every entry point per unit.

    ``limit`` defaults to 10 entry points.
    """
    try:
        request = parse_flow_request(
            mode=mode,
            **({} if limit is None else {"limit": limit}),
            detail_level=detail_level,
            target=target,
            repo_root=repo_root,
        )
    except ValidationError as exc:
        return _error(format_validation_error(exc), mode=mode, repo_root=repo_root)

    # The validated (coerced) request goes to Rust, which declines arguments
    # pydantic would have coerced or rejected.
    subtool = "entry_points"
    with ToolStoreScope(logger=logger, context=subtool) as scope:
        # Resolves the repository and creates, migrates, or waits for the
        # graph; the Rust tool reads it.
        scope.track(_get_store(request.repo_root))
        return cast(
            ToolPayload,
            native_tool(
                "flow_tool",
                mode=request.mode,
                repo_root=request.repo_root,
                target=request.target,
                limit=request.limit,
                detail_level=request.detail_level,
            ),
        )
    return _with_dispatch_metadata(
        scope.error,
        mode=request.mode,
        called_subtool=subtool,
        repo_root=request.repo_root,
    )
