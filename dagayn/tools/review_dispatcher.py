"""Unified review dispatcher."""

from __future__ import annotations

from typing import Any, Literal

from pydantic import ValidationError

from ..hints import generate_hints, get_session
from ..state_types import (
    ReviewMode,
    format_validation_error,
    parse_review_request,
    seal_dispatcher_error,
    seal_dispatcher_ok,
)
from ._common import attach_answerability
from .query import get_impact_radius
from .review import detect_changes_func, get_review_context
from .review_flows import get_affected_flows_func


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
    payload.setdefault("summary", f"Review mode {mode!r} completed.")
    payload["mode"] = mode
    payload["called_subtool"] = called_subtool
    attach_answerability(payload, repo_root)
    payload.setdefault("_hints", generate_hints("review", payload, get_session()))
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
) -> dict[str, Any]:
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

    if request.mode == "changes":
        return _with_dispatch_metadata(
            detect_changes_func(
                base=request.base,
                changed_files=request.changed_files,
                include_source=(
                    bool(request.include_source) if request.include_source is not None else False
                ),
                max_depth=request.max_depth,
                repo_root=request.repo_root,
                detail_level=request.detail_level,
            ),
            mode=request.mode,
            called_subtool="detect_changes_func",
            repo_root=request.repo_root,
        )
    if request.mode == "context":
        return _with_dispatch_metadata(
            get_review_context(
                changed_files=request.changed_files,
                max_depth=request.max_depth,
                include_source=(True if request.include_source is None else request.include_source),
                max_lines_per_file=request.max_lines_per_file,
                repo_root=request.repo_root,
                base=request.base,
                detail_level=request.detail_level,
            ),
            mode=request.mode,
            called_subtool="get_review_context",
            repo_root=request.repo_root,
        )
    if request.mode == "affected_flows":
        return _with_dispatch_metadata(
            get_affected_flows_func(
                changed_files=request.changed_files,
                base=request.base,
                repo_root=request.repo_root,
            ),
            mode=request.mode,
            called_subtool="get_affected_flows_func",
            repo_root=request.repo_root,
        )
    return _with_dispatch_metadata(
        get_impact_radius(
            changed_files=request.changed_files,
            max_depth=request.max_depth,
            max_results=request.max_nodes,
            repo_root=request.repo_root,
            base=request.base,
            detail_level=request.detail_level,
        ),
        mode=request.mode,
        called_subtool="get_impact_radius",
        repo_root=request.repo_root,
    )
