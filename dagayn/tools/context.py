"""Tool: get_minimal_context — ultra-compact context for token-efficient workflows.

The answer, the risk analysis, and any queued repair are the Rust
implementation's (``dagayn_tools::context``); this opens the graph first, so a
missing one is created and an old one migrated before Rust reads it.
"""

from __future__ import annotations

import logging
import sqlite3
import sys

from ._common import (
    ToolPayload,
    _db_path_for_repo,
    _get_store,
    handle_tool_runtime_error,
    is_sqlite_corrupt_error,
    recover_corrupt_graph,
)
from ._native import native_tool_in_session

logger = logging.getLogger(__name__)


def get_minimal_context(
    task: str = "",
    changed_files: list[str] | None = None,
    repo_root: str | None = None,
    base: str | None = None,
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
        changed_files: Explicit changed files to score for review priority.
        repo_root: Repository root path. Auto-detected if None.
        base: Git ref for diff comparison.
        detail_level: Accepted for CLI/MCP interface consistency. This tool is
              intentionally compact, so all detail levels share the same shape.
        auto_prepare: When True, enqueue a background ``prepare`` if the graph
              is empty or HEAD-drifted (or an ``embed`` for a local embedding
              gap), starting a queue worker with this interpreter. Does not wait
              for the repair. Dirty worktrees are structure-ready and do not
              auto-prepare on every call.
        local_embedding: Embedding mode for auto-prepare (serve default via MCP).
        prepare_budget_seconds: Wall-clock budget stored on the queued prepare.
    """
    _ = detail_level
    attempted_recover = False
    while True:
        try:
            # Resolves the repository and creates, migrates, or waits for the
            # graph; the Rust tool reads it.
            store, _root = _get_store(repo_root, cached=False)
            try:
                return native_tool_in_session(
                    "get_minimal_context_tool",
                    {
                        "local_embedding": local_embedding,
                        "auto_prepare": auto_prepare,
                        "python_executable": sys.executable,
                        "prepare_budget_seconds": prepare_budget_seconds,
                    },
                    task=task,
                    changed_files=changed_files,
                    repo_root=repo_root,
                    base=base,
                )
            finally:
                store.close()
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
