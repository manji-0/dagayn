"""Tests for the repository-scoped task queue (dagayn.task_queue).

The queue replaces per-edit ``dagayn update`` hook spawns: hooks enqueue a
task (coalescing with a pending twin) and a single detached worker drains
the queue. These tests cover the queue mechanics, the worker lock, and the
worker loop with fake executors — no real graph builds.
"""

from __future__ import annotations

import os
import subprocess
from collections.abc import Iterator
from pathlib import Path
from typing import Any

import pytest

from dagayn.task_queue import (
    MAX_ATTEMPTS,
    TASK_KINDS,
    TaskQueue,
    WorkerLock,
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
        assert len(queue.pending_kinds()) == 1

    def test_coalesce_merges_payload_newer_wins(self, queue: TaskQueue) -> None:
        queue.enqueue("embed", payload={"a": 1, "b": 1})
        queue.enqueue("embed", payload={"b": 2})
        task = queue.claim()
        assert task is not None
        assert task["payload"] == {"a": 1, "b": 2}

    def test_coalesce_keeps_higher_priority(self, queue: TaskQueue) -> None:
        queue.enqueue("update", priority=5)
        queue.enqueue("update", priority=1)
        task = queue.claim()
        assert task is not None
        assert task["priority"] == 5

    def test_different_kinds_do_not_coalesce(self, queue: TaskQueue) -> None:
        queue.enqueue("update")
        queue.enqueue("postprocess")
        assert queue.pending_kinds() == {"update", "postprocess"}

    def test_unknown_kind_rejected(self, queue: TaskQueue) -> None:
        with pytest.raises(ValueError):
            queue.enqueue("nope")  # type: ignore[arg-type]

    def test_all_task_kinds_accepted(self, queue: TaskQueue) -> None:
        for kind in TASK_KINDS:
            action, _ = queue.enqueue(kind)
            assert action == "added"


class TestClaim:
    def test_empty_queue_returns_none(self, queue: TaskQueue) -> None:
        assert queue.claim() is None

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

        executed = run_worker(tmp_path, idle_seconds=0.2)
        assert executed == MAX_ATTEMPTS
        stats = TaskQueue(queue_db_path(tmp_path)).stats()
        assert stats["counts"]["dead"] == 1
        assert stats["counts"]["pending"] == 0

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
        assert cmd[2:] == ["dagayn", "queue", "run", "--repo", str(tmp_path), "--idle-seconds", "5"]
        if os.name == "posix":
            assert kwargs.get("start_new_session") is True

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
