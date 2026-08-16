"""Cross-process serialization for graph reads and writes.

Before this existed, the only mutual exclusion was a flock taken by
hook-triggered ``dagayn update`` runs (``DAGAYN_HOOK_UPDATE=1``). A hand-run
``dagayn update``, a ``dagayn build``, and the daemon's ``dagayn watch`` child
all wrote the same ``graph.db`` with nothing between them, and because a batch
holds ``BEGIN IMMEDIATE`` for up to 500 files, the second writer exhausted
``busy_timeout`` and failed outright.

Readers (MCP tools, ``dagayn status``) used to keep a SQLite connection open
while another process wrote. WAL allows that, but a checkpoint against a
second mmap'd connection tore ``sqlite_master``. Reads now take a shared flock
and writes an exclusive flock on the same lock file, so the two do not overlap.

Two properties the old lock did not have:

* It is keyed on the **resolved database path**, not the repository root, so two
  worktrees with separate graphs never block each other while two processes on
  one graph always do.
* It is acquired **before** the store is opened, so schema migrations (which
  write) happen inside it too.

Reentrant within a process: a build holds the exclusive lock and then opens a
store that may migrate, and that inner acquisition must not deadlock. A nested
read during a write stays exclusive. A nested write during a read upgrades
shared → exclusive for the inner scope, then downgrades.
"""

from __future__ import annotations

import logging
import os
import threading
from collections.abc import Callable, Generator
from contextlib import contextmanager
from pathlib import Path
from typing import IO, Protocol

logger = logging.getLogger(__name__)

#: How long a reader or writer waits for another process before giving up.
#: Longer than SQLite's own ``busy_timeout`` (5 s): a large batch legitimately
#: takes longer than that, and failing early is what this lock exists to prevent.
DEFAULT_WRITE_LOCK_TIMEOUT = float(os.environ.get("DAGAYN_WRITE_LOCK_TIMEOUT", "120"))
DEFAULT_IO_LOCK_TIMEOUT = DEFAULT_WRITE_LOCK_TIMEOUT

_registry_lock = threading.Lock()
#: One reentrant lock per database path, for threads inside this process.
_thread_locks: dict[Path, threading.RLock] = {}


class _HeldFileLock:
    __slots__ = ("handle", "modes")

    def __init__(self, handle: IO[str], exclusive: bool) -> None:
        self.handle = handle
        # True = exclusive acquisition; False = shared.
        self.modes: list[bool] = [exclusive]


#: Per-path flock handle plus the stack of shared/exclusive acquisitions.
_file_locks: dict[Path, _HeldFileLock] = {}
#: ``id(store)`` → (db key, nested count, store) for ``_get_store``/``close``.
#: The store object is kept so a recycled ``id()`` cannot release the wrong lock.
_store_read_binds: dict[int, tuple[Path, int, object]] = {}


class WriteLockUnavailableError(RuntimeError):
    """Raised when the lock could not be taken within the allotted time."""


def _lock_path_for(db_path: str | Path) -> Path:
    resolved = Path(db_path).expanduser()
    try:
        resolved = resolved.resolve()
    except OSError:
        resolved = resolved.absolute()
    return resolved


def _thread_lock_for(key: Path) -> threading.RLock:
    with _registry_lock:
        lock = _thread_locks.get(key)
        if lock is None:
            lock = threading.RLock()
            _thread_locks[key] = lock
        return lock


def _lock_file_path(key: Path) -> Path:
    return key.with_name(f"{key.name}.write.lock")


def _flock_wait(
    handle: IO[str],
    *,
    exclusive: bool,
    blocking: bool,
    timeout: float,
    key: Path,
) -> None:
    try:
        import fcntl
    except ImportError:  # pragma: no cover - POSIX only in practice
        logger.debug("fcntl unavailable; graph I/O is not serialized")
        return

    flag = fcntl.LOCK_EX if exclusive else fcntl.LOCK_SH
    lock_file_path = _lock_file_path(key)
    if not blocking:
        try:
            fcntl.flock(handle.fileno(), flag | fcntl.LOCK_NB)
        except BlockingIOError:
            action = "write" if exclusive else "read"
            raise WriteLockUnavailableError(
                f"another process is using {key} (lock: {lock_file_path}, wanted {action})"
            ) from None
        return

    deadline = timeout if timeout > 0 else None
    waited = 0.0
    interval = 0.05
    while True:
        try:
            fcntl.flock(handle.fileno(), flag | fcntl.LOCK_NB)
            return
        except BlockingIOError:
            if deadline is not None and waited >= deadline:
                action = "write" if exclusive else "read"
                raise WriteLockUnavailableError(
                    f"timed out after {timeout:g}s waiting to {action} {key} "
                    f"(lock: {lock_file_path})"
                ) from None
            threading.Event().wait(interval)
            waited += interval
            interval = min(interval * 2, 0.5)
        except OSError as exc:
            logger.debug("Could not flock %s (%s); proceeding unserialized", lock_file_path, exc)
            return


def _write_pid(handle: IO[str]) -> None:
    try:
        handle.seek(0)
        handle.truncate()
        handle.write(f"{os.getpid()}\n")
        handle.flush()
    except OSError:  # pragma: no cover - diagnostics only
        pass


def _flock_convert_exclusive(handle: IO[str]) -> None:
    """Replace a shared flock on *handle* with an exclusive one.

    ``LOCK_EX | LOCK_NB`` EAGAIN's against this fd's own ``LOCK_SH`` on
    Darwin and Linux, so a nested write during a read would poll until the
    timeout. A blocking ``LOCK_EX`` on the same fd converts the lock in one
    call once other holders release.
    """
    try:
        import fcntl
    except ImportError:  # pragma: no cover - POSIX only in practice
        return
    fcntl.flock(handle.fileno(), fcntl.LOCK_EX)


def _flock_acquire(
    key: Path,
    *,
    exclusive: bool,
    blocking: bool,
    timeout: float,
) -> None:
    """Take the OS-level lock. Nested calls in this process stack modes."""
    with _registry_lock:
        held = _file_locks.get(key)
        if held is not None:
            was_exclusive = any(held.modes)
            held.modes.append(exclusive)
            handle = held.handle
            need_upgrade = exclusive and not was_exclusive
        else:
            held = None
            handle = None
            need_upgrade = False

    if held is not None:
        if need_upgrade:
            assert handle is not None
            try:
                _flock_convert_exclusive(handle)
                _write_pid(handle)
            except BaseException:
                with _registry_lock:
                    current = _file_locks.get(key)
                    if current is not None and current.modes:
                        current.modes.pop()
                raise
        return

    lock_file_path = _lock_file_path(key)
    lock_file_path.parent.mkdir(parents=True, exist_ok=True)
    new_handle = lock_file_path.open("a+", encoding="utf-8")
    try:
        _flock_wait(
            new_handle,
            exclusive=exclusive,
            blocking=blocking,
            timeout=timeout,
            key=key,
        )
        if exclusive:
            _write_pid(new_handle)
    except BaseException:
        new_handle.close()
        raise

    with _registry_lock:
        _file_locks[key] = _HeldFileLock(new_handle, exclusive)


def _flock_release(key: Path) -> None:
    with _registry_lock:
        held = _file_locks.get(key)
        if held is None or not held.modes:
            return
        held.modes.pop()
        handle = held.handle
        if held.modes:
            still_exclusive = any(held.modes)
            downgrade = not still_exclusive
            done = False
        else:
            _file_locks.pop(key, None)
            still_exclusive = False
            downgrade = False
            done = True

    if done:
        try:
            import fcntl

            fcntl.flock(handle.fileno(), fcntl.LOCK_UN)
        except (ImportError, OSError):  # pragma: no cover - best effort
            pass
        finally:
            try:
                handle.close()
            except OSError:  # pragma: no cover
                pass
        return

    if downgrade:
        try:
            import fcntl

            fcntl.flock(handle.fileno(), fcntl.LOCK_SH)
        except (ImportError, OSError):  # pragma: no cover - best effort
            pass


def acquire_graph_lock(
    db_path: str | Path,
    *,
    exclusive: bool,
    blocking: bool = True,
    timeout: float = DEFAULT_IO_LOCK_TIMEOUT,
) -> None:
    """Acquire a shared (read) or exclusive (write) lock for *db_path*."""
    key = _lock_path_for(db_path)
    thread_lock = _thread_lock_for(key)
    if not thread_lock.acquire(blocking=blocking, timeout=timeout if blocking else -1):
        action = "write" if exclusive else "read"
        raise WriteLockUnavailableError(f"another thread is using {key} ({action})")
    try:
        _flock_acquire(key, exclusive=exclusive, blocking=blocking, timeout=timeout)
    except BaseException:
        thread_lock.release()
        raise


def release_graph_lock(db_path: str | Path) -> None:
    """Release one nested acquisition of the lock for *db_path*."""
    key = _lock_path_for(db_path)
    thread_lock = _thread_lock_for(key)
    try:
        _flock_release(key)
    finally:
        thread_lock.release()


@contextmanager
def graph_write_lock(
    db_path: str | Path,
    *,
    blocking: bool = True,
    timeout: float = DEFAULT_WRITE_LOCK_TIMEOUT,
) -> Generator[None, None, None]:
    """Serialize writes to the graph at *db_path* across processes and threads.

    With ``blocking=False`` a lock already held elsewhere raises
    :class:`WriteLockUnavailableError` immediately, which is what the hook-update
    path wants: overlapping hook runs should skip, not queue.
    """
    acquire_graph_lock(db_path, exclusive=True, blocking=blocking, timeout=timeout)
    try:
        yield
    finally:
        release_graph_lock(db_path)


@contextmanager
def graph_read_lock(
    db_path: str | Path,
    *,
    blocking: bool = True,
    timeout: float = DEFAULT_IO_LOCK_TIMEOUT,
) -> Generator[None, None, None]:
    """Hold a shared lock so writers wait, while other readers may proceed."""
    acquire_graph_lock(db_path, exclusive=False, blocking=blocking, timeout=timeout)
    try:
        yield
    finally:
        release_graph_lock(db_path)


def write_lock_is_held(db_path: str | Path) -> bool:
    """True when this process already holds an exclusive lock for *db_path*."""
    with _registry_lock:
        held = _file_locks.get(_lock_path_for(db_path))
        return held is not None and any(held.modes)


def graph_lock_is_held(db_path: str | Path) -> bool:
    """True when this process holds a shared or exclusive lock for *db_path*."""
    with _registry_lock:
        return _lock_path_for(db_path) in _file_locks


def bind_store_read_lock(store: object, db_path: str | Path) -> None:
    """Record that *store* owes one shared-lock release on ``close()``."""
    key = _lock_path_for(db_path)
    sid = id(store)
    with _registry_lock:
        current = _store_read_binds.get(sid)
        if current is None or current[2] is not store:
            _store_read_binds[sid] = (key, 1, store)
        else:
            _store_read_binds[sid] = (current[0], current[1] + 1, store)


def unbind_store_read_lock(store: object) -> None:
    """Release one shared lock previously bound to *store*."""
    sid = id(store)
    with _registry_lock:
        current = _store_read_binds.get(sid)
        if current is None or current[2] is not store:
            return
        key, depth, _bound = current
        if depth <= 1:
            _store_read_binds.pop(sid, None)
        else:
            _store_read_binds[sid] = (key, depth - 1, store)
    release_graph_lock(key)


def drop_store_read_locks(store: object) -> None:
    """Release every shared lock bound to *store* (used by ``_force_close``)."""
    while True:
        with _registry_lock:
            current = _store_read_binds.get(id(store))
            if current is None or current[2] is not store:
                return
        unbind_store_read_lock(store)


class _CloseableStore(Protocol):
    """The narrow shape ``wrap_store_close_to_unbind`` needs from a store.

    Both the Python ``GraphStore`` and the PyO3 native store expose ``close``;
    only the Python one accepts the attribute assignment (the native store
    raises and the wrapper falls back to a warning).
    """

    close: Callable[..., object]


def wrap_store_close_to_unbind(store: _CloseableStore) -> None:
    """Ensure a non-Python GraphStore still releases a bound read lock."""
    inner_close = store.close

    def close(*args: object, **kwargs: object) -> object:
        try:
            return inner_close(*args, **kwargs)
        finally:
            unbind_store_read_lock(store)

    try:
        store.close = close  # type: ignore[method-assign]
    except (AttributeError, TypeError):  # pragma: no cover - native store
        logger.warning("GraphStore.close could not be wrapped; read lock may leak")


def _reset_for_tests() -> None:
    """Drop all lock state. Test helper only."""
    with _registry_lock:
        for held in list(_file_locks.values()):
            try:
                held.handle.close()
            except OSError:
                pass
        _file_locks.clear()
        _thread_locks.clear()
        _store_read_binds.clear()


__all__ = [
    "DEFAULT_IO_LOCK_TIMEOUT",
    "DEFAULT_WRITE_LOCK_TIMEOUT",
    "WriteLockUnavailableError",
    "acquire_graph_lock",
    "bind_store_read_lock",
    "drop_store_read_locks",
    "graph_lock_is_held",
    "graph_read_lock",
    "graph_write_lock",
    "release_graph_lock",
    "unbind_store_read_lock",
    "wrap_store_close_to_unbind",
    "write_lock_is_held",
]
