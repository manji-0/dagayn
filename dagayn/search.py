"""Hybrid search engine combining FTS5 (BM25) and vector embeddings.

Uses Reciprocal Rank Fusion (RRF) to merge results from full-text search
and semantic similarity, with query-aware kind boosting and context-file
boosting for relevance tuning.
"""

from __future__ import annotations

import logging
import re
import threading
from pathlib import Path
from typing import TYPE_CHECKING, Any, Optional

from .graph import GraphStore, _sanitize_name

if TYPE_CHECKING:
    from .embeddings import EmbeddingStore

logger = logging.getLogger(__name__)

# Process-level EmbeddingStore cache — mirrors the GraphStore cache in tools/_common.
# Key: (db_path, provider, model).  Invalidated when the database file mtime changes.
_emb_cache: dict[tuple[Path, str | None, str | None], tuple["EmbeddingStore", float]] = {}
_emb_lock = threading.Lock()


def _get_cached_emb_store(
    db_path: Path,
    provider: str | None,
    model: str | None,
) -> "EmbeddingStore | None":
    """Return a pinned EmbeddingStore, creating or replacing it when the DB mtime changes."""
    try:
        from .embeddings import EmbeddingStore
    except ImportError:
        return None

    try:
        mtime = db_path.stat().st_mtime
    except FileNotFoundError:
        return None

    key = (db_path, provider, model)
    with _emb_lock:
        entry = _emb_cache.get(key)
        if entry is not None:
            cached_store, cached_mtime = entry
            if cached_mtime == mtime:
                return cached_store
            try:
                cached_store.close()
            except Exception:  # noqa: BLE001  # nosec B110
                pass
            del _emb_cache[key]

        emb_store = EmbeddingStore(db_path, provider=provider, model=model)
        _emb_cache[key] = (emb_store, mtime)
    return emb_store


# ---------------------------------------------------------------------------
# FTS5 index management
# ---------------------------------------------------------------------------


def rebuild_fts_index(store: GraphStore) -> int:
    """Rebuild the FTS5 index from the nodes table.

    Checks whether the ``nodes_fts`` virtual table exists, clears it, then
    repopulates it from every row in ``nodes``.

    Returns:
        Number of rows indexed.
    """
    rust_rebuild = getattr(store, "rebuild_fts_index", None)
    if callable(rust_rebuild):
        count = int(rust_rebuild())
        logger.info("FTS index rebuilt: %d rows indexed", count)
        return count

    # NOTE: rebuild_fts_index uses store._conn directly because it manages
    # the FTS5 virtual table DDL, which is tightly coupled to SQLite internals.
    # Future Rust-backed stores should expose rebuild_fts_index() natively.
    conn = store._conn

    # Wrap the full DROP + CREATE + INSERT sequence in an explicit transaction
    # so a crash mid-rebuild cannot leave the DB without an FTS table at all
    # (DROP succeeded but CREATE/INSERT didn't).  See #259.
    if conn.in_transaction:
        logger.warning("Rolling back uncommitted transaction before BEGIN IMMEDIATE")
        conn.rollback()
    conn.execute("BEGIN IMMEDIATE")
    try:
        # Drop and recreate the FTS table with content sync to match migration v5
        conn.execute("DROP TABLE IF EXISTS nodes_fts")
        conn.execute("""
            CREATE VIRTUAL TABLE nodes_fts USING fts5(
                name, qualified_name, file_path, signature,
                content='nodes', content_rowid='rowid',
                tokenize='porter unicode61'
            )
        """)

        # Rebuild from the content table (nodes) using the FTS5 rebuild command
        conn.execute("INSERT INTO nodes_fts(nodes_fts) VALUES('rebuild')")

        conn.commit()
    except BaseException:
        conn.rollback()
        raise

    count = conn.execute("SELECT count(*) FROM nodes_fts").fetchone()[0]
    logger.info("FTS index rebuilt: %d rows indexed", count)
    return count


# ---------------------------------------------------------------------------
# Query kind boosting heuristics
# ---------------------------------------------------------------------------


_QUALIFIED_SPLIT_RE = re.compile(r"[./:]+")

# Identifier extraction for natural-language queries (Issue 3 fix).
# Matches anything that looks like a programming identifier; the structural
# filter (snake_case / camelCase / PascalCase) happens in _extract_identifiers.
_IDENT_RE = re.compile(r"[A-Za-z_][A-Za-z0-9_]+")

# English stopwords commonly seen in natural-language code queries.  Any
# token matching one of these (case-insensitively) is rejected even if the
# regex thinks it is identifier-shaped.
_QUERY_STOPWORDS = frozenset(
    {
        "the",
        "a",
        "an",
        "is",
        "are",
        "for",
        "of",
        "to",
        "in",
        "on",
        "by",
        "with",
        "from",
        "and",
        "or",
        "not",
        "where",
        "how",
        "what",
        "which",
        "find",
        "show",
        "list",
        "all",
        "any",
        "this",
        "that",
        "these",
        "those",
        "it",
        "we",
        "do",
        "does",
        "did",
    }
)

# Multiplier applied to is_test=True nodes during boosting so that source
# code outranks the tests that exercise it on semantic queries.  Tests
# remain visible (deboost, not filter) so that queries targeting a test
# by name still surface the test itself.
_TEST_DEBOOST = 0.6


def _extract_identifiers(query: str) -> list[str]:
    """Pull identifier-shaped tokens (snake_case / camelCase / PascalCase) out of
    a natural-language query so each can drive its own FTS arm.

    A token is accepted when it looks like a programming symbol — either
    containing an underscore (``embed_graph``, ``RRF_K``) or having internal
    case variation (``GraphStore``, ``rrfMerge``).  Plain lowercase English
    words (``find``, ``merge``, ``functions``) are rejected.
    """
    out: list[str] = []
    seen: set[str] = set()
    for c in _IDENT_RE.findall(query):
        if c.lower() in _QUERY_STOPWORDS:
            continue
        is_snake = "_" in c
        is_camelish = any(ch.isupper() for ch in c[1:])
        if not (is_snake or is_camelish):
            continue
        if c in seen:
            continue
        seen.add(c)
        out.append(c)
    return out


def _qualified_name_matches(query: str, qualified_name: str) -> bool:
    """True when a dotted query matches a qualified name.

    Accepts an exact lowercased substring (covers ``Class.method`` against
    ``file.py::Class.method``) or an ordered subsequence of dot-separated
    tokens against the qualified name's segments (covers ``api.get_users``
    against ``path/to/api.py::get_users``).
    """
    q = query.lower()
    qn = qualified_name.lower()
    if q in qn:
        return True
    q_tokens = [t for t in _QUALIFIED_SPLIT_RE.split(q) if t]
    if not q_tokens:
        return False
    qn_tokens = [t for t in _QUALIFIED_SPLIT_RE.split(qn) if t]
    i = 0
    for tok in qn_tokens:
        if i < len(q_tokens) and tok == q_tokens[i]:
            i += 1
    return i == len(q_tokens)


def detect_query_kind_boost(query: str) -> dict[str, float]:
    """Detect query patterns and return kind-specific boost multipliers.

    Heuristics:
    - PascalCase queries (e.g. ``MyClass``) boost Class/Type by 1.5x
    - snake_case queries (e.g. ``get_users``) boost Function by 1.5x
    - Queries containing ``.`` boost qualified name matches by 2.0x

    Returns:
        Dict mapping node kind strings to boost multipliers.
    """
    boosts: dict[str, float] = {}

    if not query or not query.strip():
        return boosts

    q = query.strip()

    # PascalCase: starts with uppercase, has at least one lowercase after
    if re.match(r"^[A-Z][a-z]", q) and not q.isupper():
        boosts["Class"] = 1.5
        boosts["Type"] = 1.5

    # snake_case or SCREAMING_SNAKE_CASE: contains underscore with letters
    if "_" in q and re.search(r"[a-zA-Z]", q):
        boosts["Function"] = 1.5

    # Dotted path: boost qualified name matches
    if "." in q:
        boosts["_qualified"] = 2.0

    return boosts


# ---------------------------------------------------------------------------
# Reciprocal Rank Fusion
# ---------------------------------------------------------------------------


def rrf_merge(*result_lists: list[tuple[int, float]], k: int = 10) -> list[tuple[int, float]]:
    """Merge multiple ranked result lists using Reciprocal Rank Fusion.

    Each input list contains ``(id, score)`` tuples, ordered by score
    descending. The RRF score for each item is the sum of
    ``1 / (k + rank + 1)`` across all lists it appears in, where rank is
    the 0-based position.

    Args:
        *result_lists: Variable number of ranked result lists.
        k: RRF constant (default 10).  The textbook value is 60; we use a
           lower constant so the resulting scores spread across ~0.05–0.2
           instead of being compressed into a 0.015–0.016 band, which
           makes the ``score`` field meaningful for comparing results
           within a single query.  Item order is invariant under positive
           ``k`` so this is purely a calibration knob.

    Returns:
        Merged list of ``(id, rrf_score)`` tuples sorted by score descending.
    """
    scores: dict[int, float] = {}

    for result_list in result_lists:
        for rank, (item_id, _score) in enumerate(result_list):
            scores[item_id] = scores.get(item_id, 0.0) + 1.0 / (k + rank + 1)

    merged = sorted(scores.items(), key=lambda x: x[1], reverse=True)
    return merged


# ---------------------------------------------------------------------------
# Embedding search (optional)
# ---------------------------------------------------------------------------


def _embedding_search(
    store: GraphStore,
    query: str,
    limit: int = 50,
    model: str | None = None,
    provider: str | None = None,
) -> list[tuple[int, float]]:
    """Run a vector similarity search using the embedding store.

    Returns list of ``(node_id, similarity_score)`` tuples.
    Gracefully returns an empty list if embeddings are not available.
    """
    try:
        emb_store = _get_cached_emb_store(store.db_path, provider, model)
        if emb_store is None or not emb_store.available or emb_store.count() == 0:
            return []

        results = emb_store.search(query, limit=limit)
        nodes_by_qn = store.get_nodes_by_qualified_names([qn for qn, _ in results])
        id_scores: list[tuple[int, float]] = []
        for qn, score in results:
            node = nodes_by_qn.get(qn)
            if node:
                id_scores.append((node.id, score))
        return id_scores
    except Exception as e:
        logger.warning("Embedding search failed: %s", e)
        return []


# ---------------------------------------------------------------------------
# Main hybrid search
# ---------------------------------------------------------------------------


def hybrid_search(
    store: GraphStore,
    query: str,
    kind: Optional[str] = None,
    limit: int = 20,
    context_files: Optional[list[str]] = None,
    model: Optional[str] = None,
    provider: Optional[str] = None,
) -> dict[str, Any]:
    """Hybrid search combining FTS5 BM25 and vector embeddings via RRF.

    Attempts FTS5 + embedding search first, falling back to FTS5-only,
    then keyword LIKE matching if FTS5 is unavailable.

    Args:
        store: The graph store to search.
        query: Search query string.
        kind: Optional node kind filter (e.g. ``"Function"``, ``"Class"``).
        limit: Maximum results to return (default 20).
        context_files: Optional list of file paths. Nodes in these files
            receive a 1.5x score boost.

    Returns:
        Dict with keys:
        - ``"mode"``: which search arms contributed — one of ``"hybrid"``,
          ``"fts_only"``, ``"embedding_only"``, ``"keyword_fallback"``,
          ``"empty"``.
        - ``"results"``: list of dicts with node metadata, ``"score"``,
          ``"rank"`` (1-based position in final sorted list),
          ``"source"`` (``"fts"``, ``"embedding"``, ``"both"``, ``"keyword"``,
          or ``"doc"`` for Markdown DocSection nodes), and ``"is_test"``
          (``True`` when the node was detected as test code; such nodes
          are deboosted so source code outranks the tests for it).
    """
    if not query or not query.strip():
        return {"mode": "empty", "results": []}

    fetch_limit = limit * 3  # Fetch extra to allow for filtering and boosting

    # ------ Phase 1: Gather ranked lists ------
    fts_results: list[tuple[int, float]] = []
    emb_results: list[tuple[int, float]] = []

    # Try FTS5 search via protocol method
    try:
        fts_results = store.fts_query(query, limit=fetch_limit)
        # Additional FTS arms for each identifier-shaped token in the query
        # so natural-language phrases like "tests for embed_graph" still
        # match the embed_graph symbol directly.  Extra hits accumulate
        # into the same list; rrf_merge handles repeated ids additively.
        for ident in _extract_identifiers(query):
            try:
                extra = store.fts_query(ident, limit=fetch_limit)
            except Exception as e:
                logger.debug("FTS5 sub-query failed for %r: %s", ident, e)
                continue
            if extra:
                fts_results.extend(extra)
    except Exception as e:
        logger.warning("FTS5 unavailable, will use fallback: %s", e)

    # Try embedding search
    emb_results = _embedding_search(
        store,
        query,
        limit=fetch_limit,
        model=model,
        provider=provider,
    )

    # ------ Phase 2: Merge via RRF or fallback ------
    keyword_mode = False
    if fts_results or emb_results:
        lists_to_merge = []
        if fts_results:
            lists_to_merge.append(fts_results)
        if emb_results:
            lists_to_merge.append(emb_results)
        merged = rrf_merge(*lists_to_merge)
    else:
        # Fallback: keyword LIKE matching via protocol method
        keyword_results = store.keyword_query(query, limit=fetch_limit)
        if not keyword_results:
            return {"mode": "empty", "results": []}
        merged = keyword_results
        keyword_mode = True

    # Determine top-level mode
    if keyword_mode:
        mode = "keyword_fallback"
    elif fts_results and emb_results:
        mode = "hybrid"
    elif fts_results:
        mode = "fts_only"
    else:
        mode = "embedding_only"

    # Track per-arm node sets for per-result source tagging
    fts_ids: set[int] = {nid for nid, _ in fts_results}
    emb_ids: set[int] = {nid for nid, _ in emb_results}

    # ------ Phase 3+4: Batch-fetch nodes, apply boosting and kind filter ------
    kind_boosts = detect_query_kind_boost(query)
    context_set = set(context_files) if context_files else set()

    candidate_ids = [node_id for node_id, _ in merged]
    node_map = store.get_nodes_by_ids(candidate_ids)

    # Apply boosting
    boosted: list[tuple[int, float]] = []
    for node_id, score in merged:
        node = node_map.get(node_id)
        if not node:
            continue

        boost = 1.0
        if node.kind in kind_boosts:
            boost *= kind_boosts[node.kind]
        if "_qualified" in kind_boosts:
            if _qualified_name_matches(query, node.qualified_name):
                boost *= kind_boosts["_qualified"]
        if context_set and node.file_path in context_set:
            boost *= 1.5
        if node.is_test:
            # Tests whose names/docstrings mirror the function under test
            # cluster next to that function in embedding space and crowd
            # out the source — deboost so the source wins on semantic
            # queries.  Tests are still returned (not filtered).
            boost *= _TEST_DEBOOST

        boosted.append((node_id, score * boost))

    boosted.sort(key=lambda x: x[1], reverse=True)

    # Build results
    results: list[dict[str, Any]] = []
    for node_id, final_score in boosted:
        if len(results) >= limit:
            break

        node = node_map.get(node_id)
        if not node:
            continue

        if kind and node.kind != kind:
            continue

        if node.kind == "DocSection":
            source = "doc"
        elif keyword_mode:
            source = "keyword"
        elif node_id in fts_ids and node_id in emb_ids:
            source = "both"
        elif node_id in fts_ids:
            source = "fts"
        else:
            source = "embedding"

        results.append(
            {
                "name": _sanitize_name(node.name),
                "qualified_name": _sanitize_name(node.qualified_name),
                "kind": node.kind,
                "file_path": node.file_path,
                "line_start": node.line_start,
                "line_end": node.line_end,
                "language": node.language or "",
                "params": node.params,
                "return_type": node.return_type,
                "signature": getattr(node, "signature", None),
                "score": round(final_score, 6),
                "rank": len(results) + 1,
                "source": source,
                "is_test": bool(node.is_test),
            }
        )

    return {"mode": mode, "results": results}
