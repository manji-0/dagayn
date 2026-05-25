from __future__ import annotations

from typing import Optional

from ._mixin_protocol import GraphStoreMixinProtocol
from .types import GraphEdge, GraphNode


class GraphStoreAccessMixin(GraphStoreMixinProtocol):
    def get_node(self, qualified_name: str) -> Optional[GraphNode]:
        normalized = self._normalize_qualified_key(qualified_name)
        keys = [qualified_name, normalized] if normalized != qualified_name else [qualified_name]
        for key in keys:
            row = self._conn.execute(
                "SELECT * FROM nodes WHERE qualified_name = ?", (key,)
            ).fetchone()
            if row:
                return self._row_to_node(row)
        return None

    def get_nodes_by_qualified_names(
        self,
        qualified_names: list[str],
    ) -> dict[str, GraphNode]:
        """Batch-fetch nodes for *qualified_names*.

        Returns a mapping from each input qualified name to its
        :class:`GraphNode`. Missing names are absent from the result.
        Both the original and normalized form of each name are tried,
        mirroring :meth:`get_node` — and the *exact* name takes
        precedence over the normalized form when both exist as
        separate rows.
        """
        if not qualified_names:
            return {}

        norm_for: dict[str, str] = {}
        keys: set[str] = set()
        for qn in qualified_names:
            normalized = self._normalize_qualified_key(qn)
            norm_for[qn] = normalized
            keys.add(qn)
            if normalized != qn:
                keys.add(normalized)

        keys_list = list(keys)
        rows_by_qn: dict[str, GraphNode] = {}
        batch_size = 450
        for i in range(0, len(keys_list), batch_size):
            batch = keys_list[i : i + batch_size]
            placeholders = ",".join("?" for _ in batch)
            rows = self._conn.execute(  # nosec B608
                f"SELECT * FROM nodes WHERE qualified_name IN ({placeholders})",
                batch,
            ).fetchall()
            for row in rows:
                qn = row["qualified_name"]
                rows_by_qn.setdefault(qn, self._row_to_node(row))

        result: dict[str, GraphNode] = {}
        for original in qualified_names:
            # Exact key wins over normalized form, matching get_node().
            node = rows_by_qn.get(original)
            if node is None:
                node = rows_by_qn.get(norm_for[original])
            if node is not None:
                result[original] = node
        return result

    def get_nodes_by_file(self, file_path: str) -> list[GraphNode]:
        normalized = self._normalize_file_path_key(file_path)
        seen_ids: set[int] = set()
        out: list[GraphNode] = []
        keys = [file_path, normalized] if normalized != file_path else [file_path]
        for key in keys:
            rows = self._conn.execute("SELECT * FROM nodes WHERE file_path = ?", (key,)).fetchall()
            for row in rows:
                if row["id"] in seen_ids:
                    continue
                seen_ids.add(row["id"])
                out.append(self._row_to_node(row))
        return out

    def get_nodes_by_files(self, file_paths: list[str]) -> dict[str, list[GraphNode]]:
        """Batch-fetch nodes for multiple file paths."""
        result: dict[str, list[GraphNode]] = {file_path: [] for file_path in file_paths}
        if not file_paths:
            return result

        key_to_originals: dict[str, list[str]] = {}
        for file_path in file_paths:
            normalized = self._normalize_file_path_key(file_path)
            keys = [file_path, normalized] if normalized != file_path else [file_path]
            for key in keys:
                key_to_originals.setdefault(key, []).append(file_path)

        seen_by_original: dict[str, set[int]] = {file_path: set() for file_path in file_paths}
        keys = list(key_to_originals)
        batch_size = 450
        for i in range(0, len(keys), batch_size):
            batch = keys[i : i + batch_size]
            placeholders = ",".join("?" for _ in batch)
            rows = self._conn.execute(  # nosec B608
                f"SELECT * FROM nodes WHERE file_path IN ({placeholders})",
                batch,
            ).fetchall()
            for row in rows:
                node = self._row_to_node(row)
                for original in key_to_originals.get(row["file_path"], []):
                    seen = seen_by_original[original]
                    if node.id in seen:
                        continue
                    seen.add(node.id)
                    result[original].append(node)
        return result

    def get_all_nodes(self, exclude_files: bool = True) -> list[GraphNode]:
        """Return all nodes, optionally excluding File nodes."""
        if exclude_files:
            rows = self._conn.execute("SELECT * FROM nodes WHERE kind != 'File'").fetchall()
        else:
            rows = self._conn.execute("SELECT * FROM nodes").fetchall()
        return [self._row_to_node(r) for r in rows]

    def get_edges_by_source(self, qualified_name: str) -> list[GraphEdge]:
        normalized = self._normalize_qualified_key(qualified_name)
        seen_ids: set[int] = set()
        out: list[GraphEdge] = []
        keys = [qualified_name, normalized] if normalized != qualified_name else [qualified_name]
        for key in keys:
            rows = self._conn.execute(
                "SELECT * FROM edges WHERE source_qualified = ?", (key,)
            ).fetchall()
            for row in rows:
                if row["id"] in seen_ids:
                    continue
                seen_ids.add(row["id"])
                out.append(self._row_to_edge(row))
        return out

    def get_edges_by_target(self, qualified_name: str) -> list[GraphEdge]:
        normalized = self._normalize_qualified_key(qualified_name)
        seen_ids: set[int] = set()
        out: list[GraphEdge] = []
        keys = [qualified_name, normalized] if normalized != qualified_name else [qualified_name]
        for key in keys:
            rows = self._conn.execute(
                "SELECT * FROM edges WHERE target_qualified = ?", (key,)
            ).fetchall()
            for row in rows:
                if row["id"] in seen_ids:
                    continue
                seen_ids.add(row["id"])
                out.append(self._row_to_edge(row))
        return out

    def get_edges_by_endpoints(
        self,
        qualified_names: list[str],
    ) -> tuple[dict[str, list[GraphEdge]], dict[str, list[GraphEdge]]]:
        """Batch-fetch edges where source OR target is in *qualified_names*.

        Returns ``(outgoing, incoming)`` where:

        - ``outgoing[qn]`` is the list of edges with ``source_qualified == qn``
        - ``incoming[qn]`` is the list of edges with ``target_qualified == qn``

        Endpoints not present as source/target return an empty list.

        This is the batch equivalent of calling
        :meth:`get_edges_by_source` and :meth:`get_edges_by_target` once per
        qualified name. It mirrors the chunking strategy used by
        :meth:`get_community_ids_by_qualified_names` to stay within SQLite's
        variable-count limit.
        """
        outgoing: dict[str, list[GraphEdge]] = {qn: [] for qn in qualified_names}
        incoming: dict[str, list[GraphEdge]] = {qn: [] for qn in qualified_names}
        if not qualified_names:
            return outgoing, incoming

        keys: set[str] = set()
        normalized_to_originals: dict[str, list[str]] = {}
        for qn in qualified_names:
            keys.add(qn)
            normalized = self._normalize_qualified_key(qn)
            keys.add(normalized)
            normalized_to_originals.setdefault(normalized, []).append(qn)

        keys_list = list(keys)
        seen_out: dict[str, set[int]] = {qn: set() for qn in qualified_names}
        seen_in: dict[str, set[int]] = {qn: set() for qn in qualified_names}
        # The two ``IN ({placeholders})`` clauses each bind one variable
        # per element, so a batch of N elements consumes 2N variables.
        # Halve the batch so we stay under SQLite's historical 999
        # variable cap even on builds compiled before the 3.32 raise.
        batch_size = 225
        for i in range(0, len(keys_list), batch_size):
            batch = keys_list[i : i + batch_size]
            placeholders = ",".join("?" for _ in batch)
            rows = self._conn.execute(  # nosec B608
                "SELECT * FROM edges "
                f"WHERE source_qualified IN ({placeholders}) "
                f"OR target_qualified IN ({placeholders})",
                [*batch, *batch],
            ).fetchall()
            for row in rows:
                edge = self._row_to_edge(row)
                src = row["source_qualified"]
                tgt = row["target_qualified"]
                # Map back to all originals whose normalized form matches.
                src_originals = normalized_to_originals.get(src, [src] if src in outgoing else [])
                tgt_originals = normalized_to_originals.get(tgt, [tgt] if tgt in incoming else [])
                for orig in src_originals:
                    if orig in outgoing and row["id"] not in seen_out[orig]:
                        seen_out[orig].add(row["id"])
                        outgoing[orig].append(edge)
                for orig in tgt_originals:
                    if orig in incoming and row["id"] not in seen_in[orig]:
                        seen_in[orig].add(row["id"])
                        incoming[orig].append(edge)
        return outgoing, incoming
