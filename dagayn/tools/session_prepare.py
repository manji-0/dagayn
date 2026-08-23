"""Session prepare — usable+synced graph bootstrap with budgeted phases."""

from __future__ import annotations

import os
import time
from pathlib import Path
from typing import Literal

from ..incremental import detect_vcs
from ..paths import get_db_path
from . import sync_status as sync_status_mod
from ._common import (
    ToolPayload,
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

#: The budget above is advisory: it decides whether to *start* another phase and
#: cannot interrupt one already running. A phase with no backstop at all held one
#: repository's exclusive graph lock for 26 hours (21.5 h of CPU), which is the
#: stall every MCP tool call on that graph was waiting behind. Killing at exactly
#: the budget is the wrong backstop though -- a phase that needs slightly more
#: than the budget would be killed on every session start and the graph would
#: never finish updating -- so the hard stop is this multiple of the budget.
PREPARE_BUDGET_HARD_STOP_FACTOR = 4


def prepare_hard_stop_seconds(budget_seconds: int | None) -> float | None:
    """Wall-clock limit after which a running prepare is killed outright.

    ``None`` when the budget is disabled, matching the advisory budget: an
    explicitly unbounded prepare stays unbounded.
    """
    if budget_seconds is None or budget_seconds <= 0:
        return None
    return float(budget_seconds) * PREPARE_BUDGET_HARD_STOP_FACTOR


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


def _seed_worktree_if_needed(repo_root: Path, *, copy_config: bool = True) -> ToolPayload:
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
    payload: ToolPayload = {
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
    # ``find_project_root`` (not ``find_repo_root``) so the IDE workspace hints
    # apply here too. Without them this resolved ``$HOME`` whenever the editor
    # launched the server with ``cwd=$HOME`` and then *built a graph there*,
    # indexing every checkout below it.
    from ..incremental import find_project_root

    return Path(find_project_root()).resolve()


def _structure_embed_files(build_result: ToolPayload | None, pending_files: list[str]) -> list[str]:
    """Files whose nodes should be hash-checked after a structure pass."""
    if not build_result or build_result.get("skipped"):
        return list(pending_files)
    return list(
        dict.fromkeys(
            [
                *(build_result.get("changed_files") or []),
                *(build_result.get("dependent_files") or []),
                *pending_files,
            ]
        )
    )


def _queue_embedding_refresh(
    root: Path,
    *,
    files: list[str] | None,
    local_embedding: str | None,
    local_embedding_mode: str | None,
    local_embedding_port: int | None,
    local_embedding_bin: str,
    keep_local_embedding_server: bool,
    local_embedding_timeout: int,
    local_embedding_request_timeout: int,
    local_embedding_batch_size: int,
) -> ToolPayload:
    from ..task_queue import enqueue_embed_refresh

    action, task_id = enqueue_embed_refresh(
        root,
        files=files,
        spawn_worker=True,
        payload={
            "local_embedding": local_embedding,
            "local_embedding_mode": local_embedding_mode,
            "local_embedding_port": local_embedding_port,
            "local_embedding_bin": local_embedding_bin,
            "keep_local_embedding_server": keep_local_embedding_server,
            "local_embedding_timeout": local_embedding_timeout,
            "local_embedding_request_timeout": local_embedding_request_timeout,
            "local_embedding_batch_size": local_embedding_batch_size,
        },
    )
    return {
        "status": "ok",
        "queued": True,
        "queue_action": action,
        "task_id": task_id,
        "files": files,
        "summary": f"{action} embed task {task_id}",
    }


def session_prepare(
    repo_root: str | None = None,
    *,
    force: bool = False,
    local_embedding: str | None = "none",
    local_embedding_mode: str | None = None,
    local_embedding_port: int | None = None,
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
) -> ToolPayload:
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
    seed_info: ToolPayload | None = None
    build_result: ToolPayload | None = None
    embedding_result: ToolPayload | None = None
    pending_files: list[str] = []
    action = "noop"
    reason = "graph_ready"

    if detect_vcs(root) == "none":
        # Refuse to bootstrap a non-repo tree. MCP auto-prepare can misresolve
        # the root (e.g. to $HOME when the editor spawns the server outside the
        # project), and a full build there would scan the entire non-repo
        # directory. Explicit `dagayn build --repo` remains available.
        return make_response(
            status="ok",
            summary=(
                f"session prepare ({action}); root {root} is not inside a git/svn "
                "repository; nothing to build"
            ),
            action=action,
            reason="not_vcs_repo",
            total_nodes=0,
            total_edges=0,
            files_count=0,
            last_updated=None,
            graph_health=None,
            sync={"state": None, "vcs": "none", "repo_root": str(root)},
            phases=phases,
            budget_seconds=budget_seconds,
            elapsed_seconds=round(time.monotonic() - started, 3),
            local_embedding=local_embedding,
            embedding_policy=embedding_policy,
            repo_root=str(root),
            next_tool_suggestions=[
                "get_minimal_context_tool",
                "review_tool",
                "query_graph_tool",
            ],
        )

    if seed_worktree:
        remaining = _remaining_seconds(deadline)
        if remaining is not None and remaining <= 0:
            phases["structure"] = "skipped_budget"
        else:
            seed_info = _seed_worktree_if_needed(root)

    _evict_store_cache()
    store, resolved_root = _get_store(str(root), cached=False)
    root = Path(resolved_root)
    try:
        # Unlimited content verification: prepare is the caller that can afford
        # it, and it is about to re-index anyway. The read-path default cap
        # would silently skip verification on any fresh checkout.
        sync_before = assess_graph_sync(store, root, max_hash_candidates=None)
        db_path = get_db_path(root)
        refresh_before = sync_status_mod.embedding_refresh_action(
            db_path, local_embedding=local_embedding
        )
        if _local_embedding_requested(local_embedding):
            phases["embedding"] = "done" if refresh_before == "skip" else "pending"
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
            elif (
                force
                and state_before in {"commit_synced", "worktree_ahead"}
                and refresh_before == "skip"
            ):
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
            # Content drift the git diff cannot see: without this the
            # ``worktree_behind`` that pending_files proves is a fixed point,
            # and every session start re-runs an update that reports
            # "No changes detected" while the graph keeps the wrong content.
            pending_files = list(sync_before.get("pending_files") or []) if not needs_full else []
            try:
                build_result = build_or_update_graph(
                    full_rebuild=needs_full,
                    repo_root=str(root),
                    base=base,
                    postprocess=_ENSURE_POSTPROCESS,
                    local_embedding="none",
                    extra_files=pending_files or None,
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
        elif sync_state(sync_before) == "worktree_ahead" or sync_before.get("worktree_dirty"):
            # Skipping a dirty worktree is the policy (edit hooks index those,
            # and re-preparing would re-hash the tree on every call) -- but
            # reporting it as "graph_ready" told the caller the graph covers
            # edits it may not have.
            reason = "graph_ready_worktree_dirty"

    # Reassess after structure
    _evict_store_cache()
    store, _ = _get_store(str(root), cached=False)
    try:
        sync_after = assess_graph_sync(store, root)
        stats = store.get_stats()
        health = graph_answerability_summary(store, stats)
        db_path = get_db_path(root)
        refresh = sync_status_mod.embedding_refresh_action(db_path, local_embedding=local_embedding)
    finally:
        store.close()

    structure_files = _structure_embed_files(build_result, pending_files)

    # --- Phase 2: embeddings ---
    if not _local_embedding_requested(local_embedding):
        phases["embedding"] = "not_requested"
    elif embedding_policy == "skip":
        phases["embedding"] = "not_requested"
    else:
        remaining = _remaining_seconds(deadline)
        allow_inline = embedding_policy == "inline" or (
            embedding_policy == "auto"
            and (remaining is None or remaining >= _EMBEDDING_MIN_REMAINING_SECONDS)
        )
        want_inline = (embedding_policy == "inline" and (refresh != "skip" or force)) or (
            embedding_policy == "auto" and (refresh == "inline" or (force and refresh != "skip"))
        )
        if want_inline and allow_inline:
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
                    embed_files=structure_files or None,
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
        elif (
            (embedding_policy == "defer" and refresh != "skip")
            or refresh == "queue"
            or (want_inline and not allow_inline)
            or (refresh == "skip" and structure_files)
        ):
            queue_files = None if refresh in {"inline", "queue"} else structure_files or None
            queued = _queue_embedding_refresh(
                root,
                files=queue_files,
                local_embedding=local_embedding,
                local_embedding_mode=local_embedding_mode,
                local_embedding_port=local_embedding_port,
                local_embedding_bin=local_embedding_bin,
                keep_local_embedding_server=keep_local_embedding_server,
                local_embedding_timeout=local_embedding_timeout,
                local_embedding_request_timeout=local_embedding_request_timeout,
                local_embedding_batch_size=local_embedding_batch_size,
            )
            embedding_result = queued
            phases["embedding"] = "pending"
            if want_inline and not allow_inline:
                phases["embedding"] = "skipped_budget"
            if action == "noop" and queued:
                action = "incremental"
                reason = "embedding_queued"
        else:
            phases["embedding"] = "done"

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
