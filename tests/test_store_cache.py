"""Tests for the process-level GraphStore cache in dagayn.tools._common.

Covers lease counting, eviction safety, and the regression where
_evict_store_cache() force-closed a connection while a tool invocation
was still in-flight (Codex review C2-6 / P1).
"""

from __future__ import annotations

import threading

import pytest

from dagayn.graph import GraphStore
from dagayn.tools._common import _evict_store_cache, _get_store

# ---------------------------------------------------------------------------
# Unit tests: GraphStore.close() state machine
# ---------------------------------------------------------------------------


class TestGraphStoreLease:
    def test_fresh_store_closes_on_first_call(self, tmp_path):
        """An uncached (non-pinned, non-leased) store closes when close() is called."""
        db = tmp_path / "g.db"
        store = GraphStore(db)
        assert store._leases == 0
        assert not store._pinned
        store.close()
        with pytest.raises(Exception):  # closed connection raises on use
            store._conn.execute("SELECT 1")

    def test_pinned_store_survives_close(self, tmp_path):
        """A pinned (cached) store ignores close() calls."""
        db = tmp_path / "g.db"
        store = GraphStore(db)
        store._pinned = True
        store._leases = 1
        store.close()  # lease 1→0, but _pinned keeps connection open
        store._conn.execute("SELECT 1")  # must still work

    def test_evicted_store_with_lease_survives_close(self, tmp_path):
        """After eviction (pinned=False) a store with leases>0 stays open."""
        db = tmp_path / "g.db"
        store = GraphStore(db)
        store._pinned = False
        store._leases = 2

        store.close()  # leases 2→1; not yet zero
        store._conn.execute("SELECT 1")  # still open

        store.close()  # leases 1→0; now closes
        with pytest.raises(Exception):
            store._conn.execute("SELECT 1")

    def test_evicted_store_zero_leases_closes(self, tmp_path):
        """After eviction with leases=0 the next close() closes the connection."""
        db = tmp_path / "g.db"
        store = GraphStore(db)
        store._pinned = False
        store._leases = 1
        store.close()  # 1→0, pinned=False → _conn.close()
        with pytest.raises(Exception):
            store._conn.execute("SELECT 1")


# ---------------------------------------------------------------------------
# Unit tests: _get_store lease accounting
# ---------------------------------------------------------------------------


class TestGetStoreLease:
    def test_cached_store_returns_same_instance(self, tmp_path):
        """Two consecutive _get_store() calls return the same GraphStore object."""
        db = tmp_path / ".dagayn" / "graph.db"
        db.parent.mkdir(parents=True)
        GraphStore(db).close()  # create DB file

        _evict_store_cache()
        store1, _ = _get_store(str(tmp_path))
        store2, _ = _get_store(str(tmp_path))
        assert store1 is store2
        store1.close()
        store2.close()
        _evict_store_cache()

    def test_cached_store_increments_leases(self, tmp_path):
        """Each _get_store() call increments the lease count."""
        db = tmp_path / ".dagayn" / "graph.db"
        db.parent.mkdir(parents=True)
        GraphStore(db).close()

        _evict_store_cache()
        store1, _ = _get_store(str(tmp_path))
        assert store1._leases == 1
        store2, _ = _get_store(str(tmp_path))
        assert store2._leases == 2
        store1.close()
        assert store1._leases == 1
        store2.close()
        assert store2._leases == 0
        _evict_store_cache()

    def test_uncached_store_has_lease_one(self, tmp_path):
        """A fresh (uncached) store starts with _leases=1."""
        db = tmp_path / ".dagayn" / "graph.db"
        db.parent.mkdir(parents=True)
        GraphStore(db).close()

        store, _ = _get_store(str(tmp_path), cached=False)
        assert store._leases == 1
        assert not store._pinned
        store.close()
        with pytest.raises(Exception):
            store._conn.execute("SELECT 1")


# ---------------------------------------------------------------------------
# Eviction tests
# ---------------------------------------------------------------------------


class TestEvictionSafety:
    def test_evict_with_no_inflight_closes_immediately(self, tmp_path):
        """Evicting a store with no active leases closes it immediately."""
        db = tmp_path / ".dagayn" / "graph.db"
        db.parent.mkdir(parents=True)
        GraphStore(db).close()

        _evict_store_cache()
        store, _ = _get_store(str(tmp_path))
        store.close()  # release lease so leases=0 in cache

        # At this point leases should be 0.  Eviction should close immediately.
        assert store._leases == 0
        _evict_store_cache(db)
        with pytest.raises(Exception):
            store._conn.execute("SELECT 1")

    def test_evict_with_inflight_keeps_connection_open(self, tmp_path):
        """Evicting a store with active leases must NOT close the connection."""
        db = tmp_path / ".dagayn" / "graph.db"
        db.parent.mkdir(parents=True)
        GraphStore(db).close()

        _evict_store_cache()
        store, _ = _get_store(str(tmp_path))
        assert store._leases == 1

        # Evict while in-flight (leases=1).
        _evict_store_cache(db)

        # Connection must still be usable.
        store._conn.execute("SELECT 1")

        # Releasing the lease now closes the connection.
        store.close()
        with pytest.raises(Exception):
            store._conn.execute("SELECT 1")

    def test_evict_full_cache_with_inflight(self, tmp_path):
        """Full-cache evict (db_path=None) also respects in-flight leases."""
        db = tmp_path / ".dagayn" / "graph.db"
        db.parent.mkdir(parents=True)
        GraphStore(db).close()

        _evict_store_cache()
        store, _ = _get_store(str(tmp_path))

        _evict_store_cache()  # no db_path → clears all
        store._conn.execute("SELECT 1")  # still open

        store.close()
        with pytest.raises(Exception):
            store._conn.execute("SELECT 1")

    def test_concurrent_evict_does_not_break_inflight_reader(self, tmp_path):
        """Race regression: eviction during an active read must not crash the reader.

        Uses threading.Barrier for deterministic ordering:
          1. Reader acquires the cached store.
          2. Reader signals it is mid-read.
          3. Evictor runs _evict_store_cache().
          4. Reader completes its query and calls close().
        """
        db = tmp_path / ".dagayn" / "graph.db"
        db.parent.mkdir(parents=True)
        GraphStore(db).close()

        _evict_store_cache()

        read_acquired = threading.Event()
        evict_done = threading.Event()
        errors: list[Exception] = []

        def reader() -> None:
            try:
                store, _ = _get_store(str(tmp_path))
                read_acquired.set()  # signal: store acquired, mid-read
                evict_done.wait(timeout=5)  # wait for eviction
                store._conn.execute("SELECT 1")  # must not raise
                store.close()
            except Exception as exc:
                errors.append(exc)

        def evictor() -> None:
            read_acquired.wait(timeout=5)
            _evict_store_cache()
            evict_done.set()

        t_read = threading.Thread(target=reader)
        t_evict = threading.Thread(target=evictor)

        t_read.start()
        t_evict.start()
        t_read.join(timeout=10)
        t_evict.join(timeout=10)

        assert not errors, f"Reader raised: {errors}"


class TestNativeStoreClosesConnection:
    """The Rust-backed store must honour the same close() contract.

    Its ``close()`` / ``_force_close()`` used to be no-ops, so every store
    leaked a SQLite connection for the life of the process. Leaked handles keep
    the database mmap'd while another connection checkpoints and truncates the
    WAL, and a later open then fails with ``sqlite error: disk I/O error``.
    """

    def _native_store_cls(self):
        try:
            from dagayn._core import GraphStore as NativeGraphStore
        except ImportError:  # pragma: no cover - wheel without the extension
            pytest.skip("native extension not built")  # ty: ignore[too-many-positional-arguments]
        return NativeGraphStore

    def test_close_releases_the_connection(self, tmp_path):
        store_cls = self._native_store_cls()
        store = store_cls(tmp_path / "g.db")
        store._leases = 1
        store.close()

        with pytest.raises(RuntimeError, match="closed"):
            store.get_metadata("repo_root")

    def test_pinned_store_survives_close(self, tmp_path):
        store_cls = self._native_store_cls()
        store = store_cls(tmp_path / "g.db")
        store._pinned = True
        store._leases = 1
        store.close()

        store.set_metadata("probe", "1")
        assert store.get_metadata("probe") == "1"

    def test_force_close_releases_regardless_of_leases(self, tmp_path):
        store_cls = self._native_store_cls()
        store = store_cls(tmp_path / "g.db")
        store._pinned = True
        store._leases = 3
        store._force_close()

        with pytest.raises(RuntimeError, match="closed"):
            store.get_metadata("repo_root")

    def test_reopen_after_write_cycle_succeeds(self, tmp_path):
        """The end-to-end shape of the bug: open, close, write, reopen."""
        store_cls = self._native_store_cls()
        db = tmp_path / "g.db"
        for _ in range(4):
            store = store_cls(db)
            store._leases = 1
            store.set_metadata("cycle", "x")
            store.close()
        store = store_cls(db)
        assert store.get_metadata("cycle") == "x"
        store._leases = 1
        store.close()


class TestCacheSeesOtherProcessWrites:
    """The cache must not outlive another connection's commit.

    ``st_mtime`` was the only staleness signal, but the journal mode is WAL:
    a commit lands in ``graph.db-wal`` and the main file's mtime does not move
    until a checkpoint. A long-lived ``dagayn serve`` therefore kept answering
    from a NetworkX snapshot taken before the write, indefinitely.
    """

    def _repo(self, tmp_path, monkeypatch):
        """A repo whose graph.db already exists.

        ``_get_store`` only caches once the file is there -- without this the
        first call returns an uncached transient store and every assertion
        about cache identity passes for the wrong reason.
        """
        repo = tmp_path / "repo"
        (repo / ".dagayn").mkdir(parents=True)
        (repo / ".git").mkdir()
        monkeypatch.setenv("CRG_DATA_DIR", str(tmp_path / "data"))
        _evict_store_cache()
        bootstrap, _ = _get_store(str(repo))
        bootstrap.set_metadata("seed", "1")
        bootstrap.commit()
        bootstrap.close()
        _evict_store_cache()
        return repo

    def test_external_commit_invalidates_the_cached_store(self, tmp_path, monkeypatch):
        import sqlite3

        from dagayn.paths import get_db_path

        repo = self._repo(tmp_path, monkeypatch)
        first, _ = _get_store(str(repo))
        assert first._pinned, "precondition: the first store must be the cached one"
        db_path = get_db_path(repo)
        mtime_before = db_path.stat().st_mtime

        # A separate connection stands in for another process.
        writer = sqlite3.connect(db_path)
        try:
            writer.execute(
                "INSERT INTO metadata (key, value) VALUES ('from_other_process', 'yes') "
                "ON CONFLICT(key) DO UPDATE SET value = excluded.value"
            )
            writer.commit()
        finally:
            writer.close()

        second, _ = _get_store(str(repo))
        try:
            assert second is not first, "cached store survived another connection's commit"
            assert second.get_metadata("from_other_process") == "yes"
            # Guard the premise: if this ever becomes True the mtime check
            # alone would have been enough and this test proves nothing.
            assert db_path.stat().st_mtime == mtime_before
        finally:
            _evict_store_cache()

    def test_unchanged_db_still_reuses_the_cached_store(self, tmp_path, monkeypatch):
        """The stricter staleness check must not defeat the cache entirely."""
        repo = self._repo(tmp_path, monkeypatch)
        first, _ = _get_store(str(repo))
        try:
            second, _ = _get_store(str(repo))
            assert second is first, "cache is not caching"
            third, _ = _get_store(str(repo))
            assert third is first
        finally:
            _evict_store_cache()
