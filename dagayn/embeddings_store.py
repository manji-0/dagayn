"""Embedding SQLite storage, vector search, and batch indexing."""

from __future__ import annotations

import hashlib
import logging
import os
import sqlite3
import struct
import sys
import time
from pathlib import Path
from typing import TYPE_CHECKING, Any

from .embeddings_providers import (
    EmbeddingProvider,
    _embedding_provider_key,
    embedding_provider_lookup_candidates,
)
from .embeddings_text import (
    _build_graph_facts_by_qualified_name,
    _embed_query_cached,
    _embedding_text_mode,
    _node_to_text,
    _slow_embed_batch_seconds,
)
from .graph import GraphNode, GraphStore
from .state_types import seal_embedding_status

_EMBED_PROVIDER_ERRORS = (OSError, RuntimeError, ValueError, TypeError, sqlite3.Error)
_EMBEDDING_SEARCH_BACKENDS = {"auto", "rust", "python"}

if TYPE_CHECKING:
    import numpy as np

    _NUMPY_AVAILABLE = True
else:
    try:
        import numpy as np

        _NUMPY_AVAILABLE = True
    except ImportError:
        np = None  # type: ignore[assignment]
        _NUMPY_AVAILABLE = False

logger = logging.getLogger(__name__)


def _get_provider(provider: str | None, *, model: str | None = None) -> EmbeddingProvider | None:
    """Resolve provider through the public embeddings shim for monkeypatch compatibility."""
    from . import embeddings as emb

    return emb.get_provider(provider, model=model)


ACTIVE_EMBEDDING_PROVIDER_METADATA_KEY = "embedding_provider"


def get_embedding_provider_counts(db_path: str | Path) -> dict[str, int]:
    """Return persisted embedding row counts grouped by provider key."""
    path = Path(db_path)
    try:
        conn = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
    except sqlite3.Error:
        return {}
    try:
        tables = {
            str(row[0])
            for row in conn.execute(
                "SELECT name FROM sqlite_master WHERE type IN ('table', 'view')"
            ).fetchall()
        }
        if "embeddings" not in tables:
            return {}
        return {
            str(provider): int(count)
            for provider, count in conn.execute(
                "SELECT provider, COUNT(*) FROM embeddings GROUP BY provider"
            ).fetchall()
        }
    except sqlite3.Error:
        return {}
    finally:
        conn.close()


def read_active_embedding_provider_metadata(
    db_path: str | Path,
    *,
    conn: sqlite3.Connection | None = None,
) -> str | None:
    """Return the active embedding provider key stored in graph metadata, if any."""
    owns_conn = conn is None
    if owns_conn:
        try:
            conn = sqlite3.connect(f"file:{Path(db_path)}?mode=ro", uri=True)
        except sqlite3.Error:
            return None
    assert conn is not None
    try:
        tables = {
            str(row[0])
            for row in conn.execute(
                "SELECT name FROM sqlite_master WHERE type IN ('table', 'view')"
            ).fetchall()
        }
        if "metadata" not in tables:
            return None
        row = conn.execute(
            "SELECT value FROM metadata WHERE key = ?",
            (ACTIVE_EMBEDDING_PROVIDER_METADATA_KEY,),
        ).fetchone()
        return str(row[0]) if row else None
    except sqlite3.Error:
        return None
    finally:
        if owns_conn:
            conn.close()


def _provider_candidates(
    provider_counts: dict[str, int],
    *,
    text_mode: str | None = None,
) -> dict[str, int]:
    if not provider_counts:
        return {}
    if not text_mode:
        return provider_counts

    matches = {
        provider_name: count
        for provider_name, count in provider_counts.items()
        if provider_name.endswith(f"#text={text_mode}")
    }
    if matches:
        return matches

    legacy = {
        provider_name: count
        for provider_name, count in provider_counts.items()
        if "#text=" not in provider_name
    }
    return legacy or provider_counts


def resolve_active_embedding_provider(
    provider_counts: dict[str, int],
    *,
    text_mode: str | None = None,
    preferred_provider: str | None = None,
) -> str | None:
    """Pick the persisted provider key to use when the caller did not specify one."""
    if not provider_counts:
        return None
    if preferred_provider and preferred_provider in provider_counts:
        return preferred_provider

    candidates = _provider_candidates(provider_counts, text_mode=text_mode)
    if not candidates:
        return None
    if len(candidates) == 1:
        return next(iter(candidates))

    return max(candidates.items(), key=lambda item: (item[1], item[0]))[0]


def _persist_active_embedding_provider_metadata(
    conn: sqlite3.Connection,
    provider_key: str,
) -> None:
    tables = {
        str(row[0])
        for row in conn.execute(
            "SELECT name FROM sqlite_master WHERE type IN ('table', 'view')"
        ).fetchall()
    }
    if "metadata" not in tables:
        return
    conn.execute(
        "INSERT OR REPLACE INTO metadata (key, value) VALUES (?, ?)",
        (ACTIVE_EMBEDDING_PROVIDER_METADATA_KEY, provider_key),
    )
    conn.commit()


def get_embedding_status(
    db_path: str | Path,
    provider: str | None = None,
) -> dict[str, Any]:
    """Return read-only embedding coverage for a graph database."""
    path = Path(db_path)
    try:
        conn = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
    except sqlite3.Error as exc:
        return seal_embedding_status(
            {
                "status": "unavailable",
                "total_embeddings": 0,
                "provider_counts": {},
                "error": str(exc),
            }
        )

    try:
        conn.row_factory = sqlite3.Row
        tables = {
            str(row["name"])
            for row in conn.execute(
                "SELECT name FROM sqlite_master WHERE type IN ('table', 'view')"
            ).fetchall()
        }
        if "embeddings" not in tables:
            return seal_embedding_status(
                {
                    "status": "not_indexed",
                    "total_embeddings": 0,
                    "provider_counts": {},
                }
            )

        total_embeddings = int(conn.execute("SELECT COUNT(*) FROM embeddings").fetchone()[0])
        provider_counts = {
            str(row["provider"]): int(row["count"])
            for row in conn.execute(
                "SELECT provider, COUNT(*) AS count FROM embeddings GROUP BY provider"
            ).fetchall()
        }
        status: dict[str, Any] = {
            "status": "empty" if total_embeddings == 0 else "unknown",
            "total_embeddings": total_embeddings,
            "provider_counts": provider_counts,
        }

        if "nodes" not in tables:
            return seal_embedding_status(status)

        metadata_provider = (
            read_active_embedding_provider_metadata(db_path, conn=conn)
            if "metadata" in tables
            else None
        )
        coverage_provider = provider or resolve_active_embedding_provider(
            provider_counts,
            preferred_provider=metadata_provider,
        )
        if coverage_provider is not None:
            status["active_provider"] = coverage_provider

        provider_clause = ""
        provider_params: list[str] = []
        if coverage_provider is not None:
            provider_clause = "AND e.provider = ?"
            provider_params = [coverage_provider]

        embeddable_nodes = int(
            conn.execute("SELECT COUNT(*) FROM nodes WHERE kind != 'File'").fetchone()[0]
        )
        missing_embeddings = int(
            conn.execute(
                f"""
                SELECT COUNT(*)
                FROM nodes n
                WHERE n.kind != 'File'
                  AND NOT EXISTS (
                      SELECT 1 FROM embeddings e
                      WHERE e.qualified_name = n.qualified_name
                      {provider_clause}
                  )
                """,
                provider_params,
            ).fetchone()[0]
        )
        indexed_embeddings = int(
            conn.execute(
                f"""
                SELECT COUNT(DISTINCT e.qualified_name)
                FROM embeddings e
                JOIN nodes n ON n.qualified_name = e.qualified_name
                WHERE n.kind != 'File'
                {provider_clause}
                """,
                provider_params,
            ).fetchone()[0]
        )
        orphan_embeddings = int(
            conn.execute(
                f"""
                SELECT COUNT(*)
                FROM embeddings e
                LEFT JOIN nodes n ON n.qualified_name = e.qualified_name
                WHERE n.qualified_name IS NULL
                {provider_clause}
                """,
                provider_params,
            ).fetchone()[0]
        )

        if total_embeddings == 0:
            state = "empty"
        elif orphan_embeddings:
            state = "stale"
        elif missing_embeddings:
            state = "partial"
        else:
            state = "complete"

        status.update(
            {
                "status": state,
                "embeddable_nodes": embeddable_nodes,
                "indexed_embeddings": indexed_embeddings,
                "missing_embeddings": missing_embeddings,
                "orphan_embeddings": orphan_embeddings,
            }
        )
        return seal_embedding_status(status)
    except sqlite3.Error as exc:
        return seal_embedding_status(
            {
                "status": "unavailable",
                "total_embeddings": 0,
                "provider_counts": {},
                "error": str(exc),
            }
        )
    finally:
        conn.close()


# ---------------------------------------------------------------------------
# SQLite vector storage
# ---------------------------------------------------------------------------

_EMBEDDINGS_SCHEMA = """
CREATE TABLE IF NOT EXISTS embeddings (
    qualified_name TEXT NOT NULL,
    vector BLOB NOT NULL,
    text_hash TEXT NOT NULL,
    provider TEXT NOT NULL DEFAULT 'unknown',
    PRIMARY KEY (qualified_name, provider)
);
"""


def _ensure_embeddings_schema(conn: sqlite3.Connection) -> None:
    """Migrate legacy single-provider embedding tables to provider-partitioned rows."""
    columns = conn.execute("PRAGMA table_info(embeddings)").fetchall()
    if not columns:
        conn.executescript(_EMBEDDINGS_SCHEMA)
        return

    names = {str(row[1]) for row in columns}
    if "provider" not in names:
        conn.execute("ALTER TABLE embeddings ADD COLUMN provider TEXT NOT NULL DEFAULT 'unknown'")
        columns = conn.execute("PRAGMA table_info(embeddings)").fetchall()

    pk_columns = [str(row[1]) for row in columns if int(row[5] or 0) > 0]
    if pk_columns == ["qualified_name"]:
        conn.execute("ALTER TABLE embeddings RENAME TO embeddings_legacy_single_provider")
        conn.executescript(_EMBEDDINGS_SCHEMA)
        conn.execute(
            """
            INSERT OR REPLACE INTO embeddings (qualified_name, vector, text_hash, provider)
            SELECT qualified_name, vector, text_hash, provider
            FROM embeddings_legacy_single_provider
            """
        )
        conn.execute("DROP TABLE embeddings_legacy_single_provider")


def _encode_vector(vec: list[float]) -> bytes:
    """Encode a float vector as a compact binary blob."""
    return struct.pack(f"{len(vec)}f", *vec)


def _decode_vector(blob: bytes) -> list[float]:
    """Decode a binary blob back to a float vector."""
    n = len(blob) // 4  # 4 bytes per float32
    return list(struct.unpack(f"{n}f", blob))


def _vector_byte_length(dim: int) -> int:
    """Return the byte length of a float32 vector with *dim* elements."""
    return dim * 4


def _cosine_similarity(a: list[float], b: list[float]) -> float:
    """Compute cosine similarity between two vectors."""
    if len(a) != len(b):
        return 0.0
    dot = sum(x * y for x, y in zip(a, b))
    norm_a = sum(x * x for x in a) ** 0.5
    norm_b = sum(x * x for x in b) ** 0.5
    if norm_a == 0 or norm_b == 0:
        return 0.0
    return dot / (norm_a * norm_b)


# ---------------------------------------------------------------------------
# Optional numpy vector cache (used when numpy is installed).
# key: (db_path_str, provider_name, stamp_ns, vector_bytes)
# value: (matrix float32 (N, D), names list[str], row_norms float32 (N,))
# stamp_ns mirrors the Rust backend: max(mtime of db, -wal, -shm).
# ---------------------------------------------------------------------------

_np_vec_cache: dict[tuple[str, str, int, int], tuple[Any, list[str], Any]] = {}


def _file_mtime_ns(path: Path) -> int:
    try:
        return int(path.stat().st_mtime_ns)
    except OSError:
        return 0


def _db_stamp_ns(db_path: Path) -> int:
    """Return a WAL-aware DB stamp matching the Rust embedding-search cache."""
    stamp = _file_mtime_ns(db_path)
    for sibling in (Path(f"{db_path}-wal"), Path(f"{db_path}-shm")):
        stamp = max(stamp, _file_mtime_ns(sibling))
    return stamp


def _load_vec_matrix(
    conn: sqlite3.Connection, provider_name: str, *, vector_bytes: int | None = None
) -> tuple[Any, list[str], Any]:
    """Load embedding rows for *provider_name* into a numpy matrix."""
    assert np is not None
    if vector_bytes is None:
        sql = "SELECT qualified_name, vector FROM embeddings WHERE provider = ?"
        params: tuple[Any, ...] = (provider_name,)
    else:
        sql = (
            "SELECT qualified_name, vector FROM embeddings "
            "WHERE provider = ? AND length(vector) = ?"
        )
        params = (provider_name, vector_bytes)
    rows = conn.execute(sql, params).fetchall()
    if not rows:
        empty = np.empty((0, 0), dtype=np.float32)
        return empty, [], np.empty((0,), dtype=np.float32)
    names = [r["qualified_name"] for r in rows]
    vecs = [np.frombuffer(r["vector"], dtype=np.float32).copy() for r in rows]
    matrix = np.stack(vecs)
    row_norms = np.linalg.norm(matrix, axis=1).astype(np.float32)
    return matrix, names, row_norms


def _numpy_vec_cache() -> dict[tuple[str, str, int, int], tuple[Any, list[str], Any]]:
    """Return the numpy vector cache via the public embeddings shim."""
    from . import embeddings as emb

    return emb._np_vec_cache


def _load_vec_matrix_for_search(
    conn: sqlite3.Connection,
    provider_name: str,
    *,
    vector_bytes: int | None = None,
) -> tuple[Any, list[str], Any]:
    """Load vectors through the public embeddings shim for monkeypatch compatibility."""
    from . import embeddings as emb

    return emb._load_vec_matrix(conn, provider_name, vector_bytes=vector_bytes)


def _invalidate_np_vec_cache(db_path: Path, provider_name: str | None = None) -> None:
    """Drop cached numpy matrices for a database (optionally one provider)."""
    path_key = str(db_path)
    for key in list(_np_vec_cache):
        if key[0] != path_key:
            continue
        if provider_name is None or key[1] == provider_name:
            del _np_vec_cache[key]


def _python_loop_search(
    conn: sqlite3.Connection,
    provider_name: str,
    query_vec: list[float],
    limit: int,
) -> list[tuple[str, float]]:
    """Pure-Python cosine scan over provider-partitioned embedding rows."""
    vector_bytes = _vector_byte_length(len(query_vec))
    scored: list[tuple[str, float]] = []
    cursor = conn.execute(
        "SELECT qualified_name, vector FROM embeddings WHERE provider = ? AND length(vector) = ?",
        (provider_name, vector_bytes),
    )
    while True:
        rows = cursor.fetchmany(500)
        if not rows:
            break
        for row in rows:
            vec = _decode_vector(row["vector"])
            sim = _cosine_similarity(query_vec, vec)
            scored.append((row["qualified_name"], sim))
    scored.sort(key=lambda x: x[1], reverse=True)
    return scored[:limit]


def _numpy_matmul_search(
    db_path: Path,
    conn: sqlite3.Connection,
    provider_name: str,
    query_vec: list[float],
    limit: int,
) -> list[tuple[str, float]]:
    """BLAS matmul cosine search over a process-level cached float32 matrix."""
    assert np is not None
    vector_bytes = _vector_byte_length(len(query_vec))
    stamp_ns = _db_stamp_ns(db_path)
    cache_key = (str(db_path), provider_name, stamp_ns, vector_bytes)
    vec_cache = _numpy_vec_cache()
    if cache_key not in vec_cache:
        vec_cache[cache_key] = _load_vec_matrix_for_search(
            conn, provider_name, vector_bytes=vector_bytes
        )
        # Evict stale entries for the same (path, provider) to bound memory
        for key in list(vec_cache):
            if (
                key != cache_key
                and key[0] == cache_key[0]
                and key[1] == cache_key[1]
                and key[3] == cache_key[3]
            ):
                del vec_cache[key]

    matrix, names, row_norms = vec_cache[cache_key]
    if not names:
        return []

    q = np.array(query_vec, dtype=np.float32)
    q_norm = float(np.linalg.norm(q))
    if q_norm == 0.0:
        return []
    q = q / q_norm

    # Single BLAS call: (N, D) @ (D,) → (N,)
    dots = matrix @ q
    safe_norms = np.where(row_norms > 0, row_norms, 1.0)
    sims = (dots / safe_norms).astype(np.float32)

    n = len(names)
    k = min(limit, n)
    if k <= 0:
        return []
    if k == n:
        top_idx = np.argsort(-sims)
    else:
        # argpartition kth is 0-based: k-1 places the k-th largest on the boundary.
        top_idx = np.argpartition(-sims, k - 1)[:k]
        top_idx = top_idx[np.argsort(-sims[top_idx])]

    return [(names[int(i)], float(sims[i])) for i in top_idx]


def _python_embedding_search(
    db_path: Path,
    conn: sqlite3.Connection,
    provider_name: str,
    query_vec: list[float],
    limit: int,
) -> list[tuple[str, float]]:
    """Python search path: optional numpy matmul, else pure-Python cosine loop."""
    if _NUMPY_AVAILABLE:
        return _numpy_matmul_search(db_path, conn, provider_name, query_vec, limit)
    return _python_loop_search(conn, provider_name, query_vec, limit)


def _embedding_search_backend() -> str:
    """Return the configured embedding search backend."""
    backend = os.environ.get("DAGAYN_EMBEDDING_SEARCH_BACKEND", "rust").strip().lower()
    return backend if backend in _EMBEDDING_SEARCH_BACKENDS else "rust"


def _native_embedding_search(
    db_path: str | Path,
    provider_name: str,
    query_vec: list[float],
    limit: int,
) -> list[tuple[str, float]]:
    """Run native Rust embedding search through the PyO3 extension."""
    from dagayn import _core

    return [
        (str(qualified_name), float(score))
        for qualified_name, score in _core.embedding_search(
            db_path,
            provider_name,
            query_vec,
            limit,
        )
    ]


def _native_embedding_search_for_search(
    db_path: str | Path,
    provider_name: str,
    query_vec: list[float],
    limit: int,
) -> list[tuple[str, float]]:
    """Run native search through the public embeddings shim for monkeypatch compatibility."""
    from . import embeddings as emb

    return emb._native_embedding_search(db_path, provider_name, query_vec, limit)


def _native_embedding_search_prewarm(db_path: str | Path, provider_name: str) -> int:
    """Preload the native Rust embedding-search matrix cache."""
    from dagayn import _core

    return int(_core.embedding_search_prewarm(db_path, provider_name))


def _native_embedding_search_prewarm_for_search(db_path: str | Path, provider_name: str) -> int:
    """Prewarm native search through the public embeddings shim for monkeypatch compatibility."""
    from . import embeddings as emb

    return emb._native_embedding_search_prewarm(db_path, provider_name)


class EmbeddingStore:
    """Manages vector embeddings for graph nodes in SQLite."""

    def __init__(
        self,
        db_path: str | Path,
        provider: str | None = None,
        model: str | None = None,
        provider_instance: EmbeddingProvider | None = None,
        text_mode: str | None = None,
        source_root: str | Path | None = None,
    ) -> None:
        self.provider = provider_instance or _get_provider(provider, model=model)
        self.available = self.provider is not None
        self.db_path = Path(db_path)
        self.text_mode = _embedding_text_mode(text_mode)
        self._provider_key_override: str | None = None
        self.source_root = Path(source_root) if source_root is not None else None
        self.graph_facts_by_qualified_name: dict[str, dict[str, list[str]]] = {}
        self._conn = sqlite3.connect(
            str(self.db_path),
            timeout=30,
            check_same_thread=False,
            isolation_level=None,
        )
        self._conn.row_factory = sqlite3.Row
        self._conn.execute("PRAGMA journal_mode=WAL")
        self._conn.execute("PRAGMA synchronous=NORMAL")
        self._conn.execute("PRAGMA cache_size=-32000")  # 32 MB page cache
        self._conn.execute("PRAGMA mmap_size=134217728")  # 128 MB memory-mapped I/O
        self._conn.execute("PRAGMA temp_store=MEMORY")
        self._conn.executescript(_EMBEDDINGS_SCHEMA)
        _ensure_embeddings_schema(self._conn)
        self.last_orphans_removed = 0

        self._conn.commit()

    def __enter__(self) -> "EmbeddingStore":
        return self

    def __exit__(self, exc_type, exc_val, exc_tb) -> None:  # type: ignore[no-untyped-def]
        self.close()

    def close(self) -> None:
        self._conn.close()

    @property
    def provider_key(self) -> str | None:
        if self._provider_key_override is not None:
            return self._provider_key_override
        if self.provider is None:
            return None
        return _embedding_provider_key(self.provider.name, self.text_mode)

    @provider_key.setter
    def provider_key(self, value: str | None) -> None:
        self._provider_key_override = value

    def checkpoint_writes(self, *, truncate: bool = False) -> None:
        """Checkpoint pending WAL pages after embedding writes."""
        mode = "TRUNCATE" if truncate else "PASSIVE"
        try:
            self._conn.execute(f"PRAGMA wal_checkpoint({mode})")
        except sqlite3.Error:
            logger.debug("Could not checkpoint embedding writes", exc_info=True)

    def _provider_key_for_lookup(self) -> str | None:
        """Prefer mode-partitioned rows, falling back to legacy provider rows."""
        if not self.provider:
            return None
        for candidate in embedding_provider_lookup_candidates(
            self.provider_key,
            self.provider.name,
        ):
            count = self._conn.execute(
                "SELECT COUNT(*) FROM embeddings WHERE provider = ?",
                (candidate,),
            ).fetchone()[0]
            if count:
                return candidate
        return self.provider_key or self.provider.name

    def embed_nodes(
        self,
        nodes: list[GraphNode],
        *,
        show_progress: bool = False,
    ) -> int:
        """Compute and store embeddings for a list of nodes."""
        if not self.provider:
            return 0

        # Filter to nodes that need embedding
        provider_name = self.provider_key or self.provider.name
        candidate_nodes = [n for n in nodes if n.kind != "File"]
        if not candidate_nodes:
            return 0

        # Batch-fetch existing hashes in one query instead of N individual SELECTs
        qns = [n.qualified_name for n in candidate_nodes]
        _hash_fetch_batch = 450  # SQLite variable limit is 999
        existing_hashes: dict[str, tuple[str, str]] = {}  # qn -> (text_hash, provider)
        for i in range(0, len(qns), _hash_fetch_batch):
            chunk = qns[i : i + _hash_fetch_batch]
            placeholders = ",".join("?" for _ in chunk)
            rows = self._conn.execute(  # nosec B608
                f"SELECT qualified_name, text_hash, provider FROM embeddings"
                f" WHERE provider = ? AND qualified_name IN ({placeholders})",
                [provider_name, *chunk],
            ).fetchall()
            for r in rows:
                existing_hashes[r["qualified_name"]] = (r["text_hash"], r["provider"])

        to_embed: list[tuple[GraphNode, str, str]] = []
        for node in candidate_nodes:
            text = _node_to_text(
                node,
                source_root=self.source_root,
                text_mode=self.text_mode,
                graph_facts=self.graph_facts_by_qualified_name.get(node.qualified_name),
            )
            text_hash = hashlib.sha256(text.encode()).hexdigest()
            ex = existing_hashes.get(node.qualified_name)
            if ex and ex[0] == text_hash and ex[1] == provider_name:
                continue
            to_embed.append((node, text, text_hash))

        if not to_embed:
            return 0

        # Encode and persist in provider-sized batches. Persisting each batch
        # makes long local embedding runs resumable if a later request stalls.
        api_batch = self.provider.preferred_batch_size
        total = len(to_embed)
        use_progress = show_progress and sys.stderr.isatty()
        start_time = time.monotonic()
        embedded = 0
        slow_batch_seconds = _slow_embed_batch_seconds()

        for i in range(0, total, api_batch):
            batch = to_embed[i : i + api_batch]
            batch_texts = [t for _, t, _ in batch]
            batch_number = (i // api_batch) + 1
            batch_total = (total + api_batch - 1) // api_batch
            batch_started = time.monotonic()
            try:
                vectors = self.provider.embed(batch_texts)
            except _EMBED_PROVIDER_ERRORS as e:
                if len(batch) > 1:
                    embedded += self._embed_nodes_individually_after_batch_failure(
                        batch,
                        provider_name=provider_name,
                        batch_number=batch_number,
                        batch_total=batch_total,
                        original_error=e,
                    )
                    if use_progress:
                        done = min(i + api_batch, total)
                        elapsed = time.monotonic() - start_time
                        _draw_embed_progress(done, total, elapsed, end=(done >= total))
                    continue
                first_qn = batch[0][0].qualified_name if batch else "<empty>"
                raise RuntimeError(
                    "Embedding batch "
                    f"{batch_number}/{batch_total} failed "
                    f"({len(batch_texts)} node(s), first={first_qn!r}): {e}"
                ) from e
            if len(vectors) != len(batch):
                first_qn = batch[0][0].qualified_name if batch else "<empty>"
                raise RuntimeError(
                    "Embedding batch "
                    f"{batch_number}/{batch_total} returned {len(vectors)} vector(s) "
                    f"for {len(batch)} node(s), first={first_qn!r}."
                )
            elapsed_batch = time.monotonic() - batch_started
            if slow_batch_seconds and elapsed_batch >= slow_batch_seconds:
                logger.warning(
                    "Embedding batch %d/%d took %.1fs (%d node(s), first=%r, last=%r)",
                    batch_number,
                    batch_total,
                    elapsed_batch,
                    len(batch),
                    batch[0][0].qualified_name,
                    batch[-1][0].qualified_name,
                )
            self._conn.executemany(
                """INSERT OR REPLACE INTO embeddings (qualified_name, vector, text_hash, provider)
                   VALUES (?, ?, ?, ?)""",
                [
                    (node.qualified_name, _encode_vector(vec), text_hash, provider_name)
                    for (node, _text, text_hash), vec in zip(batch, vectors)
                ],
            )
            self._conn.commit()
            _invalidate_np_vec_cache(self.db_path, provider_name)
            embedded += len(batch)
            if use_progress:
                done = min(i + api_batch, total)
                elapsed = time.monotonic() - start_time
                _draw_embed_progress(done, total, elapsed, end=(done >= total))

        self.checkpoint_writes()

        return embedded

    def _embed_nodes_individually_after_batch_failure(
        self,
        batch: list[tuple[GraphNode, str, str]],
        *,
        provider_name: str,
        batch_number: int,
        batch_total: int,
        original_error: Exception,
    ) -> int:
        """Retry a failed provider batch one node at a time to isolate bad inputs."""
        embedded = 0
        failures: list[tuple[str, str]] = []
        for node, text, text_hash in batch:
            try:
                vectors = self.provider.embed([text]) if self.provider else []
            except _EMBED_PROVIDER_ERRORS as e:
                failures.append((node.qualified_name, str(e)))
                continue
            if len(vectors) != 1:
                failures.append(
                    (
                        node.qualified_name,
                        f"returned {len(vectors)} vector(s) for one node",
                    )
                )
                continue
            self._conn.execute(
                """INSERT OR REPLACE INTO embeddings (qualified_name, vector, text_hash, provider)
                   VALUES (?, ?, ?, ?)""",
                (node.qualified_name, _encode_vector(vectors[0]), text_hash, provider_name),
            )
            self._conn.commit()
            _invalidate_np_vec_cache(self.db_path, provider_name)
            embedded += 1

        if failures:
            sample = "; ".join(f"{qn}: {err}" for qn, err in failures[:5])
            more = "" if len(failures) <= 5 else f"; ... +{len(failures) - 5} more"
            raise RuntimeError(
                "Embedding batch "
                f"{batch_number}/{batch_total} failed as a batch "
                f"({len(batch)} node(s)): {original_error}. "
                f"Isolated {len(failures)} failing node(s): {sample}{more}"
            ) from original_error

        logger.warning(
            "Embedding batch %d/%d failed as a batch but all %d node(s) succeeded "
            "when retried individually: %s",
            batch_number,
            batch_total,
            len(batch),
            original_error,
        )
        return embedded

    def search(self, query: str, limit: int = 20) -> list[tuple[str, float]]:
        """Search for nodes by semantic similarity.

        The default ``rust`` backend uses the Rust native search path. Set
        ``DAGAYN_EMBEDDING_SEARCH_BACKEND=auto`` to fall back to the Python
        path when native search is unavailable, or ``python`` to force the
        Python path for A/B testing.

        On the Python path, an optional numpy BLAS matmul is used when numpy is
        installed (``pip install dagayn[numpy]``); otherwise a pure-Python
        cosine loop runs. Ranking matches within float tolerance across both.
        """
        if not self.provider:
            return []

        provider_name = self._provider_key_for_lookup()
        if provider_name is None:
            return []
        query_vec = _embed_query_cached(self.provider, query)
        backend = _embedding_search_backend()

        if backend in {"auto", "rust"}:
            try:
                return _native_embedding_search_for_search(
                    self.db_path,
                    provider_name,
                    query_vec,
                    limit,
                )
            except (ImportError, AttributeError, RuntimeError, ValueError, TypeError):
                if backend == "rust":
                    raise
                logger.debug("Native embedding search unavailable; falling back", exc_info=True)

        # Python path: optional numpy matmul, else pure-Python cosine loop
        return _python_embedding_search(
            self.db_path,
            self._conn,
            provider_name,
            query_vec,
            limit,
        )

    def prewarm_search(self) -> int:
        """Preload the configured provider's native search matrix cache."""
        if not self.provider:
            return 0
        provider_name = self._provider_key_for_lookup()
        if provider_name is None:
            return 0
        return _native_embedding_search_prewarm_for_search(self.db_path, provider_name)

    def remove_node(self, qualified_name: str) -> None:
        self._conn.execute("DELETE FROM embeddings WHERE qualified_name = ?", (qualified_name,))
        self._conn.commit()
        _invalidate_np_vec_cache(self.db_path)

    def remove_orphans(
        self,
        live_qualified_names: set[str],
        *,
        all_providers: bool = False,
    ) -> int:
        """Delete embeddings whose nodes no longer exist.

        When ``all_providers`` is false (default), only rows for the configured
        provider are removed. Pass ``all_providers=True`` to reclaim abandoned
        vectors from other providers as well.
        """
        if all_providers:
            rows = self._conn.execute("SELECT qualified_name, provider FROM embeddings").fetchall()
            orphan_rows = [
                (row["qualified_name"], row["provider"])
                for row in rows
                if row["qualified_name"] not in live_qualified_names
            ]
        else:
            if not self.provider:
                return 0

            provider_name = self.provider_key or self.provider.name
            provider_names = [provider_name]
            if self.provider.name != provider_name:
                provider_names.append(self.provider.name)
            placeholders = ",".join("?" for _ in provider_names)
            rows = self._conn.execute(
                f"SELECT qualified_name, provider FROM embeddings WHERE provider IN ({placeholders})",  # nosec B608
                provider_names,
            ).fetchall()
            orphan_rows = [
                (row["qualified_name"], row["provider"])
                for row in rows
                if row["qualified_name"] not in live_qualified_names
            ]
        if not orphan_rows:
            return 0

        batch_size = 450
        deleted = 0
        providers_to_clean = (
            sorted({provider for _, provider in orphan_rows}) if all_providers else provider_names
        )
        for provider_to_clean in providers_to_clean:
            names = [qn for qn, provider in orphan_rows if provider == provider_to_clean]
            for i in range(0, len(names), batch_size):
                chunk = names[i : i + batch_size]
                name_placeholders = ",".join("?" for _ in chunk)
                cursor = self._conn.execute(  # nosec B608
                    "DELETE FROM embeddings "
                    f"WHERE provider = ? AND qualified_name IN ({name_placeholders})",
                    [provider_to_clean, *chunk],
                )
                deleted += cursor.rowcount if cursor.rowcount is not None else len(chunk)
        self._conn.commit()
        if deleted:
            _invalidate_np_vec_cache(self.db_path)
        return deleted

    def persist_active_provider_metadata(self) -> None:
        """Record the configured provider key as the active embedding provider."""
        provider_name = self._provider_key_for_lookup()
        if provider_name is None:
            return
        _persist_active_embedding_provider_metadata(self._conn, provider_name)

    def count(self) -> int:
        return self._conn.execute("SELECT COUNT(*) FROM embeddings").fetchone()[0]

    def count_provider(self, *, dimension: int | None = None) -> int:
        if not self.provider:
            return 0
        provider_name = self._provider_key_for_lookup()
        if provider_name is None:
            return 0
        if dimension is None:
            return self._conn.execute(
                "SELECT COUNT(*) FROM embeddings WHERE provider = ?",
                (provider_name,),
            ).fetchone()[0]
        return self._conn.execute(
            "SELECT COUNT(*) FROM embeddings WHERE provider = ? AND length(vector) = ?",
            (provider_name, _vector_byte_length(dimension)),
        ).fetchone()[0]

    def stored_vector_dimension(self) -> int | None:
        """Return the dimension of the first stored vector for this provider, if any."""
        provider_name = self._provider_key_for_lookup()
        if provider_name is None:
            return None
        row = self._conn.execute(
            "SELECT length(vector) FROM embeddings WHERE provider = ? LIMIT 1",
            (provider_name,),
        ).fetchone()
        if not row or not row[0]:
            return None
        byte_len = int(row[0])
        if byte_len % 4 != 0:
            return None
        return byte_len // 4


def _draw_embed_progress(done: int, total: int, elapsed: float, *, end: bool = False) -> None:
    """Draw a single-line embedding progress bar to stderr."""
    if total == 0:
        return
    pct = done / total
    width = 20
    filled = int(width * pct)
    bar = "█" * filled + "░" * (width - filled)
    rate = done / elapsed if elapsed > 0 else 0
    if rate > 0 and done < total:
        secs_left = (total - done) / rate
        eta = f"{int(secs_left // 60)}:{int(secs_left % 60):02d}"
    else:
        eta = "--:--"
    line = f"\rEmbedding  [{bar}]  {done}/{total}  {pct:3.0%}  {rate:.1f} nodes/s  ETA {eta}"
    print(line, end="\n" if end else "", flush=True, file=sys.stderr)


def embed_all_nodes(
    graph_store: GraphStore,
    embedding_store: EmbeddingStore,
    *,
    show_progress: bool = False,
) -> int:
    """Embed all non-file nodes in the graph."""
    if not embedding_store.available:
        return 0

    all_nodes = graph_store.get_all_nodes(exclude_files=True)
    embedding_store.last_orphans_removed = embedding_store.remove_orphans(
        {node.qualified_name for node in all_nodes},
        all_providers=True,
    )
    embedding_store.persist_active_provider_metadata()

    if embedding_store.source_root is None:
        get_repo_root = getattr(graph_store, "get_repo_root", None)
        if callable(get_repo_root):
            embedding_store.source_root = get_repo_root()

    if embedding_store.text_mode == "narrative":
        embedding_store.graph_facts_by_qualified_name = _build_graph_facts_by_qualified_name(
            graph_store,
            all_nodes,
        )
    else:
        embedding_store.graph_facts_by_qualified_name = {}

    return embedding_store.embed_nodes(all_nodes, show_progress=show_progress)
