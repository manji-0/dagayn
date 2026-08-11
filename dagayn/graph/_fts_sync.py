"""Incremental FTS5 index maintenance and staleness tracking."""

from __future__ import annotations

import logging
import time
from pathlib import Path
from typing import Any

from ._fts_content import build_fts_insert_row

logger = logging.getLogger(__name__)

FTS_COUNT_KEY = "fts_indexed_node_count"
FTS_BUILT_AT_KEY = "fts_indexed_at"
_FTS_DDL = """
    CREATE VIRTUAL TABLE IF NOT EXISTS nodes_fts USING fts5(
        name, qualified_name, file_path, signature, identifier_tokens, doc_text,
        tokenize='porter unicode61'
    )
"""
_FTS_DELETE_SQL = "DELETE FROM nodes_fts WHERE rowid = ?"
_FTS_INSERT_SQL = (
    "INSERT INTO nodes_fts(rowid, name, qualified_name, file_path, signature, "
    "identifier_tokens, doc_text) VALUES (?, ?, ?, ?, ?, ?, ?)"
)


def _table_exists(conn: Any, table: str) -> bool:
    row = conn.execute(
        "SELECT count(*) FROM sqlite_master WHERE type IN ('table', 'view') AND name = ?",
        (table,),
    ).fetchone()
    return bool(row[0] if isinstance(row, tuple) else row[0])


def ensure_fts_table(conn: Any) -> None:
    """Create the FTS virtual table when it is missing."""
    if not _table_exists(conn, "nodes_fts"):
        conn.execute(_FTS_DDL)


def _metadata_value(conn: Any, key: str) -> str | None:
    row = conn.execute("SELECT value FROM metadata WHERE key = ?", (key,)).fetchone()
    if row is None:
        return None
    return row[0] if isinstance(row, tuple) else row["value"]


def set_fts_watermark(conn: Any, *, node_count: int | None = None) -> None:
    """Persist the FTS row-count watermark after a successful sync/rebuild."""
    if node_count is None:
        if not _table_exists(conn, "nodes_fts"):
            node_count = 0
        else:
            row = conn.execute("SELECT count(*) FROM nodes_fts").fetchone()
            node_count = int(row[0] if isinstance(row, tuple) else row[0])
    conn.execute(
        "INSERT OR REPLACE INTO metadata (key, value) VALUES (?, ?)",
        (FTS_COUNT_KEY, str(node_count)),
    )
    conn.execute(
        "INSERT OR REPLACE INTO metadata (key, value) VALUES (?, ?)",
        (FTS_BUILT_AT_KEY, str(time.time())),
    )


def fts_index_counts(conn: Any) -> tuple[int, int]:
    """Return ``(nodes_count, fts_count)``."""
    nodes_row = conn.execute("SELECT count(*) FROM nodes").fetchone()
    nodes_count = int(nodes_row[0] if isinstance(nodes_row, tuple) else nodes_row[0])
    if not _table_exists(conn, "nodes_fts"):
        return nodes_count, 0
    fts_row = conn.execute("SELECT count(*) FROM nodes_fts").fetchone()
    fts_count = int(fts_row[0] if isinstance(fts_row, tuple) else fts_row[0])
    return nodes_count, fts_count


def fts_index_health(conn: Any) -> dict[str, Any]:
    """Return FTS sync health metadata for search callers."""
    nodes_count, fts_count = fts_index_counts(conn)
    watermark_raw = _metadata_value(conn, FTS_COUNT_KEY)
    watermark = int(watermark_raw) if watermark_raw is not None else None
    stale = nodes_count != fts_count or (watermark is not None and watermark != fts_count)
    status = "stale" if stale else "synced"
    return {
        "status": status,
        "nodes_count": nodes_count,
        "fts_count": fts_count,
        "watermark_count": watermark,
    }


def delete_fts_for_node_ids(conn: Any, node_ids: list[int]) -> int:
    """Delete FTS rows for the given node rowids."""
    if not node_ids or not _table_exists(conn, "nodes_fts"):
        return 0
    deleted = 0
    for node_id in node_ids:
        conn.execute(_FTS_DELETE_SQL, (node_id,))
        deleted += 1
    return deleted


def delete_fts_for_file_paths(conn: Any, file_paths: list[str]) -> int:
    """Delete FTS rows for all nodes in the given files."""
    if not file_paths or not _table_exists(conn, "nodes_fts"):
        return 0
    placeholders = ",".join("?" for _ in file_paths)
    rows = conn.execute(
        f"SELECT n.id FROM nodes n "
        f"INNER JOIN nodes_fts fts ON fts.rowid = n.id "
        f"WHERE n.file_path IN ({placeholders})",
        tuple(file_paths),
    ).fetchall()
    node_ids = [row[0] if isinstance(row, tuple) else row["id"] for row in rows]
    return delete_fts_for_node_ids(conn, node_ids)


def upsert_fts_for_node_ids(
    conn: Any,
    node_ids: list[int],
    repo_root: Path | None,
) -> int:
    """Rebuild FTS rows for the given node ids."""
    if not node_ids:
        return 0
    ensure_fts_table(conn)
    placeholders = ",".join("?" for _ in node_ids)
    rows = conn.execute(
        "SELECT rowid AS node_rowid, kind, name, qualified_name, file_path, line_start, "
        "line_end, signature, extra FROM nodes "
        f"WHERE rowid IN ({placeholders})",
        tuple(node_ids),
    ).fetchall()
    file_lines_cache: dict[Path, list[str] | None] = {}
    indexed = 0
    for row in rows:
        node_rowid = row["node_rowid"] if hasattr(row, "keys") else row[0]
        conn.execute(_FTS_DELETE_SQL, (node_rowid,))
        conn.execute(
            _FTS_INSERT_SQL,
            build_fts_insert_row(node_rowid, row, repo_root, file_lines_cache),
        )
        indexed += 1
    return indexed


def sync_fts_for_file_paths(
    conn: Any,
    file_paths: list[str],
    repo_root: Path | None,
) -> int:
    """Replace FTS rows for all nodes in the given files."""
    if not file_paths:
        return 0
    delete_fts_for_file_paths(conn, file_paths)
    placeholders = ",".join("?" for _ in file_paths)
    rows = conn.execute(
        "SELECT rowid AS node_rowid, kind, name, qualified_name, file_path, line_start, "
        "line_end, signature, extra FROM nodes "
        f"WHERE file_path IN ({placeholders})",
        tuple(file_paths),
    ).fetchall()
    if not rows:
        return 0
    ensure_fts_table(conn)
    file_lines_cache: dict[Path, list[str] | None] = {}
    for row in rows:
        node_rowid = row["node_rowid"] if hasattr(row, "keys") else row[0]
        conn.execute(
            _FTS_INSERT_SQL,
            build_fts_insert_row(node_rowid, row, repo_root, file_lines_cache),
        )
    set_fts_watermark(conn)
    return len(rows)


def rebuild_fts_index_tx(conn: Any, repo_root: Path | None) -> int:
    """Rebuild the FTS index inside an existing transaction."""
    conn.execute("DROP TABLE IF EXISTS nodes_fts")
    conn.execute(_FTS_DDL)
    rows = conn.execute(
        "SELECT rowid AS node_rowid, kind, name, qualified_name, file_path, line_start, "
        "line_end, signature, extra FROM nodes"
    ).fetchall()
    file_lines_cache: dict[Path, list[str] | None] = {}
    if rows:
        conn.executemany(
            _FTS_INSERT_SQL,
            [
                build_fts_insert_row(row["node_rowid"], row, repo_root, file_lines_cache)
                for row in rows
            ],
        )
    count = len(rows)
    set_fts_watermark(conn, node_count=count)
    return count


def sync_fts_after_node_write(
    conn: Any,
    *,
    deleted_node_ids: list[int] | None = None,
    upserted_node_id: int | None = None,
    file_paths: list[str] | None = None,
    repo_root: Path | None,
) -> None:
    """Maintain FTS rows for a node write path."""
    if deleted_node_ids:
        delete_fts_for_node_ids(conn, deleted_node_ids)
    if file_paths:
        sync_fts_for_file_paths(conn, file_paths, repo_root)
        return
    if upserted_node_id is not None:
        upsert_fts_for_node_ids(conn, [upserted_node_id], repo_root)
    set_fts_watermark(conn)
