from __future__ import annotations

from dagayn.bridge_types import BridgeTransitionRecord
from ._mixin_protocol import GraphStoreMixinProtocol
from .types import FlowAdjacency, GraphNode


class GraphStoreFlowMixin(GraphStoreMixinProtocol):
    def count_flow_memberships(self, node_id: int) -> int:
        """Return the number of flows a node participates in."""
        row = self._conn.execute(
            "SELECT COUNT(*) as cnt FROM flow_memberships WHERE node_id = ?",
            (node_id,),
        ).fetchone()
        return row["cnt"] if row else 0

    def count_flow_memberships_for_nodes(self, node_ids: list[int]) -> dict[int, int]:
        """Batch variant of :meth:`count_flow_memberships`.

        Returns a mapping from each input node id to its flow membership
        count. Node ids without memberships map to ``0``.
        """
        result: dict[int, int] = {nid: 0 for nid in node_ids}
        if not node_ids:
            return result
        batch_size = 450
        for i in range(0, len(node_ids), batch_size):
            batch = node_ids[i : i + batch_size]
            placeholders = ",".join("?" for _ in batch)
            rows = self._conn.execute(  # nosec B608
                "SELECT node_id, COUNT(*) as cnt FROM flow_memberships "
                f"WHERE node_id IN ({placeholders}) GROUP BY node_id",
                batch,
            ).fetchall()
            for r in rows:
                result[r["node_id"]] = r["cnt"]
        return result

    def get_flow_criticalities_for_node(self, node_id: int) -> list[float]:
        """Return criticality values for all flows a node participates in."""
        rows = self._conn.execute(
            "SELECT f.criticality FROM flows f "
            "JOIN flow_memberships fm ON fm.flow_id = f.id "
            "WHERE fm.node_id = ?",
            (node_id,),
        ).fetchall()
        return [r["criticality"] for r in rows]

    def get_flow_criticalities_for_nodes(
        self,
        node_ids: list[int],
    ) -> dict[int, list[float]]:
        """Batch variant of :meth:`get_flow_criticalities_for_node`."""
        result: dict[int, list[float]] = {nid: [] for nid in node_ids}
        if not node_ids:
            return result
        batch_size = 450
        for i in range(0, len(node_ids), batch_size):
            batch = node_ids[i : i + batch_size]
            placeholders = ",".join("?" for _ in batch)
            rows = self._conn.execute(  # nosec B608
                "SELECT fm.node_id as nid, f.criticality as crit FROM flows f "
                "JOIN flow_memberships fm ON fm.flow_id = f.id "
                f"WHERE fm.node_id IN ({placeholders})",
                batch,
            ).fetchall()
            for r in rows:
                result[r["nid"]].append(r["crit"])
        return result

    def get_node_ids_by_files(
        self,
        file_paths: list[str],
    ) -> set[int]:
        """Return node IDs belonging to the given file paths."""
        if not file_paths:
            return set()

        keys: set[str] = set()
        for file_path in file_paths:
            normalized = self._normalize_file_path_key(file_path)
            keys.add(file_path)
            if normalized != file_path:
                keys.add(normalized)

        result: set[int] = set()
        keys_list = list(keys)
        batch_size = 450
        for i in range(0, len(keys_list), batch_size):
            batch = keys_list[i : i + batch_size]
            placeholders = ",".join("?" for _ in batch)
            rows = self._conn.execute(  # nosec B608
                f"SELECT id FROM nodes WHERE file_path IN ({placeholders})",
                batch,
            ).fetchall()
            result.update(r["id"] for r in rows)
        return result

    def get_flow_ids_by_node_ids(
        self,
        node_ids: set[int],
    ) -> list[int]:
        """Return distinct flow IDs that contain any of *node_ids*."""
        if not node_ids:
            return []
        nids = list(node_ids)
        result: list[int] = []
        batch_size = 450
        for i in range(0, len(nids), batch_size):
            batch = nids[i : i + batch_size]
            placeholders = ",".join("?" for _ in batch)
            rows = self._conn.execute(  # nosec B608
                f"SELECT DISTINCT flow_id FROM flow_memberships WHERE node_id IN ({placeholders})",
                batch,
            ).fetchall()
            result.extend(r["flow_id"] for r in rows)
        # Deduplicate across batches
        return list(dict.fromkeys(result))

    def get_flow_qualified_names(self, flow_id: int) -> set[str]:
        """Return the set of qualified names for nodes in a flow."""
        rows = self._conn.execute(
            "SELECT n.qualified_name FROM flow_memberships fm "
            "JOIN nodes n ON fm.node_id = n.id WHERE fm.flow_id = ?",
            (flow_id,),
        ).fetchall()
        return {r["qualified_name"] for r in rows}

    def get_flow_qualified_names_for_flows(self, flow_ids: list[int]) -> dict[int, set[str]]:
        """Batch-return qualified node names keyed by flow id."""
        result: dict[int, set[str]] = {flow_id: set() for flow_id in flow_ids}
        if not flow_ids:
            return result

        unique_ids = list(dict.fromkeys(flow_ids))
        batch_size = 450
        for i in range(0, len(unique_ids), batch_size):
            batch = unique_ids[i : i + batch_size]
            placeholders = ",".join("?" for _ in batch)
            rows = self._conn.execute(  # nosec B608
                "SELECT fm.flow_id, n.qualified_name FROM flow_memberships fm "
                "JOIN nodes n ON fm.node_id = n.id "
                f"WHERE fm.flow_id IN ({placeholders})",
                batch,
            ).fetchall()
            for row in rows:
                result.setdefault(row["flow_id"], set()).add(row["qualified_name"])
        return result

    def load_flow_adjacency(self) -> "FlowAdjacency":
        """Load all nodes and CALLS/TESTED_BY/CROSS_ARTIFACT edges for traversal.

        Reads the entire ``nodes`` and ``edges`` tables in two streaming
        queries and returns an in-memory adjacency structure suitable for
        flow tracing and criticality scoring.  At ~500k nodes / 3M edges
        this fits in a few hundred MB and eliminates tens of millions of
        single-row SQLite point queries that otherwise dominate
        ``trace_flows`` / ``compute_criticality`` runtime.

        Reportable ``CROSS_ARTIFACT`` edges that target an existing graph
        node are included in ``calls_out`` so flows can cross artifact
        boundaries; ``bridge_edges`` preserves the transition metadata so
        hydration can mark bridge steps distinctly.
        """
        from dagayn.cross_artifact import bridge_transition_dict, is_reportable_bridge

        nodes_by_qn: dict[str, GraphNode] = {}
        nodes_by_id: dict[int, GraphNode] = {}
        for row in self._conn.execute("SELECT * FROM nodes"):
            node = self._row_to_node(row)
            nodes_by_qn[node.qualified_name] = node
            nodes_by_id[node.id] = node

        calls_out: dict[str, list[str]] = {}
        bridge_edges: dict[str, dict[str, BridgeTransitionRecord]] = {}
        has_tested_by: set[str] = set()
        for row in self._conn.execute(
            "SELECT * FROM edges WHERE kind IN ('CALLS', 'TESTED_BY', 'CROSS_ARTIFACT')"
        ):
            edge = self._row_to_edge(row)
            kind = edge.kind
            src = edge.source_qualified
            tgt = edge.target_qualified
            if kind == "CALLS":
                calls_out.setdefault(src, []).append(tgt)
            elif kind == "TESTED_BY":
                has_tested_by.add(src)
            elif is_reportable_bridge(edge) and tgt in nodes_by_qn:
                calls_out.setdefault(src, []).append(tgt)
                bridge_edges.setdefault(src, {})[tgt] = bridge_transition_dict(edge)

        return FlowAdjacency(
            calls_out=calls_out,
            has_tested_by=has_tested_by,
            nodes_by_qn=nodes_by_qn,
            nodes_by_id=nodes_by_id,
            bridge_edges=bridge_edges,
        )
