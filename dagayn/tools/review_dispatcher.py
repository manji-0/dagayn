"""Unified review dispatcher."""

from __future__ import annotations

from typing import Any, Literal

from ..hints import generate_hints, get_session
from ._common import attach_answerability
from .query import get_impact_radius
from .review import detect_changes_func, get_affected_flows_func, get_review_context

ReviewMode = Literal["changes", "context", "affected_flows", "impact"]


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
    if mode == "changes":
        return _with_dispatch_metadata(
            detect_changes_func(
                base=base,
                changed_files=changed_files,
                include_source=bool(include_source) if include_source is not None else False,
                max_depth=max_depth,
                repo_root=repo_root,
                detail_level=detail_level,
            ),
            mode=mode,
            called_subtool="detect_changes_func",
            repo_root=repo_root,
        )
    if mode == "context":
        return _with_dispatch_metadata(
            get_review_context(
                changed_files=changed_files,
                max_depth=max_depth,
                include_source=True if include_source is None else include_source,
                max_lines_per_file=max_lines_per_file,
                repo_root=repo_root,
                base=base,
                detail_level=detail_level,
            ),
            mode=mode,
            called_subtool="get_review_context",
            repo_root=repo_root,
        )
    if mode == "affected_flows":
        return _with_dispatch_metadata(
            get_affected_flows_func(
                changed_files=changed_files,
                base=base,
                repo_root=repo_root,
            ),
            mode=mode,
            called_subtool="get_affected_flows_func",
            repo_root=repo_root,
        )
    if mode == "impact":
        return _with_dispatch_metadata(
            get_impact_radius(
                changed_files=changed_files,
                max_depth=max_depth,
                max_results=max_nodes,
                repo_root=repo_root,
                base=base,
                detail_level=detail_level,
            ),
            mode=mode,
            called_subtool="get_impact_radius",
            repo_root=repo_root,
        )
    return _error(f"Unknown review mode: {mode!r}.", mode=str(mode), repo_root=repo_root)
