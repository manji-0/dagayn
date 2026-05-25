"""Unified execution-flow dispatcher."""

from __future__ import annotations

from typing import Any, Literal

from ..hints import generate_hints, get_session
from ._common import attach_answerability
from .flows_tools import get_flow, list_flows

FlowMode = Literal["list", "get"]


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
    payload.setdefault("_hints", generate_hints("flow", payload, get_session()))
    return payload


def _error(message: str, *, mode: str, repo_root: str | None) -> dict[str, Any]:
    return attach_answerability(
        {
            "status": "error",
            "summary": message,
            "error": message,
            "mode": mode,
            "called_subtool": None,
        },
        repo_root,
    )


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
    if mode == "list":
        return _with_dispatch_metadata(
            list_flows(
                repo_root=repo_root,
                sort_by=sort_by,
                limit=limit,
                kind=kind,
                detail_level=detail_level,
            ),
            mode=mode,
            called_subtool="list_flows",
            repo_root=repo_root,
        )
    if mode == "get":
        if flow_id is None and not flow_name:
            return _error(
                'mode="get" requires flow_id or flow_name.',
                mode=mode,
                repo_root=repo_root,
            )
        return _with_dispatch_metadata(
            get_flow(
                flow_id=flow_id,
                flow_name=flow_name,
                include_source=include_source,
                repo_root=repo_root,
            ),
            mode=mode,
            called_subtool="get_flow",
            repo_root=repo_root,
        )
    return _error(f"Unknown flow mode: {mode!r}.", mode=str(mode), repo_root=repo_root)
