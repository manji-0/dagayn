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

    def get_edges_by_kind(
        self,
        kind: str,
        unresolved_target_only: bool = False,
    ) -> list[GraphEdge]:
        """Return all edges of one kind.

        Used by coverage heuristics that scan candidate import edges without
        re-parsing every edge kind (the kind-filtered query only returns the
        rows the caller needs).

        With *unresolved_target_only*, restricts to edges whose target is an
        ``<unresolved:...>`` placeholder — the shape entry-point bridges take
        before a later pass resolves them.
        """
        if unresolved_target_only:
            rows = self._conn.execute(
                "SELECT * FROM edges WHERE kind = ? AND target_qualified LIKE '<unresolved:%'",
                (kind,),
            ).fetchall()
        else:
            rows = self._conn.execute("SELECT * FROM edges WHERE kind = ?", (kind,)).fetchall()
        return [self._row_to_edge(r) for r in rows]

    def get_edges_by_sources(
        self,
        source_qns: list[str],
        kinds: list[str] | None = None,
    ) -> dict[str, list[GraphEdge]]:
        """Batch-fetch edges grouped by ``source_qualified``.

        Unlike :meth:`get_edges_by_endpoints` this does not normalize keys or
        fetch the incoming side, so callers that already hold graph-native
        qualified names and only want one direction pay for one query.
        """
        return self._edges_by_endpoint_column("source_qualified", source_qns, kinds)

    def get_edges_by_targets(
        self,
        target_qns: list[str],
        kinds: list[str] | None = None,
    ) -> dict[str, list[GraphEdge]]:
        """Batch-fetch edges grouped by ``target_qualified``."""
        return self._edges_by_endpoint_column("target_qualified", target_qns, kinds)

    def _edges_by_endpoint_column(
        self,
        column: str,
        qns: list[str],
        kinds: list[str] | None,
    ) -> dict[str, list[GraphEdge]]:
        result: dict[str, list[GraphEdge]] = {}
        if not qns:
            return result
        unique = list(dict.fromkeys(qns))
        kind_list = list(kinds or [])
        batch_size = 450
        for i in range(0, len(unique), batch_size):
            batch = unique[i : i + batch_size]
            placeholders = ",".join("?" for _ in batch)
            kind_filter = ""
            if kind_list:
                kind_placeholders = ",".join("?" for _ in kind_list)
                kind_filter = f" AND kind IN ({kind_placeholders})"
            rows = self._conn.execute(  # nosec B608
                f"SELECT * FROM edges WHERE {column} IN ({placeholders}){kind_filter}",
                (*batch, *kind_list),
            ).fetchall()
            for row in rows:
                result.setdefault(row[column], []).append(self._row_to_edge(row))
        return result

    def get_edges_by_target_names(
        self,
        names: list[str],
        kind: str = "CALLS",
        qualified_only: bool = False,
    ) -> dict[str, list[GraphEdge]]:
        """Batch-fetch edges grouped by the normalized ``target_name`` column.

        With *qualified_only*, drops rows whose ``target_qualified`` is just the
        bare name — those are the unqualified call edges a caller matching by
        bare name already handles separately.
        """
        result: dict[str, list[GraphEdge]] = {}
        if not names:
            return result
        batch_size = 450
        qualified_filter = " AND target_qualified != target_name" if qualified_only else ""
        for i in range(0, len(names), batch_size):
            batch = names[i : i + batch_size]
            placeholders = ",".join("?" for _ in batch)
            rows = self._conn.execute(  # nosec B608
                f"SELECT * FROM edges WHERE target_name IN ({placeholders}) "
                f"AND kind = ?{qualified_filter}",
                (*batch, kind),
            ).fetchall()
            for row in rows:
                result.setdefault(row["target_name"], []).append(self._row_to_edge(row))
        return result

    def has_edge_to_target(self, target_qualified: str, kind: str = "CALLS") -> bool:
        """True when any *kind* edge points at *target_qualified*."""
        row = self._conn.execute(
            "SELECT 1 FROM edges WHERE target_qualified = ? AND kind = ? LIMIT 1",
            (target_qualified, kind),
        ).fetchone()
        return row is not None

    def count_nodes_by_name(
        self,
        kinds: list[str],
        include_tests: bool = False,
    ) -> dict[str, int]:
        """Return ``name -> definition count`` over the requested kinds.

        Dead-code analysis uses this to tell an ambiguous bare name (many
        same-named definitions) from a unique one.
        """
        if not kinds:
            return {}
        placeholders = ",".join("?" for _ in kinds)
        test_filter = "" if include_tests else "AND is_test = 0 "
        rows = self._conn.execute(  # nosec B608
            f"SELECT name, COUNT(*) FROM nodes WHERE kind IN ({placeholders}) "
            f"{test_filter}GROUP BY name",
            tuple(kinds),
        ).fetchall()
        return {row[0]: row[1] for row in rows}

    def get_nodes_by_parent_and_name(
        self,
        parent_name: str,
        name: str,
        kinds: list[str],
    ) -> list[GraphNode]:
        """Return nodes declared inside *parent_name* under *name*.

        Used to resolve a method against its declared base class without
        guessing at qualified-name shapes.
        """
        if not kinds:
            return []
        placeholders = ",".join("?" for _ in kinds)
        rows = self._conn.execute(  # nosec B608
            f"SELECT * FROM nodes WHERE parent_name = ? AND name = ? AND kind IN ({placeholders})",
            (parent_name, name, *kinds),
        ).fetchall()
        return [self._row_to_node(row) for row in rows]

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
