from __future__ import annotations

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

    def prune_orphaned_graph_structures(self) -> dict[str, int]:
        """Delete derived rows whose nodes no longer exist.

        Re-parsing a file deletes its nodes and inserts new ones, and node ids
        are autoincremented, so every re-parse orphans the flow memberships,
        community assignments, and risk rows that pointed at the old ids.
        Nothing else removes them: ``remove_files_data`` drops nodes and edges
        only, and flow/community detection runs at ``postprocess=full``, which
        no hook uses. Left alone, ``flow_tool`` keeps serving flows whose whole
        path was deleted commits ago.

        Returns ``{table: rows_deleted}`` for the tables that lost rows.
        """
        deleted: dict[str, int] = {}
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
