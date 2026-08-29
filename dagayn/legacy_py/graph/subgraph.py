from __future__ import annotations

from ._mixin_protocol import GraphStoreMixinProtocol
from .types import GraphEdge, GraphNode


class GraphStoreSubgraphMixin(GraphStoreMixinProtocol):
    def get_outgoing_targets(
        self,
        source_qns: list[str],
    ) -> list[str]:
        """Return ``target_qualified`` for edges sourced from *source_qns*."""
        results: list[str] = []
        batch_size = 450
        for i in range(0, len(source_qns), batch_size):
            batch = source_qns[i : i + batch_size]
            placeholders = ",".join("?" for _ in batch)
            rows = self._conn.execute(  # nosec B608
                f"SELECT target_qualified FROM edges WHERE source_qualified IN ({placeholders})",
                batch,
            ).fetchall()
            results.extend(r["target_qualified"] for r in rows)
        return results

    def get_incoming_sources(
        self,
        target_qns: list[str],
    ) -> list[str]:
        """Return ``source_qualified`` for edges targeting *target_qns*."""
        results: list[str] = []
        batch_size = 450
        for i in range(0, len(target_qns), batch_size):
            batch = target_qns[i : i + batch_size]
            placeholders = ",".join("?" for _ in batch)
            rows = self._conn.execute(  # nosec B608
                f"SELECT source_qualified FROM edges WHERE target_qualified IN ({placeholders})",
                batch,
            ).fetchall()
            results.extend(r["source_qualified"] for r in rows)
        return results

    def get_all_edges(self) -> list[GraphEdge]:
        """Return all edges in the graph."""
        rows = self._conn.execute("SELECT * FROM edges").fetchall()
        return [self._row_to_edge(r) for r in rows]

    def get_edges_among(self, qualified_names: set[str]) -> list[GraphEdge]:
        """Return edges where both source and target are in the given set.

        Batches the source-side IN clause to stay under SQLite's default
        SQLITE_MAX_VARIABLE_NUMBER limit, then filters targets in Python.
        """
        if not qualified_names:
            return []
        qns = list(qualified_names)
        results: list[GraphEdge] = []
        batch_size = 450  # Stay well under SQLite's default 999 limit
        for i in range(0, len(qns), batch_size):
            batch = qns[i : i + batch_size]
            placeholders = ",".join("?" for _ in batch)
            rows = self._conn.execute(  # nosec B608
                f"SELECT * FROM edges WHERE source_qualified IN ({placeholders})",
                batch,
            ).fetchall()
            for r in rows:
                edge = self._row_to_edge(r)
                if edge.target_qualified in qualified_names:
                    results.append(edge)
        return results

    def _batch_get_nodes(self, qualified_names: set[str]) -> list[GraphNode]:
        """Batch-fetch nodes by qualified name, staying under SQLite variable limits."""
        if not qualified_names:
            return []
        qns = list(qualified_names)
        results: list[GraphNode] = []
        batch_size = 450
        for i in range(0, len(qns), batch_size):
            batch = qns[i : i + batch_size]
            placeholders = ",".join("?" for _ in batch)
            rows = self._conn.execute(  # nosec B608
                f"SELECT * FROM nodes WHERE qualified_name IN ({placeholders})",
                batch,
            ).fetchall()
            results.extend(self._row_to_node(r) for r in rows)
        return results

    def get_local_subgraph(
        self,
        start_qn: str,
        max_depth: int,
    ) -> tuple[dict[str, "GraphNode"], dict[str, list[str]]]:
        """Return all nodes and a bidirectional adjacency dict reachable from
        *start_qn* within *max_depth* hops.

        Uses a single recursive CTE instead of one round-trip per node,
        reducing O(N) SQL calls to 3 (CTE + nodes batch + edges batch).

        Returns:
            nodes_map: qualified_name → GraphNode
            adj: qualified_name → [neighbor_qualified_names]
        """
        cte_sql = """
        WITH RECURSIVE reach(qn, depth) AS (
            SELECT ?, 0
            UNION
            SELECT e.target_qualified, r.depth + 1
            FROM reach r JOIN edges e ON e.source_qualified = r.qn
            WHERE r.depth < ?
            UNION
            SELECT e.source_qualified, r.depth + 1
            FROM reach r JOIN edges e ON e.target_qualified = r.qn
            WHERE r.depth < ?
        )
        SELECT DISTINCT qn FROM reach
        """
        rows = self._conn.execute(cte_sql, (start_qn, max_depth, max_depth)).fetchall()
        all_qns: set[str] = {r[0] for r in rows}

        nodes_map: dict[str, GraphNode] = {}
        for node in self._batch_get_nodes(all_qns):
            nodes_map[node.qualified_name] = node

        edges = self.get_edges_among(all_qns)
        adj: dict[str, list[str]] = {qn: [] for qn in all_qns}
        for e in edges:
            if e.source_qualified in adj:
                adj[e.source_qualified].append(e.target_qualified)
            if e.target_qualified in adj:
                adj[e.target_qualified].append(e.source_qualified)

        return nodes_map, adj
