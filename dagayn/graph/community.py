from __future__ import annotations

import logging
import sqlite3

from .types import GraphNode

logger = logging.getLogger(__name__)


class GraphStoreCommunityMixin:
    def get_community_ids_by_node_ids(
        self,
        node_ids: list[int],
    ) -> dict[int, int | None]:
        """Batch-fetch ``community_id`` for a list of node ids."""
        result: dict[int, int | None] = {nid: None for nid in node_ids}
        if not node_ids:
            return result
        batch_size = 450
        for i in range(0, len(node_ids), batch_size):
            batch = node_ids[i : i + batch_size]
            placeholders = ",".join("?" for _ in batch)
            rows = self._conn.execute(  # nosec B608
                f"SELECT id, community_id FROM nodes WHERE id IN ({placeholders})",
                batch,
            ).fetchall()
            for r in rows:
                result[r["id"]] = r["community_id"]
        return result

    def get_node_community_id(self, node_id: int) -> int | None:
        """Return the ``community_id`` for a node, or ``None``."""
        row = self._conn.execute(
            "SELECT community_id FROM nodes WHERE id = ?",
            (node_id,),
        ).fetchone()
        if row and row["community_id"] is not None:
            return row["community_id"]
        return None

    def get_community_ids_by_qualified_names(
        self,
        qns: list[str],
    ) -> dict[str, int | None]:
        """Batch-fetch ``community_id`` for a list of qualified names.

        Returns a mapping from qualified name to community_id (may be
        ``None`` if the node has no assigned community).
        """
        result: dict[str, int | None] = {}
        batch_size = 450
        for i in range(0, len(qns), batch_size):
            batch = qns[i : i + batch_size]
            placeholders = ",".join("?" for _ in batch)
            rows = self._conn.execute(  # nosec B608
                "SELECT qualified_name, community_id FROM nodes "
                f"WHERE qualified_name IN ({placeholders})",
                batch,
            ).fetchall()
            for r in rows:
                result[r["qualified_name"]] = r["community_id"]
        return result

    def get_all_community_ids(self) -> dict[str, int | None]:
        """Return a mapping of *all* qualified names to their community_id.

        Used primarily by the visualization exporter.
        """
        try:
            rows = self._conn.execute("SELECT qualified_name, community_id FROM nodes").fetchall()
            return {r["qualified_name"]: r["community_id"] for r in rows}
        except sqlite3.OperationalError as exc:
            # community_id column may not exist yet on pre-v6 schemas
            logger.debug("Community IDs unavailable (schema not yet migrated): %s", exc)
            return {}

    def get_communities_list(
        self,
    ) -> list[sqlite3.Row]:
        """Return raw rows from the ``communities`` table."""
        try:
            return self._conn.execute("SELECT id, name FROM communities").fetchall()
        except sqlite3.OperationalError as exc:
            # communities table doesn't exist yet on pre-v4 schemas
            logger.debug("Communities list unavailable (table missing): %s", exc)
            return []

    def get_community_member_qns(
        self,
        community_id: int,
    ) -> list[str]:
        """Return qualified names of nodes in a community."""
        rows = self._conn.execute(
            "SELECT qualified_name FROM nodes WHERE community_id = ?",
            (community_id,),
        ).fetchall()
        return [r["qualified_name"] for r in rows]

    def get_all_community_member_qns(self) -> dict[int, list[str]]:
        """Return a mapping ``community_id -> [qualified_name, ...]`` for all
        nodes that have a non-NULL ``community_id``.

        Single-query alternative to calling :meth:`get_community_member_qns`
        in a loop over every community.
        """
        result: dict[int, list[str]] = {}
        rows = self._conn.execute(
            "SELECT community_id, qualified_name FROM nodes "
            "WHERE community_id IS NOT NULL "
            "ORDER BY community_id"
        ).fetchall()
        for r in rows:
            result.setdefault(r["community_id"], []).append(r["qualified_name"])
        return result

    def get_nodes_by_community_id(
        self,
        community_id: int,
    ) -> list[GraphNode]:
        """Return all nodes belonging to a community."""
        rows = self._conn.execute(
            "SELECT * FROM nodes WHERE community_id = ?",
            (community_id,),
        ).fetchall()
        return [self._row_to_node(r) for r in rows]
