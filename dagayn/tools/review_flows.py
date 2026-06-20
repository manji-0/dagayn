"""Review tools focused on execution-flow impact."""

from __future__ import annotations

import logging
from typing import Any

from ..flows import get_affected_flows as _get_affected_flows
from ..hints import generate_hints, get_session
from ..incremental import get_changed_file_sources, get_staged_and_unstaged
from ._common import (
    _get_store,
    graph_answerability_summary,
    handle_tool_runtime_error,
    missingness_from_answerability,
)

logger = logging.getLogger(__name__)


def get_affected_flows_func(
    changed_files: list[str] | None = None,
    base: str = "HEAD~1",
    repo_root: str | None = None,
) -> dict[str, Any]:
    """Find execution flows affected by changed files.

    [REVIEW] Identifies which execution flows pass through nodes in the
    changed files.  Useful during code review to understand which user-facing
    or critical paths are affected by a change.

    Args:
        changed_files: List of changed file paths (relative to repo root).
                       Auto-detected from git diff if omitted.
        base: Git ref for auto-detecting changes (default: HEAD~1).
        repo_root: Repository root path. Auto-detected if omitted.

    Returns:
        Affected flows sorted by criticality, with step details.
    """
    store = None
    try:
        store, root = _get_store(repo_root)
        answerability = graph_answerability_summary(store)
        missingness = missingness_from_answerability(answerability)
        change_file_sources: dict[str, list[str]]
        if changed_files is None:
            change_file_sources = get_changed_file_sources(root, base)
            changed_files = change_file_sources["files"]
            if not changed_files:
                changed_files = get_staged_and_unstaged(root)
                change_file_sources = {"files": changed_files, "worktree": changed_files}
        else:
            change_file_sources = {"files": changed_files, "explicit": changed_files}

        if not changed_files:
            return {
                "status": "ok",
                "summary": "No changed files detected.",
                "affected_flows": [],
                "total": 0,
                "answerability": answerability,
                "missingness": missingness,
            }

        abs_files = [str(root / f) for f in changed_files]
        result = _get_affected_flows(store, abs_files)

        total = result["total"]
        out = {
            "status": "ok",
            "summary": (f"{total} flow(s) affected by changes in {len(changed_files)} file(s)"),
            "changed_files": changed_files,
            "change_file_sources": change_file_sources,
            "affected_flows": result["affected_flows"],
            "total": total,
            "answerability": answerability,
            "missingness": missingness,
        }
        out["_hints"] = generate_hints("get_affected_flows", out, get_session())
        return out
    except Exception as exc:
        return handle_tool_runtime_error(exc, logger=logger, context="get_affected_flows")
    finally:
        if store is not None:
            store.close()
