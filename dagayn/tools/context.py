"""Tool: get_minimal_context — ultra-compact context for token-efficient workflows."""

from __future__ import annotations

import logging
import os
import sqlite3
from collections.abc import Mapping, Sequence
from pathlib import Path
from typing import Any

from ..paths import get_db_path
from ..state_types import AnswerabilityRecord
from ._common import (
    ToolPayload,
    _db_path_for_repo,
    _get_store,
    compact_response,
    graph_answerability_summary,
    handle_tool_runtime_error,
    is_sqlite_corrupt_error,
    recover_corrupt_graph,
)

logger = logging.getLogger(__name__)

_MAX_RISK_FILES = int(os.environ.get("DAGAYN_MINIMAL_CONTEXT_MAX_RISK_FILES", "100"))

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

_REVIEW_TOOL_SUGGESTIONS = ["review_tool", "flow_tool", "query_graph_tool"]
_DEBUG_TOOL_SUGGESTIONS = ["semantic_search_nodes_tool", "query_graph_tool", "flow_tool"]
_FEATURE_TOOL_SUGGESTIONS = ["semantic_search_nodes_tool", "query_graph_tool", "review_tool"]
_REFACTOR_TOOL_SUGGESTIONS = [
    "refactor_tool",
    "query_graph_tool",
    "architecture_analysis_tool",
]
_EXPLORE_TOOL_SUGGESTIONS = [
    "architecture_analysis_tool",
    "flow_tool",
    "query_graph_tool",
]
_DEFAULT_TOOL_SUGGESTIONS = [
    "review_tool",
    "semantic_search_nodes_tool",
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


def _names_from_items(items: Sequence[Mapping[str, object]], *, limit: int) -> list[str]:
    """Return up to *limit* non-empty names from tool payload items."""
    names: list[str] = []
    for item in items:
        name = item.get("name")
        if isinstance(name, str) and name:
            names.append(name)
        if len(names) >= limit:
            break
    return names


def _graph_answerability(store: Any, stats: Any) -> AnswerabilityRecord:
    """Summarize whether the graph can answer review/exploration questions."""
    summary = graph_answerability_summary(store, stats)
    # get_minimal_context has a strict compactness budget; detailed counts are
    # available from non-minimal dispatcher calls.
    summary.pop("counts", None)
    summary.pop("reason_codes", None)
    return summary


def _task_mentions(task: str, keywords: tuple[str, ...]) -> bool:
    """Return whether *task* contains any workflow keyword."""
    task_folded = task.casefold()
    return any(keyword.casefold() in task_folded for keyword in keywords)


def _suggest_tools_for_task(task: str) -> list[str]:
    """Choose next MCP tool suggestions from a natural-language task."""
    from ..tool_surface import filter_tool_names

    workflow = _workflow_for_task(task)
    if workflow == "review":
        names = list(_REVIEW_TOOL_SUGGESTIONS)
    elif workflow == "debug":
        names = list(_DEBUG_TOOL_SUGGESTIONS)
    elif workflow == "refactor":
        names = list(_REFACTOR_TOOL_SUGGESTIONS)
    elif workflow == "explore":
        names = list(_EXPLORE_TOOL_SUGGESTIONS)
    elif workflow == "feature":
        names = list(_FEATURE_TOOL_SUGGESTIONS)
    else:
        names = list(_DEFAULT_TOOL_SUGGESTIONS)
    return filter_tool_names(names)


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
    detail_level: str = "minimal",
    *,
    auto_prepare: bool = False,
    local_embedding: str | None = "none",
    prepare_budget_seconds: int | None = 300,
) -> ToolPayload:
    """Return minimum context an agent needs to start any task (~100 tokens).

    Combines graph stats, top communities, top flows, risk score,
    and suggested next tools into an ultra-compact response.

    Args:
        task: Natural language description of what the agent is doing
              (e.g. "review PR #42", "debug login timeout").
        changed_files: Explicit changed files. Auto-detected from git if None.
        repo_root: Repository root path. Auto-detected if None.
        base: Git ref for diff comparison.
        detail_level: Accepted for CLI/MCP interface consistency. This tool is
              intentionally compact, so all detail levels share the same shape.
        auto_prepare: When True, enqueue a background ``session_prepare`` if
              the graph is empty or HEAD-drifted (and queue deferred embeddings
              when needed). Does not wait for the repair. Dirty worktrees are
              structure-ready and do not auto-prepare on every call.
        local_embedding: Embedding mode for auto-prepare (serve default via MCP).
        prepare_budget_seconds: Wall-clock budget stored on the queued prepare.
    """
    _ = detail_level
    attempted_recover = False
    while True:
        try:
            return _get_minimal_context_body(
                task=task,
                changed_files=changed_files,
                repo_root=repo_root,
                base=base,
                auto_prepare=auto_prepare,
                local_embedding=local_embedding,
                prepare_budget_seconds=prepare_budget_seconds,
            )
        except Exception as exc:
            if is_sqlite_corrupt_error(exc) and not attempted_recover:
                recover_corrupt_graph(_db_path_for_repo(repo_root))
                attempted_recover = True
                logger.warning(
                    "get_minimal_context: sqlite corrupt (%s); retrying after closing live stores",
                    exc,
                )
                continue
            if is_sqlite_corrupt_error(exc) or isinstance(exc, sqlite3.Error):
                return handle_tool_runtime_error(
                    exc,
                    logger=logger,
                    context="get_minimal_context",
                    repo_root=repo_root,
                )
            raise


def _get_minimal_context_body(
    *,
    task: str,
    changed_files: list[str] | None,
    repo_root: str | None,
    base: str,
    auto_prepare: bool,
    local_embedding: str | None,
    prepare_budget_seconds: int | None,
) -> ToolPayload:
    prepare_result: ToolPayload | None = None
    repair_info: dict[str, object] | None = None
    if auto_prepare:
        from .sync_status import (
            assess_graph_sync,
            embedding_refresh_action,
            needs_mcp_auto_prepare,
        )

        probe_store, probe_root = _get_store(repo_root, cached=False)
        try:
            sync = assess_graph_sync(probe_store, probe_root)
            db_path = get_db_path(Path(probe_root))
            refresh = embedding_refresh_action(db_path, local_embedding=local_embedding)
        finally:
            probe_store.close()

        # Observe vs Repair: first-tool never waits on parse/embed. Queue a
        # single prepare (or embed) lane and return the current sync state.
        # Dirty worktrees are HEAD-aligned; re-preparing on every tool call
        # loops. A non-repo root (misdetected, e.g. $HOME) is never queued.
        if (needs_mcp_auto_prepare(sync) or refresh == "inline") and sync.get("vcs") != "none":
            from ..task_queue import enqueue_session_prepare

            action, task_id = enqueue_session_prepare(
                probe_root,
                spawn_worker=True,
                payload={
                    "local_embedding": local_embedding,
                    "keep_local_embedding_server": True,
                    "budget_seconds": prepare_budget_seconds,
                },
            )
            repair_info = {
                "state": "queued" if action == "added" else "coalesced",
                "kind": "prepare",
                "task_id": task_id,
                "action": action,
            }
            prepare_result = {
                "status": "queued",
                "action": "queued",
                "reason": "enqueued_background_prepare",
            }
            repo_root = str(probe_root)
        elif refresh == "queue" and sync.get("vcs") != "none":
            from ..task_queue import enqueue_embed_refresh

            action, task_id = enqueue_embed_refresh(
                probe_root,
                spawn_worker=True,
                payload={
                    "local_embedding": local_embedding,
                    "keep_local_embedding_server": True,
                },
            )
            repair_info = {
                "state": "queued" if action == "added" else "coalesced",
                "kind": "embed",
                "task_id": task_id,
                "action": action,
            }

    # Use a dedicated GraphStore connection for this tool to avoid sharing a
    # cached sqlite handle across concurrent MCP calls.
    store, root = _get_store(repo_root, cached=False)
    try:
        from .sync_status import assess_graph_sync, sync_state

        # 1. Quick stats
        stats = store.get_stats()
        graph_health = _graph_answerability(store, stats)
        sync = assess_graph_sync(store, root)

        # 2. Route the task before optional risk analysis so non-review entry
        # points stay cheap even when the default base has a large diff.
        workflow = _workflow_for_task(task)
        suggestions = _suggest_tools_for_task(task)
        guidance = _WORKFLOW_GUIDANCE[workflow]

        # 3. Risk from explicitly provided changed files
        risk = "unknown"
        risk_score = 0.0
        review_priority_score = 0.0
        top_affected: list[str] = []
        affected_flows: list[str] = []
        test_gap_count = 0
        risk_skipped_count = 0
        files = changed_files
        if files and len(files) > _MAX_RISK_FILES:
            risk = "skipped"
            risk_skipped_count = len(files)
            files = []
        if files:
            try:
                from ..changes import analyze_changes

                abs_files = [str(root / f) for f in files]
                analysis = analyze_changes(
                    store,
                    abs_files,
                    repo_root=str(root),
                    base=base,
                    # The heuristic coverage scan re-scores every test-like
                    # node per changed function; bound it like review does so
                    # a large diff cannot turn minimal-context into a full
                    # graph scan.
                    heuristic_test_gap_node_limit=10,
                )
                review_priority_score = analysis.review_priority_score
                risk_score: float = (
                    review_priority_score if review_priority_score is not None else 0.0
                )
                risk = "high" if risk_score > 0.7 else "medium" if risk_score > 0.4 else "low"
                priorities = analysis.review_priorities
                if not priorities:
                    priorities = analysis.changed_functions
                top_affected = _names_from_items(priorities, limit=5)
                affected_flows = _names_from_items(
                    analysis.affected_flows,
                    limit=5,
                )
                test_gap_count = len(analysis.test_gaps)
            except (
                ImportError,
                OSError,
                ValueError,
                sqlite3.Error,
                AttributeError,
                RuntimeError,
            ):
                logger.debug("Risk analysis failed in get_minimal_context", exc_info=True)

        # 3. Top 3 communities
        communities: list[str] = []
        try:
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
            from ..flows import get_flows

            top_flows = _names_from_items(get_flows(store, limit=3), limit=3)
        except (sqlite3.OperationalError, RuntimeError, ImportError, KeyError, TypeError):  # nosec B110
            logger.debug("flows table not yet populated")

        # Build summary
        summary_parts = [
            f"{stats.total_nodes} nodes, {stats.total_edges} edges"
            f" across {stats.files_count} files.",
        ]
        if risk != "unknown":
            summary_parts.append(f"Review priority: {risk} ({review_priority_score:.2f}).")
        if risk_skipped_count:
            summary_parts.append(f"Risk analysis skipped for {risk_skipped_count} files.")
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
        # Keep sync compact: the state name alone stays within the ~800-char
        # budget. ``status`` rides along for pre-``state`` MCP clients, and
        # ``vcs`` tells the agent when the root is not a repository at all.
        response["sync"] = {
            "state": sync_state(sync),
            "status": sync.get("status"),
            "vcs": sync.get("vcs"),
        }
        if prepare_result is not None:
            response["prepare"] = {
                "status": prepare_result.get("status"),
                "action": prepare_result.get("action"),
                "reason": prepare_result.get("reason"),
                "phases": prepare_result.get("phases"),
            }
        if repair_info is not None:
            response["repair"] = repair_info
        if graph_health.get("status") == "empty" or sync_state(sync) == "unbuilt":
            response["recommended_action"] = (
                "Call ensure_graph_tool first; the graph is empty and analysis "
                "tools will return nothing useful."
            )
            response["why"] = (
                "graph_health reports an empty graph, so build/bootstrap must "
                "precede review, search, or architecture analysis."
            )
            response["confidence"] = "high"
            response["next_tool_suggestions"] = [
                "ensure_graph_tool",
                *[s for s in suggestions if s != "ensure_graph_tool"],
            ]
            hints = response.get("_hints")
            if isinstance(hints, dict):
                next_steps = [
                    {
                        "tool": "ensure_graph_tool",
                        "suggestion": "ensure_graph_tool",
                    },
                    *[
                        step
                        for step in hints.get("next_steps", [])
                        if step.get("tool") != "ensure_graph_tool"
                    ],
                ]
                hints["next_steps"] = next_steps[:3]
                response["_hints"] = hints
        elif sync_state(sync) == "commit_drift":
            # worktree_behind / worktree_ahead are HEAD-aligned structure-ready;
            # do not loop on ensure_graph — edit hooks / session prepare handle
            # dirty indexing.
            if repair_info is not None:
                response["recommended_action"] = (
                    "Graph repair is queued; call ensure_graph_tool only if you must wait for it."
                )
            else:
                response["recommended_action"] = "Call ensure_graph_tool to sync the graph."
            response["why"] = f"sync.state={sync_state(sync)}"
            response["confidence"] = "high"
            if "ensure_graph_tool" not in response["next_tool_suggestions"]:
                response["next_tool_suggestions"] = [
                    "ensure_graph_tool",
                    *response["next_tool_suggestions"],
                ]
        return response
    finally:
        store.close()
