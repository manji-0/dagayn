"""Tools 17, 18: refactor_func, apply_refactor_func."""

from __future__ import annotations

import logging
from pathlib import Path
from typing import Any, Literal, overload

from pydantic import ValidationError

from ..hints import generate_hints, get_session
from ..incremental import find_project_root
from ..refactor import (
    apply_refactor,
    find_dead_code,
    rename_preview,
    suggest_refactorings,
)
from ..stability_policy import component_stability_profiles, scope_key_for_file
from ..state_types import RefactorMode, format_validation_error, parse_refactor_request
from ._common import (
    _get_store,
    _validate_repo_root,
    attach_answerability,
    graph_answerability_summary,
    guidance_actions_to_hints,
    handle_tool_runtime_error,
    make_guidance_item,
    missingness_from_answerability,
)

logger = logging.getLogger(__name__)


def _apply_stability_policy_to_suggestions(
    suggestions: list[dict[str, Any]],
    profiles: dict[str, dict[str, Any]],
) -> list[dict[str, Any]]:
    for suggestion in suggestions:
        affected_files = [str(path) for path in suggestion.get("affected_files", [])]
        stable_profiles = [
            profiles[scope_key]
            for scope_key in (scope_key_for_file(path) for path in affected_files)
            if scope_key
            and profiles.get(scope_key, {}).get("stable")
            or scope_key
            and profiles.get(scope_key, {}).get("should_be_stable")
        ]
        if not stable_profiles:
            continue
        suggestion["stability_policy"] = {
            "status": "stable_component_guard",
            "profiles": [
                {
                    "scope_key": profile.get("scope_key"),
                    "instability": profile.get("instability"),
                    "reason_codes": profile.get("reason_codes", []),
                    "thresholds": profile.get("thresholds", {}),
                }
                for profile in stable_profiles[:3]
            ],
        }
        work_pack = suggestion.setdefault("work_pack", {})
        defer = work_pack.setdefault("defer_conditions", [])
        defer.append("The affected component is stable or should be stable by shared policy.")
        execution_plan = suggestion.setdefault("execution_plan", {})
        plan_defer = execution_plan.setdefault("defer_if", [])
        plan_defer.append("Stable component policy requires contract and test evidence first.")
        if suggestion.get("type") in {"remove", "move", "split"}:
            suggestion["confidence"] = "low" if suggestion.get("confidence") != "high" else "medium"
            reason_codes = suggestion.setdefault("reason_codes", [])
            if "stable_component_guard" not in reason_codes:
                reason_codes.append("stable_component_guard")
    return suggestions


def _refactor_guidance(
    suggestions: list[dict[str, Any]],
    *,
    limit: int = 3,
) -> list[dict[str, Any]]:
    guidance: list[dict[str, Any]] = []
    for suggestion in suggestions[:limit]:
        work_pack = suggestion.get("work_pack", {})
        evidence = suggestion.get("evidence", {})
        evidence_type = "computed" if evidence else "evaluated"
        missingness = []
        if suggestion.get("type") in {"remove", "move"}:
            missingness.append(
                {
                    "reason_code": "dynamic_dispatch_not_proven_absent",
                    "severity": "medium",
                    "claim_effect": "verify runtime registration, generated code, and public APIs",
                }
            )
        for condition in work_pack.get("defer_conditions", [])[:3]:
            missingness.append(
                {
                    "reason_code": "defer_condition",
                    "severity": "medium",
                    "claim_effect": str(condition),
                }
            )
        guidance.append(
            make_guidance_item(
                claim=str(suggestion.get("description", "Review refactor suggestion.")),
                evidence={
                    "type": evidence_type,
                    "suggestion_type": suggestion.get("type"),
                    "symbols": suggestion.get("symbols", []),
                    "reason_codes": suggestion.get("reason_codes", []),
                    "raw": evidence,
                },
                confidence=str(suggestion.get("confidence", "unknown")),
                missingness=missingness,
                action=(
                    'refactor_tool mode="suggest" -- inspect work_pack, then run the '
                    "verification commands before editing"
                ),
                reason_codes=list(suggestion.get("reason_codes", [])),
                counts=work_pack.get("blast_radius", {}),
                work_pack={
                    key: work_pack.get(key)
                    for key in (
                        "safe_first_commit",
                        "required_tests",
                        "documentation_obligations",
                        "rollback_path",
                        "defer_conditions",
                    )
                },
            )
        )
    return guidance


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
) -> dict[str, Any]: ...


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
) -> dict[str, Any]: ...


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
) -> dict[str, Any]:
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
        return attach_answerability(
            {
                "status": "error",
                "error": format_validation_error(exc),
            },
            repo_root,
        )

    store = None
    try:
        store, root = _get_store(request.repo_root)
        answerability = graph_answerability_summary(store)
        missingness = missingness_from_answerability(answerability)
        if request.mode == "rename":
            preview = rename_preview(store, request.old_name, request.new_name)
            if preview is None:
                return {
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
            result: dict[str, Any] = {
                "status": "ok",
                "summary": (
                    f"Rename preview: {request.old_name} -> {request.new_name}, "
                    f"{len(preview['edits'])} edit(s). "
                    f"Use apply_refactor_tool(refactor_id="
                    f"'{preview['refactor_id']}') to apply."
                ),
                **preview,
                "answerability": answerability,
                "missingness": missingness,
                "next_tool_suggestions": [
                    f"apply_refactor_tool(refactor_id='{preview['refactor_id']}', dry_run=True)"
                    " -- preview unified diff before writing files",
                    f"apply_refactor_tool(refactor_id='{preview['refactor_id']}')"
                    " -- apply the rename",
                ],
            }
            result["_hints"] = generate_hints("refactor", result, get_session())
            return result

        if request.mode == "dead_code":
            dead = find_dead_code(
                store,
                kind=request.kind,
                file_pattern=request.file_pattern,
            )
            total = len(dead)
            truncated = total > request.limit
            result: dict[str, Any] = {
                "status": "ok",
                "summary": f"Found {total} dead code symbol(s)."
                + (f" Showing first {request.limit}." if truncated else ""),
                "dead_code": dead[: request.limit],
                "total": total,
                "truncated": truncated,
                "caveats": [
                    "Dead-code results are graph-backed candidates; verify dynamic dispatch, "
                    "plugin registration, reflection, and generated entry points before deleting."
                ],
                "answerability": answerability,
                "missingness": [
                    *missingness,
                    {
                        "reason_code": "absence_evidence_requires_manual_verification",
                        "severity": "medium",
                        "claim_effect": "dead-code claims do not cover dynamic runtime references",
                    },
                ],
            }
            result["_hints"] = generate_hints("refactor", result, get_session())
            return result

        suggestions = suggest_refactorings(store)
        suggestions = _apply_stability_policy_to_suggestions(
            suggestions,
            component_stability_profiles(store),
        )
        total = len(suggestions)
        truncated = total > request.limit
        counts_by_type: dict[str, int] = {}
        for suggestion in suggestions:
            stype = str(suggestion.get("type", "unknown"))
            counts_by_type[stype] = counts_by_type.get(stype, 0) + 1
        result: dict[str, Any] = {
            "status": "ok",
            "summary": f"Generated {total} refactoring suggestion(s)."
            + (f" Showing first {request.limit}." if truncated else ""),
            "suggestions": suggestions[: request.limit],
            "work_packs": [
                {
                    "symbols": suggestion.get("symbols", []),
                    "type": suggestion.get("type"),
                    **suggestion.get("work_pack", {}),
                }
                for suggestion in suggestions[: min(request.limit, 5)]
            ],
            "guidance": _refactor_guidance(suggestions[: request.limit]),
            "total": total,
            "truncated": truncated,
            "counts_by_type": counts_by_type,
            "answerability": answerability,
            "missingness": missingness,
        }
        result["_hints"] = guidance_actions_to_hints(result["guidance"])
        if not result["_hints"]["next_steps"]:
            result["_hints"] = generate_hints("refactor", result, get_session())
        return result

    except Exception as exc:
        return handle_tool_runtime_error(exc, logger=logger, context="refactor_func")
    finally:
        if store is not None:
            store.close()


# ---------------------------------------------------------------------------
# Tool 18: apply_refactor_tool  [REFACTOR]
# ---------------------------------------------------------------------------


def apply_refactor_func(
    refactor_id: str,
    repo_root: str | None = None,
    dry_run: bool = False,
) -> dict[str, Any]:
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

    result = apply_refactor(refactor_id, root, dry_run=dry_run)
    return result
