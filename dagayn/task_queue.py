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
  empty for the idle window. On startup it recovers tasks left ``running`` by
  a worker that died mid-execution (:meth:`TaskQueue.requeue_stale`);
* task execution reuses the existing building blocks —
  ``build_or_update_graph`` with hook-update lock semantics, the budget
  watchdog, and the ``.dagayn/hook-skip`` opt-out.

The worker is deliberately one serial lane: every kind ends up writing the
graph, and the graph's write lock is the real serializer, so running kinds in
parallel would only turn waiting into skipping. Ordering is therefore the only
lever available (:data:`DEFAULT_PRIORITIES`), and it cannot preempt a task that
is already running.

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
from datetime import datetime
from pathlib import Path
from typing import Any

logger = logging.getLogger(__name__)

#: Task kinds the worker knows how to execute.
TASK_KINDS = ("update", "embed", "postprocess")

#: Default priority per kind, used when the caller does not pass one. The
#: worker is a single serial lane (the graph write lock is the real
#: serializer), so ordering is the only lever there is: an edit-triggered
#: ``update`` must not sit behind a minutes-long ``embed`` that was queued
#: first. This cannot preempt a task that is *already* running — an update
#: enqueued mid-embed still waits for it, bounded by the embed budget.
DEFAULT_PRIORITIES = {"update": 10, "embed": 0, "postprocess": 0}

#: A failing task is retried this many times before it is parked as ``dead``.
MAX_ATTEMPTS = 3

#: Base delay before a failed task is retried, multiplied by the attempts
#: already spent and capped. Retrying instantly three times just reproduces
#: whatever transient state (a held lock, a busy disk) caused the failure.
RETRY_BACKOFF_SECONDS = 1.0
MAX_RETRY_BACKOFF_SECONDS = 10.0

#: Keep the task log bounded: it is a diagnostic tail for ``queue status``,
#: not an audit trail, and every task writes two or three rows to it.
LOG_RETENTION = 200

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
    """Local time with its UTC offset.

    Offset-naive local time is ambiguous across a DST fold and cannot be
    compared between machines; keeping the offset costs six characters and
    still reads as wall-clock time in ``queue status``. Row ordering never
    depends on this — it uses ``id``.
    """
    return datetime.now().astimezone().isoformat(timespec="seconds")


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
        priority: int | None = None,
    ) -> tuple[str, int]:
        """Add *kind* to the queue, coalescing with a pending twin.

        Returns ``(action, task_id)`` where *action* is ``"added"`` or
        ``"coalesced"``. Coalescing merges payloads (newer keys win) and keeps
        the higher priority, so a burst of identical hook fires collapses to
        one task. ``priority`` defaults to the kind's entry in
        :data:`DEFAULT_PRIORITIES`.

        The lookup and the write share one ``BEGIN IMMEDIATE`` transaction so
        they serialize against :meth:`claim`. Without it, the twin could be
        claimed between the two statements and the new work would be folded
        into a task the worker had already read — and then dropped when that
        task completed, leaving the last edits of a burst unindexed.
        """
        if kind not in TASK_KINDS:
            raise ValueError(f"unknown task kind: {kind!r} (expected one of {TASK_KINDS})")
        if priority is None:
            priority = DEFAULT_PRIORITIES.get(kind, 0)
        now = _now()
        self._conn.execute("BEGIN IMMEDIATE")
        try:
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
        except BaseException:
            self._conn.rollback()
            raise
        # sqlite3 types ``lastrowid`` as optional; after a successful INSERT on
        # a rowid table it is always set.
        task_id = cur.lastrowid
        if task_id is None:  # pragma: no cover - defensive
            raise RuntimeError("INSERT into tasks did not produce a rowid")
        return "added", task_id

    def claim(self) -> dict[str, Any] | None:
        """Atomically take the next pending task, or ``None`` when idle.

        The idle poll runs twice a second for the whole idle window, so it
        first asks a plain read whether there is anything to take. Opening
        ``BEGIN IMMEDIATE`` unconditionally would grab the queue's write lock
        ~120 times per idle minute and make hooks wait behind an empty poll.
        The read is only a fast path: the transaction below re-selects, so a
        task appearing in between is picked up on the next poll rather than
        claimed twice.
        """
        if (
            self._conn.execute("SELECT 1 FROM tasks WHERE state = 'pending' LIMIT 1").fetchone()
            is None
        ):
            return None
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

    def requeue_stale(self) -> int:
        """Recover tasks abandoned by a worker that died mid-execution.

        The budget watchdog stops an overrunning worker with ``os._exit``, and
        a crash or a ``SIGKILL`` has the same effect: the row stays ``running``
        and :meth:`claim` — which only looks at ``pending`` — would never touch
        it again. Callers must hold the worker lock, which is what makes
        ``running`` mean *orphaned*: no other worker can be executing anything.

        A task whose ``attempts`` are already spent is parked ``dead`` instead
        of requeued, so a task that reliably blows the budget cannot loop
        forever. Returns how many rows were recovered (requeued or parked).
        """
        rows = self._conn.execute(
            "SELECT id, kind, attempts FROM tasks WHERE state = 'running'"
        ).fetchall()
        for row in rows:
            note = "worker exited mid-task"
            if row["attempts"] >= MAX_ATTEMPTS:
                self._log(row["id"], row["kind"], "dead", note)
                self._conn.execute(
                    "UPDATE tasks SET state = 'dead', last_error = ?, updated_at = ? WHERE id = ?",
                    (note, _now(), row["id"]),
                )
            else:
                self._log(row["id"], row["kind"], "requeued", note)
                self._conn.execute(
                    "UPDATE tasks SET state = 'pending', last_error = ?, updated_at = ?"
                    " WHERE id = ?",
                    (note, _now(), row["id"]),
                )
        self._conn.commit()
        return len(rows)

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
        cur = self._conn.execute(
            "INSERT INTO task_log (task_id, kind, state, note, at) VALUES (?, ?, ?, ?, ?)",
            (task_id, kind, state, note, _now()),
        )
        # Trim as we go so a long session cannot grow the log without bound.
        # Cheap: a rowid range delete that usually matches nothing.
        if cur.lastrowid is not None:
            self._conn.execute(
                "DELETE FROM task_log WHERE id <= ?", (cur.lastrowid - LOG_RETENTION,)
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


def _worker_env() -> dict[str, str]:
    """Environment for the worker: our own import location on ``PYTHONPATH``.

    Paired with ``-P`` (see :func:`ensure_worker`), this makes the worker import
    the same dagayn as the process spawning it, whether that is site-packages
    or an uninstalled checkout.
    """
    env = dict(os.environ)
    package_parent = str(Path(__file__).resolve().parent.parent)
    existing = env.get("PYTHONPATH")
    env["PYTHONPATH"] = f"{package_parent}{os.pathsep}{existing}" if existing else package_parent
    return env


def ensure_worker(repo_root: str | Path, *, idle_seconds: float = DEFAULT_IDLE_SECONDS) -> bool:
    """Spawn a detached queue worker when one is not already running.

    Returns True when a worker was spawned. The probe and the spawn are not
    atomic: two concurrent ``queue add`` calls can both spawn, and the losing
    worker exits immediately when it cannot take the lock.

    The worker must run *this* dagayn, not whatever happens to sit in the
    hook's working directory. ``python -m`` prepends the cwd to ``sys.path``,
    so a hook firing inside a checkout that has a ``dagayn/`` package (dagayn's
    own repository, most obviously) made the worker import that source tree and
    any stale compiled ``_core`` next to it, which then failed post-processing
    with a confusing "requires dagayn._core support" error. ``-P`` drops the
    cwd entry and ``PYTHONPATH`` carries the caller's own import location over
    instead, so an uninstalled source checkout keeps working too.
    """
    lock = WorkerLock(worker_lock_path(repo_root))
    if not lock.acquire():
        # A live worker holds the lock; let it drain the new task.
        return False
    lock.release()
    cmd = [
        sys.executable,
        "-P",
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
        env=_worker_env(),
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
                if queue.fail(task, note) and retry_backoff:
                    # Back off before the retry is claimable again. This parks
                    # the whole lane, which is the point: the usual transient
                    # cause is something else holding a lock we need.
                    time.sleep(
                        min(
                            MAX_RETRY_BACKOFF_SECONDS,
                            RETRY_BACKOFF_SECONDS * task["attempts"],
                        )
                    )
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
