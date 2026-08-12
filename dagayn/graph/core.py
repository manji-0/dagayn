"""SQLite-backed knowledge graph storage and query engine.

Stores code structure as nodes (File, Class, Function, Type, Test) and
edges (CALLS, IMPORTS_FROM, INHERITS, IMPLEMENTS, CONTAINS, TESTED_BY, DEPENDS_ON, REFERENCES,
CROSS_ARTIFACT).
Supports impact-radius queries and subgraph extraction.
"""

from __future__ import annotations

import json
import logging
import sqlite3
import threading
from importlib import import_module
from pathlib import Path
from typing import TYPE_CHECKING, Optional

import networkx as nx

from ..state_types import normalize_confidence_tier
from ._sql import _SCHEMA_SQL
from .access import GraphStoreAccessMixin
from .analysis import GraphStoreAnalysisMixin
from .analysis_impact import GraphStoreImpactMixin
from .community import GraphStoreCommunityMixin
from .dependencies import GraphStoreDependencyMixin
from .flow import GraphStoreFlowMixin
from .helpers import _sanitize_name, edge_to_dict, node_to_dict  # noqa: F401
from .maintenance import GraphStoreMaintenanceMixin
from .search import GraphStoreSearchMixin
from .storage import GraphStoreStorageMixin
from .storage_batch import GraphStoreStorageBatchMixin
from .storage_metadata import GraphStoreStorageMetadataMixin
from .subgraph import GraphStoreSubgraphMixin
from .topology import GraphStoreTopologyMixin
from .types import FlowAdjacency, GraphEdge, GraphNode, GraphStats  # noqa: F401

if TYPE_CHECKING:
    from ..parser._base.types import NodeInfo

logger = logging.getLogger(__name__)


# ---------------------------------------------------------------------------
# GraphStore
# ---------------------------------------------------------------------------


class GraphStore(
    GraphStoreStorageMixin,
    GraphStoreStorageBatchMixin,
    GraphStoreStorageMetadataMixin,
    GraphStoreAccessMixin,
    GraphStoreDependencyMixin,
    GraphStoreSearchMixin,
    GraphStoreImpactMixin,
    GraphStoreAnalysisMixin,
    GraphStoreCommunityMixin,
    GraphStoreFlowMixin,
    GraphStoreMaintenanceMixin,
    GraphStoreSubgraphMixin,
    GraphStoreTopologyMixin,
):
    """SQLite-backed code knowledge graph."""

    def __init__(self, db_path: str | Path) -> None:
        self.db_path = Path(db_path)
        self.db_path.parent.mkdir(parents=True, exist_ok=True)
        self._conn = sqlite3.connect(
            str(self.db_path),
            timeout=30,
            check_same_thread=False,
            isolation_level=None,  # Disable implicit transactions (#135)
        )
        self._conn.row_factory = sqlite3.Row
        self._conn.execute("PRAGMA journal_mode=WAL")
        self._conn.execute("PRAGMA busy_timeout=5000")
        self._conn.execute("PRAGMA synchronous=NORMAL")
        self._conn.execute("PRAGMA cache_size=-64000")  # 64 MB page cache
        self._conn.execute("PRAGMA mmap_size=268435456")  # 256 MB memory-mapped I/O
        self._conn.execute("PRAGMA temp_store=MEMORY")
        self._init_schema()
        # Ensure schema_version is set, then run pending migrations
        migrations = import_module("dagayn.migrations")
        if migrations.get_schema_version(self._conn) < 1:
            # Fresh DB — metadata table just created by _init_schema
            self._conn.execute(
                "INSERT OR IGNORE INTO metadata (key, value) VALUES ('schema_version', '1')"
            )
            self._conn.commit()
        migrations.run_migrations(self._conn)
        migrations.ensure_edge_target_name_column(self._conn)
        self._nxg_cache: nx.DiGraph | None = None
        self._cache_lock = threading.Lock()
        # Cached ``repo_root`` metadata — avoids one SELECT per path
        # normalization during batch deletes (see ``remove_files_data``).
        # ``False`` means unset; ``None`` means metadata has no repo_root.
        self._repo_root_cache: Optional[Path] | bool = False
        # When *True*, :meth:`close` becomes a no-op so that the
        # process-level store cache in ``dagayn.tools._common`` can
        # keep the underlying ``sqlite3.Connection`` alive across
        # tool invocations.  Use :meth:`_force_close` to actually
        # close the connection.
        self._pinned: bool = False
        # Counts outstanding borrows issued by ``_get_store()``.  Incremented
        # atomically (under ``_store_lock``) when the cache returns this
        # instance; decremented by :meth:`close`.  When ``_pinned`` is
        # cleared by eviction and ``_leases`` drops to zero the connection
        # is closed so in-flight callers finish cleanly.
        self._leases: int = 0

    def __enter__(self) -> "GraphStore":
        return self

    def __exit__(self, exc_type, exc_val, exc_tb) -> None:
        self.close()

    def _init_schema(self) -> None:
        self._conn.executescript(_SCHEMA_SQL)
        self._conn.commit()

    def _invalidate_cache(self) -> None:
        """Invalidate the cached NetworkX graph after write operations."""
        with self._cache_lock:
            self._nxg_cache = None
        self._conn.execute("DELETE FROM hub_scores")
        self._conn.execute("DELETE FROM bridge_scores")

    def close(self) -> None:
        if self._leases > 0:
            self._leases -= 1
        if self._pinned:
            # Still held by the process-level cache; keep the connection alive.
            return
        if self._leases > 0:
            # Evicted but other callers still hold leases; the last one closes.
            return
        self._conn.close()

    def _force_close(self) -> None:
        """Close the underlying sqlite connection, ignoring ``_pinned``."""
        self._conn.close()

    def get_repo_root(self) -> Optional[Path]:
        cached = self._repo_root_cache
        if cached is not False:
            return cached  # type: ignore[return-value]
        raw = self.get_metadata("repo_root")
        resolved = Path(raw) if raw else None
        self._repo_root_cache = resolved
        return resolved

    def resolve_file_path(self, file_path: str | Path) -> Path:
        path = Path(file_path)
        if path.is_absolute():
            return path
        repo_root = self.get_repo_root()
        return (repo_root / path) if repo_root is not None else path

    def _normalize_file_path_key(self, file_path: str | Path) -> str:
        path = Path(file_path)
        if not path.is_absolute():
            return str(path)
        repo_root = self.get_repo_root()
        if repo_root is None:
            return str(path)
        candidates = [repo_root]
        try:
            resolved = repo_root.resolve()
        except (OSError, RuntimeError):
            resolved = None
        if resolved is not None and resolved not in candidates:
            candidates.append(resolved)
        try:
            path_resolved = path.resolve()
        except (OSError, RuntimeError):
            path_resolved = path
        for root in candidates:
            for candidate in (path, path_resolved):
                try:
                    return str(candidate.relative_to(root))
                except ValueError:
                    continue
        return str(path)

    def _normalize_qualified_key(self, qualified_name: str) -> str:
        if "::" not in qualified_name:
            return self._normalize_file_path_key(qualified_name)
        file_path, rest = qualified_name.split("::", 1)
        return f"{self._normalize_file_path_key(file_path)}::{rest}"

    # --- Internal helpers ---

    def _build_networkx_graph(self) -> nx.DiGraph:
        """Build (or return cached) in-memory NetworkX directed graph from all edges."""
        with self._cache_lock:
            if self._nxg_cache is not None:
                return self._nxg_cache
            g: nx.DiGraph = nx.DiGraph()
            rows = self._conn.execute("SELECT * FROM edges").fetchall()
            for r in rows:
                keys = r.keys()
                extra = json.loads(r["extra"]) if r["extra"] else {}
                g.add_edge(
                    r["source_qualified"],
                    r["target_qualified"],
                    kind=r["kind"],
                    confidence_tier=r["confidence_tier"] if "confidence_tier" in keys else None,
                    confidence=r["confidence"] if "confidence" in keys else 1.0,
                    extra=extra,
                    file_path=r["file_path"],
                    line=r["line"],
                )
            self._nxg_cache = g
            return g

    def _make_qualified(self, node: NodeInfo) -> str:
        if node.kind == "File":
            return node.file_path
        if node.parent_name:
            return f"{node.file_path}::{node.parent_name}.{node.name}"
        return f"{node.file_path}::{node.name}"

    def _row_to_node(self, row: sqlite3.Row) -> GraphNode:
        return GraphNode(
            id=row["id"],
            kind=row["kind"],
            name=row["name"],
            qualified_name=row["qualified_name"],
            file_path=row["file_path"],
            line_start=row["line_start"],
            line_end=row["line_end"],
            language=row["language"] or "",
            parent_name=row["parent_name"],
            params=row["params"],
            return_type=row["return_type"],
            is_test=bool(row["is_test"]),
            file_hash=row["file_hash"],
            extra=json.loads(row["extra"]) if row["extra"] else {},
            signature=row["signature"] if "signature" in row.keys() else None,
        )

    def _row_to_edge(self, row: sqlite3.Row) -> GraphEdge:
        extra = json.loads(row["extra"]) if row["extra"] else {}
        confidence = row["confidence"] if "confidence" in row.keys() else 1.0
        confidence_tier = normalize_confidence_tier(
            row["confidence_tier"] if "confidence_tier" in row.keys() else "EXTRACTED"
        )
        return GraphEdge(
            id=row["id"],
            kind=row["kind"],
            source_qualified=row["source_qualified"],
            target_qualified=row["target_qualified"],
            file_path=row["file_path"],
            line=row["line"],
            extra=extra,
            confidence=confidence,
            confidence_tier=confidence_tier,
        )
