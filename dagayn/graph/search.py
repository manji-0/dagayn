from __future__ import annotations

import logging
import re
import sqlite3

from dagayn.fts_tokenize import segment_japanese_fts_text

from ._mixin_protocol import GraphStoreMixinProtocol
from .types import GraphEdge, GraphNode

logger = logging.getLogger(__name__)


class GraphStoreSearchMixin(GraphStoreMixinProtocol):
    def search_edges_by_target_name(self, name: str, kind: str = "CALLS") -> list[GraphEdge]:
        """Search for edges where target_qualified matches an unqualified name.

        CALLS edges often store unqualified target names (e.g. ``generateTestCode``)
        rather than fully qualified ones (``file.ts::generateTestCode``).  This
        method finds those edges by exact match on the plain function name so that
        reverse call tracing (callers_of) works even when qualified-name lookup
        returns nothing.
        """
        rows = self._conn.execute(
            "SELECT * FROM edges WHERE target_name = ? AND kind = ?",
            (name, kind),
        ).fetchall()
        return [self._row_to_edge(r) for r in rows]

    def count_edges_by_target_name_prefix(self, prefix: str, kind: str = "CALLS") -> int:
        """Count edges whose normalized target name starts with *prefix*."""
        row = self._conn.execute(
            "SELECT COUNT(*) FROM edges WHERE kind = ? AND target_name LIKE ?",
            (kind, f"{prefix}%"),
        ).fetchone()
        return int(row[0] if row else 0)

    def get_all_files(self) -> list[str]:
        rows = self._conn.execute(
            "SELECT DISTINCT file_path FROM nodes WHERE kind = 'File'"
        ).fetchall()
        return [r["file_path"] for r in rows]

    def get_file_hashes(self, file_paths: list[str]) -> dict[str, str]:
        """Return stored file hashes for the requested repo-relative files."""
        if not file_paths:
            return {}
        out: dict[str, str] = {}
        for i in range(0, len(file_paths), 900):
            chunk = file_paths[i : i + 900]
            placeholders = ",".join("?" for _ in chunk)
            rows = self._conn.execute(
                "SELECT file_path, file_hash FROM nodes "
                f"WHERE kind = 'File' AND file_path IN ({placeholders}) AND file_hash IS NOT NULL",
                tuple(chunk),
            ).fetchall()
            out.update({row["file_path"]: row["file_hash"] for row in rows})
        return out

    def fts_query(self, query: str, limit: int = 50) -> list[tuple[int, float]]:
        """FTS5 BM25 search. Returns (node_id, score) with higher = better.

        Builds an AND-of-quoted-segments query when the input contains
        separators (``.``, ``/``, ``::``) so that ``api.get_users`` matches
        ``api.py::get_users`` (where the tokens are not adjacent). Otherwise
        wraps the whole query as a single phrase. Quotes prevent FTS5
        operator injection.

        Returns [] when the FTS index is unavailable.
        """
        fts_query = segment_japanese_fts_text(query)
        segments = [seg for seg in re.split(r"[./:\s]+", fts_query) if seg]
        quoted_segments = ['"' + seg.replace('"', '""') + '"' for seg in segments]
        if len(quoted_segments) > 1:
            safe_query = " AND ".join(quoted_segments)
            fallback_query = " OR ".join(quoted_segments)
        else:
            safe_query = '"' + fts_query.replace('"', '""') + '"'
            fallback_query = safe_query
        sql = (
            "SELECT rowid, bm25(nodes_fts, 8.0, 6.0, 3.0, 4.0, 5.0, 1.0) AS score "
            "FROM nodes_fts WHERE nodes_fts MATCH ? ORDER BY score LIMIT ?"
        )
        try:
            rows = self._conn.execute(sql, (safe_query, limit)).fetchall()
            if not rows and fallback_query != safe_query:
                rows = self._conn.execute(sql, (fallback_query, limit)).fetchall()
            # FTS5 rank is negative BM25 (lower = better), negate for consistency
            return [(row[0], -row[1]) for row in rows]
        except sqlite3.OperationalError as e:
            logger.warning("FTS5 search failed: %s", e)
            return []

    def keyword_query(self, query: str, limit: int = 50) -> list[tuple[int, float]]:
        """AND-of-words LIKE fallback. Returns (node_id, score) with 3/2/1 scoring.

        Used only when FTS5 is unavailable (index not yet built).
        """
        words = query.lower().split()
        if not words:
            return []

        conditions: list[str] = []
        params: list[str | int] = []
        for word in words:
            conditions.append("(LOWER(name) LIKE ? OR LOWER(qualified_name) LIKE ?)")
            params.extend([f"%{word}%", f"%{word}%"])

        where = " AND ".join(conditions)
        params.append(limit)
        sql = f"SELECT id, name FROM nodes WHERE {where} LIMIT ?"  # nosec B608

        try:
            rows = self._conn.execute(sql, params).fetchall()
        except sqlite3.OperationalError:
            return []

        q_lower = query.lower()
        results: list[tuple[int, float]] = []
        for row in rows:
            name_lower = row["name"].lower()
            if name_lower == q_lower:
                score = 3.0
            elif name_lower.startswith(q_lower):
                score = 2.0
            else:
                score = 1.0
            results.append((row["id"], score))

        results.sort(key=lambda x: x[1], reverse=True)
        return results

    def search_nodes(self, query: str, limit: int = 20) -> list[GraphNode]:
        """Keyword search across node names.

        Tries FTS5 first (fast, tokenized matching), then falls back to
        LIKE-based substring search when FTS5 returns no results.
        """
        fts_results = self.fts_query(query, limit=limit)
        if fts_results:
            node_ids = [nid for nid, _ in fts_results]
            by_id = self.get_nodes_by_ids(node_ids)
            return [by_id[nid] for nid in node_ids if nid in by_id]

        keyword_results = self.keyword_query(query, limit=limit)
        if keyword_results:
            node_ids = [nid for nid, _ in keyword_results]
            by_id = self.get_nodes_by_ids(node_ids)
            return [by_id[nid] for nid in node_ids if nid in by_id]

        return []
