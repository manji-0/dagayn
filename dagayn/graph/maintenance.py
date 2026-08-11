from __future__ import annotations

import json
import sqlite3

from ._mixin_protocol import GraphStoreMixinProtocol

#: Derived tables that reference ``nodes.id`` / each other, ordered so a parent
#: is pruned only after the children that could keep it alive. Each entry is
#: ``(table, DELETE predicate)``.
_ORPHAN_PRUNE_STEPS: tuple[tuple[str, str], ...] = (
    (
        "flow_memberships",
        "NOT EXISTS (SELECT 1 FROM nodes n WHERE n.id = flow_memberships.node_id)",
    ),
    (
        "flows",
        "NOT EXISTS (SELECT 1 FROM flow_memberships m WHERE m.flow_id = flows.id)",
    ),
    (
        "flow_snapshots",
        "NOT EXISTS (SELECT 1 FROM flows f WHERE f.id = flow_snapshots.flow_id)",
    ),
    (
        "communities",
        "NOT EXISTS (SELECT 1 FROM nodes n WHERE n.community_id = communities.id)",
    ),
    (
        "community_summaries",
        "NOT EXISTS (SELECT 1 FROM communities c WHERE c.id = community_summaries.community_id)",
    ),
    (
        "risk_index",
        "NOT EXISTS (SELECT 1 FROM nodes n WHERE n.id = risk_index.node_id)",
    ),
)


#: Tables keyed directly on ``nodes.id``. Rows for a file's nodes have to go
#: before those nodes do, or they dangle the moment the file is re-parsed.
_NODE_KEYED_TABLES: tuple[str, ...] = ("flow_memberships", "risk_index")


class GraphStoreMaintenanceMixin(GraphStoreMixinProtocol):
    def remove_node_keyed_rows_for_files(self, file_keys: list[str]) -> None:
        """Drop node-keyed derived rows for *file_keys* before their nodes go.

        Called from the file-replacement paths, which delete a file's nodes and
        insert new ones with new autoincrement ids. Scoped by file so this stays
        cheap on the per-file hot path; the repository-wide sweep is
        :meth:`prune_orphaned_graph_structures`.
        """
        if not file_keys:
            return
        placeholders = ",".join("?" for _ in file_keys)
        for table in _NODE_KEYED_TABLES:
            try:
                self._conn.execute(
                    f"DELETE FROM {table} WHERE node_id IN "  # noqa: S608
                    f"(SELECT id FROM nodes WHERE file_path IN ({placeholders}))",
                    tuple(file_keys),
                )
            except sqlite3.OperationalError:
                # Table absent on an older schema — nothing to remove.
                continue

    def _repair_stale_flow_paths(self) -> dict[str, int]:
        """Rewrite ``path_json`` / counts for flows that lost node ids on re-parse."""
        repaired = 0
        rows = self._conn.execute(
            "SELECT id, entry_point_id, path_json FROM flows"
        ).fetchall()
        for row in rows:
            flow_id = int(row["id"])
            entry_point_id = int(row["entry_point_id"])
            path_ids: list[int] = json.loads(row["path_json"])

            mem_rows = self._conn.execute(
                "SELECT fm.node_id FROM flow_memberships fm "
                "JOIN nodes n ON n.id = fm.node_id "
                "WHERE fm.flow_id = ? ORDER BY fm.position",
                (flow_id,),
            ).fetchall()
            if mem_rows:
                live_path = [int(r["node_id"]) for r in mem_rows]
            else:
                live_path = [
                    node_id
                    for node_id in path_ids
                    if self._conn.execute(
                        "SELECT 1 FROM nodes WHERE id = ?",
                        (node_id,),
                    ).fetchone()
                    is not None
                ]

            entry_live = (
                self._conn.execute(
                    "SELECT 1 FROM nodes WHERE id = ?",
                    (entry_point_id,),
                ).fetchone()
                is not None
            )
            new_entry_point_id = (
                entry_point_id
                if entry_live
                else (live_path[0] if live_path else entry_point_id)
            )

            if live_path == path_ids and entry_live:
                continue
            if not live_path:
                continue

            id_placeholders = ",".join("?" * len(live_path))
            file_rows = self._conn.execute(
                f"SELECT DISTINCT file_path FROM nodes WHERE id IN ({id_placeholders})",
                live_path,
            ).fetchall()
            file_count = len(file_rows)

            self._conn.execute(
                "UPDATE flows SET path_json = ?, node_count = ?, file_count = ?, "
                "entry_point_id = ? WHERE id = ?",
                (
                    json.dumps(live_path),
                    len(live_path),
                    file_count,
                    new_entry_point_id,
                    flow_id,
                ),
            )
            repaired += 1

        return {"flows_repaired": repaired} if repaired else {}

    def prune_orphaned_graph_structures(self) -> dict[str, int]:
        """Delete derived rows whose nodes no longer exist.

        Re-parsing a file deletes its nodes and inserts new ones, and node ids
        are autoincremented, so every re-parse orphans the flow memberships,
        community assignments, and risk rows that pointed at the old ids.
        Nothing else removes them: ``remove_files_data`` drops nodes and edges
        only, and flow/community detection runs at ``postprocess=full``, which
        no hook uses. Left alone, ``flow_tool`` keeps serving flows whose whole
        path was deleted commits ago.

        Flow rows whose ``path_json`` still references deleted node ids are
        rewritten from surviving memberships (or filtered live ids) before the
        orphan sweep deletes empty flows.

        Returns ``{table: rows_deleted}`` for the tables that lost rows.
        """
        deleted: dict[str, int] = {}
        repaired = self._repair_stale_flow_paths()
        if repaired:
            deleted.update(repaired)
        for table, predicate in _ORPHAN_PRUNE_STEPS:
            try:
                cursor = self._conn.execute(f"DELETE FROM {table} WHERE {predicate}")  # noqa: S608
            except sqlite3.OperationalError:
                # Table absent on an older schema — nothing to prune.
                continue
            if cursor.rowcount and cursor.rowcount > 0:
                deleted[table] = cursor.rowcount
        return deleted

    def get_files_matching(self, pattern: str) -> list[str]:
        """Return distinct ``file_path`` values matching a LIKE suffix."""
        rows = self._conn.execute(
            "SELECT DISTINCT file_path FROM nodes WHERE file_path LIKE ?",
            (f"%{pattern}",),
        ).fetchall()
        return [r["file_path"] for r in rows]

    def get_nodes_without_signature(self) -> list[sqlite3.Row]:
        """Return raw rows for nodes that have no signature yet."""
        return self._conn.execute(
            "SELECT id, name, kind, params, return_type FROM nodes WHERE signature IS NULL"
        ).fetchall()

    def update_node_signature(
        self,
        node_id: int,
        signature: str,
    ) -> None:
        """Set the ``signature`` column for a single node."""
        self._conn.execute(
            "UPDATE nodes SET signature = ? WHERE id = ?",
            (signature, node_id),
        )

    def get_node_kind_by_id(self, node_id: int) -> str | None:
        """Return just the ``kind`` column for a node, or ``None``."""
        row = self._conn.execute(
            "SELECT kind FROM nodes WHERE id = ?",
            (node_id,),
        ).fetchone()
        return row["kind"] if row else None

    def get_all_call_targets(self, include_file_sources: bool = True) -> set[str]:
        """Return the set of all CALLS-edge target qualified names.

        When ``include_file_sources`` is False, CALLS edges whose source is a
        File node (module-scope calls from top-level script glue, CLI
        entrypoints, or notebook cells) are excluded. Callers that treat "has
        an incoming call" as "is not a root" (e.g. entry-point detection)
        should pass ``include_file_sources=False`` — otherwise a script-only
        callee looks called and is hidden from flow analysis.

        The File-node filter joins against ``nodes.kind`` rather than pattern-
        matching ``source_qualified`` so that file paths containing ``::`` or
        any future change to the File-node naming convention cannot silently
        miscategorize edges.
        """
        if include_file_sources:
            rows = self._conn.execute(
                "SELECT DISTINCT target_qualified FROM edges WHERE kind = 'CALLS'"
            ).fetchall()
        else:
            rows = self._conn.execute(
                "SELECT DISTINCT e.target_qualified FROM edges e "
                "LEFT JOIN nodes n ON n.qualified_name = e.source_qualified "
                "WHERE e.kind = 'CALLS' "
                "AND (n.kind IS NULL OR n.kind != 'File')"
            ).fetchall()
        return {r["target_qualified"] for r in rows}
