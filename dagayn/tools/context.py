"""Tool: get_minimal_context — ultra-compact context for token-efficient workflows."""

from __future__ import annotations

import logging
import sqlite3
import subprocess
from pathlib import Path
from typing import Any

from ._common import _get_store, compact_response, graph_answerability_summary

logger = logging.getLogger(__name__)

_REVIEW_TASK_KEYWORDS = (
    "review",
    "pr",
    "merge",
    "diff",
    "レビュー",
    "差分",
    "プルリク",
    "マージ",
)
_DEBUG_TASK_KEYWORDS = (
    "debug",
    "bug",
    "error",
    "fix",
    "デバッグ",
    "バグ",
    "不具合",
    "エラー",
    "修正",
)
_FEATURE_TASK_KEYWORDS = (
    "feature",
    "add",
    "implement",
    "機能追加",
    "新規機能",
    "実装",
    "追加",
)
_REFACTOR_TASK_KEYWORDS = (
    "refactor",
    "rename",
    "dead",
    "clean",
    "リファクタ",
    "リファクタリング",
    "名称変更",
    "改名",
    "デッドコード",
    "整理",
)
_EXPLORE_TASK_KEYWORDS = (
    "onboard",
    "understand",
    "explore",
    "arch",
    "探索",
    "理解",
    "アーキテクチャ",
    "構造",
    "オンボーディング",
)

_REVIEW_TOOL_SUGGESTIONS = ["review_tool", "flow_tool", "query_graph"]
_DEBUG_TOOL_SUGGESTIONS = ["semantic_search_nodes", "query_graph", "flow_tool"]
_FEATURE_TOOL_SUGGESTIONS = ["semantic_search_nodes", "query_graph", "review_tool"]
_REFACTOR_TOOL_SUGGESTIONS = ["refactor", "find_large_functions", "architecture_analysis_tool"]
_EXPLORE_TOOL_SUGGESTIONS = [
    "architecture_analysis_tool",
    "flow_tool",
    "query_graph",
]
_DEFAULT_TOOL_SUGGESTIONS = [
    "review_tool",
    "semantic_search_nodes",
    "architecture_analysis_tool",
]
_WORKFLOW_GUIDANCE: dict[str, dict[str, str]] = {
    "review": {
        "recommended_action": (
            "Run review_tool mode=changes first, then drill into context only when needed."
        ),
        "why": (
            "The task mentions reviewing a diff or PR, so risk and changed-node ranking "
            "are the fastest entry point."
        ),
        "confidence": "high",
    },
    "debug": {
        "recommended_action": (
            "Search for the failing concept, then trace callers and callees around the "
            "matching node."
        ),
        "why": (
            "The task mentions a bug or failure, so locating the relevant symbol before "
            "graph traversal reduces noise."
        ),
        "confidence": "high",
    },
    "refactor": {
        "recommended_action": (
            "Get graph-backed refactor suggestions, then verify impact before editing."
        ),
        "why": (
            "The task mentions cleanup or refactoring, so candidate ranking and safety "
            "checks should precede file edits."
        ),
        "confidence": "high",
    },
    "explore": {
        "recommended_action": (
            "Start with architecture_analysis_tool mode=overview, "
            "then drill into communities or flow_tool mode=list."
        ),
        "why": (
            "The task asks to understand structure, so a broad graph summary is cheaper "
            "than reading files first."
        ),
        "confidence": "high",
    },
    "feature": {
        "recommended_action": (
            "Search for related symbols, trace dependencies, then run change review "
            "after implementation."
        ),
        "why": (
            "The task mentions adding behavior, so finding extension points should come "
            "before editing."
        ),
        "confidence": "medium",
    },
    "general": {
        "recommended_action": (
            "Use minimal change review, semantic search, or architecture overview based "
            "on the first concrete finding."
        ),
        "why": (
            "No specific workflow keyword was detected, so the default keeps broad "
            "options available."
        ),
        "confidence": "low",
    },
}


def _has_git_changes(root: Path, base: str) -> bool:
    """Quick check for uncommitted or diffed changes."""
    try:
        result = subprocess.run(
            ["git", "diff", "--name-only", base, "--"],
            capture_output=True,
            text=True,
            cwd=str(root),
            timeout=10,
        )
        if result.returncode == 0 and result.stdout.strip():
            return True
        # Also check staged/unstaged
        result2 = subprocess.run(
            ["git", "status", "--porcelain"],
            capture_output=True,
            text=True,
            cwd=str(root),
            timeout=10,
        )
        return bool(result2.stdout.strip())
    except (FileNotFoundError, subprocess.TimeoutExpired):
        return False


def _row_name(row: Any) -> str | None:
    """Extract a ``name`` value from a sqlite row/tuple/dict-like object."""
    if row is None:
        return None
    if hasattr(row, "keys"):
        name = row["name"]
    else:
        name = row[0]
    return name if isinstance(name, str) and name else None


def _names_from_rows(rows: list[Any], *, limit: int) -> list[str]:
    """Return up to *limit* non-empty names from query rows."""
    names: list[str] = []
    for row in rows:
        name = _row_name(row)
        if name:
            names.append(name)
        if len(names) >= limit:
            break
    return names


def _names_from_items(items: list[dict[str, Any]], *, limit: int) -> list[str]:
    """Return up to *limit* non-empty names from tool payload items."""
    names: list[str] = []
    for item in items:
        name = item.get("name")
        if isinstance(name, str) and name:
            names.append(name)
        if len(names) >= limit:
            break
    return names


def _graph_answerability(store: Any, stats: Any) -> dict[str, Any]:
    """Summarize whether the graph can answer review/exploration questions."""
    summary = graph_answerability_summary(store, stats)
    # get_minimal_context has a strict compactness budget; detailed counts are
    # available from list_graph_stats_tool and from non-minimal dispatcher calls.
    summary.pop("counts", None)
    summary.pop("reason_codes", None)
    return summary


def _task_mentions(task: str, keywords: tuple[str, ...]) -> bool:
    """Return whether *task* contains any workflow keyword."""
    task_folded = task.casefold()
    return any(keyword.casefold() in task_folded for keyword in keywords)


def _suggest_tools_for_task(task: str) -> list[str]:
    """Choose next MCP tool suggestions from a natural-language task."""
    workflow = _workflow_for_task(task)
    if workflow == "review":
        return list(_REVIEW_TOOL_SUGGESTIONS)
    if workflow == "debug":
        return list(_DEBUG_TOOL_SUGGESTIONS)
    if workflow == "refactor":
        return list(_REFACTOR_TOOL_SUGGESTIONS)
    if workflow == "explore":
        return list(_EXPLORE_TOOL_SUGGESTIONS)
    if workflow == "feature":
        return list(_FEATURE_TOOL_SUGGESTIONS)
    return list(_DEFAULT_TOOL_SUGGESTIONS)


def _workflow_for_task(task: str) -> str:
    """Classify a natural-language task into a coarse workflow."""
    if _task_mentions(task, _REVIEW_TASK_KEYWORDS):
        return "review"
    if _task_mentions(task, _DEBUG_TASK_KEYWORDS):
        return "debug"
    if _task_mentions(task, _REFACTOR_TASK_KEYWORDS):
        return "refactor"
    if _task_mentions(task, _EXPLORE_TASK_KEYWORDS):
        return "explore"
    if _task_mentions(task, _FEATURE_TASK_KEYWORDS):
        return "feature"
    return "general"


def get_minimal_context(
    task: str = "",
    changed_files: list[str] | None = None,
    repo_root: str | None = None,
    base: str = "HEAD~1",
) -> dict[str, Any]:
    """Return minimum context an agent needs to start any task (~100 tokens).

    Combines graph stats, top communities, top flows, risk score,
    and suggested next tools into an ultra-compact response.

    Args:
        task: Natural language description of what the agent is doing
              (e.g. "review PR #42", "debug login timeout").
        changed_files: Explicit changed files. Auto-detected from git if None.
        repo_root: Repository root path. Auto-detected if None.
        base: Git ref for diff comparison.
    """
    # Use a dedicated GraphStore connection for this tool to avoid sharing a
    # cached sqlite handle across concurrent MCP calls.
    store, root = _get_store(repo_root, cached=False)
    try:
        # 1. Quick stats
        stats = store.get_stats()
        graph_health = _graph_answerability(store, stats)

        # 2. Risk from changed files
        risk = "unknown"
        risk_score = 0.0
        top_affected: list[str] = []
        affected_flows: list[str] = []
        test_gap_count = 0
        if changed_files or _has_git_changes(root, base):
            try:
                from ..changes import analyze_changes
                from ..incremental import get_changed_files as _get_changed

                files = changed_files
                if not files:
                    files = _get_changed(root, base)
                if files:
                    abs_files = [str(root / f) for f in files]
                    analysis = analyze_changes(
                        store,
                        abs_files,
                        repo_root=str(root),
                        base=base,
                    )
                    risk_score = analysis.get("risk_score", 0.0)
                    risk = "high" if risk_score > 0.7 else "medium" if risk_score > 0.4 else "low"
                    priorities = analysis.get("review_priorities", [])
                    if not priorities:
                        priorities = analysis.get("changed_functions", [])
                    top_affected = _names_from_items(priorities, limit=5)
                    affected_flows = _names_from_items(
                        analysis.get("affected_flows", []),
                        limit=5,
                    )
                    test_gap_count = len(analysis.get("test_gaps", []))
            except (
                ImportError,
                OSError,
                ValueError,
                sqlite3.Error,
                subprocess.SubprocessError,
                AttributeError,
            ):
                logger.debug("Risk analysis failed in get_minimal_context", exc_info=True)

        # 3. Top 3 communities
        communities: list[str] = []
        try:
            conn = getattr(store, "_conn", None)
            if conn is not None:
                rows = conn.execute(
                    "SELECT name FROM communities ORDER BY size DESC LIMIT 3"
                ).fetchall()
                communities = _names_from_rows(rows, limit=3)
            else:
                from ..communities import get_communities

                communities = _names_from_items(
                    get_communities(store, sort_by="size")[:3],
                    limit=3,
                )
        except (sqlite3.OperationalError, RuntimeError, ImportError, KeyError, TypeError):  # nosec B110
            logger.debug("communities table not yet populated")

        # 4. Top 3 critical flows
        top_flows: list[str] = []
        try:
            conn = getattr(store, "_conn", None)
            if conn is not None:
                rows = conn.execute(
                    "SELECT name FROM flows ORDER BY criticality DESC LIMIT 3"
                ).fetchall()
                top_flows = _names_from_rows(rows, limit=3)
            else:
                from ..flows import get_flows

                top_flows = _names_from_items(get_flows(store, limit=3), limit=3)
        except (sqlite3.OperationalError, RuntimeError, ImportError, KeyError, TypeError):  # nosec B110
            logger.debug("flows table not yet populated")

        # 5. Suggest next tools based on task keywords
        workflow = _workflow_for_task(task)
        suggestions = _suggest_tools_for_task(task)
        guidance = _WORKFLOW_GUIDANCE[workflow]

        # Build summary
        summary_parts = [
            f"{stats.total_nodes} nodes, {stats.total_edges} edges"
            f" across {stats.files_count} files.",
        ]
        if risk != "unknown":
            summary_parts.append(f"Risk: {risk} ({risk_score:.2f}).")
        if test_gap_count:
            summary_parts.append(f"{test_gap_count} test gaps.")

        response = compact_response(
            summary=" ".join(summary_parts),
            key_entities=top_affected or None,
            risk=risk,
            communities=communities or None,
            top_flows=top_flows or None,
            flows_affected=affected_flows or None,
            next_tool_suggestions=suggestions,
        )
        response["workflow"] = workflow
        response["recommended_action"] = guidance["recommended_action"]
        response["why"] = guidance["why"]
        response["confidence"] = guidance["confidence"]
        response["graph_health"] = graph_health
        return response
    finally:
        store.close()
