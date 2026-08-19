"""Concurrency and transaction-boundary regressions (issues #94-#99).

These cover the class of bug where a write appears to succeed while the graph
is left incomplete, or where two writers corrupt each other instead of queuing.
"""

from __future__ import annotations

import collections
import sqlite3
import subprocess
import sys
import threading
import time
from pathlib import Path

import pytest

from dagayn.graph import GraphStore
from dagayn.parser import NodeInfo
from dagayn.write_lock import (
    WriteLockUnavailableError,
    graph_lock_is_held,
    graph_read_lock,
    graph_write_lock,
    write_lock_is_held,
)


def _node(name: str, file_path: str = "f.py") -> NodeInfo:
    return NodeInfo(
        kind="Function",
        name=name,
        file_path=file_path,
        line_start=1,
        line_end=2,
        language="python",
    )


class TestGraphWriteLock:
    """#95: the only lock used to cover hook-update vs hook-update."""

    def test_second_process_waits_instead_of_failing(self, tmp_path):
        db = tmp_path / "graph.db"
        db.touch()
        holder = (
            "import sys, time\n"
            "from dagayn.write_lock import graph_write_lock\n"
            "with graph_write_lock(sys.argv[1]):\n"
            "    print('held', flush=True)\n"
            "    time.sleep(0.35)\n"
        )
        proc = subprocess.Popen(
            [sys.executable, "-c", holder, str(db)],
            stdout=subprocess.PIPE,
            text=True,
        )
        try:
            assert proc.stdout is not None
            assert proc.stdout.readline().strip() == "held"
            started = time.monotonic()
            with graph_write_lock(db, timeout=30):
                waited = time.monotonic() - started
            assert waited > 0.12, f"did not wait for the other process ({waited:.2f}s)"
        finally:
            proc.wait(timeout=30)

    def test_non_blocking_acquisition_reports_contention(self, tmp_path):
        """Hook-triggered runs must skip, not queue."""
        db = tmp_path / "graph.db"
        db.touch()
        holder = (
            "import sys, time\n"
            "from dagayn.write_lock import graph_write_lock\n"
            "with graph_write_lock(sys.argv[1]):\n"
            "    print('held', flush=True)\n"
            "    time.sleep(0.35)\n"
        )
        proc = subprocess.Popen(
            [sys.executable, "-c", holder, str(db)],
            stdout=subprocess.PIPE,
            text=True,
        )
        try:
            assert proc.stdout is not None
            assert proc.stdout.readline().strip() == "held"
            with pytest.raises(WriteLockUnavailableError):
                with graph_write_lock(db, blocking=False):
                    pass
        finally:
            proc.wait(timeout=30)

    def test_reentrant_within_one_process(self, tmp_path):
        """A build holds the lock and then opens a store that may migrate."""
        db = tmp_path / "graph.db"
        db.touch()
        with graph_write_lock(db):
            assert write_lock_is_held(db)
            with graph_write_lock(db):
                assert write_lock_is_held(db)
            # Still held by the outer acquisition.
            assert write_lock_is_held(db)
        assert not write_lock_is_held(db)

    def test_separate_databases_do_not_block_each_other(self, tmp_path):
        first = tmp_path / "a" / "graph.db"
        second = tmp_path / "b" / "graph.db"
        for path in (first, second):
            path.parent.mkdir(parents=True)
            path.touch()
        with graph_write_lock(first), graph_write_lock(second, blocking=False):
            assert write_lock_is_held(first)
            assert write_lock_is_held(second)


class TestGraphReadWriteLock:
    """Readers and writers must not overlap on the same graph.db."""

    def test_writer_waits_for_reader(self, tmp_path):
        db = tmp_path / "graph.db"
        db.touch()
        holder = (
            "import sys, time\n"
            "from dagayn.write_lock import graph_read_lock\n"
            "with graph_read_lock(sys.argv[1]):\n"
            "    print('held', flush=True)\n"
            "    time.sleep(0.35)\n"
        )
        proc = subprocess.Popen(
            [sys.executable, "-c", holder, str(db)],
            stdout=subprocess.PIPE,
            text=True,
        )
        try:
            assert proc.stdout is not None
            assert proc.stdout.readline().strip() == "held"
            started = time.monotonic()
            with graph_write_lock(db, timeout=30):
                waited = time.monotonic() - started
            assert waited > 0.12, f"writer did not wait for reader ({waited:.2f}s)"
        finally:
            proc.wait(timeout=30)

    def test_reader_waits_for_writer(self, tmp_path):
        db = tmp_path / "graph.db"
        db.touch()
        holder = (
            "import sys, time\n"
            "from dagayn.write_lock import graph_write_lock\n"
            "with graph_write_lock(sys.argv[1]):\n"
            "    print('held', flush=True)\n"
            "    time.sleep(0.35)\n"
        )
        proc = subprocess.Popen(
            [sys.executable, "-c", holder, str(db)],
            stdout=subprocess.PIPE,
            text=True,
        )
        try:
            assert proc.stdout is not None
            assert proc.stdout.readline().strip() == "held"
            started = time.monotonic()
            with graph_read_lock(db, timeout=30):
                waited = time.monotonic() - started
            assert waited > 0.12, f"reader did not wait for writer ({waited:.2f}s)"
        finally:
            proc.wait(timeout=30)

    def test_two_readers_do_not_block_each_other(self, tmp_path):
        db = tmp_path / "graph.db"
        db.touch()
        holder = (
            "import sys, time\n"
            "from dagayn.write_lock import graph_read_lock\n"
            "with graph_read_lock(sys.argv[1]):\n"
            "    print('held', flush=True)\n"
            "    time.sleep(0.35)\n"
        )
        proc = subprocess.Popen(
            [sys.executable, "-c", holder, str(db)],
            stdout=subprocess.PIPE,
            text=True,
        )
        try:
            assert proc.stdout is not None
            assert proc.stdout.readline().strip() == "held"
            started = time.monotonic()
            with graph_read_lock(db, timeout=30, blocking=False):
                waited = time.monotonic() - started
            assert waited < 0.5, f"second reader blocked ({waited:.2f}s)"
        finally:
            proc.wait(timeout=30)

    def test_nested_read_during_write_does_not_deadlock(self, tmp_path):
        db = tmp_path / "graph.db"
        db.touch()
        with graph_write_lock(db):
            assert write_lock_is_held(db)
            with graph_read_lock(db):
                assert write_lock_is_held(db)
            assert write_lock_is_held(db)
        assert not write_lock_is_held(db)

    def test_nested_write_during_read_does_not_deadlock(self, tmp_path):
        db = tmp_path / "graph.db"
        db.touch()
        started = time.monotonic()
        with graph_read_lock(db):
            with graph_write_lock(db):
                assert write_lock_is_held(db)
            assert graph_lock_is_held(db)
            assert not write_lock_is_held(db)
        assert time.monotonic() - started < 2.0

    def test_store_open_under_read_lock_migrates_without_deadlock(self, tmp_path):
        db = tmp_path / "graph.db"
        started = time.monotonic()
        with graph_read_lock(db):
            store = GraphStore(db)
            store.close()
        assert time.monotonic() - started < 5.0

    def test_get_store_releases_read_lock_on_close(self, tmp_path):
        from dagayn.graph import GraphStore as PyGraphStore
        from dagayn.tools._common import _evict_store_cache, _get_store
        from dagayn.write_lock import graph_lock_is_held

        db = tmp_path / ".dagayn" / "graph.db"
        db.parent.mkdir(parents=True)
        PyGraphStore(db).close()
        _evict_store_cache()
        store, _ = _get_store(str(tmp_path))
        assert graph_lock_is_held(db)
        store.close()
        assert not graph_lock_is_held(db)
        started = time.monotonic()
        with graph_write_lock(db, blocking=False):
            waited = time.monotonic() - started
        assert waited < 0.5


class TestSharedConnectionThreadSafety:
    """#97: one connection, several threads, and a rollback-anything recovery."""

    def test_concurrent_writes_all_succeed(self, tmp_path):
        store = GraphStore(tmp_path / "g.db")
        errors: collections.Counter[str] = collections.Counter()
        succeeded = 0
        counter_lock = threading.Lock()

        def worker(tid: int) -> None:
            nonlocal succeeded
            for i in range(20):
                try:
                    store.store_file_nodes_edges(
                        f"f{tid}.py",
                        [_node(f"fn{tid}_{i}", f"f{tid}.py")],
                        [],
                        fhash=f"h{i}",
                        mtime_ns=i,
                    )
                    with counter_lock:
                        succeeded += 1
                except Exception as exc:  # noqa: BLE001 - the point of the test
                    errors[f"{type(exc).__name__}: {exc}"] += 1

        threads = [threading.Thread(target=worker, args=(t,)) for t in range(4)]
        for thread in threads:
            thread.start()
        for thread in threads:
            thread.join()
        try:
            assert not errors, dict(errors)
            assert succeeded == 80
            # Last write per file wins; four files, one node each.
            indexed = store._conn.execute("SELECT COUNT(*) FROM nodes").fetchone()[0]
            assert indexed == 4
        finally:
            store.close()

    def test_batch_writes_are_serialized(self, tmp_path):
        store = GraphStore(tmp_path / "g.db")
        errors: list[str] = []

        def worker(tid: int) -> None:
            batch = [
                (f"b{tid}_{i}.py", [_node(f"g{tid}_{i}", f"b{tid}_{i}.py")], [], f"h{i}", i)
                for i in range(5)
            ]
            try:
                store.store_file_batch(batch)
            except Exception as exc:  # noqa: BLE001
                errors.append(f"{type(exc).__name__}: {exc}")

        threads = [threading.Thread(target=worker, args=(t,)) for t in range(4)]
        for thread in threads:
            thread.start()
        for thread in threads:
            thread.join()
        try:
            assert not errors, errors
            assert store._conn.execute("SELECT COUNT(*) FROM nodes").fetchone()[0] == 20
        finally:
            store.close()


class TestMigrationAtomicity:
    """#99: rollback() was a no-op under isolation_level=None."""

    def _bare_db(self, tmp_path: Path) -> sqlite3.Connection:
        conn = sqlite3.connect(tmp_path / "m.db", isolation_level=None)
        conn.execute("CREATE TABLE metadata (key TEXT PRIMARY KEY, value TEXT)")
        conn.execute("INSERT INTO metadata VALUES ('schema_version', '1')")
        conn.execute("CREATE TABLE nodes (id INTEGER PRIMARY KEY)")
        return conn

    def test_failed_migration_leaves_no_trace(self, tmp_path, monkeypatch):
        import dagayn.migrations as migrations

        conn = self._bare_db(tmp_path)

        def half_applies_then_fails(target: sqlite3.Connection) -> None:
            target.execute("ALTER TABLE nodes ADD COLUMN partial TEXT")
            target.execute("INSERT INTO metadata VALUES ('side_effect', 'yes')")
            raise sqlite3.OperationalError("boom")

        monkeypatch.setattr(migrations, "MIGRATIONS", {2: half_applies_then_fails})
        with pytest.raises(sqlite3.OperationalError):
            migrations._apply_pending_migrations(conn, 1)

        columns = {row[1] for row in conn.execute("PRAGMA table_info(nodes)")}
        assert "partial" not in columns, "DDL survived a failed migration"
        assert (
            conn.execute("SELECT value FROM metadata WHERE key='side_effect'").fetchone() is None
        ), "DML survived a failed migration"
        assert (
            conn.execute("SELECT value FROM metadata WHERE key='schema_version'").fetchone()[0]
            == "1"
        )
        conn.close()

    def test_migration_that_commits_internally_still_advances(self, tmp_path, monkeypatch):
        """``ensure_edge_target_name_column`` commits on its own."""
        import dagayn.migrations as migrations

        conn = self._bare_db(tmp_path)

        def commits_internally(target: sqlite3.Connection) -> None:
            target.execute("ALTER TABLE nodes ADD COLUMN added TEXT")
            target.commit()

        monkeypatch.setattr(migrations, "MIGRATIONS", {2: commits_internally})
        migrations._apply_pending_migrations(conn, 1)
        assert (
            conn.execute("SELECT value FROM metadata WHERE key='schema_version'").fetchone()[0]
            == "2"
        )
        conn.close()


class TestStoreFailureIsNotSilent:
    """#94: a dropped chunk must not leave the graph claiming to describe HEAD."""

    def test_store_phase_failures_are_distinguished_from_parse_errors(self):
        from dagayn.incremental_build import store_phase_failures

        errors = [
            {"file": "a.py", "error": "syntax error"},
            {"file": "b.py", "error": "database is locked", "phase": "store"},
        ]
        assert store_phase_failures(errors) == ["b.py"]
        assert store_phase_failures([]) == []
        assert store_phase_failures(None) == []


class TestDaemonForkOrder:
    """#96: forking after start() killed the supervisor threads."""

    def test_daemonize_runs_before_start(self):
        import inspect

        from dagayn import daemon_cli

        source = inspect.getsource(daemon_cli._handle_start)
        assert "daemon.daemonize()" in source
        assert "daemon.start()" in source
        assert source.index("daemon.daemonize()") < source.index("daemon.start()"), (
            "state must be built after the fork, or the threads it starts are lost "
            "and the children are reparented away from the daemon"
        )

    def test_stdout_is_flushed_before_forking(self):
        """An unflushed buffer is duplicated into both children."""
        import inspect

        from dagayn import daemon_cli

        source = inspect.getsource(daemon_cli._handle_start)
        assert source.index("sys.stdout.flush()") < source.index("daemon.daemonize()")


class TestDaemonPidfileLiveness:
    """#102: os.kill(pid, 0) says "some process has this id", not "our daemon"."""

    def test_recycled_pid_is_not_reported_as_running(self, tmp_path):
        import os

        from dagayn.daemon import is_daemon_running

        pid_path = tmp_path / "daemon.pid"
        # A crashed daemon leaves the pidfile; the OS later hands that id to an
        # unrelated process (here, this test's own).
        pid_path.write_text(str(os.getpid()), encoding="utf-8")

        assert is_daemon_running(pid_path) is False
        assert not pid_path.exists(), "a stale pidfile should be cleaned up"

    def test_held_lock_reports_running(self, tmp_path):
        from dagayn.daemon import clear_pid, is_daemon_running, write_pid

        pid_path = tmp_path / "daemon.pid"
        write_pid(path=pid_path)
        try:
            assert is_daemon_running(pid_path) is True
        finally:
            clear_pid(pid_path)
        assert is_daemon_running(pid_path) is False


class TestInteractiveReadTimeout:
    """An MCP tool call must not go silent for the writer's whole budget.

    A reader has to hold the shared lock for as long as its connection is open
    (an open connection across a writer's WAL checkpoint is what tore
    ``sqlite_master``), so it cannot simply skip the lock. What it can do is
    stop waiting early and say why.
    """

    def test_reader_timeout_is_shorter_than_the_writer_budget(self):
        from dagayn.write_lock import DEFAULT_READ_LOCK_TIMEOUT, DEFAULT_WRITE_LOCK_TIMEOUT

        assert DEFAULT_READ_LOCK_TIMEOUT < DEFAULT_WRITE_LOCK_TIMEOUT

    def test_tool_entry_reports_the_writer_instead_of_hanging(self, tmp_path, monkeypatch):
        from dagayn.tools._common import _get_store

        repo = tmp_path / "repo"
        (repo / ".dagayn").mkdir(parents=True)
        (repo / ".git").mkdir()
        db = repo / ".dagayn" / "graph.db"
        GraphStore(db).close()

        # The writer has to be another process: a reader nested inside this
        # process's own write lock deliberately skips the shared lock.
        holder = (
            "import sys, time\n"
            "from dagayn.write_lock import graph_write_lock\n"
            "with graph_write_lock(sys.argv[1]):\n"
            "    print('held', flush=True)\n"
            "    time.sleep(3)\n"
        )
        proc = subprocess.Popen(
            [sys.executable, "-c", holder, str(db)],
            stdout=subprocess.PIPE,
            text=True,
        )
        monkeypatch.setattr("dagayn.tools._common.DEFAULT_READ_LOCK_TIMEOUT", 0.3)
        try:
            assert proc.stdout is not None
            assert proc.stdout.readline().strip() == "held"
            started = time.monotonic()
            with pytest.raises(WriteLockUnavailableError) as excinfo:
                _get_store(str(repo))
            elapsed = time.monotonic() - started
        finally:
            proc.wait(timeout=30)

        assert elapsed < 2.5, f"the tool entry outwaited its own read timeout ({elapsed:.2f}s)"
        message = str(excinfo.value)
        assert "being written" in message
        assert f"pid {proc.pid}" in message, "the holder should be named"
        assert "DAGAYN_READ_LOCK_TIMEOUT" in message, "the message should say how to wait longer"

    def test_reader_still_gets_the_lock_when_no_writer_holds_it(self, tmp_path):
        from dagayn.tools._common import _get_store
        from dagayn.write_lock import graph_lock_is_held

        repo = tmp_path / "repo"
        (repo / ".dagayn").mkdir(parents=True)
        (repo / ".git").mkdir()
        db = repo / ".dagayn" / "graph.db"
        GraphStore(db).close()

        store, _root = _get_store(str(repo))
        try:
            assert graph_lock_is_held(db), "an open reader must hold the shared lock"
        finally:
            store.close()
        assert not graph_lock_is_held(db), "closing the store must release it"


class TestEmbeddingLockScope:
    """The embedding pass must hold the graph lock only for its database work.

    Sidecar startup and model load take up to ``local_embedding_timeout``
    seconds and touch no sqlite at all; holding the exclusive lock across that
    made every concurrent MCP tool call wait out its own budget and fail.
    """

    def test_lock_is_free_during_sidecar_startup_and_held_for_the_write(
        self, tmp_path, monkeypatch
    ):
        import contextlib
        import types

        from dagayn.paths import get_db_path
        from dagayn.tools.build import _run_local_embedding
        from dagayn.write_lock import graph_lock_is_held

        repo = tmp_path / "repo"
        (repo / ".dagayn").mkdir(parents=True)
        (repo / ".git").mkdir()
        db = get_db_path(repo)
        GraphStore(db).close()

        observed: dict[str, bool] = {}
        preset = types.SimpleNamespace(
            model="m",
            text_mode="signature",
            request_max_length=None,
            level="bge-m3",
            dimension=8,
        )

        @contextlib.contextmanager
        def fake_server(*_args, **_kwargs):
            # Stands in for starting the sidecar and loading the model.
            observed["locked_during_startup"] = graph_lock_is_held(db)
            yield types.SimpleNamespace(
                preset=preset,
                base_url="http://127.0.0.1:1",
                started=True,
                command=["fake"],
            )

        def fake_embed_graph(**_kwargs):
            observed["locked_during_write"] = graph_lock_is_held(db)
            return {"status": "ok"}

        monkeypatch.setattr("dagayn.local_embeddings.local_embedding_server", fake_server)
        monkeypatch.setattr(
            "dagayn.local_embeddings.resolve_local_embedding_port", lambda *_a, **_k: 1
        )
        monkeypatch.setattr("dagayn.tools.docs.embed_graph", fake_embed_graph)

        _run_local_embedding(
            repo,
            local_embedding="bge-m3",
            local_embedding_port=None,
            local_embedding_bin="auto",
            keep_local_embedding_server=False,
            local_embedding_timeout=1,
            local_embedding_request_timeout=1,
            local_embedding_batch_size=1,
        )

        assert observed["locked_during_startup"] is False, (
            "the sidecar started while the exclusive lock was held"
        )
        assert observed["locked_during_write"] is True, (
            "the database write ran without the lock that keeps checkpoints safe"
        )


class TestEmbeddingSlices:
    """A long embedding pass must give the lock back between slices.

    Embedding a large corpus in one lock acquisition outlasts the reader
    timeout, so MCP tool calls fail for the whole run, and a queued update
    waits it out. Slicing bounds both to a single slice, and the pass budget
    keeps an oversized corpus from being killed by the watchdog every attempt.
    """

    @staticmethod
    def _fake_env(tmp_path, monkeypatch, embed_graph):
        """Wire ``_run_local_embedding`` to a fake sidecar and embed_graph.

        Returns ``(repo, db, acquisitions)`` where *acquisitions* counts graph
        lock acquisitions made by the embedding pass.
        """
        import contextlib
        import types

        from dagayn.paths import get_db_path
        from dagayn.write_lock import graph_write_lock

        repo = tmp_path / "repo"
        (repo / ".dagayn").mkdir(parents=True)
        (repo / ".git").mkdir()
        db = get_db_path(repo)
        GraphStore(db).close()

        preset = types.SimpleNamespace(
            model="m",
            text_mode="signature",
            request_max_length=None,
            level="bge-m3",
            dimension=8,
        )

        @contextlib.contextmanager
        def fake_server(*_args, **_kwargs):
            yield types.SimpleNamespace(
                preset=preset,
                base_url="http://127.0.0.1:1",
                started=True,
                command=["fake"],
            )

        acquisitions: list[int] = []

        @contextlib.contextmanager
        def counting_lock(*args, **kwargs):
            acquisitions.append(1)
            with graph_write_lock(*args, **kwargs):
                yield

        monkeypatch.setattr("dagayn.local_embeddings.local_embedding_server", fake_server)
        monkeypatch.setattr(
            "dagayn.local_embeddings.resolve_local_embedding_port", lambda *_a, **_k: 1
        )
        monkeypatch.setattr("dagayn.tools.docs.embed_graph", embed_graph)
        monkeypatch.setattr("dagayn.tools.build.graph_write_lock", counting_lock)
        return repo, db, acquisitions

    @staticmethod
    def _run(repo, **kwargs):
        from dagayn.tools.build import _run_local_embedding

        return _run_local_embedding(
            repo,
            local_embedding="bge-m3",
            local_embedding_port=None,
            local_embedding_bin="auto",
            keep_local_embedding_server=True,
            local_embedding_timeout=1,
            local_embedding_request_timeout=1,
            local_embedding_batch_size=1,
            **kwargs,
        )

    def test_each_slice_takes_and_releases_the_lock(self, tmp_path, monkeypatch):
        from dagayn.write_lock import graph_lock_is_held

        calls: list[dict] = []

        def fake_embed_graph(**kwargs):
            calls.append(kwargs)
            remaining = max(0, 3 - len(calls))
            return {
                "status": "ok",
                "newly_embedded": 2,
                "orphans_removed": 1 if len(calls) == 1 else 0,
                "total_embeddings": 2 * len(calls),
                "remaining": remaining,
            }

        repo, db, acquisitions = self._fake_env(tmp_path, monkeypatch, fake_embed_graph)
        assert graph_lock_is_held(db) is False
        result = self._run(repo)

        assert len(calls) == 3
        assert len(acquisitions) == 3, "the pass did not re-take the lock per slice"
        assert graph_lock_is_held(db) is False
        # Every slice is time-bounded, and the whole-corpus sweeps run once.
        assert all(call["slice_seconds"] is not None for call in calls)
        assert [call["prune_orphans"] for call in calls] == [True, False, False]
        assert result["newly_embedded"] == 6
        assert result["orphans_removed"] == 1
        assert result["embedding_remaining"] == 0
        assert result["embedding_slices"] == 3

    def test_handoff_outlasts_the_lock_poll_interval(self):
        from dagayn import write_lock as write_lock_module
        from dagayn.tools.build import _EMBED_SLICE_HANDOFF_SECONDS

        # Waiters poll for the file lock, so a writer that releases and
        # immediately re-takes it hands over to nobody: measured on a 710-node
        # repository, slicing without this pause still failed 2 of 165 reads.
        assert _EMBED_SLICE_HANDOFF_SECONDS > write_lock_module._MAX_POLL_INTERVAL

    def test_slicing_can_be_disabled(self, tmp_path, monkeypatch):
        calls: list[dict] = []

        def fake_embed_graph(**kwargs):
            calls.append(kwargs)
            return {"status": "ok", "newly_embedded": 5, "remaining": 0}

        monkeypatch.setenv("DAGAYN_EMBED_SLICE_SECONDS", "0")
        repo, _db, acquisitions = self._fake_env(tmp_path, monkeypatch, fake_embed_graph)
        self._run(repo)

        assert len(acquisitions) == 1
        assert calls[0]["slice_seconds"] is None

    def test_a_slice_making_no_progress_stops_the_pass(self, tmp_path, monkeypatch):
        calls: list[dict] = []

        def fake_embed_graph(**kwargs):
            calls.append(kwargs)
            # A node the provider rejects every time: leftovers, no progress.
            return {"status": "ok", "newly_embedded": 0, "remaining": 7}

        repo, _db, _acq = self._fake_env(tmp_path, monkeypatch, fake_embed_graph)
        result = self._run(repo)

        assert len(calls) == 1, "the pass spun on a slice that embedded nothing"
        assert result["embedding_remaining"] == 7

    def test_pass_budget_stops_at_a_slice_boundary(self, tmp_path, monkeypatch):
        calls: list[dict] = []

        def fake_embed_graph(**kwargs):
            calls.append(kwargs)
            time.sleep(0.05)
            return {"status": "ok", "newly_embedded": 1, "remaining": 100}

        repo, _db, _acq = self._fake_env(tmp_path, monkeypatch, fake_embed_graph)
        result = self._run(repo, pass_seconds=0.04)

        # One slice always runs; the budget is spent by the time it ends.
        assert len(calls) == 1
        assert result["embedding_remaining"] == 100
        assert result["embedding_slices"] == 1

    def test_failed_slice_is_reported_without_further_slices(self, tmp_path, monkeypatch):
        calls: list[dict] = []

        def fake_embed_graph(**kwargs):
            calls.append(kwargs)
            return {"status": "error", "error": "provider unreachable"}

        repo, _db, _acq = self._fake_env(tmp_path, monkeypatch, fake_embed_graph)
        with pytest.raises(RuntimeError, match="provider unreachable"):
            self._run(repo)
        assert len(calls) == 1


class TestInProcessReaderParallelism:
    """Two reads on one graph must overlap inside a process, as they do across.

    The per-path thread lock used to be an ``RLock``, so a long-lived
    ``dagayn serve`` serialized tool calls on the same graph even though the
    flock they take is shared. With the interactive read timeout that turned a
    slow call into a *failed* neighbour rather than a queued one.
    """

    def _timed_reader(self, db, hold, results, name):
        def run():
            t0 = time.monotonic()
            with graph_read_lock(db, timeout=30):
                results[name] = time.monotonic() - t0
                time.sleep(hold)

        return run

    def test_two_readers_overlap(self, tmp_path):
        db = tmp_path / "graph.db"
        db.touch()
        results: dict[str, float] = {}
        first = threading.Thread(target=self._timed_reader(db, 1.0, results, "first"))
        second = threading.Thread(target=self._timed_reader(db, 0.0, results, "second"))
        first.start()
        time.sleep(0.2)
        second.start()
        for t in (first, second):
            t.join(timeout=30)

        assert results["second"] < 0.5, (
            f"the second reader queued behind the first ({results['second']:.2f}s)"
        )

    def test_writer_waits_for_an_in_process_reader(self, tmp_path):
        db = tmp_path / "graph.db"
        db.touch()
        results: dict[str, float] = {}
        reader = threading.Thread(target=self._timed_reader(db, 0.6, results, "reader"))
        reader.start()
        time.sleep(0.2)

        started = time.monotonic()
        with graph_write_lock(db, timeout=30):
            waited = time.monotonic() - started
        reader.join(timeout=30)

        assert waited > 0.2, f"the writer ran alongside a reader ({waited:.2f}s)"

    def test_reader_waits_for_an_in_process_writer(self, tmp_path):
        db = tmp_path / "graph.db"
        db.touch()
        released = threading.Event()

        def writer():
            with graph_write_lock(db, timeout=30):
                time.sleep(0.5)
            released.set()

        t = threading.Thread(target=writer)
        t.start()
        time.sleep(0.2)
        started = time.monotonic()
        with graph_read_lock(db, timeout=30):
            waited = time.monotonic() - started
        t.join(timeout=30)

        assert released.is_set()
        assert waited > 0.1, f"the reader ran alongside a writer ({waited:.2f}s)"

    def test_upgrade_waits_for_the_other_reader_then_succeeds(self, tmp_path):
        db = tmp_path / "graph.db"
        db.touch()
        results: dict[str, float] = {}
        other = threading.Thread(target=self._timed_reader(db, 0.6, results, "other"))
        other.start()
        time.sleep(0.2)

        # Our own shared hold may be upgraded; the other thread's may not.
        with graph_read_lock(db, timeout=30):
            started = time.monotonic()
            with graph_write_lock(db, timeout=30):
                waited = time.monotonic() - started
                assert write_lock_is_held(db)
        other.join(timeout=30)

        assert waited > 0.2, f"upgraded while another reader held it ({waited:.2f}s)"

    def test_non_blocking_reader_reports_a_writer_immediately(self, tmp_path):
        db = tmp_path / "graph.db"
        db.touch()
        started_holding = threading.Event()
        release = threading.Event()

        def writer():
            with graph_write_lock(db, timeout=30):
                started_holding.set()
                release.wait(30)

        t = threading.Thread(target=writer)
        t.start()
        try:
            assert started_holding.wait(30)
            started = time.monotonic()
            with pytest.raises(WriteLockUnavailableError):
                with graph_read_lock(db, blocking=False):
                    pass
            assert time.monotonic() - started < 1.0
        finally:
            release.set()
            t.join(timeout=30)


class TestLockFairness:
    """Neither side may be starved: waiters are served in arrival order."""

    def test_a_writer_is_not_shut_out_by_continuous_readers(self, tmp_path):
        db = tmp_path / "graph.db"
        db.touch()
        stop = threading.Event()
        reader_rounds = collections.Counter()

        def reader(name):
            while not stop.is_set():
                with graph_read_lock(db, timeout=10):
                    reader_rounds[name] += 1
                    time.sleep(0.005)

        readers = [threading.Thread(target=reader, args=(i,)) for i in range(4)]
        for t in readers:
            t.start()
        try:
            time.sleep(0.1)  # let the readers get going
            writer_acquisitions = 0
            deadline = time.monotonic() + 2.0
            while time.monotonic() < deadline:
                with graph_write_lock(db, timeout=10):
                    writer_acquisitions += 1
                time.sleep(0.005)
        finally:
            stop.set()
            for t in readers:
                t.join(timeout=30)

        assert writer_acquisitions > 5, (
            f"continuous readers starved the writer ({writer_acquisitions} acquisitions)"
        )
        assert sum(reader_rounds.values()) > 5, "the writer starved the readers"

    def test_queued_reader_is_served_after_the_writer_ahead_of_it(self, tmp_path):
        db = tmp_path / "graph.db"
        db.touch()
        order: list[str] = []
        guard = threading.Lock()
        holding = threading.Event()
        release = threading.Event()

        def first_reader():
            with graph_read_lock(db, timeout=10):
                holding.set()
                release.wait(10)

        def writer():
            with graph_write_lock(db, timeout=10):
                with guard:
                    order.append("writer")
                time.sleep(0.05)

        def late_reader():
            with graph_read_lock(db, timeout=10):
                with guard:
                    order.append("reader")

        t0 = threading.Thread(target=first_reader)
        t0.start()
        assert holding.wait(10)
        t1 = threading.Thread(target=writer)
        t1.start()
        time.sleep(0.15)  # the writer is queued, waiting for the first reader
        t2 = threading.Thread(target=late_reader)
        t2.start()
        time.sleep(0.15)  # the late reader must not overtake the queued writer
        release.set()
        for t in (t0, t1, t2):
            t.join(timeout=30)

        assert order == ["writer", "reader"], f"served out of order: {order}"
