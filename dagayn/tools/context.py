"""Tool: get_minimal_context — ultra-compact context for token-efficient workflows."""

from __future__ import annotations

import logging
import sqlite3
import subprocess
from pathlib import Path
from typing import Any

from ._common import _get_store, compact_response

logger = logging.getLogger(__name__)


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
            ):
                logger.debug("Risk analysis failed in get_minimal_context", exc_info=True)

        # 3. Top 3 communities
        communities: list[str] = []
        try:
            rows = store._conn.execute(
                "SELECT name FROM communities ORDER BY size DESC LIMIT 3"
            ).fetchall()
            communities = _names_from_rows(rows, limit=3)
        except sqlite3.OperationalError:  # nosec B110 — table may not exist yet
            logger.debug("communities table not yet populated")

        # 4. Top 3 critical flows
        top_flows: list[str] = []
        try:
            rows = store._conn.execute(
                "SELECT name FROM flows ORDER BY criticality DESC LIMIT 3"
            ).fetchall()
            top_flows = _names_from_rows(rows, limit=3)
        except sqlite3.OperationalError:  # nosec B110 — table may not exist yet
            logger.debug("flows table not yet populated")

        # 5. Suggest next tools based on task keywords
        task_lower = task.lower()
        if any(w in task_lower for w in ("review", "pr", "merge", "diff")):
            suggestions = ["detect_changes", "get_affected_flows", "get_review_context"]
        elif any(w in task_lower for w in ("debug", "bug", "error", "fix")):
            suggestions = ["semantic_search_nodes", "query_graph", "get_flow"]
        elif any(w in task_lower for w in ("refactor", "rename", "dead", "clean")):
            suggestions = ["refactor", "find_large_functions", "get_architecture_overview"]
        elif any(w in task_lower for w in ("onboard", "understand", "explore", "arch")):
            suggestions = [
                "get_architecture_overview",
                "list_communities",
                "list_flows",
            ]
        else:
            suggestions = [
                "detect_changes",
                "semantic_search_nodes",
                "get_architecture_overview",
            ]

        # Build summary
        summary_parts = [
            f"{stats.total_nodes} nodes, {stats.total_edges} edges"
            f" across {stats.files_count} files.",
        ]
        if risk != "unknown":
            summary_parts.append(f"Risk: {risk} ({risk_score:.2f}).")
        if test_gap_count:
            summary_parts.append(f"{test_gap_count} test gaps.")

        return compact_response(
            summary=" ".join(summary_parts),
            key_entities=top_affected or None,
            risk=risk,
            communities=communities or None,
            top_flows=top_flows or None,
            flows_affected=affected_flows or None,
            next_tool_suggestions=suggestions,
        )
    finally:
        store.close()
