"""Repository-scoped task queue for background graph processing.

Every edit-triggered hook used to spawn its own ``dagayn update`` process.
That is fine when edits are rare, but on an active session it means a fresh
Python interpreter, a git diff, and a store open *per keystroke batch*, with
only the write lock standing between the pile of processes and the database.
Overlapping runs do not queue — they skip — so the last edit of a burst can
stay unindexed until the next trigger.

This module replaces that with a small SQLite task queue per repository:

* hooks call :func:`TaskQueue.enqueue` (a single INSERT, or a coalesce into
  the already-pending task of the same kind) and then
  :func:`ensure_worker`, which spawns one detached worker when none is live;
* the worker (:func:`run_worker`) drains the queue, re-running a kind while
  new work of that kind keeps arriving, and exits after the queue has been
  empty for the idle window;
* task execution reuses the existing building blocks —
  ``build_or_update_graph`` with hook-update lock semantics, the budget
  watchdog, and the ``.dagayn/hook-skip`` opt-out.

The queue lives in the repository's data directory (``.dagayn`` or
``CRG_DATA_DIR``), next to ``graph.db``. It is deliberately a separate file:
the graph's own write lock must stay the single serializer of graph writes,
and the queue must remain writable while a build holds that lock.
"""

from __future__ import annotations

import json
import logging
import os
import sqlite3
import subprocess
import sys
import time
from pathlib import Path
from typing import Any

logger = logging.getLogger(__name__)

#: Task kinds the worker knows how to execute.
TASK_KINDS = ("update", "embed", "postprocess")

#: A failing task is retried this many times before it is parked as ``dead``.
MAX_ATTEMPTS = 3

#: Seconds the worker keeps living after the queue has gone empty. Hook
#: bursts are short; a long-lived idle worker would just hold the lock.
DEFAULT_IDLE_SECONDS = 60.0

#: Wall-clock budgets per task kind. ``update`` matches the hook-update budget
#: (``dagayn.hook_guard.DEFAULT_HOOK_BUDGET_SECONDS``); embedding and full
#: post-processing get more headroom because they are explicit, rare tasks.
DEFAULT_UPDATE_BUDGET_SECONDS = 120
DEFAULT_EMBED_BUDGET_SECONDS = 600
DEFAULT_POSTPROCESS_BUDGET_SECONDS = 600

QUEUE_DB_NAME = "task_queue.db"
WORKER_LOCK_NAME = "queue_worker.lock"

_SCHEMA = """
CREATE TABLE IF NOT EXISTS tasks (
    id INTEGER PRIMARY KEY,
    kind TEXT NOT NULL,
    priority INTEGER NOT NULL DEFAULT 0,
    payload TEXT NOT NULL DEFAULT '{}',
    state TEXT NOT NULL DEFAULT 'pending',
    attempts INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    last_error TEXT
);
CREATE TABLE IF NOT EXISTS task_log (
    id INTEGER PRIMARY KEY,
    task_id INTEGER NOT NULL,
    kind TEXT NOT NULL,
    state TEXT NOT NULL,
    note TEXT,
    at TEXT NOT NULL
);
"""


def _now() -> str:
    return time.strftime("%Y-%m-%dT%H:%M:%S")


class TaskQueue:
    """A coalescing FIFO of graph-processing tasks for one repository."""

    def __init__(self, db_path: str | Path) -> None:
        self.db_path = Path(db_path)
        self.db_path.parent.mkdir(parents=True, exist_ok=True)
        self._conn = sqlite3.connect(str(self.db_path), timeout=10)
        self._conn.row_factory = sqlite3.Row
        self._conn.execute("PRAGMA journal_mode=WAL")
        self._conn.execute("PRAGMA busy_timeout=5000")
        self._conn.executescript(_SCHEMA)
        self._conn.commit()

    # ------------------------------------------------------------------
    # Enqueue / claim / finish
    # ------------------------------------------------------------------

    def enqueue(
        self,
        kind: str,
        payload: dict[str, Any] | None = None,
        priority: int = 0,
    ) -> tuple[str, int]:
        """Add *kind* to the queue, coalescing with a pending twin.

        Returns ``(action, task_id)`` where *action* is ``"added"`` or
        ``"coalesced"``. Coalescing merges payloads (newer keys win) and keeps
        the higher priority, so a burst of identical hook fires collapses to
        one task.
        """
        if kind not in TASK_KINDS:
            raise ValueError(f"unknown task kind: {kind!r} (expected one of {TASK_KINDS})")
        now = _now()
        row = self._conn.execute(
            "SELECT id, payload, priority FROM tasks WHERE kind = ? AND state = 'pending'",
            (kind,),
        ).fetchone()
        if row is not None:
            merged = {**json.loads(row["payload"]), **(payload or {})}
            self._conn.execute(
                "UPDATE tasks SET payload = ?, priority = MAX(priority, ?), updated_at = ?"
                " WHERE id = ?",
                (json.dumps(merged), priority, now, row["id"]),
            )
            self._conn.commit()
            return "coalesced", row["id"]
        cur = self._conn.execute(
            "INSERT INTO tasks (kind, priority, payload, state, created_at, updated_at)"
            " VALUES (?, ?, ?, 'pending', ?, ?)",
            (kind, priority, json.dumps(payload or {}), now, now),
        )
        self._conn.commit()
        return "added", int(cur.lastrowid)

    def claim(self) -> dict[str, Any] | None:
        """Atomically take the next pending task, or ``None`` when idle."""
        self._conn.execute("BEGIN IMMEDIATE")
        try:
            row = self._conn.execute(
                "SELECT * FROM tasks WHERE state = 'pending' ORDER BY priority DESC, id ASC LIMIT 1"
            ).fetchone()
            if row is None:
                self._conn.rollback()
                return None
            self._conn.execute(
                "UPDATE tasks SET state = 'running', attempts = attempts + 1,"
                " updated_at = ? WHERE id = ?",
                (_now(), row["id"]),
            )
            self._log(row["id"], row["kind"], "running", None)
            # Re-read after the update: callers (fail/retry logic) rely on
            # ``attempts`` already including this claim.
            row = self._conn.execute("SELECT * FROM tasks WHERE id = ?", (row["id"],)).fetchone()
            self._conn.commit()
        except BaseException:
            self._conn.rollback()
            raise
        return self._task_dict(row)

    def complete(self, task: dict[str, Any], note: str | None = None) -> None:
        """Drop a running task, logging the outcome."""
        self._log(task["id"], task["kind"], "done", note)
        self._conn.execute("DELETE FROM tasks WHERE id = ? AND state = 'running'", (task["id"],))
        self._conn.commit()

    def fail(self, task: dict[str, Any], error: str, *, fatal: bool = False) -> bool:
        """Record a failure. Returns True when the task went back to pending.

        ``fatal=True`` parks the task as dead immediately (no retry) — for
        failures that cannot possibly succeed on re-run.
        """
        if fatal or task["attempts"] >= MAX_ATTEMPTS:
            self._log(task["id"], task["kind"], "dead", error)
            self._conn.execute(
                "UPDATE tasks SET state = 'dead', last_error = ?, updated_at = ? WHERE id = ?",
                (error, _now(), task["id"]),
            )
            self._conn.commit()
            return False
        self._log(task["id"], task["kind"], "retry", error)
        self._conn.execute(
            "UPDATE tasks SET state = 'pending', last_error = ?, updated_at = ? WHERE id = ?",
            (error, _now(), task["id"]),
        )
        self._conn.commit()
        return True

    # ------------------------------------------------------------------
    # Inspection / maintenance
    # ------------------------------------------------------------------

    def pending_kinds(self) -> set[str]:
        rows = self._conn.execute(
            "SELECT DISTINCT kind FROM tasks WHERE state IN ('pending', 'running')"
        ).fetchall()
        return {row["kind"] for row in rows}

    def stats(self) -> dict[str, Any]:
        counts: dict[str, int] = {"pending": 0, "running": 0, "dead": 0}
        for row in self._conn.execute("SELECT state, COUNT(*) AS n FROM tasks GROUP BY state"):
            counts[row["state"]] = row["n"]
        recent = [
            self._log_dict(row)
            for row in self._conn.execute("SELECT * FROM task_log ORDER BY id DESC LIMIT 10")
        ]
        return {"counts": counts, "recent": list(reversed(recent))}

    def clear(self) -> int:
        """Drop every queued task. Returns how many were removed."""
        cur = self._conn.execute("DELETE FROM tasks")
        self._conn.commit()
        return cur.rowcount

    def close(self) -> None:
        try:
            self._conn.close()
        except sqlite3.Error:  # pragma: no cover - best effort
            pass

    # ------------------------------------------------------------------
    # Internals
    # ------------------------------------------------------------------

    def _task_dict(self, row: sqlite3.Row) -> dict[str, Any]:
        return {
            "id": row["id"],
            "kind": row["kind"],
            "priority": row["priority"],
            "payload": json.loads(row["payload"]),
            "state": row["state"],
            "attempts": row["attempts"],
            "created_at": row["created_at"],
            "updated_at": row["updated_at"],
            "last_error": row["last_error"],
        }

    def _log(self, task_id: int, kind: str, state: str, note: str | None) -> None:
        self._conn.execute(
            "INSERT INTO task_log (task_id, kind, state, note, at) VALUES (?, ?, ?, ?, ?)",
            (task_id, kind, state, note, _now()),
        )

    def _log_dict(self, row: sqlite3.Row) -> dict[str, Any]:
        return {
            "task_id": row["task_id"],
            "kind": row["kind"],
            "state": row["state"],
            "note": row["note"],
            "at": row["at"],
        }


class WorkerLock:
    """An exclusive flock held by the queue worker for its whole lifetime.

    ``queue add`` probes it non-blocking to decide whether to spawn a worker;
    the worker holds it until it exits. Mirrors the ``fcntl``-optional pattern
    from :mod:`dagayn.write_lock` (no serialization on platforms without
    flock, where hook processes were never serialized by it either).
    """

    def __init__(self, lock_path: str | Path) -> None:
        self.lock_path = Path(lock_path)
        self._handle: Any = None

    def acquire(self) -> bool:
        if self._handle is not None:
            return True
        try:
            import fcntl
        except ImportError:  # pragma: no cover - POSIX only in practice
            return True
        self.lock_path.parent.mkdir(parents=True, exist_ok=True)
        handle = self.lock_path.open("a+", encoding="utf-8")
        try:
            fcntl.flock(handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        except (BlockingIOError, OSError):
            handle.close()
            return False
        try:
            handle.seek(0)
            handle.truncate()
            handle.write(f"{os.getpid()}\n")
            handle.flush()
        except OSError:  # pragma: no cover - diagnostics only
            pass
        self._handle = handle
        return True

    def release(self) -> None:
        if self._handle is None:
            return
        try:
            import fcntl

            fcntl.flock(self._handle.fileno(), fcntl.LOCK_UN)
        except (ImportError, OSError):  # pragma: no cover - best effort
            pass
        try:
            self._handle.close()
        except OSError:  # pragma: no cover
            pass
        self._handle = None

    def __enter__(self) -> "WorkerLock":
        if not self.acquire():
            raise RuntimeError(f"queue worker already holds {self.lock_path}")
        return self

    def __exit__(self, *exc_info: object) -> None:
        self.release()


def queue_db_path(repo_root: str | Path) -> Path:
    """Where this repository's task queue lives (created on demand)."""
    from .paths import get_data_dir

    return get_data_dir(Path(repo_root)) / QUEUE_DB_NAME


def worker_lock_path(repo_root: str | Path) -> Path:
    from .paths import get_data_dir

    return get_data_dir(Path(repo_root)) / WORKER_LOCK_NAME


def ensure_worker(repo_root: str | Path, *, idle_seconds: float = DEFAULT_IDLE_SECONDS) -> bool:
    """Spawn a detached queue worker when one is not already running.

    Returns True when a worker was spawned. The probe and the spawn are not
    atomic: two concurrent ``queue add`` calls can both spawn, and the losing
    worker exits immediately when it cannot take the lock.
    """
    lock = WorkerLock(worker_lock_path(repo_root))
    if not lock.acquire():
        # A live worker holds the lock; let it drain the new task.
        return False
    lock.release()
    cmd = [
        sys.executable,
        "-m",
        "dagayn",
        "queue",
        "run",
        "--repo",
        str(repo_root),
        "--idle-seconds",
        str(idle_seconds),
    ]
    kwargs: dict[str, Any] = dict(
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    if os.name == "posix":
        # Detach from the hook's process group: the editor reaps its own
        # children, and a reparented worker is what we want here.
        kwargs["start_new_session"] = True
    try:
        subprocess.Popen(cmd, **kwargs)  # noqa: S603 - fixed argv, no shell
    except OSError as exc:
        logger.warning("Could not spawn queue worker: %s", exc)
        return False
    return True


def _resolve_repo_root(repo_root: str | None) -> Path:
    if repo_root:
        return Path(repo_root).expanduser().resolve()
    from .incremental_files import find_project_root

    found = find_project_root()
    if found is not None:
        return found
    return Path.cwd().resolve()


def _stored_base(repo_root: Path) -> str:
    """Git ref the graph describes — same order as ``dagayn update``."""
    db_path = queue_db_path(repo_root).parent / "graph.db"
    if not db_path.exists():
        return "HEAD~1"
    from .graph import GraphStore
    from .write_lock import graph_read_lock

    try:
        with graph_read_lock(db_path):
            store = GraphStore(db_path)
            try:
                return store.get_metadata("git_head_sha") or "HEAD~1"
            finally:
                store.close()
    except Exception:  # noqa: BLE001 - a bad peek must not kill the worker
        logger.warning("Could not read stored git_head_sha; falling back to HEAD~1")
        return "HEAD~1"


def _execute_update(task: dict[str, Any], repo_root: Path) -> str | None:
    """Run a structure-only incremental update. Returns a skip note or None."""
    from .hook_guard import (
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
        from .tools.build import build_or_update_graph

        result = build_or_update_graph(
            full_rebuild=False,
            repo_root=str(repo_root),
            base=_stored_base(repo_root),
            postprocess="minimal",
            local_embedding="none",
        )
        if result.get("skipped"):
            return f"skipped: {result.get('skip_reason')}"
        return None
    finally:
        if watchdog is not None:
            watchdog.cancel()
        os.environ.pop("DAGAYN_HOOK_UPDATE", None)


def _execute_embed(task: dict[str, Any], repo_root: Path) -> str | None:
    """Run an explicit embedding pass with the payload's configuration."""
    from .hook_guard import start_budget_watchdog

    payload = task["payload"]
    watchdog = start_budget_watchdog(DEFAULT_EMBED_BUDGET_SECONDS, label="queue embed")
    try:
        from .tools.build import build_or_update_graph

        result = build_or_update_graph(
            full_rebuild=False,
            repo_root=str(repo_root),
            base=_stored_base(repo_root),
            postprocess="minimal",
            local_embedding=str(payload.get("local_embedding") or "bge-m3"),
            local_embedding_mode=payload.get("local_embedding_mode"),
            local_embedding_port=payload.get("local_embedding_port"),
            local_embedding_bin=str(payload.get("local_embedding_bin") or "auto"),
            keep_local_embedding_server=bool(payload.get("keep_local_embedding_server", True)),
            local_embedding_timeout=int(payload.get("local_embedding_timeout", 300)),
            local_embedding_request_timeout=int(payload.get("local_embedding_request_timeout", 60)),
            local_embedding_batch_size=int(payload.get("local_embedding_batch_size", 1)),
        )
        if result.get("skipped"):
            return f"skipped: {result.get('skip_reason')}"
        if result.get("status") == "error":
            raise RuntimeError(result.get("summary") or "embedding pass failed")
        return None
    finally:
        if watchdog is not None:
            watchdog.cancel()


def _execute_postprocess(task: dict[str, Any], repo_root: Path) -> str | None:
    """Run flows/communities/FTS post-processing."""
    from .hook_guard import start_budget_watchdog

    watchdog = start_budget_watchdog(DEFAULT_POSTPROCESS_BUDGET_SECONDS, label="queue postprocess")
    try:
        from .tools.build import run_postprocess

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
}


def run_worker(
    repo_root: str | Path,
    *,
    idle_seconds: float = DEFAULT_IDLE_SECONDS,
    max_tasks: int | None = None,
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
        while True:
            task = queue.claim()
            if task is None:
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
                queue.fail(task, note)
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
