from __future__ import annotations

import json
import logging
import time
from typing import TYPE_CHECKING

from ._edge_records import edge_insert_values
from ._mixin_protocol import GraphStoreMixinProtocol

if TYPE_CHECKING:
    from ..parser._base.types import EdgeInfo, NodeInfo

logger = logging.getLogger(__name__)


class GraphStoreStorageBatchMixin(GraphStoreMixinProtocol):
    def _bulk_insert_nodes(self, nodes: list[NodeInfo], fhash: str, mtime_ns: int = 0) -> None:
        """Bulk-insert nodes via executemany. Caller must have cleared the file first."""
        self._bulk_insert_nodes_with_meta(
            [(n, fhash, mtime_ns) for n in nodes],
        )

    def _bulk_insert_nodes_with_meta(
        self,
        nodes: list[tuple[NodeInfo, str, int]],
    ) -> None:
        """Bulk-insert nodes with per-node file hash and mtime metadata."""
        if not nodes:
            return
        now = time.time()
        self._conn.executemany(
            """INSERT INTO nodes
               (kind, name, qualified_name, file_path, line_start, line_end,
                language, parent_name, params, return_type, modifiers, is_test,
                file_hash, mtime_ns, extra, updated_at)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
               ON CONFLICT(qualified_name) DO UPDATE SET
                   kind=excluded.kind,
                   name=excluded.name,
                   file_path=excluded.file_path,
                   line_start=excluded.line_start,
                   line_end=excluded.line_end,
                   language=excluded.language,
                   parent_name=excluded.parent_name,
                   params=excluded.params,
                   return_type=excluded.return_type,
                   modifiers=excluded.modifiers,
                   is_test=excluded.is_test,
                   file_hash=excluded.file_hash,
                   mtime_ns=excluded.mtime_ns,
                   extra=excluded.extra,
                   updated_at=excluded.updated_at""",
            [
                (
                    n.kind,
                    n.name,
                    self._make_qualified(n),
                    n.file_path,
                    n.line_start,
                    n.line_end,
                    n.language,
                    n.parent_name,
                    n.params,
                    n.return_type,
                    n.modifiers,
                    int(n.is_test),
                    fhash,
                    mtime_ns,
                    json.dumps(n.extra) if n.extra else "{}",
                    now,
                )
                for n, fhash, mtime_ns in nodes
            ],
        )

    def _bulk_insert_edges(self, edges: list[EdgeInfo]) -> None:
        """Bulk-insert edges via executemany. Caller must have cleared the file first."""
        if not edges:
            return
        now = time.time()
        self._conn.executemany(
            """INSERT INTO edges
               (kind, source_qualified, target_qualified, target_name, file_path, line, extra,
                confidence, confidence_tier, updated_at)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)""",
            [edge_insert_values(e, now) for e in edges],
        )

    def remove_files_data(self, file_paths: list[str], *, invalidate: bool = True) -> None:
        """Remove graph data for multiple files in one store operation."""
        keys: list[str] = []
        seen: set[str] = set()
        for file_path in file_paths:
            for key in (file_path, self._normalize_file_path_key(file_path)):
                if key not in seen:
                    seen.add(key)
                    keys.append(key)
        for i in range(0, len(keys), 450):
            chunk = keys[i : i + 450]
            if not chunk:
                continue
            placeholders = ",".join("?" for _ in chunk)
            self.remove_node_keyed_rows_for_files(chunk)
            self._conn.execute(
                f"DELETE FROM nodes WHERE file_path IN ({placeholders})",
                chunk,
            )
            self._conn.execute(
                f"DELETE FROM edges WHERE file_path IN ({placeholders})",
                chunk,
            )
        if invalidate:
            self._invalidate_cache()

    def store_file_nodes_edges(
        self,
        file_path: str,
        nodes: list[NodeInfo],
        edges: list[EdgeInfo],
        fhash: str = "",
        mtime_ns: int = 0,
    ) -> None:
        """Atomically replace all data for a file."""
        if self._conn.in_transaction:
            logger.warning("Rolling back uncommitted transaction before BEGIN IMMEDIATE")
            self._conn.rollback()
        self._conn.execute("BEGIN IMMEDIATE")
        try:
            self.remove_file_data(file_path, invalidate=False)
            self._bulk_insert_nodes(nodes, fhash, mtime_ns)
            self._bulk_insert_edges(edges)
            self._conn.commit()
        except BaseException:
            self._conn.rollback()
            raise
        self._invalidate_cache()

    def store_file_batch(
        self, batch: list[tuple[str, list[NodeInfo], list[EdgeInfo], str, int]]
    ) -> None:
        """Atomically replace data for a batch of files in one transaction.

        Each tuple is ``(file_path, nodes, edges, fhash, mtime_ns)``.
        Pass ``mtime_ns=0`` when not available.
        """
        if self._conn.in_transaction:
            logger.warning("Rolling back uncommitted transaction before BEGIN IMMEDIATE")
            self._conn.rollback()
        self._conn.execute("BEGIN IMMEDIATE")
        try:
            file_paths: list[str] = []
            all_nodes: list[tuple[NodeInfo, str, int]] = []
            all_edges: list[EdgeInfo] = []
            for item in batch:
                if len(item) == 4:
                    file_path, nodes, edges, fhash = item
                    mtime_ns = 0
                else:
                    file_path, nodes, edges, fhash, mtime_ns = item
                file_paths.append(file_path)
                all_nodes.extend((node, fhash, mtime_ns) for node in nodes)
                all_edges.extend(edges)
            self.remove_files_data(file_paths, invalidate=False)
            self._bulk_insert_nodes_with_meta(all_nodes)
            self._bulk_insert_edges(all_edges)
            self._conn.commit()
        except BaseException:
            self._conn.rollback()
            raise
        self._invalidate_cache()
