from __future__ import annotations

from typing import Any

from ._sql import BFS_ENGINE, MAX_IMPACT_DEPTH, MAX_IMPACT_NODES
from .types import GraphEdge


class GraphStoreImpactMixin:
    def get_impact_radius(
        self,
        changed_files: list[str],
        max_depth: int = MAX_IMPACT_DEPTH,
        max_nodes: int = MAX_IMPACT_NODES,
    ) -> dict[str, Any]:
        """BFS from changed files to find all impacted nodes within depth N.

        Delegates to ``get_impact_radius_sql()`` by default (faster for
        large graphs).  Set ``CRG_BFS_ENGINE=networkx`` to use the legacy
        Python-side BFS via NetworkX.

        Returns dict with:
          - changed_nodes: nodes in changed files
          - impacted_nodes: nodes reachable via edges
          - impacted_files: unique set of affected files
          - edges: connecting edges
        """
        if BFS_ENGINE == "networkx":
            return self._get_impact_radius_networkx(
                changed_files,
                max_depth=max_depth,
                max_nodes=max_nodes,
            )
        return self.get_impact_radius_sql(
            changed_files,
            max_depth=max_depth,
            max_nodes=max_nodes,
        )

    def get_impact_radius_sql(
        self,
        changed_files: list[str],
        max_depth: int = MAX_IMPACT_DEPTH,
        max_nodes: int = MAX_IMPACT_NODES,
    ) -> dict[str, Any]:
        """Impact radius via SQLite recursive CTE.

        Faster than NetworkX for large graphs because it avoids
        materialising the full graph in Python.
        """
        if not changed_files:
            return {
                "changed_nodes": [],
                "impacted_nodes": [],
                "impacted_files": [],
                "edges": [],
                "truncated": False,
                "total_impacted": 0,
            }

        seeds: set[str] = set()
        nodes_by_file = self.get_nodes_by_files(changed_files)
        for f in changed_files:
            nodes = nodes_by_file.get(f, [])
            for n in nodes:
                seeds.add(n.qualified_name)

        if not seeds:
            return {
                "changed_nodes": [],
                "impacted_nodes": [],
                "impacted_files": [],
                "edges": [],
                "truncated": False,
                "total_impacted": 0,
            }

        self._conn.execute("CREATE TEMP TABLE IF NOT EXISTS _impact_seeds (qn TEXT PRIMARY KEY)")
        self._conn.execute("DELETE FROM _impact_seeds")
        batch_size = 450
        seed_list = list(seeds)
        for i in range(0, len(seed_list), batch_size):
            batch = seed_list[i : i + batch_size]
            placeholders = ",".join("(?)" for _ in batch)
            self._conn.execute(  # nosec B608
                f"INSERT OR IGNORE INTO _impact_seeds (qn) VALUES {placeholders}",
                batch,
            )

        cte_sql = """
        WITH RECURSIVE impacted(node_qn, depth) AS (
            SELECT qn, 0 FROM _impact_seeds
            UNION
            SELECT e.target_qualified, i.depth + 1
            FROM impacted i
            JOIN edges e ON e.source_qualified = i.node_qn
            WHERE i.depth < ?
            UNION
            SELECT e.source_qualified, i.depth + 1
            FROM impacted i
            JOIN edges e ON e.target_qualified = i.node_qn
            WHERE i.depth < ?
        )
        SELECT DISTINCT node_qn, MIN(depth) AS min_depth
        FROM impacted
        GROUP BY node_qn
        LIMIT ?
        """
        rows = self._conn.execute(
            cte_sql,
            (max_depth, max_depth, max_nodes + len(seeds)),
        ).fetchall()

        impacted_qns: set[str] = set()
        for r in rows:
            qn = r[0]
            if qn not in seeds:
                impacted_qns.add(qn)

        changed_nodes = self._batch_get_nodes(seeds)
        impacted_nodes = self._batch_get_nodes(impacted_qns)

        total_impacted = len(impacted_nodes)
        truncated = total_impacted > max_nodes
        if truncated:
            impacted_nodes = impacted_nodes[:max_nodes]

        impacted_files = list({n.file_path for n in impacted_nodes})

        if impacted_nodes:
            impacted_qns_list = [n.qualified_name for n in impacted_nodes]
            for i in range(0, len(impacted_qns_list), batch_size):
                batch = impacted_qns_list[i : i + batch_size]
                placeholders = ",".join("(?)" for _ in batch)
                self._conn.execute(  # nosec B608
                    f"INSERT OR IGNORE INTO _impact_seeds (qn) VALUES {placeholders}",
                    batch,
                )

        relevant_edges: list[GraphEdge] = []
        if seeds or impacted_nodes:
            edge_rows = self._conn.execute("""
                SELECT e.* FROM edges e
                INNER JOIN _impact_seeds s ON e.source_qualified = s.qn
                INNER JOIN _impact_seeds t ON e.target_qualified = t.qn
            """).fetchall()
            relevant_edges = [self._row_to_edge(r) for r in edge_rows]

        return {
            "changed_nodes": changed_nodes,
            "impacted_nodes": impacted_nodes,
            "impacted_files": impacted_files,
            "edges": relevant_edges,
            "truncated": truncated,
            "total_impacted": total_impacted,
        }

    def _get_impact_radius_networkx(
        self,
        changed_files: list[str],
        max_depth: int = MAX_IMPACT_DEPTH,
        max_nodes: int = MAX_IMPACT_NODES,
    ) -> dict[str, Any]:
        """BFS via NetworkX (legacy). Used when CRG_BFS_ENGINE=networkx."""
        nxg = self._build_networkx_graph()

        seeds: set[str] = set()
        nodes_by_file = self.get_nodes_by_files(changed_files)
        for f in changed_files:
            nodes = nodes_by_file.get(f, [])
            for n in nodes:
                seeds.add(n.qualified_name)

        visited: set[str] = set()
        frontier = seeds.copy()
        depth = 0
        impacted: set[str] = set()

        while frontier and depth < max_depth:
            visited.update(frontier)
            next_frontier: set[str] = set()
            for qn in frontier:
                if qn in nxg:
                    for neighbor in nxg.neighbors(qn):
                        if neighbor not in visited:
                            next_frontier.add(neighbor)
                            impacted.add(neighbor)
                if qn in nxg:
                    for pred in nxg.predecessors(qn):
                        if pred not in visited:
                            next_frontier.add(pred)
                            impacted.add(pred)
            next_frontier -= visited
            if len(visited) + len(next_frontier) > max_nodes:
                break
            frontier = next_frontier
            depth += 1

        changed_nodes = self._batch_get_nodes(seeds)
        impacted_qns = impacted - seeds
        impacted_nodes = self._batch_get_nodes(impacted_qns)

        total_impacted = len(impacted_nodes)
        truncated = total_impacted > max_nodes
        if truncated:
            impacted_nodes = impacted_nodes[:max_nodes]

        impacted_files = list({n.file_path for n in impacted_nodes})

        relevant_edges: list[GraphEdge] = []
        all_qns = seeds | {n.qualified_name for n in impacted_nodes}
        if all_qns:
            relevant_edges = self.get_edges_among(all_qns)

        return {
            "changed_nodes": changed_nodes,
            "impacted_nodes": impacted_nodes,
            "impacted_files": impacted_files,
            "edges": relevant_edges,
            "truncated": truncated,
            "total_impacted": total_impacted,
        }
