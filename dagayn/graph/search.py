from __future__ import annotations

import logging
import re
import sqlite3

from ._fts_tokenize import FTS_SEGMENTER_METADATA_KEY, segment_japanese_fts_text
from ._mixin_protocol import GraphStoreMixinProtocol
from .types import FtsQueryResult, GraphEdge, GraphNode

logger = logging.getLogger(__name__)

_COMMON_FTS_SEGMENTS = frozenset(
    {
        "py",
        "rs",
        "ts",
        "js",
        "go",
        "src",
        "test",
        "tests",
        "index",
        "main",
        "lib",
        "mod",
        "api",
        "app",
        "util",
        "utils",
        "common",
        "core",
    }
)


def _most_selective_segment(segments: list[str]) -> str:
    """Pick the segment least likely to match spuriously via OR fallback."""
    return max(
        segments,
        key=lambda segment: (
            segment not in _COMMON_FTS_SEGMENTS,
            len(segment),
            segment,
        ),
    )


def _build_fts_match_queries(fts_query: str) -> tuple[str, str, str]:
    """Return (primary_query, or_fallback_query, expected_match_mode_if_primary_misses)."""
    segments = [seg for seg in re.split(r"[./:\s]+", fts_query) if seg]
    quoted_segments = ['"' + seg.replace('"', '""') + '"' for seg in segments]
    if len(quoted_segments) > 1:
        safe_query = " AND ".join(quoted_segments)
        anchor = _most_selective_segment(segments)
        quoted_anchor = '"' + anchor.replace('"', '""') + '"'
        fallback_query = f"({' OR '.join(quoted_segments)}) AND {quoted_anchor}"
        fallback_mode = "or"
    else:
        safe_query = '"' + fts_query.replace('"', '""') + '"'
        fallback_query = safe_query
        fallback_mode = "phrase"
    return safe_query, fallback_query, fallback_mode


class GraphStoreSearchMixin(GraphStoreMixinProtocol):
    def search_import_edges_for_symbol(
        self,
        defining_file: str,
        _symbol_name: str,
    ) -> list[GraphEdge]:
        """Return IMPORTS_FROM edges whose target is the defining file path.

        ``IMPORTS_FROM.target_qualified`` stores the resolved module file path, not
        a symbol qualified name. The symbol argument is accepted for rename-preview
        call-site symmetry but is not used in the lookup itself.
        """
        normalized = self._normalize_file_path_key(defining_file)
        keys = [defining_file]
        if normalized != defining_file:
            keys.append(normalized)

        placeholders = ",".join("?" for _ in keys)
        rows = self._conn.execute(  # nosec B608
            f"SELECT * FROM edges WHERE kind = 'IMPORTS_FROM' "
            f"AND target_qualified IN ({placeholders})",
            tuple(keys),
        ).fetchall()
        return [self._row_to_edge(row) for row in rows]

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

    def fts_query(self, query: str, limit: int = 50) -> FtsQueryResult:
        """FTS5 BM25 search. Returns hits and how the query was matched.

        Builds an AND-of-quoted-segments query when the input contains
        separators (``.``, ``/``, ``::``) so that ``api.get_users`` matches
        ``api.py::get_users`` (where the tokens are not adjacent). Otherwise
        wraps the whole query as a single phrase. Quotes prevent FTS5
        operator injection.

        When the AND arm misses, retries with an OR arm that still requires
        the most selective segment so path-shaped junk queries do not match
        on shared tokens like ``py`` or ``src``.

        Returns an empty result when the FTS index is unavailable.
        """
        segmenter = self.get_metadata(FTS_SEGMENTER_METADATA_KEY)
        fts_query = segment_japanese_fts_text(query, segmenter=segmenter)
        safe_query, fallback_query, fallback_mode = _build_fts_match_queries(fts_query)
        sql = (
            "SELECT rowid, bm25(nodes_fts, 8.0, 6.0, 3.0, 4.0, 5.0, 1.0) AS score "
            "FROM nodes_fts WHERE nodes_fts MATCH ? ORDER BY score LIMIT ?"
        )
        try:
            rows = self._conn.execute(sql, (safe_query, limit)).fetchall()
            match_mode = "and" if len(re.split(r"[./:\s]+", fts_query)) > 1 else "phrase"
            if not rows and fallback_query != safe_query:
                rows = self._conn.execute(sql, (fallback_query, limit)).fetchall()
                match_mode = fallback_mode if rows else "none"
            elif not rows:
                match_mode = "none"
            # FTS5 rank is negative BM25 (lower = better), negate for consistency
            hits = [(row[0], -row[1]) for row in rows]
            return FtsQueryResult(hits=hits, match_mode=match_mode)
        except sqlite3.OperationalError as e:
            logger.warning("FTS5 search failed: %s", e)
            return FtsQueryResult(hits=[], match_mode="none")

    def keyword_query(self, query: str, limit: int = 50) -> list[tuple[int, float]]:
        """AND-of-words LIKE fallback. Returns (node_id, score) with 3/2/1 scoring.

        Used only when FTS5 is unavailable (index not yet built).

        SQLite's ``LOWER()`` and ``LIKE`` only fold ASCII, so non-ASCII
        identifiers (e.g. Greek/Cyrillic uppercase, accented letters) would
        never match a ``LOWER(name) LIKE`` clause. We therefore fold case in
        Python; SQL is used only as a cheap pre-filter for pure-ASCII words,
        while queries with non-ASCII words scan rows in Python so
        case/accent variants match.
        """
        words = query.lower().split()
        if not words:
            return []

        if all(word.isascii() for word in words):
            conditions: list[str] = []
            params: list[str | int] = []
            for word in words:
                conditions.append("(name LIKE ? OR qualified_name LIKE ?)")
                params.extend([f"%{word}%", f"%{word}%"])
            where = " AND ".join(conditions)
            params.append(limit * 4)
            sql = f"SELECT id, name FROM nodes WHERE {where} LIMIT ?"  # nosec B608
        else:
            # SQLite LIKE/LOWER are ASCII-only: a non-ASCII query cannot be
            # narrowed in SQL without dropping case/accent variants, so scan
            # names directly (this is a fallback path only).
            sql = "SELECT id, name FROM nodes"  # nosec B608
            params = []

        try:
            rows = self._conn.execute(sql, params).fetchall()
        except sqlite3.OperationalError:
            return []

        q_lower = query.lower()
        results: list[tuple[int, float]] = []
        for row in rows:
            name_lower = row["name"].lower()
            if not all(word in name_lower for word in words):
                continue
            if name_lower == q_lower:
                score = 3.0
            elif name_lower.startswith(q_lower):
                score = 2.0
            else:
                score = 1.0
            results.append((row["id"], score))

        results.sort(key=lambda x: x[1], reverse=True)
        return results[:limit]

    def search_nodes(self, query: str, limit: int = 20) -> list[GraphNode]:
        """Keyword search across node names.

        Tries FTS5 first (fast, tokenized matching), then falls back to
        LIKE-based substring search when FTS5 returns no results.
        """
        fts_result = self.fts_query(query, limit=limit)
        if fts_result.hits:
            node_ids = [nid for nid, _ in fts_result.hits]
            by_id = self.get_nodes_by_ids(node_ids)
            return [by_id[nid] for nid in node_ids if nid in by_id]

        keyword_results = self.keyword_query(query, limit=limit)
        if keyword_results:
            node_ids = [nid for nid, _ in keyword_results]
            by_id = self.get_nodes_by_ids(node_ids)
            return [by_id[nid] for nid in node_ids if nid in by_id]

        return []
