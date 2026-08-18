from __future__ import annotations

from collections.abc import Mapping

from ..bridge_types import BridgeMissingnessRecord
from ..cross_artifact import (
    collect_bridge_transitions,
    is_low_confidence_bridge,
    is_reportable_bridge,
)
from ._mixin_protocol import GraphStoreMixinProtocol
from ._sql import BFS_ENGINE, MAX_IMPACT_DEPTH, MAX_IMPACT_NODES
from .types import GraphEdge, ImpactRadiusResult

# SQL predicate: expand through non-bridge edges and reportable CROSS_ARTIFACT only.
_REPORTABLE_BRIDGE_SQL = """
(
    e.kind != 'CROSS_ARTIFACT'
    OR (
        UPPER(COALESCE(e.confidence_tier, 'EXTRACTED')) IN ('EXACT', 'HIGH', 'EXTRACTED')
        AND e.target_qualified NOT LIKE '<unresolved:%'
        AND e.source_qualified NOT LIKE '<unresolved:%'
    )
)
"""


def _nx_edge_allows_impact(
    edge_data: Mapping[str, object] | None,
    source: str,
    target: str,
) -> bool:
    """Match SQL/_REPORTABLE_BRIDGE_SQL: skip non-reportable CROSS_ARTIFACT hops."""
    data = edge_data or {}
    if data.get("kind") != "CROSS_ARTIFACT":
        return True
    from types import SimpleNamespace

    edge = SimpleNamespace(
        kind="CROSS_ARTIFACT",
        source_qualified=source,
        target_qualified=target,
        confidence_tier=data.get("confidence_tier"),
        extra=data.get("extra") if isinstance(data.get("extra"), dict) else {},
        confidence=data.get("confidence", 1.0),
        file_path=data.get("file_path"),
        line=data.get("line"),
    )
    return is_reportable_bridge(edge)


class GraphStoreImpactMixin(GraphStoreMixinProtocol):
    def get_impact_radius(
        self,
        changed_files: list[str],
        max_depth: int = MAX_IMPACT_DEPTH,
        max_nodes: int = MAX_IMPACT_NODES,
    ) -> ImpactRadiusResult:
        """BFS from changed files to find all impacted nodes within depth N.

        Delegates to ``get_impact_radius_sql()`` by default (faster for
        large graphs).  Set ``CRG_BFS_ENGINE=networkx`` to use the legacy
        Python-side BFS via NetworkX.

        Returns dict with:
          - changed_nodes: nodes in changed files
          - impacted_nodes: nodes reachable via edges
          - impacted_files: unique set of affected files
          - edges: connecting edges
          - bridge_transitions: explainable reportable CROSS_ARTIFACT hops
          - low_confidence_bridges: caveats for non-reportable bridges
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
    ) -> ImpactRadiusResult:
        """Impact radius via SQLite recursive CTE.

        Faster than NetworkX for large graphs because it avoids
        materialising the full graph in Python.

        Reportable ``CROSS_ARTIFACT`` edges are traversed as first-class
        hops. Low-confidence bridges are omitted from expansion and returned
        as caveats instead of hard impact claims.
        """
        empty: ImpactRadiusResult = {
            "changed_nodes": [],
            "impacted_nodes": [],
            "impacted_files": [],
            "edges": [],
            "bridge_transitions": [],
            "low_confidence_bridges": [],
            "truncated": False,
            "total_impacted": 0,
        }
        if not changed_files:
            return empty

        seeds: set[str] = set()
        nodes_by_file = self.get_nodes_by_files(changed_files)
        for f in changed_files:
            nodes = nodes_by_file.get(f, [])
            for n in nodes:
                seeds.add(n.qualified_name)

        if not seeds:
            return empty

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

        cte_sql = f"""
        WITH RECURSIVE impacted(node_qn, depth) AS (
            SELECT qn, 0 FROM _impact_seeds
            UNION
            SELECT e.target_qualified, i.depth + 1
            FROM impacted i
            JOIN edges e ON e.source_qualified = i.node_qn
            WHERE i.depth < ?
              AND {_REPORTABLE_BRIDGE_SQL}
            UNION
            SELECT e.source_qualified, i.depth + 1
            FROM impacted i
            JOIN edges e ON e.target_qualified = i.node_qn
            WHERE i.depth < ?
              AND {_REPORTABLE_BRIDGE_SQL}
        ),
        aggregated AS (
            SELECT node_qn, MIN(depth) AS min_depth
            FROM impacted
            GROUP BY node_qn
        )
        """
        count_row = self._conn.execute(
            f"""
            {cte_sql}
            SELECT COUNT(*) FROM aggregated
            WHERE node_qn NOT IN (SELECT qn FROM _impact_seeds)
            """,
            (max_depth, max_depth),
        ).fetchone()
        total_impacted = int(count_row[0]) if count_row else 0

        rows = self._conn.execute(
            f"""
            {cte_sql}
            SELECT node_qn, min_depth FROM aggregated
            WHERE node_qn NOT IN (SELECT qn FROM _impact_seeds)
            ORDER BY min_depth, node_qn
            LIMIT ?
            """,
            (max_depth, max_depth, max_nodes),
        ).fetchall()

        impacted_qns = {r[0] for r in rows}
        depth_by_qn = {r[0]: r[1] for r in rows}

        changed_nodes = self._batch_get_nodes(seeds)
        impacted_nodes = self._batch_get_nodes(impacted_qns)
        impacted_nodes.sort(key=lambda n: (depth_by_qn.get(n.qualified_name, 0), n.qualified_name))

        truncated = total_impacted > max_nodes

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

        bridge_transitions, _ = collect_bridge_transitions(relevant_edges)
        low_confidence_bridges = self._low_confidence_bridges_near_seeds(seeds)

        return {
            "changed_nodes": changed_nodes,
            "impacted_nodes": impacted_nodes,
            "impacted_files": impacted_files,
            "edges": relevant_edges,
            "bridge_transitions": bridge_transitions,
            "low_confidence_bridges": low_confidence_bridges,
            "truncated": truncated,
            "total_impacted": total_impacted,
        }

    def _low_confidence_bridges_near_seeds(self, seeds: set[str]) -> list[BridgeMissingnessRecord]:
        """Collect low-confidence CROSS_ARTIFACT edges touching seed nodes as caveats."""
        if not seeds:
            return []
        seed_list = list(seeds)
        caveats: list[BridgeMissingnessRecord] = []
        seen: set[tuple[str, str]] = set()
        batch_size = 450
        for i in range(0, len(seed_list), batch_size):
            batch = seed_list[i : i + batch_size]
            placeholders = ",".join("?" for _ in batch)
            rows = self._conn.execute(  # nosec B608
                f"""
                SELECT * FROM edges
                WHERE kind = 'CROSS_ARTIFACT'
                  AND (source_qualified IN ({placeholders})
                       OR target_qualified IN ({placeholders}))
                """,
                (*batch, *batch),
            ).fetchall()
            for row in rows:
                edge = self._row_to_edge(row)
                if not is_low_confidence_bridge(edge):
                    continue
                key = (edge.source_qualified, edge.target_qualified)
                if key in seen:
                    continue
                seen.add(key)
                from ..cross_artifact import low_confidence_bridge_missingness

                caveats.append(low_confidence_bridge_missingness(edge))
        return caveats

    def _get_impact_radius_networkx(
        self,
        changed_files: list[str],
        max_depth: int = MAX_IMPACT_DEPTH,
        max_nodes: int = MAX_IMPACT_NODES,
    ) -> ImpactRadiusResult:
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
        impacted_depth: dict[str, int] = {}

        while frontier and depth < max_depth:
            visited.update(frontier)
            next_frontier: set[str] = set()
            next_depth = depth + 1
            for qn in frontier:
                if qn in nxg:
                    for neighbor in nxg.neighbors(qn):
                        if neighbor in visited or neighbor in seeds:
                            continue
                        edge_data = nxg.get_edge_data(qn, neighbor)
                        if not _nx_edge_allows_impact(edge_data, qn, neighbor):
                            continue
                        next_frontier.add(neighbor)
                        if neighbor not in impacted_depth:
                            impacted_depth[neighbor] = next_depth
                            impacted.add(neighbor)
                    for pred in nxg.predecessors(qn):
                        if pred in visited or pred in seeds:
                            continue
                        edge_data = nxg.get_edge_data(pred, qn)
                        if not _nx_edge_allows_impact(edge_data, pred, qn):
                            continue
                        next_frontier.add(pred)
                        if pred not in impacted_depth:
                            impacted_depth[pred] = next_depth
                            impacted.add(pred)
            next_frontier -= visited
            frontier = next_frontier
            depth += 1

        changed_nodes = self._batch_get_nodes(seeds)
        total_impacted = len(impacted)
        truncated = total_impacted > max_nodes
        limited_qns = sorted(impacted, key=lambda qn: (impacted_depth[qn], qn))[:max_nodes]
        impacted_nodes = self._batch_get_nodes(set(limited_qns))
        impacted_nodes.sort(key=lambda n: (impacted_depth[n.qualified_name], n.qualified_name))

        impacted_files = list({n.file_path for n in impacted_nodes})

        relevant_edges: list[GraphEdge] = []
        all_qns = seeds | {n.qualified_name for n in impacted_nodes}
        if all_qns:
            relevant_edges = self.get_edges_among(all_qns)

        # Reportable transitions as explainable paths; low-confidence as caveats.
        bridge_transitions, _ = collect_bridge_transitions(relevant_edges)

        return {
            "changed_nodes": changed_nodes,
            "impacted_nodes": impacted_nodes,
            "impacted_files": impacted_files,
            "edges": relevant_edges,
            "bridge_transitions": bridge_transitions,
            "low_confidence_bridges": self._low_confidence_bridges_near_seeds(seeds),
            "truncated": truncated,
            "total_impacted": total_impacted,
        }
