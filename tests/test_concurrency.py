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
            "    time.sleep(1.5)\n"
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
            assert waited > 0.5, f"did not wait for the other process ({waited:.2f}s)"
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
            "    time.sleep(1.5)\n"
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
