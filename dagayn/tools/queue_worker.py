"""Executes queued graph tasks on the single worker lane.

The queue itself (:mod:`dagayn.task_queue`) only stores and schedules tasks;
running them needs the build and session-prepare tools, which in turn enqueue
follow-up work, so execution lives on the tools side of that dependency.
"""

from __future__ import annotations

import logging
import os
import time
from pathlib import Path
from typing import Any

from ..task_queue import (
    DEFAULT_EMBED_BUDGET_SECONDS,
    DEFAULT_IDLE_SECONDS,
    DEFAULT_POSTPROCESS_BUDGET_SECONDS,
    DEFAULT_PREPARE_BUDGET_SECONDS,
    DEFAULT_UPDATE_BUDGET_SECONDS,
    EMBED_PASS_SECONDS,
    MAX_RETRY_BACKOFF_SECONDS,
    RETRY_BACKOFF_SECONDS,
    TaskQueue,
    WorkerLock,
    _resolve_repo_root,
    enqueue_embed_refresh,
    queue_db_path,
    worker_lock_path,
)

logger = logging.getLogger(__name__)


def _stored_base(repo_root: Path) -> str:
    """Git ref the graph describes — same order as ``dagayn update``."""
    db_path = queue_db_path(repo_root).parent / "graph.db"
    if not db_path.exists():
        return "HEAD~1"
    from ..graph import GraphStore
    from ..write_lock import DEFAULT_READ_LOCK_TIMEOUT, graph_read_lock

    try:
        # Bounded wait, not the writer's full 120 s budget: a writer (manual
        # build, embedding pass) holding the lock makes this task's own write
        # lock skip soon anyway, so there is no point stalling the worker for
        # its whole budget just to peek the stored sha. After the wait we fall
        # back to HEAD~1 and let the non-blocking write lock decide whether the
        # task runs or is skipped with a note.
        with graph_read_lock(db_path, timeout=DEFAULT_READ_LOCK_TIMEOUT):
            store = GraphStore(db_path)
            try:
                return store.get_metadata("git_head_sha") or "HEAD~1"
            finally:
                store.close()
    except Exception:  # noqa: BLE001 - a bad peek must not kill the worker
        logger.warning("Could not read stored git_head_sha; falling back to HEAD~1")
        return "HEAD~1"


def _embed_files_from_payload(payload: dict[str, Any]) -> list[str] | None:
    files = payload.get("files")
    if not isinstance(files, list):
        return None
    cleaned = [str(path) for path in files if str(path)]
    return cleaned or None


def _execute_update(task: dict[str, Any], repo_root: Path) -> str | None:
    """Run a structure-only incremental update. Returns a skip note or None."""
    from ..hook_guard import (
        hook_updates_disabled,
        start_budget_watchdog,
    )

    if hook_updates_disabled(repo_root):
        return "hook updates disabled (.dagayn/hook-skip)"
    # Hook-update semantics: non-blocking write lock (a manual build in
    # progress wins and will index the same dirty state) and a self budget.
    os.environ["DAGAYN_HOOK_UPDATE"] = "1"
    watchdog = start_budget_watchdog(DEFAULT_UPDATE_BUDGET_SECONDS, label="queue update")
    try:
        from .build import build_or_update_graph

        result = build_or_update_graph(
            full_rebuild=False,
            repo_root=str(repo_root),
            base=_stored_base(repo_root),
            postprocess="minimal",
            local_embedding="none",
        )
        if result.get("skipped"):
            return f"skipped: {result.get('skip_reason')}"
        note = _enqueue_scoped_embed_after_update(repo_root, result)
        return note
    finally:
        if watchdog is not None:
            watchdog.cancel()
        os.environ.pop("DAGAYN_HOOK_UPDATE", None)


def _execute_embed(task: dict[str, Any], repo_root: Path) -> str | None:
    """Run an explicit embedding pass with the payload's configuration."""
    from ..hook_guard import start_budget_watchdog

    payload = task["payload"]
    watchdog = start_budget_watchdog(DEFAULT_EMBED_BUDGET_SECONDS, label="queue embed")
    try:
        from .build import build_or_update_graph, run_embedding_pass

        embedding_kwargs: dict[str, Any] = dict(
            local_embedding=str(payload.get("local_embedding") or "bge-m3"),
            local_embedding_mode=payload.get("local_embedding_mode"),
            local_embedding_port=payload.get("local_embedding_port"),
            local_embedding_bin=str(payload.get("local_embedding_bin") or "auto"),
            keep_local_embedding_server=bool(payload.get("keep_local_embedding_server", True)),
            local_embedding_timeout=int(payload.get("local_embedding_timeout", 300)),
            local_embedding_request_timeout=int(payload.get("local_embedding_request_timeout", 60)),
            local_embedding_batch_size=int(payload.get("local_embedding_batch_size", 1)),
            embed_pass_seconds=EMBED_PASS_SECONDS,
            embed_files=_embed_files_from_payload(payload),
        )
        if payload.get("skip_structure"):
            result = run_embedding_pass(repo_root=str(repo_root), **embedding_kwargs)
        else:
            result = build_or_update_graph(
                full_rebuild=False,
                repo_root=str(repo_root),
                base=_stored_base(repo_root),
                postprocess="minimal",
                **embedding_kwargs,
            )
        if result.get("skipped"):
            return f"skipped: {result.get('skip_reason')}"
        if result.get("status") == "error":
            raise RuntimeError(result.get("summary") or "embedding pass failed")
        return _requeue_unfinished_embedding(repo_root, payload, result)
    finally:
        if watchdog is not None:
            watchdog.cancel()


def _enqueue_scoped_embed_after_update(repo_root: Path, result: dict[str, Any]) -> str | None:
    """Queue a file-scoped embed when a structure update touched local vectors.

    Comment-only edits keep coverage ``complete``, so session prepare will not
    start the sidecar. The hash-skip for those files has to ride the edit
    queue instead. Remote/cloud partitions are left alone.
    """
    files = list(
        dict.fromkeys(
            [
                *(result.get("changed_files") or []),
                *(result.get("dependent_files") or []),
            ]
        )
    )
    if not files:
        return None
    from ..paths import get_db_path
    from .sync_status import sidecar_embed_payload

    payload = sidecar_embed_payload(get_db_path(repo_root))
    if payload is None:
        return None
    payload["skip_structure"] = True
    action, task_id = enqueue_embed_refresh(
        repo_root,
        files=files,
        spawn_worker=False,
        payload=payload,
    )
    return f"{action} embed task {task_id} for {len(files)} file(s)"


def _requeue_unfinished_embedding(
    repo_root: Path,
    payload: dict[str, Any],
    result: dict[str, Any],
) -> str | None:
    """Queue a follow-up ``embed`` when the pass budget cut the run short.

    Only re-queues when this pass actually embedded something: a corpus that
    keeps reporting leftovers while making no progress (a node the provider
    rejects every time) would otherwise loop forever, and ``MAX_ATTEMPTS`` does
    not catch it because each pass *succeeds*.
    """
    embedding = result.get("local_embedding") or {}
    if not isinstance(embedding, dict):
        return None
    remaining = int(embedding.get("embedding_remaining", 0) or 0)
    embedded = int(embedding.get("newly_embedded", 0) or 0)
    if remaining <= 0 or embedded <= 0:
        return None
    queue = TaskQueue(queue_db_path(repo_root))
    try:
        action, task_id = queue.enqueue("embed", dict(payload))
    finally:
        queue.close()
    return f"embedded {embedded}, {remaining} left; {action} embed task {task_id}"


def _execute_prepare(task: dict[str, Any], repo_root: Path) -> str | None:
    """Run session prepare on the single worker lane.

    Does not set ``DAGAYN_HOOK_UPDATE``: this *is* the repair lane, so it
    waits on the write lock (with timeout) instead of skip-when-busy.
    """
    from ..hook_guard import start_budget_watchdog
    from .session_prepare import prepare_hard_stop_seconds, session_prepare

    payload = task["payload"]
    raw = payload.get("budget_seconds", DEFAULT_PREPARE_BUDGET_SECONDS)
    budget = None if raw is None or int(raw) <= 0 else int(raw)
    watchdog = start_budget_watchdog(
        prepare_hard_stop_seconds(budget),
        label="queue prepare",
    )
    try:
        result = session_prepare(
            repo_root=str(repo_root),
            local_embedding=str(payload.get("local_embedding") or "none"),
            keep_local_embedding_server=bool(payload.get("keep_local_embedding_server", True)),
            budget_seconds=budget,
            embedding_policy="auto",
            from_hook=False,
            seed_worktree=True,
        )
        if result.get("status") == "error":
            raise RuntimeError(result.get("summary") or "session prepare failed")
        if result.get("action") == "skipped" or result.get("skipped"):
            return f"skipped: {result.get('reason')}"
        return result.get("reason")
    finally:
        if watchdog is not None:
            watchdog.cancel()


def _execute_postprocess(task: dict[str, Any], repo_root: Path) -> str | None:
    """Run flows/communities/FTS post-processing."""
    from ..hook_guard import start_budget_watchdog

    watchdog = start_budget_watchdog(DEFAULT_POSTPROCESS_BUDGET_SECONDS, label="queue postprocess")
    try:
        from .build import run_postprocess

        result = run_postprocess(repo_root=str(repo_root))
        if result.get("skipped"):
            return f"skipped: {result.get('skip_reason')}"
        return None
    finally:
        if watchdog is not None:
            watchdog.cancel()


_TASK_EXECUTORS = {
    "update": _execute_update,
    "embed": _execute_embed,
    "postprocess": _execute_postprocess,
    "prepare": _execute_prepare,
}


def run_worker(
    repo_root: str | Path,
    *,
    idle_seconds: float = DEFAULT_IDLE_SECONDS,
    max_tasks: int | None = None,
    retry_backoff: bool = True,
) -> int:
    """Drain the queue until it has been empty for *idle_seconds*.

    Returns the number of tasks executed. A second concurrent worker exits
    immediately (0) because it cannot take the worker lock.
    """
    root = _resolve_repo_root(str(repo_root) if repo_root else None)
    lock = WorkerLock(worker_lock_path(root))
    if not lock.acquire():
        logger.info("queue worker: another worker is already running for %s", root)
        return 0
    queue = TaskQueue(queue_db_path(root))
    executed = 0
    idle_since: float | None = None
    try:
        # We hold the worker lock, so anything still ``running`` was left
        # behind by a worker that died (budget watchdog, crash, SIGKILL).
        recovered = queue.requeue_stale()
        if recovered:
            logger.info("queue worker: recovered %d task(s) from a dead worker", recovered)
        while True:
            task = queue.claim()
            if task is None:
                due_in = queue.next_due_in()
                if due_in is not None:
                    # Only retries in backoff are left: waiting them out is
                    # not idleness, so the idle window must not end the worker.
                    idle_since = None
                    time.sleep(min(0.5, max(0.01, due_in)))
                    continue
                if idle_since is None:
                    idle_since = time.monotonic()
                elif time.monotonic() - idle_since >= idle_seconds:
                    break
                time.sleep(min(0.5, max(0.05, idle_seconds - (time.monotonic() - idle_since))))
                continue
            idle_since = None
            executor = _TASK_EXECUTORS.get(task["kind"])
            if executor is None:
                note = f"unknown task kind {task['kind']!r}"
                queue.fail(task, note, fatal=True)
                executed += 1
                continue
            started = time.monotonic()
            try:
                note = executor(task, root)
            except Exception as exc:  # noqa: BLE001 - one bad task must not kill the worker
                logger.exception("queue task %s failed", task["kind"])
                note = f"failed: {type(exc).__name__}: {exc}"
                # The retry waits out its backoff (the usual transient cause is
                # something else holding a lock it needs); other queued kinds
                # are claimable meanwhile instead of sleeping behind it.
                delay = (
                    min(MAX_RETRY_BACKOFF_SECONDS, RETRY_BACKOFF_SECONDS * task["attempts"])
                    if retry_backoff
                    else 0.0
                )
                queue.fail(task, note, retry_delay=delay)
            else:
                queue.complete(task, note)
            executed += 1
            logger.info(
                "queue task %s (%s) finished in %.1fs%s",
                task["kind"],
                task["id"],
                time.monotonic() - started,
                f"; {note}" if note else "",
            )
            if max_tasks is not None and executed >= max_tasks:
                break
    finally:
        queue.close()
        lock.release()
    return executed
