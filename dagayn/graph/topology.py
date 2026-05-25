from __future__ import annotations

from typing import Optional

from ._mixin_protocol import GraphStoreMixinProtocol
from .types import GraphNode


class GraphStoreTopologyMixin(GraphStoreMixinProtocol):
    def get_node_by_id(self, node_id: int) -> Optional[GraphNode]:
        """Fetch a single node by its integer primary key."""
        row = self._conn.execute("SELECT * FROM nodes WHERE id = ?", (node_id,)).fetchone()
        return self._row_to_node(row) if row else None

    def get_nodes_by_ids(self, node_ids: list[int]) -> dict[int, GraphNode]:
        """Batch-fetch nodes by their integer primary keys.

        Returns a mapping ``{node_id: GraphNode}`` for ids that exist.
        """
        result: dict[int, GraphNode] = {}
        if not node_ids:
            return result
        unique_ids = list(dict.fromkeys(node_ids))
        batch_size = 450
        for i in range(0, len(unique_ids), batch_size):
            batch = unique_ids[i : i + batch_size]
            placeholders = ",".join("?" for _ in batch)
            rows = self._conn.execute(  # nosec B608
                f"SELECT * FROM nodes WHERE id IN ({placeholders})",
                batch,
            ).fetchall()
            for row in rows:
                result[row["id"]] = self._row_to_node(row)
        return result

    def get_nodes_by_kind(
        self,
        kinds: list[str],
        file_pattern: str | None = None,
    ) -> list[GraphNode]:
        """Return nodes matching any of *kinds*, optionally filtered by file.

        Args:
            kinds: List of node kind strings (e.g. ``["Function", "Test"]``).
            file_pattern: If provided, only nodes whose ``file_path``
                contains *file_pattern* (SQL LIKE ``%pattern%``) are
                returned.
        """
        if not kinds:
            return []
        placeholders = ",".join("?" for _ in kinds)
        conditions = [f"kind IN ({placeholders})"]
        params: list[str] = list(kinds)
        if file_pattern:
            conditions.append("file_path LIKE ?")
            params.append(f"%{file_pattern}%")
        where = " AND ".join(conditions)
        rows = self._conn.execute(  # nosec B608
            f"SELECT * FROM nodes WHERE {where}",
            params,
        ).fetchall()
        return [self._row_to_node(r) for r in rows]
