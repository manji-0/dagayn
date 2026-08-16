"""Unified execution-flow dispatcher."""

from __future__ import annotations

from typing import Any, Literal, overload

from pydantic import ValidationError

from ..hints import generate_hints, get_session
from ..state_types import (
    FlowMode,
    format_validation_error,
    parse_flow_request,
    seal_dispatcher_error,
    seal_dispatcher_ok,
)
from ._common import attach_answerability
from .flows_tools import get_flow, list_flows


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
    payload.setdefault("summary", f"Flow mode {mode!r} completed.")
    payload["mode"] = mode
    payload["called_subtool"] = called_subtool
    attach_answerability(payload, repo_root)
    if payload.get("status") == "error":
        payload.setdefault("error", payload["summary"])
        return seal_dispatcher_error(payload)
    payload.setdefault("_hints", generate_hints("flow", payload, get_session()))
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
) -> dict[str, Any]: ...


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
) -> dict[str, Any]: ...


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
) -> dict[str, Any]:
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

    if request.mode == "list":
        return _with_dispatch_metadata(
            list_flows(
                repo_root=request.repo_root,
                sort_by=request.sort_by,
                limit=request.limit,
                kind=request.kind,
                detail_level=request.detail_level,
            ),
            mode=request.mode,
            called_subtool="list_flows",
            repo_root=request.repo_root,
        )

    return _with_dispatch_metadata(
        get_flow(
            flow_id=request.flow_id,
            flow_name=request.flow_name,
            include_source=request.include_source,
            repo_root=request.repo_root,
        ),
        mode=request.mode,
        called_subtool="get_flow",
        repo_root=request.repo_root,
    )
