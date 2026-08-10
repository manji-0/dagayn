"""Session prepare — usable+synced graph bootstrap with budgeted phases."""

from __future__ import annotations

import os
import time
from pathlib import Path
from typing import Any, Literal

from . import sync_status as sync_status_mod
from ._common import (
    _evict_store_cache,
    _get_store,
    graph_answerability_summary,
    make_response,
)
from .build import _local_embedding_requested, build_or_update_graph
from .sync_status import (
    assess_graph_sync,
    is_structure_ready,
    needs_structure_prepare,
    sync_state,
)

_ENSURE_POSTPROCESS = "minimal"
_DEFAULT_HOOK_BUDGET_SECONDS = 45
_DEFAULT_MCP_BUDGET_SECONDS = 300
_EMBEDDING_MIN_REMAINING_SECONDS = 15

EmbeddingPolicy = Literal["auto", "defer", "skip", "inline"]

PhaseState = Literal[
    "done",
    "skipped_budget",
    "pending",
    "not_requested",
    "failed",
    "noop",
]


def default_prepare_budget_seconds(*, mcp: bool = False) -> int | None:
    """Return the default wall-clock budget for session prepare."""
    env = os.environ.get("DAGAYN_SESSION_PREPARE_BUDGET_SECONDS")
    if env is not None and env.strip() != "":
        try:
            value = int(env)
            return None if value <= 0 else value
        except ValueError:
            pass
    return _DEFAULT_MCP_BUDGET_SECONDS if mcp else _DEFAULT_HOOK_BUDGET_SECONDS


def _remaining_seconds(deadline: float | None) -> float | None:
    if deadline is None:
        return None
    return max(0.0, deadline - time.monotonic())


def _seed_worktree_if_needed(repo_root: Path, *, copy_config: bool = True) -> dict[str, Any]:
    """Seed a linked worktree graph from the main checkout when needed."""
    from ..worktree import copy_worktree_config, is_linked_worktree, seed_worktree_graph

    if not is_linked_worktree(repo_root):
        return {"seeded": False, "skipped": True, "reason": "not_linked_worktree"}

    copied: list[str] = []
    if copy_config:
        try:
            copied = copy_worktree_config(repo_root)
        except Exception as exc:
            copied = []
            copy_error = str(exc)
        else:
            copy_error = None
    else:
        copy_error = None

    seed = seed_worktree_graph(repo_root)
    payload: dict[str, Any] = {
        "seeded": seed.status == "seeded",
        "status": seed.status,
        "reason": seed.reason,
        "base_sha": seed.base_sha,
        "copied_config": copied,
    }
    if copy_error:
        payload["copy_error"] = copy_error
    return payload


def _resolve_repo(repo_root: str | None, *, from_hook: bool = False) -> Path:
    if from_hook and not repo_root:
        import sys

        from ..worktree import parse_hook_payload, resolve_hook_repo

        payload = parse_hook_payload(sys.stdin.read() if not sys.stdin.isatty() else "")
        resolved = resolve_hook_repo(payload)
        if resolved is not None:
            return resolved
    if repo_root:
        return Path(repo_root).expanduser().resolve()
    from ..incremental import find_repo_root

    found = find_repo_root()
    if found is None:
        return Path.cwd().resolve()
    return Path(found).resolve()


def session_prepare(
    repo_root: str | None = None,
    *,
    force: bool = False,
    local_embedding: str | None = "none",
    local_embedding_mode: str | None = None,
    local_embedding_port: int = 18080,
    local_embedding_bin: str = "auto",
    keep_local_embedding_server: bool = False,
    local_embedding_timeout: int = 300,
    local_embedding_request_timeout: int = 60,
    local_embedding_batch_size: int = 1,
    budget_seconds: int | None = None,
    embedding_policy: EmbeddingPolicy = "auto",
    from_hook: bool = False,
    build_if_missing: bool = True,
    seed_worktree: bool = True,
) -> dict[str, Any]:
    """Ensure a usable+synced graph for the current repository root.

    Phase 1 refreshes structure (``postprocess=minimal``). Phase 2 optionally
    refreshes local embeddings when *local_embedding* matches the serve/install
    mode and the remaining budget allows it.
    """
    started = time.monotonic()
    if budget_seconds is None:
        budget_seconds = default_prepare_budget_seconds(mcp=False)
    deadline = None if budget_seconds is None else started + float(budget_seconds)

    root = _resolve_repo(repo_root, from_hook=from_hook)
    phases: dict[str, PhaseState] = {
        "structure": "noop",
        "embedding": "not_requested",
    }
    seed_info: dict[str, Any] | None = None
    build_result: dict[str, Any] | None = None
    embedding_result: dict[str, Any] | None = None
    action = "noop"
    reason = "graph_ready"

    if seed_worktree:
        remaining = _remaining_seconds(deadline)
        if remaining is not None and remaining <= 0:
            phases["structure"] = "skipped_budget"
        else:
            seed_info = _seed_worktree_if_needed(root)

    _evict_store_cache()
    store, resolved_root = _get_store(str(root), cached=False, use_backend_default=True)
    root = Path(resolved_root)
    try:
        sync_before = assess_graph_sync(store, root)
        db_path = root / ".dagayn" / "graph.db"
        emb_pending = sync_status_mod.embedding_needs_refresh(
            db_path, local_embedding=local_embedding
        )
        if _local_embedding_requested(local_embedding):
            phases["embedding"] = "pending" if emb_pending else "done"
    finally:
        store.close()

    # --- Phase 1: structure ---
    if needs_structure_prepare(sync_before, force=force):
        remaining = _remaining_seconds(deadline)
        if remaining is not None and remaining <= 0:
            phases["structure"] = "skipped_budget"
            reason = "budget_exhausted_before_structure"
        else:
            state_before = sync_state(sync_before)
            needs_full = state_before == "unbuilt"
            action = "full" if needs_full else "incremental"
            if needs_full:
                reason = "empty_graph"
            elif force and state_before in {"commit_synced", "worktree_ahead"} and not emb_pending:
                reason = "forced_refresh"
            elif state_before == "commit_drift":
                reason = "git_drift"
            elif state_before == "worktree_behind":
                reason = "dirty_worktree"
            else:
                reason = "forced_refresh" if force else "missing_last_updated"

            # Structure phase never spends budget on embeddings.
            # Diff against the commit the graph actually describes — same order
            # as ``dagayn worktree sync``: just-seeded base, else stored
            # ``git_head_sha``, else HEAD~1.
            base = "HEAD~1"
            if seed_info and seed_info.get("base_sha"):
                base = str(seed_info["base_sha"])
            elif sync_before.get("git_head_sha"):
                base = str(sync_before["git_head_sha"])
            try:
                build_result = build_or_update_graph(
                    full_rebuild=needs_full,
                    repo_root=str(root),
                    base=base,
                    postprocess=_ENSURE_POSTPROCESS,
                    local_embedding="none",
                )
                if build_result.get("skipped"):
                    phases["structure"] = "noop"
                    reason = build_result.get("skip_reason") or "skipped"
                else:
                    phases["structure"] = "done"
            except Exception as exc:
                phases["structure"] = "failed"
                build_result = {"status": "error", "summary": str(exc), "errors": [str(exc)]}
                reason = "structure_failed"
    else:
        phases["structure"] = "noop"
        if force:
            # force with already-synced structure still allows embedding refresh below
            action = "noop"
            reason = "graph_ready"

    # Reassess after structure
    _evict_store_cache()
    store, _ = _get_store(str(root), cached=False, use_backend_default=True)
    try:
        sync_after = assess_graph_sync(store, root)
        health = graph_answerability_summary(store, store.get_stats())
        stats = store.get_stats()
        db_path = root / ".dagayn" / "graph.db"
        emb_pending = sync_status_mod.embedding_needs_refresh(
            db_path, local_embedding=local_embedding
        )
    finally:
        store.close()

    # --- Phase 2: embeddings ---
    if not _local_embedding_requested(local_embedding):
        phases["embedding"] = "not_requested"
    elif embedding_policy == "skip":
        phases["embedding"] = "not_requested"
    elif embedding_policy == "defer":
        phases["embedding"] = "pending" if emb_pending else "done"
    elif not emb_pending and not force:
        phases["embedding"] = "done"
    else:
        remaining = _remaining_seconds(deadline)
        allow_inline = embedding_policy == "inline" or (
            embedding_policy == "auto"
            and (remaining is None or remaining >= _EMBEDDING_MIN_REMAINING_SECONDS)
        )
        if not allow_inline:
            phases["embedding"] = "skipped_budget" if embedding_policy == "auto" else "pending"
        else:
            try:
                # Incremental path with local embedding refreshes vectors even
                # when there are no file changes (non-hook callers).
                emb_build = build_or_update_graph(
                    full_rebuild=False,
                    repo_root=str(root),
                    postprocess=_ENSURE_POSTPROCESS,
                    local_embedding=local_embedding,
                    local_embedding_mode=local_embedding_mode,
                    local_embedding_port=local_embedding_port,
                    local_embedding_bin=local_embedding_bin,
                    keep_local_embedding_server=keep_local_embedding_server,
                    local_embedding_timeout=local_embedding_timeout,
                    local_embedding_request_timeout=local_embedding_request_timeout,
                    local_embedding_batch_size=local_embedding_batch_size,
                )
                embedding_result = emb_build.get("local_embedding") or emb_build
                if emb_build.get("status") == "error":
                    phases["embedding"] = "failed"
                else:
                    phases["embedding"] = "done"
                    if action == "noop":
                        action = "incremental"
                        reason = "embedding_refresh"
            except Exception as exc:
                phases["embedding"] = "failed"
                embedding_result = {"status": "error", "error": str(exc)}

    elapsed = time.monotonic() - started
    structure_skipped = bool(build_result and build_result.get("skipped"))
    structure_ready = is_structure_ready(sync_after)
    status = "ok"
    if phases["structure"] == "failed":
        status = "error"
    elif not structure_ready and (phases["structure"] == "skipped_budget" or structure_skipped):
        # Budget miss or hook lock contention left structure not ready — do not
        # report success so callers / MCP can retry.
        status = "partial"
    elif phases["embedding"] in {"pending", "skipped_budget"} and _local_embedding_requested(
        local_embedding
    ):
        status = "partial" if structure_ready else status

    summary_bits = [
        f"session prepare ({action})",
        f"sync={sync_state(sync_after)}",
        f"structure={phases['structure']}",
        f"embedding={phases['embedding']}",
        f"{stats.total_nodes} nodes / {stats.files_count} files",
    ]
    if build_result and build_result.get("summary"):
        summary_bits.append(str(build_result["summary"]))

    response = make_response(
        status=status,
        summary="; ".join(summary_bits),
        action=action,
        reason=reason,
        total_nodes=stats.total_nodes,
        total_edges=stats.total_edges,
        files_count=stats.files_count,
        last_updated=stats.last_updated,
        graph_health=health,
        sync=sync_after,
        phases=phases,
        budget_seconds=budget_seconds,
        elapsed_seconds=round(elapsed, 3),
        local_embedding=local_embedding,
        embedding_policy=embedding_policy,
        repo_root=str(root),
        next_tool_suggestions=[
            "get_minimal_context_tool",
            "review_tool",
            "query_graph_tool",
        ],
    )
    if seed_info is not None:
        response["worktree_seed"] = seed_info
    if build_result is not None:
        response["build"] = build_result
    if embedding_result is not None:
        response["embedding"] = embedding_result
    if build_result and build_result.get("warnings"):
        response["warnings"] = build_result["warnings"]
    if build_result and build_result.get("errors"):
        response["errors"] = build_result["errors"]
    return response
