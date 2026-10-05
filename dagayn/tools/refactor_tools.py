"""Tools 17, 18: refactor_func, apply_refactor_func."""

from __future__ import annotations

import logging
import re
from pathlib import Path
from typing import Any, Literal, overload

from pydantic import ValidationError

from ..contracts.state_types import (
    RefactorMode,
    format_validation_error,
    parse_refactor_request,
    seal_refactor_error,
    seal_refactor_not_found,
    seal_refactor_ok,
)
from ..hints import generate_hints, get_session
from ..incremental_files import find_project_root
from ._common import (
    ToolStoreScope,
    _error_response,
    _get_store,
    _validate_repo_root,
    attach_answerability,
    graph_answerability_summary,
    missingness_from_answerability,
)
from ._native import native_tool

logger = logging.getLogger(__name__)

type RefactorValue = Any
type RefactorPayload = dict[str, RefactorValue]

#: Conservative identifier shape shared by the languages dagayn parses: a
#: leading letter or underscore followed by word characters. Language-specific
#: extras (``$`` in JS, ``!``/``?`` in Ruby) are deliberately excluded -- a
#: rejected valid name is a nuisance, an accepted invalid one writes code that
#: does not parse.
_IDENTIFIER_RE = re.compile(r"^[^\W\d]\w*$", re.UNICODE)


def _is_valid_identifier(name: str | None) -> bool:
    """True when *name* can be substituted into source as an identifier."""
    return bool(name) and bool(_IDENTIFIER_RE.match(name))


# ---------------------------------------------------------------------------
# Tool 17: refactor_tool  [REFACTOR]
# ---------------------------------------------------------------------------


@overload
def refactor_func(
    mode: Literal["rename"] = "rename",
    old_name: str | None = None,
    new_name: str | None = None,
    kind: str | None = None,
    file_pattern: str | None = None,
    limit: int = 50,
    top_n: int | None = None,
    detail_level: str = "standard",
    repo_root: str | None = None,
) -> RefactorPayload: ...


@overload
def refactor_func(
    mode: Literal["dead_code", "suggest"],
    old_name: str | None = None,
    new_name: str | None = None,
    kind: str | None = None,
    file_pattern: str | None = None,
    limit: int = 50,
    top_n: int | None = None,
    detail_level: str = "standard",
    repo_root: str | None = None,
) -> RefactorPayload: ...


def refactor_func(
    mode: RefactorMode | str = "rename",
    old_name: str | None = None,
    new_name: str | None = None,
    kind: str | None = None,
    file_pattern: str | None = None,
    limit: int = 50,
    top_n: int | None = None,
    detail_level: str = "standard",
    repo_root: str | None = None,
) -> RefactorPayload:
    """Unified refactoring entry point.

    [REFACTOR] Supports three modes:
    - ``rename``: Preview renaming a symbol (requires *old_name* and
      *new_name*).
    - ``dead_code``: Find unreferenced functions/classes.
    - ``suggest``: Get graph-backed refactoring suggestions: remove, move,
      split, and document candidates.

    Args:
        mode: One of ``"rename"``, ``"dead_code"``, or ``"suggest"``.
        old_name: (rename mode) Current symbol name.
        new_name: (rename mode) Desired new name.
        kind: (dead_code mode) Optional node kind filter.
        file_pattern: (dead_code mode) Optional file path substring filter.
        limit: (dead_code, suggest) Maximum results to return. Default: 50.
        top_n: (dead_code, suggest) Alias for limit used by other dispatcher tools.
        detail_level: Accepted for CLI/MCP consistency; refactor payloads are
            already bounded by limit/top_n.
        repo_root: Repository root path. Auto-detected if omitted.

    Returns:
        Mode-specific results dict.
    """
    if top_n is not None:
        limit = top_n
    _ = detail_level

    try:
        request = parse_refactor_request(
            mode=mode,
            old_name=old_name,
            new_name=new_name,
            kind=kind,
            file_pattern=file_pattern,
            limit=limit,
            top_n=top_n,
            detail_level=detail_level,
            repo_root=repo_root,
        )
    except ValidationError as exc:
        return seal_refactor_error(
            attach_answerability(
                {
                    "status": "error",
                    "error": format_validation_error(exc),
                },
                repo_root,
            )
        )

    if request.mode != "rename":
        # `dead_code` and `suggest` are the Rust tool's; `rename` stays here,
        # where `\w` decides a non-ASCII name's validity.
        with ToolStoreScope(logger=logger, context="refactor_func") as scope:
            # Resolves the repository and creates, migrates, or waits for the
            # graph; the Rust tool reads it.
            scope.track(_get_store(request.repo_root))
            return native_tool(
                "refactor_tool",
                mode=request.mode,
                kind=request.kind,
                file_pattern=request.file_pattern,
                limit=request.limit,
                repo_root=request.repo_root,
            )
        return scope.error

    with ToolStoreScope(logger=logger, context="refactor_func") as scope:
        store, root = scope.track(_get_store(request.repo_root))
        answerability = graph_answerability_summary(store)
        missingness = missingness_from_answerability(answerability)
        from ..refactor import rename_preview

        # Without this the preview happily produced edits turning
        # ``def beta():`` into ``def 1 bad name():`` and a non-dry-run
        # apply committed that to disk.
        if not _is_valid_identifier(request.new_name):
            return _error_response(
                f"new_name is not a valid identifier: {request.new_name!r}",
                status="error",
                old_name=request.old_name,
                new_name=request.new_name,
            )
        preview = rename_preview(store, request.old_name, request.new_name)
        if preview is None:
            return seal_refactor_not_found(
                {
                    "status": "not_found",
                    "summary": (
                        f"No node found matching '{request.old_name}' in the current graph."
                    ),
                    "answerability": answerability,
                    "missingness": [
                        *missingness,
                        {
                            "reason_code": "rename_target_not_found_in_graph",
                            "severity": "medium",
                            "claim_effect": (
                                "absence is graph-limited, not proof the symbol does not exist"
                            ),
                        },
                    ],
                }
            )
        result: RefactorPayload = {
            "status": "ok",
            "summary": (
                f"Rename preview: {request.old_name} -> {request.new_name}, "
                f"{len(preview['edits'])} edit(s). Apply with "
                f"apply_refactor_tool in the same `dagayn serve` MCP session "
                f"(refactor_id is session-scoped, expires after 10 min) using "
                f"refactor_id='{preview['refactor_id']}'."
            ),
            **preview,
            "answerability": answerability,
            "missingness": [*missingness, *preview.get("missingness", [])],
            "next_tool_suggestions": [
                "apply_refactor_tool(refactor_id='"
                f"{preview['refactor_id']}', dry_run=true)"
                " in the same session -- preview unified diff before writing files",
                "apply_refactor_tool(refactor_id='"
                f"{preview['refactor_id']}')"
                " in the same session -- apply the rename",
            ],
        }
        result["_hints"] = generate_hints("refactor", result, get_session())
        return seal_refactor_ok(result)

    return scope.error


# ---------------------------------------------------------------------------
# Tool 18: apply_refactor_tool  [REFACTOR]
# ---------------------------------------------------------------------------


def apply_refactor_func(
    refactor_id: str,
    repo_root: str | None = None,
    dry_run: bool = False,
) -> RefactorPayload:
    """Apply a previously previewed refactoring to source files.

    [REFACTOR] Validates the refactor_id, checks expiry, ensures all edit
    paths are within the repo root, then performs exact string replacements.

    Args:
        refactor_id: ID returned by a prior ``refactor_tool(mode="rename")``
            call.
        repo_root: Repository root path. Auto-detected if omitted.
        dry_run: If True, return a unified diff of what would change
            without touching disk. The refactor_id remains valid so the
            user can review the diff, then call again with ``dry_run=False``
            to actually write the changes. See: #176

    Returns:
        Status with count of applied edits and modified files. When
        ``dry_run=True`` the response additionally contains ``would_modify``
        (list of file paths) and ``diffs`` (map of file -> unified-diff
        string).
    """
    try:
        root = _validate_repo_root(Path(repo_root)) if repo_root else find_project_root()
    except (RuntimeError, ValueError) as exc:
        return {"status": "error", "error": str(exc)}

    from ..refactor import apply_refactor

    result = apply_refactor(refactor_id, root, dry_run=dry_run)
    return result
