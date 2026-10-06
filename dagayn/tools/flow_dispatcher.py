"""Unified execution-flow dispatcher."""

from __future__ import annotations

import logging
from functools import partial
from typing import Literal, cast, overload

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


@overload
def flow_func(
    mode: Literal["list"] = "list",
    sort_by: Literal["criticality", "depth", "node_count", "file_count", "name"] = "criticality",
    limit: int = 50,
    kind: str | None = None,
    detail_level: Literal["minimal", "standard"] = "standard",
    flow_id: None = None,
    flow_name: None = None,
    include_source: bool = False,
    repo_root: str | None = None,
) -> ToolPayload: ...


@overload
def flow_func(
    mode: Literal["get"],
    sort_by: Literal["criticality", "depth", "node_count", "file_count", "name"] = "criticality",
    limit: int = 50,
    kind: str | None = None,
    detail_level: Literal["minimal", "standard"] = "standard",
    flow_id: int | None = None,
    flow_name: str | None = None,
    include_source: bool = False,
    repo_root: str | None = None,
) -> ToolPayload: ...


def flow_func(
    mode: FlowMode = "list",
    sort_by: Literal["criticality", "depth", "node_count", "file_count", "name"] = "criticality",
    limit: int = 50,
    kind: str | None = None,
    detail_level: Literal["minimal", "standard"] = "standard",
    flow_id: int | None = None,
    flow_name: str | None = None,
    include_source: bool = False,
    repo_root: str | None = None,
) -> ToolPayload:
    """Run execution-flow analysis by dispatching to the requested internal mode."""
    try:
        request = parse_flow_request(
            mode=mode,
            sort_by=sort_by,
            limit=limit,
            kind=kind,
            detail_level=detail_level,
            flow_id=flow_id,
            flow_name=flow_name,
            include_source=include_source,
            repo_root=repo_root,
        )
    except ValidationError as exc:
        return _error(format_validation_error(exc), mode=mode, repo_root=repo_root)

    # The validated (coerced) request goes to Rust, which declines arguments
    # pydantic would have coerced or rejected.
    if request.mode == "list":
        subtool = "list_flows"
        arguments: dict[str, object] = {
            "sort_by": request.sort_by,
            "limit": request.limit,
            "kind": request.kind,
            "detail_level": request.detail_level,
        }
    else:
        subtool = "get_flow"
        arguments = {
            "flow_id": request.flow_id,
            "flow_name": request.flow_name,
            "include_source": request.include_source,
            "detail_level": request.detail_level,
        }
    with ToolStoreScope(logger=logger, context=subtool) as scope:
        # Resolves the repository and creates, migrates, or waits for the
        # graph; the Rust tool reads it.
        scope.track(_get_store(request.repo_root))
        return cast(
            ToolPayload,
            native_tool("flow_tool", mode=request.mode, repo_root=request.repo_root, **arguments),
        )
    return _with_dispatch_metadata(
        scope.error,
        mode=request.mode,
        called_subtool=subtool,
        repo_root=request.repo_root,
    )
