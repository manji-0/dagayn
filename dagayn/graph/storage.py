from __future__ import annotations

import json
import time
from typing import TYPE_CHECKING

from ._edge_records import edge_identity_update_values, edge_insert_values
from ._mixin_protocol import GraphStoreMixinProtocol

if TYPE_CHECKING:
    from ..parser._base.types import EdgeInfo, NodeInfo


class GraphStoreStorageMixin(GraphStoreMixinProtocol):
    def upsert_node(self, node: NodeInfo, file_hash: str = "", mtime_ns: int = 0) -> int:
        """Insert or update a node. Returns the node ID."""
        now = time.time()
        qualified = self._make_qualified(node)
        extra = json.dumps(node.extra) if node.extra else "{}"

        row = self._conn.execute(
            """INSERT INTO nodes
               (kind, name, qualified_name, file_path, line_start, line_end,
                language, parent_name, params, return_type, modifiers, is_test,
                file_hash, mtime_ns, extra, updated_at)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
               ON CONFLICT(qualified_name) DO UPDATE SET
                 kind=excluded.kind, name=excluded.name,
                 file_path=excluded.file_path, line_start=excluded.line_start,
                 line_end=excluded.line_end, language=excluded.language,
                 parent_name=excluded.parent_name, params=excluded.params,
                 return_type=excluded.return_type, modifiers=excluded.modifiers,
                 is_test=excluded.is_test, file_hash=excluded.file_hash,
                 mtime_ns=excluded.mtime_ns,
                 extra=excluded.extra, updated_at=excluded.updated_at
               RETURNING id
            """,
            (
                node.kind,
                node.name,
                qualified,
                node.file_path,
                node.line_start,
                node.line_end,
                node.language,
                node.parent_name,
                node.params,
                node.return_type,
                node.modifiers,
                int(node.is_test),
                file_hash,
                mtime_ns,
                extra,
                now,
            ),
        ).fetchone()
        self._invalidate_cache()
        return row["id"]

    def upsert_edge(self, edge: EdgeInfo) -> int:
        """Insert or update an edge.

        Uses ``UPDATE … RETURNING id`` for the existing-edge path and
        ``INSERT … RETURNING id`` for inserts, so callers avoid a follow-up
        ``SELECT id``. Hot build paths still prefer ``store_file_batch`` /
        ``_bulk_insert_edges`` (delete-then-bulk) over per-row upserts.
        """
        now = time.time()

        # Identity includes line so multiple call sites are preserved.
        existing = self._conn.execute(
            """UPDATE edges
               SET target_name=?, extra=?, confidence=?, confidence_tier=?, updated_at=?
               WHERE kind=? AND source_qualified=? AND target_qualified=?
                     AND file_path=? AND line=?
               RETURNING id""",
            edge_identity_update_values(edge, now),
        ).fetchone()
        if existing:
            self._invalidate_cache()
            return existing["id"]

        row = self._conn.execute(
            """INSERT INTO edges
               (kind, source_qualified, target_qualified, target_name, file_path, line, extra,
                confidence, confidence_tier, updated_at)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
               RETURNING id""",
            edge_insert_values(edge, now),
        ).fetchone()
        self._invalidate_cache()
        return row["id"]

    def remove_file_data(self, file_path: str, *, invalidate: bool = True) -> None:
        """Remove all nodes and edges associated with a file."""
        normalized = self._normalize_file_path_key(file_path)
        keys = [file_path]
        if normalized != file_path:
            keys.append(normalized)
        placeholders = ",".join("?" for _ in keys)
        self._conn.execute(
            f"DELETE FROM nodes WHERE file_path IN ({placeholders})",
            tuple(keys),
        )
        self._conn.execute(
            f"DELETE FROM edges WHERE file_path IN ({placeholders})",
            tuple(keys),
        )
        if invalidate:
            self._invalidate_cache()
