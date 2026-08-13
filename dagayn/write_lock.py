"""Cross-process serialization for graph writes.

Before this existed, the only mutual exclusion was a flock taken by
hook-triggered ``dagayn update`` runs (``DAGAYN_HOOK_UPDATE=1``). A hand-run
``dagayn update``, a ``dagayn build``, and the daemon's ``dagayn watch`` child
all wrote the same ``graph.db`` with nothing between them, and because a batch
holds ``BEGIN IMMEDIATE`` for up to 500 files, the second writer exhausted
``busy_timeout`` and failed outright.

Two properties the old lock did not have:

* It is keyed on the **resolved database path**, not the repository root, so two
  worktrees with separate graphs never block each other while two processes on
  one graph always do.
* It is acquired **before** the store is opened, so schema migrations (which
  write) happen inside it too.

Reentrant within a process: a build holds the lock and then opens a store that
may migrate, and that inner acquisition must not deadlock.
"""

from __future__ import annotations

import logging
import os
import threading
from collections.abc import Iterator
from contextlib import contextmanager
from pathlib import Path
from typing import IO

logger = logging.getLogger(__name__)

#: How long a writer waits for another process before giving up. Longer than
#: SQLite's own ``busy_timeout`` (5 s): a large batch legitimately takes longer
#: than that, and failing early is what this lock exists to prevent.
DEFAULT_WRITE_LOCK_TIMEOUT = float(os.environ.get("DAGAYN_WRITE_LOCK_TIMEOUT", "120"))

_registry_lock = threading.Lock()
#: One reentrant lock per database path, for threads inside this process.
_thread_locks: dict[Path, threading.RLock] = {}
#: Per-path flock handle plus the recursion depth that holds it.
_file_locks: dict[Path, tuple[IO[str], int]] = {}


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


def _flock_acquire(key: Path, *, blocking: bool, timeout: float) -> IO[str] | None:
    """Take the OS-level lock, or return ``None`` when it is already held here."""
    with _registry_lock:
        held = _file_locks.get(key)
        if held is not None:
            handle, depth = held
            _file_locks[key] = (handle, depth + 1)
            return None

    lock_file_path = key.with_name(f"{key.name}.write.lock")
    lock_file_path.parent.mkdir(parents=True, exist_ok=True)
    handle = lock_file_path.open("a+", encoding="utf-8")
    try:
        import fcntl
    except ImportError:  # pragma: no cover - POSIX only in practice
        # No advisory locking available: proceed unserialized rather than
        # refusing to write at all.
        logger.debug("fcntl unavailable; graph writes are not serialized")
        handle.close()
        return None

    deadline = None if not blocking else (timeout if timeout > 0 else None)
    waited = 0.0
    interval = 0.05
    while True:
        try:
            fcntl.flock(handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
            break
        except BlockingIOError:
            if not blocking:
                handle.close()
                raise WriteLockUnavailableError(
                    f"another process is writing {key} (lock: {lock_file_path})"
                ) from None
            if deadline is not None and waited >= deadline:
                handle.close()
                raise WriteLockUnavailableError(
                    f"timed out after {timeout:g}s waiting to write {key} (lock: {lock_file_path})"
                ) from None
            threading.Event().wait(interval)
            waited += interval
            interval = min(interval * 2, 0.5)
        except OSError as exc:
            # e.g. a filesystem without locking support.
            logger.debug("Could not flock %s (%s); proceeding unserialized", lock_file_path, exc)
            handle.close()
            return None

    try:
        handle.seek(0)
        handle.truncate()
        handle.write(f"{os.getpid()}\n")
        handle.flush()
    except OSError:  # pragma: no cover - diagnostics only
        pass

    with _registry_lock:
        _file_locks[key] = (handle, 1)
    return handle


def _flock_release(key: Path) -> None:
    with _registry_lock:
        held = _file_locks.get(key)
        if held is None:
            return
        handle, depth = held
        if depth > 1:
            _file_locks[key] = (handle, depth - 1)
            return
        _file_locks.pop(key, None)
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


@contextmanager
def graph_write_lock(
    db_path: str | Path,
    *,
    blocking: bool = True,
    timeout: float = DEFAULT_WRITE_LOCK_TIMEOUT,
) -> Iterator[None]:
    """Serialize writes to the graph at *db_path* across processes and threads.

    With ``blocking=False`` a lock already held elsewhere raises
    :class:`WriteLockUnavailableError` immediately, which is what the hook-update
    path wants: overlapping hook runs should skip, not queue.
    """
    key = _lock_path_for(db_path)
    thread_lock = _thread_lock_for(key)
    if not thread_lock.acquire(blocking=blocking, timeout=timeout if blocking else -1):
        raise WriteLockUnavailableError(f"another thread is writing {key}")
    try:
        _flock_acquire(key, blocking=blocking, timeout=timeout)
        try:
            yield
        finally:
            _flock_release(key)
    finally:
        thread_lock.release()


def write_lock_is_held(db_path: str | Path) -> bool:
    """True when this process already holds the write lock for *db_path*."""
    with _registry_lock:
        return _lock_path_for(db_path) in _file_locks


def _reset_for_tests() -> None:
    """Drop all lock state. Test helper only."""
    with _registry_lock:
        for handle, _depth in list(_file_locks.values()):
            try:
                handle.close()
            except OSError:
                pass
        _file_locks.clear()
        _thread_locks.clear()


__all__ = [
    "DEFAULT_WRITE_LOCK_TIMEOUT",
    "WriteLockUnavailableError",
    "graph_write_lock",
    "write_lock_is_held",
]
