"""Tests for the repository-scoped task queue (dagayn.task_queue).

The queue replaces per-edit ``dagayn update`` hook spawns: hooks enqueue a
task (coalescing with a pending twin) and a single detached worker drains
the queue. These tests cover the queue mechanics, the worker lock, and the
worker loop with fake executors — no real graph builds.
"""

from __future__ import annotations

import os
import subprocess
import threading
import time
from collections.abc import Iterator
from datetime import datetime
from pathlib import Path
from typing import Any

import pytest

import dagayn.task_queue
from dagayn.task_queue import (
    DEFAULT_EMBED_BUDGET_SECONDS,
    DEFAULT_PRIORITIES,
    EMBED_PASS_SECONDS,
    LOG_RETENTION,
    MAX_ATTEMPTS,
    TASK_KINDS,
    TaskQueue,
    WorkerLock,
    _requeue_unfinished_embedding,
    ensure_worker,
    queue_db_path,
    run_worker,
    worker_lock_path,
)


@pytest.fixture
def queue(tmp_path: Path) -> Iterator[TaskQueue]:
    q = TaskQueue(tmp_path / "task_queue.db")
    yield q
    q.close()


class TestEnqueue:
    def test_first_enqueue_adds(self, queue: TaskQueue) -> None:
        action, task_id = queue.enqueue("update")
        assert action == "added"
        assert task_id >= 1

    def test_pending_twin_coalesces(self, queue: TaskQueue) -> None:
        _, first_id = queue.enqueue("update")
        action, second_id = queue.enqueue("update")
        assert action == "coalesced"
        assert second_id == first_id
        assert queue.stats()["counts"]["pending"] == 1

    def test_coalesce_merges_payload_newer_wins(self, queue: TaskQueue) -> None:
        queue.enqueue("embed", payload={"a": 1, "b": 1})
        queue.enqueue("embed", payload={"b": 2})
        task = queue.claim()
        assert task is not None
        assert task["payload"] == {"a": 1, "b": 2}

    def test_coalesce_unions_scoped_embed_files(self, queue: TaskQueue) -> None:
        queue.enqueue("embed", payload={"files": ["a.py"], "local_embedding": "bge-m3"})
        queue.enqueue("embed", payload={"files": ["b.py"], "local_embedding": "bge-m3"})
        task = queue.claim()
        assert task is not None
        assert task["payload"]["files"] == ["a.py", "b.py"]

    def test_coalesce_scoped_into_full_embed_stays_full(self, queue: TaskQueue) -> None:
        queue.enqueue("embed", payload={"local_embedding": "bge-m3"})
        queue.enqueue("embed", payload={"files": ["a.py"], "local_embedding": "bge-m3"})
        task = queue.claim()
        assert task is not None
        assert "files" not in task["payload"]

    def test_coalesce_keeps_higher_priority(self, queue: TaskQueue) -> None:
        queue.enqueue("update", priority=5)
        queue.enqueue("update", priority=1)
        task = queue.claim()
        assert task is not None
        assert task["priority"] == 5

    def test_different_kinds_do_not_coalesce(self, queue: TaskQueue) -> None:
        queue.enqueue("update")
        queue.enqueue("postprocess")
        assert queue.stats()["counts"]["pending"] == 2

    def test_unknown_kind_rejected(self, queue: TaskQueue) -> None:
        with pytest.raises(ValueError):
            queue.enqueue("nope")  # type: ignore[arg-type]

    def test_all_task_kinds_accepted(self, queue: TaskQueue) -> None:
        for kind in TASK_KINDS:
            action, _ = queue.enqueue(kind)
            assert action == "added"

    def test_enqueue_session_prepare_coalesces(self, tmp_path: Path) -> None:
        from dagayn.task_queue import enqueue_session_prepare

        first, id1 = enqueue_session_prepare(tmp_path, spawn_worker=False)
        second, id2 = enqueue_session_prepare(tmp_path, spawn_worker=False)
        assert first == "added"
        assert second == "coalesced"
        assert id1 == id2


class TestPriorities:
    def test_update_outranks_an_already_queued_embed(self, queue: TaskQueue) -> None:
        """An edit must not wait behind a minutes-long embed queued before it."""
        queue.enqueue("embed")
        queue.enqueue("update")
        first = queue.claim()
        assert first is not None and first["kind"] == "update"

    def test_explicit_priority_still_wins(self, queue: TaskQueue) -> None:
        queue.enqueue("embed", priority=DEFAULT_PRIORITIES["update"] + 1)
        queue.enqueue("update")
        first = queue.claim()
        assert first is not None and first["kind"] == "embed"


class TestClaim:
    def test_empty_queue_returns_none(self, queue: TaskQueue) -> None:
        assert queue.claim() is None

    def test_idle_poll_does_not_take_the_write_lock(self, tmp_path: Path) -> None:
        """An empty poll must not queue behind a writer — hooks write here too.

        The worker polls twice a second for the whole idle window; if that poll
        opened a write transaction it would contend with every ``queue add``.
        """
        db = tmp_path / "task_queue.db"
        worker = TaskQueue(db)
        writer = TaskQueue(db)
        writer._conn.execute("BEGIN IMMEDIATE")
        writer._conn.execute(
            "INSERT INTO tasks (kind, state, created_at, updated_at)"
            " VALUES ('update', 'x', 'now', 'now')"
        )
        try:
            started = time.monotonic()
            assert worker.claim() is None
            assert time.monotonic() - started < 1.0, "the idle poll waited for the write lock"
        finally:
            writer._conn.rollback()
            worker.close()
            writer.close()

    def test_claim_marks_running_and_counts_attempt(self, queue: TaskQueue) -> None:
        queue.enqueue("update")
        task = queue.claim()
        assert task is not None
        assert task["state"] == "running"
        assert task["attempts"] == 1
        assert queue.claim() is None

    def test_priority_order_then_id(self, queue: TaskQueue) -> None:
        queue.enqueue("update", priority=0)
        queue.enqueue("postprocess", priority=10)
        queue.enqueue("embed", priority=0)
        first = queue.claim()
        second = queue.claim()
        third = queue.claim()
        assert first is not None and first["kind"] == "postprocess"
        assert second is not None and second["kind"] == "update"
        assert third is not None and third["kind"] == "embed"

    def test_claim_is_atomic_across_connections(self, tmp_path: Path) -> None:
        """Two TaskQueue handles on the same file must not double-claim."""
        db = tmp_path / "task_queue.db"
        a = TaskQueue(db)
        b = TaskQueue(db)
        try:
            a.enqueue("update")
            claimed = [q.claim() for q in (a, b)]
            assert sum(t is not None for t in claimed) == 1
        finally:
            a.close()
            b.close()


class TestCompleteAndFail:
    def test_complete_drops_task_and_logs(self, queue: TaskQueue) -> None:
        queue.enqueue("update")
        task = queue.claim()
        assert task is not None
        queue.complete(task, "note")
        assert queue.claim() is None
        stats = queue.stats()
        assert stats["counts"]["pending"] == 0
        assert stats["recent"][-1]["state"] == "done"
        assert stats["recent"][-1]["note"] == "note"

    def test_fail_retries_until_dead(self, queue: TaskQueue) -> None:
        queue.enqueue("update")
        for attempt in range(1, MAX_ATTEMPTS):
            task = queue.claim()
            assert task is not None
            assert queue.fail(task, "boom") is True
        task = queue.claim()
        assert task is not None
        assert queue.fail(task, "boom") is False
        assert queue.claim() is None
        assert queue.stats()["counts"]["dead"] == 1

    def test_failed_task_is_reclaimable(self, queue: TaskQueue) -> None:
        queue.enqueue("update")
        task = queue.claim()
        assert task is not None
        queue.fail(task, "boom")
        again = queue.claim()
        assert again is not None
        assert again["attempts"] == 2

    def test_fatal_failure_skips_retries(self, queue: TaskQueue) -> None:
        queue.enqueue("update")
        task = queue.claim()
        assert task is not None
        assert queue.fail(task, "impossible", fatal=True) is False
        assert queue.claim() is None
        assert queue.stats()["counts"]["dead"] == 1


class TestInspection:
    def test_stats_counts_and_recent(self, queue: TaskQueue) -> None:
        queue.enqueue("update")
        queue.enqueue("embed")
        queue.claim()
        stats = queue.stats()
        assert stats["counts"]["pending"] == 1
        assert stats["counts"]["running"] == 1
        assert len(stats["recent"]) >= 1

    def test_timestamps_carry_a_utc_offset(self, queue: TaskQueue) -> None:
        queue.enqueue("update")
        queue.claim()
        at = queue.stats()["recent"][-1]["at"]
        assert datetime.fromisoformat(at).tzinfo is not None

    def test_task_log_stays_bounded(self, queue: TaskQueue) -> None:
        """The log is a diagnostic tail, not an audit trail of a long session."""
        for _ in range(LOG_RETENTION):
            queue.enqueue("update")
            task = queue.claim()
            assert task is not None
            queue.complete(task)

        rows = queue._conn.execute("SELECT COUNT(*) AS n FROM task_log").fetchone()["n"]
        assert rows <= LOG_RETENTION
        # The tail that ``queue status`` shows must survive the trimming.
        assert len(queue.stats()["recent"]) == 10

    def test_clear_removes_queued_tasks(self, queue: TaskQueue) -> None:
        queue.enqueue("update")
        queue.enqueue("embed")
        assert queue.clear() == 2
        assert queue.claim() is None

    def test_persistence_across_handles(self, tmp_path: Path) -> None:
        db = tmp_path / "task_queue.db"
        q1 = TaskQueue(db)
        q1.enqueue("update")
        q1.close()
        q2 = TaskQueue(db)
        try:
            task = q2.claim()
            assert task is not None and task["kind"] == "update"
        finally:
            q2.close()


class TestWorkerLock:
    def test_acquire_and_release(self, tmp_path: Path) -> None:
        lock = WorkerLock(tmp_path / "worker.lock")
        assert lock.acquire() is True
        lock.release()
        assert lock.acquire() is True
        lock.release()

    def test_second_holder_cannot_acquire(self, tmp_path: Path) -> None:
        path = tmp_path / "worker.lock"
        first = WorkerLock(path)
        second = WorkerLock(path)
        assert first.acquire() is True
        assert second.acquire() is False
        first.release()
        assert second.acquire() is True
        second.release()

    def test_context_manager_raises_when_held(self, tmp_path: Path) -> None:
        path = tmp_path / "worker.lock"
        holder = WorkerLock(path)
        holder.acquire()
        try:
            with pytest.raises(RuntimeError):
                with WorkerLock(path):
                    pass
        finally:
            holder.release()

    def test_lock_file_contains_pid(self, tmp_path: Path) -> None:
        lock = WorkerLock(tmp_path / "worker.lock")
        lock.acquire()
        try:
            content = (tmp_path / "worker.lock").read_text()
            assert str(os.getpid()) in content
        finally:
            lock.release()


class TestRunWorker:
    def _fake_executors(self, calls: list[tuple[str, dict[str, Any]]]) -> dict:
        def make(kind: str):
            def executor(task: dict[str, Any], repo_root: Path) -> str | None:
                calls.append((kind, task))
                return None

            return executor

        return {kind: make(kind) for kind in TASK_KINDS}

    def test_executes_pending_tasks(self, tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
        calls: list[tuple[str, dict[str, Any]]] = []
        monkeypatch.setattr("dagayn.task_queue._TASK_EXECUTORS", self._fake_executors(calls))
        queue = TaskQueue(queue_db_path(tmp_path))
        queue.enqueue("update")
        queue.enqueue("embed")
        queue.close()

        executed = run_worker(tmp_path, idle_seconds=0.2, max_tasks=2)
        assert executed == 2
        assert [kind for kind, _ in calls] == ["update", "embed"]

    def test_unknown_kind_is_parked_dead(
        self, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        monkeypatch.setattr("dagayn.task_queue._TASK_EXECUTORS", {})
        queue = TaskQueue(queue_db_path(tmp_path))
        queue._conn.execute(
            "INSERT INTO tasks (kind, state, created_at, updated_at)"
            " VALUES ('mystery', 'pending', 'now', 'now')"
        )
        queue._conn.commit()
        queue.close()

        executed = run_worker(tmp_path, idle_seconds=0.2)
        assert executed == 1
        stats = TaskQueue(queue_db_path(tmp_path)).stats()
        assert stats["counts"]["dead"] == 1

    def test_failing_task_retries_then_dies(
        self, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        def boom(task: dict[str, Any], repo_root: Path) -> str | None:
            raise RuntimeError("always fails")

        monkeypatch.setattr("dagayn.task_queue._TASK_EXECUTORS", {"update": boom})
        queue = TaskQueue(queue_db_path(tmp_path))
        queue.enqueue("update")
        queue.close()

        executed = run_worker(tmp_path, idle_seconds=0.2, retry_backoff=False)
        assert executed == MAX_ATTEMPTS
        stats = TaskQueue(queue_db_path(tmp_path)).stats()
        assert stats["counts"]["dead"] == 1
        assert stats["counts"]["pending"] == 0

    def _always_fails(self, task: dict[str, Any], repo_root: Path) -> str | None:
        raise RuntimeError("always fails")

    def test_retry_backs_off_between_attempts(
        self, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        """Retries wait, and the wait grows with the attempts already spent.

        The delay is asserted as a lower bound on elapsed time rather than by
        patching ``time.sleep``: that attribute is shared with every other
        thread in the process, which made this test depend on what else the
        suite happened to be running.
        """
        monkeypatch.setattr("dagayn.task_queue._TASK_EXECUTORS", {"update": self._always_fails})
        monkeypatch.setattr("dagayn.task_queue.RETRY_BACKOFF_SECONDS", 0.05)
        queue = TaskQueue(queue_db_path(tmp_path))
        queue.enqueue("update")
        queue.close()

        started = time.monotonic()
        executed = run_worker(tmp_path, idle_seconds=0.0)
        elapsed = time.monotonic() - started

        assert executed == MAX_ATTEMPTS
        # One wait per requeue, growing with attempts; the last attempt is
        # parked dead rather than retried, so it does not wait.
        assert elapsed >= 0.05 + 0.10

    def test_backoff_is_capped(self, tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
        monkeypatch.setattr("dagayn.task_queue._TASK_EXECUTORS", {"update": self._always_fails})
        monkeypatch.setattr("dagayn.task_queue.RETRY_BACKOFF_SECONDS", 5.0)
        monkeypatch.setattr("dagayn.task_queue.MAX_RETRY_BACKOFF_SECONDS", 0.05)
        queue = TaskQueue(queue_db_path(tmp_path))
        queue.enqueue("update")
        queue.close()

        started = time.monotonic()
        run_worker(tmp_path, idle_seconds=0.0)
        elapsed = time.monotonic() - started

        # Uncapped this would wait 5s then 10s.
        assert elapsed < 1.0

    def test_backoff_can_be_disabled(self, tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
        monkeypatch.setattr("dagayn.task_queue._TASK_EXECUTORS", {"update": self._always_fails})
        monkeypatch.setattr("dagayn.task_queue.RETRY_BACKOFF_SECONDS", 5.0)
        queue = TaskQueue(queue_db_path(tmp_path))
        queue.enqueue("update")
        queue.close()

        started = time.monotonic()
        run_worker(tmp_path, idle_seconds=0.0, retry_backoff=False)

        assert time.monotonic() - started < 1.0

    def test_second_worker_exits_immediately(self, tmp_path: Path) -> None:
        lock = WorkerLock(worker_lock_path(tmp_path))
        assert lock.acquire() is True
        try:
            assert run_worker(tmp_path, idle_seconds=0.2) == 0
        finally:
            lock.release()

    def test_worker_releases_lock_on_exit(
        self, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        monkeypatch.setattr("dagayn.task_queue._TASK_EXECUTORS", self._fake_executors([]))
        run_worker(tmp_path, idle_seconds=0.2)
        probe = WorkerLock(worker_lock_path(tmp_path))
        assert probe.acquire() is True
        probe.release()


class TestClaimRace:
    """A claim in flight must not swallow work enqueued alongside it.

    ``enqueue`` looks up its pending twin and then writes. If those two steps
    do not serialize against ``claim``, the twin can flip to ``running`` in
    between, the new work is folded into a task the worker has already read,
    and it disappears when that task completes — the burst-tail staleness the
    queue exists to prevent.
    """

    def test_enqueue_does_not_fold_into_a_task_being_claimed(self, tmp_path: Path) -> None:
        db_path = tmp_path / "task_queue.db"
        writer = TaskQueue(db_path)
        _, first_id = writer.enqueue("update", {"edit": 1})
        holding = threading.Event()

        def _hold_a_claim() -> None:
            # A sqlite connection belongs to its creating thread, so the
            # stand-in worker opens its own. It then holds exactly the write
            # transaction ``claim`` takes, which is the window the enqueue used
            # to slip into.
            worker = TaskQueue(db_path)
            worker._conn.execute("BEGIN IMMEDIATE")
            worker._conn.execute("UPDATE tasks SET state = 'running' WHERE id = ?", (first_id,))
            holding.set()
            time.sleep(0.3)
            worker._conn.commit()
            worker.close()

        thread = threading.Thread(target=_hold_a_claim)
        thread.start()
        try:
            assert holding.wait(timeout=5)
            action, task_id = writer.enqueue("update", {"edit": 2})
        finally:
            thread.join(timeout=5)

        assert action == "added", "second edit was folded into a task already claimed"
        assert task_id != first_id

        # The worker finishing the claimed task deletes that row; the new work
        # has to survive it.
        row = writer._conn.execute("SELECT * FROM tasks WHERE id = ?", (first_id,)).fetchone()
        writer.complete(dict(row))
        assert writer.stats()["counts"] == {"pending": 1, "running": 0, "dead": 0}
        assert writer.claim() is not None

        writer.close()


class TestRequeueStale:
    def test_running_task_is_requeued(self, queue: TaskQueue) -> None:
        queue.enqueue("update")
        claimed = queue.claim()
        assert claimed is not None

        assert queue.requeue_stale() == 1
        assert queue.stats()["counts"] == {"pending": 1, "running": 0, "dead": 0}
        assert queue.claim() is not None

    def test_exhausted_task_is_parked_dead(self, queue: TaskQueue) -> None:
        """A task that always kills its worker must not be recovered forever."""
        queue.enqueue("update")
        for _ in range(MAX_ATTEMPTS - 1):
            assert queue.claim() is not None  # claimed, then the worker dies
            assert queue.requeue_stale() == 1
        assert queue.claim() is not None  # the attempt that spends the budget

        assert queue.requeue_stale() == 1
        assert queue.stats()["counts"]["dead"] == 1
        assert queue.claim() is None

    def test_no_running_tasks_is_a_noop(self, queue: TaskQueue) -> None:
        queue.enqueue("update")
        assert queue.requeue_stale() == 0
        assert queue.stats()["counts"] == {"pending": 1, "running": 0, "dead": 0}

    def test_worker_recovers_a_task_left_by_a_dead_worker(
        self, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        calls: list[str] = []

        def executor(task: dict[str, Any], repo_root: Path) -> str | None:
            calls.append(task["kind"])
            return None

        monkeypatch.setattr("dagayn.task_queue._TASK_EXECUTORS", {"update": executor})
        queue = TaskQueue(queue_db_path(tmp_path))
        queue.enqueue("update")
        queue.claim()  # the worker dies here (os._exit), leaving the row running
        queue.close()

        executed = run_worker(tmp_path, idle_seconds=0.2)

        assert executed == 1
        assert calls == ["update"]
        assert TaskQueue(queue_db_path(tmp_path)).stats()["counts"] == {
            "pending": 0,
            "running": 0,
            "dead": 0,
        }


class TestEnsureWorker:
    def test_spawns_worker_when_lock_is_free(
        self, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        spawned: list[Any] = []

        class FakePopen:
            def __init__(self, cmd: list[str], **kwargs: Any) -> None:
                spawned.append((cmd, kwargs))

        monkeypatch.setattr(subprocess, "Popen", FakePopen)
        assert ensure_worker(tmp_path, idle_seconds=5) is True

        assert len(spawned) == 1
        cmd, kwargs = spawned[0]
        assert cmd[3:] == ["dagayn", "queue", "run", "--repo", str(tmp_path), "--idle-seconds", "5"]
        if os.name == "posix":
            assert kwargs.get("start_new_session") is True

    def test_worker_does_not_import_from_the_hooks_cwd(
        self, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        """A hook firing inside a checkout must not decide what the worker imports.

        ``python -m`` prepends the cwd to ``sys.path``, so a repository with its
        own ``dagayn/`` package (dagayn's own, most obviously) used to shadow the
        installed one — including a stale compiled ``_core`` beside it.
        """
        spawned: list[Any] = []

        class FakePopen:
            def __init__(self, cmd: list[str], **kwargs: Any) -> None:
                spawned.append((cmd, kwargs))

        monkeypatch.setattr(subprocess, "Popen", FakePopen)
        assert ensure_worker(tmp_path) is True

        cmd, kwargs = spawned[0]
        assert cmd[1] == "-P", "the worker must not get the cwd on sys.path"
        # ...and it still has to find the dagayn that spawned it.
        package_parent = str(Path(dagayn.task_queue.__file__).resolve().parent.parent)
        assert kwargs["env"]["PYTHONPATH"].split(os.pathsep)[0] == package_parent

    def test_worker_env_keeps_an_existing_pythonpath(
        self, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        spawned: list[Any] = []

        class FakePopen:
            def __init__(self, cmd: list[str], **kwargs: Any) -> None:
                spawned.append((cmd, kwargs))

        monkeypatch.setattr(subprocess, "Popen", FakePopen)
        monkeypatch.setenv("PYTHONPATH", "/somewhere/else")
        assert ensure_worker(tmp_path) is True

        entries = spawned[0][1]["env"]["PYTHONPATH"].split(os.pathsep)
        assert entries[-1] == "/somewhere/else"
        assert len(entries) == 2

    def test_no_spawn_when_lock_held(self, tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
        held = WorkerLock(worker_lock_path(tmp_path))
        assert held.acquire() is True
        spawned: list[Any] = []

        class FakePopen:
            def __init__(self, cmd: list[str], **kwargs: Any) -> None:
                spawned.append((cmd, kwargs))

        monkeypatch.setattr(subprocess, "Popen", FakePopen)
        try:
            assert ensure_worker(tmp_path) is False
        finally:
            held.release()

        assert spawned == []

    def test_spawn_failure_returns_false(
        self, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        def raise_popen(cmd: list[str], **kwargs: Any) -> None:
            raise OSError("no such interpreter")

        monkeypatch.setattr(subprocess, "Popen", raise_popen)
        assert ensure_worker(tmp_path) is False


class TestPaths:
    def test_queue_db_lives_in_data_dir(self, tmp_path: Path) -> None:
        path = queue_db_path(tmp_path)
        assert path.parent == tmp_path / ".dagayn"
        assert path.name == "task_queue.db"

    def test_worker_lock_lives_in_data_dir(self, tmp_path: Path) -> None:
        path = worker_lock_path(tmp_path)
        assert path.parent == tmp_path / ".dagayn"
        assert path.name == "queue_worker.lock"


class TestUnfinishedEmbedding:
    """A budget-capped embedding pass must queue the rest of the corpus.

    Without this, a corpus too large to embed inside one budget is killed by
    the watchdog on every attempt and ends up parked ``dead`` with the
    embeddings permanently incomplete.
    """

    @staticmethod
    def _result(*, remaining: int, embedded: int) -> dict[str, Any]:
        return {
            "status": "ok",
            "local_embedding": {
                "newly_embedded": embedded,
                "embedding_remaining": remaining,
            },
        }

    @staticmethod
    def _claim_pending(repo: Path) -> dict[str, Any] | None:
        q = TaskQueue(queue_db_path(repo))
        try:
            return q.claim()
        finally:
            q.close()

    @staticmethod
    def _pending_count(repo: Path) -> int:
        q = TaskQueue(queue_db_path(repo))
        try:
            return int(q.stats()["counts"]["pending"])
        finally:
            q.close()

    def test_leftovers_with_progress_are_requeued(self, tmp_path: Path) -> None:
        payload = {"local_embedding": "bge-m3", "keep_local_embedding_server": True}
        note = _requeue_unfinished_embedding(
            tmp_path,
            payload,
            self._result(remaining=900, embedded=1200),
        )
        assert note is not None
        assert "900 left" in note

        assert self._pending_count(tmp_path) == 1
        queued = self._claim_pending(tmp_path)
        assert queued is not None
        assert queued["kind"] == "embed"
        # The follow-up must keep the sidecar settings, or each slice reloads
        # the model and startup dominates the run.
        assert queued["payload"] == payload

    def test_finished_pass_is_not_requeued(self, tmp_path: Path) -> None:
        note = _requeue_unfinished_embedding(tmp_path, {}, self._result(remaining=0, embedded=50))
        assert note is None
        assert self._pending_count(tmp_path) == 0

    def test_pass_without_progress_is_not_requeued(self, tmp_path: Path) -> None:
        # Leftovers the provider keeps rejecting: re-queueing would spin
        # forever, and MAX_ATTEMPTS does not catch it because each pass is a
        # success.
        note = _requeue_unfinished_embedding(tmp_path, {}, self._result(remaining=7, embedded=0))
        assert note is None
        assert self._pending_count(tmp_path) == 0

    def test_pass_budget_leaves_room_inside_the_watchdog_budget(self) -> None:
        # The watchdog kills the task at DEFAULT_EMBED_BUDGET_SECONDS; the pass
        # has to stop first so the leftovers get queued instead of lost.
        assert 0 < EMBED_PASS_SECONDS < DEFAULT_EMBED_BUDGET_SECONDS

    def test_missing_embedding_section_is_tolerated(self, tmp_path: Path) -> None:
        assert _requeue_unfinished_embedding(tmp_path, {}, {"status": "ok"}) is None
        assert self._pending_count(tmp_path) == 0
