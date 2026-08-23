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
import time
from collections.abc import Callable, Generator
from contextlib import contextmanager
from pathlib import Path
from typing import IO, Protocol, cast

logger = logging.getLogger(__name__)

#: How long a reader or writer waits for another process before giving up.
#: Longer than SQLite's own ``busy_timeout`` (5 s): a large batch legitimately
#: takes longer than that, and failing early is what this lock exists to prevent.
DEFAULT_WRITE_LOCK_TIMEOUT = float(os.environ.get("DAGAYN_WRITE_LOCK_TIMEOUT", "120"))
DEFAULT_IO_LOCK_TIMEOUT = DEFAULT_WRITE_LOCK_TIMEOUT

#: How long an *interactive* reader waits — the MCP tool entry point. A tool
#: call that inherits the 120 s budget goes silent for two minutes and then
#: fails anyway, which is the worst of both outcomes for an agent waiting on it;
#: reporting "a build is writing the graph" quickly lets the caller retry or
#: move on. Batch readers (`dagayn detect-changes` at commit time, enrichment)
#: keep the long timeout, because for them waiting *is* the useful behaviour.
DEFAULT_READ_LOCK_TIMEOUT = float(os.environ.get("DAGAYN_READ_LOCK_TIMEOUT", "10"))

_registry_lock = threading.Lock()


class _RWThreadLock:
    """Reader/writer lock over one database path, for threads in this process.

    This used to be a plain ``RLock``, which made two concurrent *read* calls
    on the same graph serialize even though the flock they take is shared and
    two separate processes read in parallel. For a long-lived ``dagayn serve``
    that meant tool calls never overlapped, and with the short interactive read
    timeout the second call could fail rather than wait its turn.

    The flock is per process — one file handle per path — so something has to
    keep a reader and a writer inside the same process from believing they hold
    that one handle in different modes. That is this lock's job: readers run
    together, a writer waits until every *other* thread's shared hold is gone.

    Reentrancy matches what the callers rely on: nesting the same mode, taking
    a read inside our own write, and upgrading our own read to a write. Two
    threads that each hold a read and then both ask to upgrade cannot both win;
    they wait, and the caller's timeout turns that into
    :class:`WriteLockUnavailableError` rather than a hang.

    Waiters are served in arrival order, with consecutive readers granted as a
    batch. Mode preference in either direction starves the other side under
    load — measured on an 8 s hammer with 6 readers and 3 writers: reader
    preference gave 6783 reader acquisitions to 3 writer ones, writer
    preference gave 12 to 904. FIFO keeps both sides moving, and it means the
    queue worker's update cannot be shut out by a busy MCP server.
    """

    class _Waiter:
        __slots__ = ("exclusive", "granted", "tid")

        def __init__(self, tid: int, exclusive: bool) -> None:
            self.tid = tid
            self.exclusive = exclusive
            self.granted = False

    __slots__ = ("_cond", "_queue", "_shared", "_stacks", "_writer_count", "_writer_tid")

    def __init__(self) -> None:
        self._cond = threading.Condition()
        #: thread id → number of shared holds
        self._shared: dict[int, int] = {}
        self._writer_tid: int | None = None
        self._writer_count = 0
        #: Waiters in arrival order.
        self._queue: list[_RWThreadLock._Waiter] = []
        #: thread id → modes granted, newest last, so ``release`` knows which
        #: acquisition it is undoing (callers release without naming a mode).
        self._stacks: dict[int, list[bool]] = {}

    def acquire(self, *, exclusive: bool, blocking: bool, timeout: float) -> bool:
        tid = threading.get_ident()
        deadline: float | None = None
        if blocking and timeout > 0:
            deadline = time.monotonic() + timeout
        with self._cond:
            upgrading = exclusive and tid in self._shared
            if self._writer_tid == tid or (tid in self._shared and not exclusive):
                # Nesting on a hold we already have must never queue: a waiter
                # ahead of us is waiting for that very hold to go away. An
                # upgrade is not in this set — it has to wait for the *other*
                # readers, so it goes through the queue below.
                self._take(tid, exclusive)
                return True
            waiter = _RWThreadLock._Waiter(tid, exclusive)
            if upgrading:
                # Jump the queue: we are still holding the shared lock the
                # waiters ahead of us are waiting to see gone, so taking our
                # turn in order would deadlock against them. (Two threads
                # upgrading at once still cannot both win; they time out.)
                self._queue.insert(0, waiter)
            else:
                self._queue.append(waiter)
            self._dispatch()
            while not waiter.granted:
                if not blocking:
                    self._drop(waiter)
                    return False
                remaining = None
                if deadline is not None:
                    remaining = deadline - time.monotonic()
                    if remaining <= 0:
                        self._drop(waiter)
                        return False
                self._cond.wait(remaining)
            # ``_dispatch`` already accounted for the hold; record the mode so
            # ``release`` knows what to undo.
            self._stacks.setdefault(tid, []).append(exclusive)
            return True

    def _drop(self, waiter: _RWThreadLock._Waiter) -> None:
        """Give up a queued place, and let whoever we blocked move up."""
        try:
            self._queue.remove(waiter)
        except ValueError:  # pragma: no cover - granted between wait and drop
            return
        self._dispatch()

    def _dispatch(self) -> None:
        """Grant what the head of the queue is entitled to."""
        while self._queue:
            head = self._queue[0]
            if head.exclusive:
                # A writer needs the field to itself, except for its own
                # upgrade: our shared hold is the one it is replacing.
                others = [tid for tid in self._shared if tid != head.tid]
                if self._writer_tid is not None or others:
                    return
                self._writer_tid = head.tid
                self._writer_count += 1
            else:
                if self._writer_tid is not None:
                    return
                self._shared[head.tid] = self._shared.get(head.tid, 0) + 1
            self._queue.pop(0)
            head.granted = True
            self._cond.notify_all()

    def _take(self, tid: int, exclusive: bool) -> None:
        if exclusive:
            self._writer_tid = tid
            self._writer_count += 1
        else:
            self._shared[tid] = self._shared.get(tid, 0) + 1
        self._stacks.setdefault(tid, []).append(exclusive)

    def release(self) -> None:
        tid = threading.get_ident()
        with self._cond:
            stack = self._stacks.get(tid)
            if not stack:
                raise RuntimeError("release of a graph lock this thread does not hold")
            exclusive = stack.pop()
            if not stack:
                self._stacks.pop(tid, None)
            if exclusive:
                self._writer_count -= 1
                if self._writer_count <= 0:
                    self._writer_count = 0
                    self._writer_tid = None
            else:
                remaining = self._shared.get(tid, 0) - 1
                if remaining > 0:
                    self._shared[tid] = remaining
                else:
                    self._shared.pop(tid, None)
            self._dispatch()
            self._cond.notify_all()


#: One reader/writer lock per database path, for threads inside this process.
_thread_locks: dict[Path, _RWThreadLock] = {}


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


def _thread_lock_for(key: Path) -> _RWThreadLock:
    with _registry_lock:
        lock = _thread_locks.get(key)
        if lock is None:
            lock = _RWThreadLock()
            _thread_locks[key] = lock
        return lock


def _lock_file_path(key: Path) -> Path:
    return key.with_name(f"{key.name}.write.lock")


#: Longest gap between ``LOCK_NB`` retries while waiting for the file lock.
#: This is also the smallest lock gap a waiter can reliably notice, so a writer
#: that releases and re-takes the lock (an embedding pass between slices) has to
#: stay out at least this long for the handoff to happen.
_MAX_POLL_INTERVAL = 0.1


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
            interval = min(interval * 2, _MAX_POLL_INTERVAL)
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


def _flock_release_then_exclusive(
    handle: IO[str],
    *,
    blocking: bool,
    timeout: float,
    key: Path,
) -> None:
    """Drop this fd's shared flock, then take exclusive with a timeout.

    Same-fd ``LOCK_EX | LOCK_NB`` EAGAIN's against our own ``LOCK_SH`` on
    Darwin and Linux. A blocking ``LOCK_EX`` waits forever for *other*
    holders, which is what wedged MCP auto-prepare behind leaked readers.
    Unlocking first lets ``_flock_wait`` poll with the caller's timeout.
    """
    try:
        import fcntl
    except ImportError:  # pragma: no cover - POSIX only in practice
        return
    fcntl.flock(handle.fileno(), fcntl.LOCK_UN)
    try:
        _flock_wait(
            handle,
            exclusive=True,
            blocking=blocking,
            timeout=timeout,
            key=key,
        )
    except BaseException:
        try:
            restore_timeout = timeout if timeout > 0 else DEFAULT_IO_LOCK_TIMEOUT
            _flock_wait(
                handle,
                exclusive=False,
                blocking=True,
                timeout=restore_timeout,
                key=key,
            )
        except Exception:
            logger.warning("Could not restore shared graph lock after a failed exclusive wait")
        raise


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
                _flock_release_then_exclusive(
                    handle,
                    blocking=blocking,
                    timeout=timeout,
                    key=key,
                )
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
    if not thread_lock.acquire(exclusive=exclusive, blocking=blocking, timeout=timeout):
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


def lock_holder_pid(db_path: str | Path) -> int | None:
    """PID recorded in the lock file, for diagnostics only.

    Whoever takes the lock writes its pid there (:func:`_write_pid`). It is a
    hint, not a fact: the writer may have exited, and a shared lock has several
    holders of which only the last one is recorded.
    """
    try:
        text = _lock_file_path(_lock_path_for(db_path)).read_text(encoding="utf-8")
    except OSError:
        return None
    try:
        return int(text.strip().splitlines()[0])
    except (IndexError, ValueError):
        return None


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
    """The narrow shape ``wrap_store_close_to_unbind`` needs from a store."""

    close: Callable[..., object]


class _ReadLockBoundStore:
    """Proxy that always unbinds the shared flock on ``close()``.

    PyO3 ``GraphStore.close`` cannot be assigned, so monkey-patching leaked
    the shared lock for the life of ``dagayn serve``. Bind the lock to this
    proxy, not the native object.
    """

    __slots__ = ("_inner",)

    def __init__(self, inner: object) -> None:
        object.__setattr__(self, "_inner", inner)

    def close(self, *args: object, **kwargs: object) -> object:
        inner = object.__getattribute__(self, "_inner")
        try:
            close = getattr(inner, "close")
            return close(*args, **kwargs)
        finally:
            unbind_store_read_lock(self)

    def _force_close(self, *args: object, **kwargs: object) -> object:
        inner = object.__getattribute__(self, "_inner")
        try:
            force = getattr(inner, "_force_close", None)
            if callable(force):
                return force(*args, **kwargs)
            close = getattr(inner, "close")
            return close(*args, **kwargs)
        finally:
            drop_store_read_locks(self)

    def __getattr__(self, name: str) -> object:
        return getattr(object.__getattribute__(self, "_inner"), name)

    def __setattr__(self, name: str, value: object) -> None:
        setattr(object.__getattribute__(self, "_inner"), name, value)


def wrap_store_close_to_unbind(store: object) -> _CloseableStore:
    """Ensure ``store.close()`` releases a bound read lock.

    Returns *store* when ``close`` can be patched, otherwise a proxy whose
    ``close`` always unbinds. Callers must bind the lock to the returned object.
    """
    inner_close = getattr(store, "close", None)
    if not callable(inner_close):
        return _ReadLockBoundStore(store)

    def close(*args: object, **kwargs: object) -> object:
        try:
            return inner_close(*args, **kwargs)
        finally:
            unbind_store_read_lock(store)

    try:
        store.close = close  # type: ignore[method-assign]
    except (AttributeError, TypeError):
        return _ReadLockBoundStore(store)

    inner_force = getattr(store, "_force_close", None)
    if callable(inner_force):

        def _force_close(*args: object, **kwargs: object) -> object:
            try:
                return inner_force(*args, **kwargs)
            finally:
                drop_store_read_locks(store)

        try:
            store._force_close = _force_close  # type: ignore[method-assign]
        except (AttributeError, TypeError):
            pass
    return cast(_CloseableStore, store)


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
