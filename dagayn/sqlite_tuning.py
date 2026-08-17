"""Shared SQLite WAL tuning for every graph connection.

``journal_mode=WAL`` alone lets ``graph.db-wal`` grow without bound: SQLite's
auto-checkpoint only *copies* pages back into the main database, it never
shrinks the WAL file, and a long-running write transaction (a full parse of a
large monorepo) blocks checkpointing entirely. A 514 MB graph was observed
with a 9.1 GB WAL, at which point every read and write paged through those
9 GB and an incremental update stopped making progress.

``journal_size_limit`` makes SQLite truncate the WAL back down after each
checkpoint, which bounds the file at the cost of re-growing it on the next
large transaction.

The Rust backend (``crates/dagayn-graph/src/core.rs``) sets the same limit; the
two must stay in sync because they take turns writing the same file.
"""

from __future__ import annotations

import logging
import sqlite3

logger = logging.getLogger(__name__)

#: Upper bound for ``graph.db-wal`` after a checkpoint. Large enough that an
#: ordinary incremental update never hits it, small enough that a runaway
#: transaction cannot fill the disk.
WAL_SIZE_LIMIT_BYTES = 256 * 1024 * 1024


def apply_wal_size_limit(conn: sqlite3.Connection) -> None:
    """Bound the WAL file for *conn*.

    Safe to call on any connection: a failure here only means the WAL keeps
    the SQLite default (unbounded) behaviour, so it is logged and swallowed
    rather than aborting the caller's work.
    """
    try:
        conn.execute(f"PRAGMA journal_size_limit={WAL_SIZE_LIMIT_BYTES}")
    except sqlite3.Error:
        logger.debug("Could not set journal_size_limit", exc_info=True)
