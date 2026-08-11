"""Hybrid search engine combining FTS5 (BM25) and vector embeddings.

Uses Reciprocal Rank Fusion (RRF) to merge results from full-text search
and semantic similarity, with query-aware kind boosting and context-file
boosting for relevance tuning.
"""

from __future__ import annotations

import json
import logging
import re
import sqlite3
import threading
import time
from pathlib import Path
from typing import TYPE_CHECKING, Any, Optional

from .graph import GraphStore, _sanitize_name
from .graph._fts_tokenize import segment_japanese_fts_text

if TYPE_CHECKING:
    from .embeddings import EmbeddingStore

logger = logging.getLogger(__name__)

_STORE_CLOSE_ERRORS = (OSError, sqlite3.Error, RuntimeError, AttributeError)
_FTS_QUERY_ERRORS = (sqlite3.Error, TypeError, AttributeError, ValueError, RuntimeError)
_EMBEDDING_SEARCH_ERRORS = (OSError, sqlite3.Error, RuntimeError, TypeError, AttributeError)

# Process-level EmbeddingStore cache — mirrors the GraphStore cache in tools/_common.
# Key: (db_path, provider, model).  Invalidated when the database file mtime changes.
_emb_cache: dict[
    tuple[Path, str | None, str | None, str | None, str | None],
    tuple["EmbeddingStore", float],
] = {}
_emb_lock = threading.Lock()
_emb_failure_cache: dict[str, tuple[float, str]] = {}
_EMBEDDING_FAILURE_TTL_SECONDS = 30.0


def _get_cached_emb_store(
    db_path: Path,
    provider: str | None,
    model: str | None,
    provider_name_hint: str | None = None,
    text_mode: str | None = None,
) -> "EmbeddingStore | None":
    """Return a pinned EmbeddingStore, creating or replacing it when the DB mtime changes."""
    try:
        from .embeddings import (
            EmbeddingStore,
            embedding_provider_base_name,
            provider_from_persisted_name,
        )
    except ImportError:
        return None

    try:
        mtime = db_path.stat().st_mtime
    except FileNotFoundError:
        return None

    provider_instance = (
        provider_from_persisted_name(provider_name_hint)
        if provider is None and model is None and provider_name_hint
        else None
    )
    key_hint = embedding_provider_base_name(provider_name_hint) if provider_name_hint else None
    key = (db_path, provider, model, key_hint, text_mode)
    with _emb_lock:
        entry = _emb_cache.get(key)
        if entry is not None:
            cached_store, cached_mtime = entry
            if cached_mtime == mtime:
                return cached_store
            try:
                cached_store.close()
            except _STORE_CLOSE_ERRORS:  # nosec B110
                pass
            del _emb_cache[key]

        emb_store = EmbeddingStore(
            db_path,
            provider=provider,
            model=model,
            provider_instance=provider_instance,
            text_mode=text_mode,
        )
        _emb_cache[key] = (emb_store, mtime)
    return emb_store


# ---------------------------------------------------------------------------
# FTS5 index management
# ---------------------------------------------------------------------------


_FTS_DDL = """
    CREATE VIRTUAL TABLE nodes_fts USING fts5(
        name, qualified_name, file_path, signature, identifier_tokens, doc_text,
        tokenize='porter unicode61'
    )
"""

_IDENT_BOUNDARY_RE = re.compile(r"(?<=[a-z0-9])(?=[A-Z])|(?<=[A-Z])(?=[A-Z][a-z])")
_IDENT_SPLIT_RE = re.compile(r"[^A-Za-z0-9]+")
_MARKDOWN_HEADING_RE = re.compile(r"^(#{1,6})\s+")


def _identifier_search_text(*values: object) -> str:
    """Return identifier-friendly tokens for FTS (camel/snake/path split)."""
    tokens: list[str] = []
    for value in values:
        if not value:
            continue
        for chunk in _IDENT_SPLIT_RE.split(str(value)):
            if not chunk:
                continue
            tokens.extend(part.lower() for part in _IDENT_BOUNDARY_RE.sub(" ", chunk).split())
    return " ".join(tokens)


def _resolve_node_file(repo_root: Path | None, file_path_value: str) -> Path | None:
    file_path = Path(file_path_value)
    if not file_path.is_absolute():
        if repo_root is None:
            return None
        file_path = repo_root / file_path
    return file_path


def _read_node_source_excerpt(
    repo_root: Path | None,
    row: Any,
    file_lines_cache: dict[Path, list[str] | None] | None = None,
) -> str:
    """Read a bounded source/doc span for FTS, best-effort and side-effect free."""
    file_path = _resolve_node_file(repo_root, row["file_path"])
    if file_path is None:
        return ""
    if file_lines_cache is not None and file_path in file_lines_cache:
        lines = file_lines_cache[file_path]
    else:
        try:
            lines = file_path.read_text(encoding="utf-8", errors="replace").splitlines()
        except OSError:
            lines = None
        if file_lines_cache is not None:
            file_lines_cache[file_path] = lines
    if lines is None:
        return ""

    line_start = row["line_start"] or 1
    line_end = row["line_end"] or line_start
    start = max(int(line_start) - 1, 0)
    end = min(max(int(line_end), int(line_start)), len(lines))

    if row["kind"] == "DocSection":
        level = None
        if start < len(lines):
            match = _MARKDOWN_HEADING_RE.match(lines[start])
            if match:
                level = len(match.group(1))
        end = len(lines)
        for idx in range(start + 1, len(lines)):
            match = _MARKDOWN_HEADING_RE.match(lines[idx])
            if match and (level is None or len(match.group(1)) <= level):
                end = idx
                break

    return "\n".join(lines[start:end])[:4096]


def _fts_rows(conn: Any, repo_root: Path | None) -> list[tuple]:
    rows = conn.execute(
        "SELECT rowid AS node_rowid, kind, name, qualified_name, file_path, line_start, line_end, "
        "signature, extra FROM nodes"
    ).fetchall()
    out = []
    file_lines_cache: dict[Path, list[str] | None] = {}
    for row in rows:
        try:
            extra = json.loads(row["extra"] or "{}")
        except (TypeError, json.JSONDecodeError):
            extra = {}
        display_name = extra.get("display_name", "")
        identifier_tokens = _identifier_search_text(
            row["name"], row["qualified_name"], row["file_path"], display_name
        )
        doc_text = " ".join(
            part
            for part in (
                str(display_name) if display_name else "",
                _read_node_source_excerpt(repo_root, row, file_lines_cache),
            )
            if part
        )
        doc_text = segment_japanese_fts_text(doc_text)
        out.append(
            (
                row["node_rowid"],
                row["name"],
                row["qualified_name"],
                row["file_path"],
                row["signature"] or "",
                identifier_tokens,
                doc_text,
            )
        )
    return out


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
        # Drop and recreate the FTS table. This is intentionally not an
        # external-content FTS table: identifier_tokens/doc_text are generated
        # search fields, not columns in nodes.
        conn.execute("DROP TABLE IF EXISTS nodes_fts")
        conn.execute(_FTS_DDL)

        repo_root_value = getattr(store, "get_metadata", lambda _key: None)("repo_root")
        repo_root = Path(repo_root_value) if repo_root_value else None
        conn.executemany(
            "INSERT INTO nodes_fts(rowid, name, qualified_name, file_path, signature, "
            "identifier_tokens, doc_text) VALUES (?, ?, ?, ?, ?, ?, ?)",
            _fts_rows(conn, repo_root),
        )

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

_CODE_INTENT_TERMS = frozenset(
    {
        "code",
        "function",
        "implementation",
        "implements",
        "logic",
        "helper",
        "wrapper",
        "path",
        "handler",
        "method",
        "class",
        "rust",
        "python",
        "typescript",
        "test",
        "tests",
    }
)

_DOC_INTENT_TERMS = frozenset(
    {
        "documentation",
        "readme",
        "usage",
        "guide",
        "section",
        "instructions",
    }
)

_PROCESS_PATTERN_TERMS = frozenset(
    {
        "assigns",
        "branches",
        "builds",
        "calls",
        "computes",
        "converts",
        "creates",
        "deletes",
        "detects",
        "embedding",
        "embeddings",
        "embeds",
        "fetches",
        "filters",
        "inserts",
        "iterates",
        "loads",
        "loops",
        "merges",
        "opens",
        "parses",
        "queries",
        "ranks",
        "reads",
        "rebuilds",
        "renders",
        "returns",
        "searches",
        "stores",
        "tested",
        "updates",
        "uses",
        "validates",
        "writes",
    }
)

_PURPOSE_QUERY_TERMS = frozenset(
    {
        "behavior",
        "feature",
        "goal",
        "handles",
        "logic",
        "purpose",
        "responsible",
        "supports",
        "workflow",
    }
)

_TOKEN_RE = re.compile(r"[A-Za-z0-9_]+")


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


def _query_tokens(query: str) -> set[str]:
    return {t.lower() for t in _TOKEN_RE.findall(query)}


def _query_rerank_intent(query: str, query_tokens: set[str]) -> str:
    stripped = query.strip()
    tokens = list(_TOKEN_RE.findall(stripped))
    if not stripped:
        return "empty"
    if "." in stripped or "::" in stripped or _extract_identifiers(stripped):
        return "exact"
    if query_tokens & _DOC_INTENT_TERMS:
        return "documentation"
    if query_tokens & _PROCESS_PATTERN_TERMS:
        return "process_pattern"
    if len(tokens) >= 2 or query_tokens & _PURPOSE_QUERY_TERMS:
        return "purpose"
    return "exact"


def _embedding_text_mode_for_intent(rerank_intent: str) -> str:
    if rerank_intent == "process_pattern":
        return "narrative"
    return "material"


def _is_markdown_node(node: Any) -> bool:
    return node.kind == "DocSection" or str(node.file_path).lower().endswith(".md")


def _split_identifier_terms(value: str) -> set[str]:
    return {
        part.lower()
        for part in _IDENT_BOUNDARY_RE.sub(" ", value.replace("_", " ")).split()
        if part
    }


def _intent_boost(
    query_tokens: set[str],
    node: Any,
    fts_rank: int | None,
    emb_rank: int | None,
    *,
    hybrid_mode: bool,
    rerank_intent: str | None = None,
) -> float:
    """Return a small reranking multiplier from query intent and arm ranks.

    RRF is intentionally broad: it is good at recall, but long natural-language
    queries often put nearby docs/tests/wrappers ahead of the specific code
    artifact the user asked for.  This boost keeps the ranking explainable:
    exact FTS evidence still matters, semantic-only hits still surface, and
    docs are favored only when the query asks for docs.
    """
    if not hybrid_mode:
        return 1.0

    boost = 1.0
    rerank_intent = rerank_intent or "purpose"
    code_intent = bool(query_tokens & _CODE_INTENT_TERMS)
    doc_intent = bool(query_tokens & _DOC_INTENT_TERMS)
    test_intent = bool(query_tokens & {"test", "tests", "coverage", "proves"})
    markdown_node = _is_markdown_node(node)
    code_node = node.kind in {"Function", "Class", "Type", "Test"}

    if fts_rank is not None and fts_rank <= 3:
        boost *= 1.25
    if fts_rank == 1:
        boost *= 1.15
    if emb_rank == 1:
        boost *= 1.30
    if fts_rank is not None and emb_rank is not None:
        boost *= 1.15

    if rerank_intent == "process_pattern":
        if emb_rank is not None:
            boost *= 1.55
            if emb_rank <= 5:
                boost *= 1.35
            elif emb_rank <= 20:
                boost *= 1.15
        if code_node:
            boost *= 1.60
        if node.kind == "Function":
            boost *= 1.25
        if markdown_node:
            boost *= 0.18
        if bool(getattr(node, "is_test", False)) and not test_intent:
            boost *= 0.55
    elif rerank_intent == "purpose":
        if fts_rank is not None and emb_rank is not None:
            boost *= 1.40
        elif emb_rank is not None and emb_rank <= 5:
            boost *= 1.15
        if code_node:
            boost *= 1.10
        if markdown_node and not doc_intent:
            boost *= 0.75
            if code_intent:
                boost *= 0.55

    if code_intent and not doc_intent:
        if markdown_node:
            boost *= 0.45
        elif code_node:
            boost *= 1.18

    if doc_intent:
        if node.kind == "DocSection":
            boost *= 1.35
        elif markdown_node:
            boost *= 1.15

    if test_intent:
        if bool(getattr(node, "is_test", False)) or str(node.file_path).startswith("tests/"):
            boost *= 1.55

    name_terms = _split_identifier_terms(str(node.name))
    if name_terms and name_terms.issubset(query_tokens):
        boost *= 1.70 if node.kind in {"Function", "Class", "Type", "Test"} else 1.30

    return boost


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
    text_mode: str | None = None,
) -> list[tuple[int, float]]:
    """Run a vector similarity search using the embedding store.

    Returns list of ``(node_id, similarity_score)`` tuples.
    Gracefully returns an empty list if embeddings are not available.
    """
    return _embedding_search_with_health(store, query, limit, model, provider, text_mode)[0]


def _embedding_provider_counts(db_path: Path) -> dict[str, int]:
    try:
        conn = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
        try:
            rows = conn.execute(
                "SELECT provider, COUNT(*) FROM embeddings GROUP BY provider"
            ).fetchall()
        finally:
            conn.close()
    except sqlite3.Error:
        return {}
    return {str(provider): int(count) for provider, count in rows}


def _single_provider_name(
    provider_counts: dict[str, int],
    *,
    text_mode: str | None = None,
) -> str | None:
    if text_mode:
        matches = [
            provider_name
            for provider_name in provider_counts
            if provider_name.endswith(f"#text={text_mode}")
        ]
        if len(matches) == 1:
            return matches[0]
        legacy = [
            provider_name for provider_name in provider_counts if "#text=" not in provider_name
        ]
        return legacy[0] if len(legacy) == 1 and len(provider_counts) == 1 else None
    return next(iter(provider_counts)) if len(provider_counts) == 1 else None


def _largest_populated_text_mode_partition(
    provider_counts: dict[str, int],
    *,
    base_provider: str | None = None,
) -> tuple[str, int] | None:
    """Return the largest populated ``#text=`` partition, optionally scoped to one provider."""
    from .embeddings import embedding_provider_base_name

    candidates: list[tuple[str, int]] = []
    for provider_key, count in provider_counts.items():
        if count <= 0 or "#text=" not in provider_key:
            continue
        if base_provider and embedding_provider_base_name(provider_key) != base_provider:
            continue
        candidates.append((provider_key, count))
    if not candidates:
        return None
    return max(candidates, key=lambda item: item[1])


def _embedding_search_with_health(
    store: GraphStore,
    query: str,
    limit: int = 50,
    model: str | None = None,
    provider: str | None = None,
    text_mode: str | None = None,
) -> tuple[list[tuple[int, float]], dict[str, Any]]:
    """Run vector search and return health metadata for fallback diagnosis."""
    provider_counts = _embedding_provider_counts(store.db_path)
    health: dict[str, Any] = {
        "status": "unknown",
        "requested_provider": provider,
        "requested_model": model,
        "requested_text_mode": text_mode,
        "resolved_provider": None,
        "resolved_provider_key": None,
        "auto_resolved_provider": None,
        "matching_vector_count": 0,
        "provider_counts": provider_counts,
    }
    provider_name_hint = (
        _single_provider_name(provider_counts, text_mode=text_mode)
        if provider is None and model is None
        else None
    )

    try:
        emb_store = _get_cached_emb_store(
            store.db_path,
            provider,
            model,
            provider_name_hint=provider_name_hint,
            text_mode=text_mode,
        )
        if emb_store is None or not emb_store.available or emb_store.provider is None:
            health["status"] = "provider_unavailable"
            return [], health

        provider_name = emb_store.provider.name
        provider_key = emb_store.provider_key or provider_name
        matching_count = emb_store.count_provider()
        if (
            matching_count == 0
            and provider_name_hint
            and "#text=" not in provider_name_hint
            and provider_name_hint == provider_name
        ):
            emb_store.provider_key = provider_name_hint
            provider_key = provider_name_hint
            matching_count = emb_store.count_provider()
        health["resolved_provider"] = provider_name
        health["resolved_provider_key"] = provider_key
        if provider_name_hint in {provider_name, provider_key}:
            health["auto_resolved_provider"] = provider_name_hint
        health["matching_vector_count"] = matching_count

        if matching_count == 0 and text_mode:
            from .embeddings import embedding_provider_text_mode

            fallback = _largest_populated_text_mode_partition(
                provider_counts,
                base_provider=provider_name,
            )
            if fallback is not None:
                fallback_key, fallback_count = fallback
                fallback_mode = embedding_provider_text_mode(fallback_key)
                if fallback_mode and fallback_mode != text_mode:
                    emb_store = _get_cached_emb_store(
                        store.db_path,
                        provider,
                        model,
                        provider_name_hint=fallback_key,
                        text_mode=fallback_mode,
                    )
                    if emb_store is not None and emb_store.available and emb_store.provider is not None:
                        provider_key = emb_store.provider_key or fallback_key
                        matching_count = emb_store.count_provider() or fallback_count
                        health["resolved_provider_key"] = provider_key
                        health["resolved_text_mode"] = fallback_mode
                        health["text_mode_fallback"] = {
                            "from": text_mode,
                            "to": fallback_mode,
                            "provider_key": fallback_key,
                            "vector_count": matching_count,
                        }
                        health["matching_vector_count"] = matching_count

        if matching_count == 0:
            health["status"] = "provider_mismatch" if provider_counts else "missing_vectors"
            return [], health

        from .embeddings import embedding_provider_text_mode

        if "resolved_text_mode" not in health:
            health["resolved_text_mode"] = (
                embedding_provider_text_mode(provider_key) or text_mode
            )

        failed_at, failure = _emb_failure_cache.get(provider_key, (0.0, ""))
        if failed_at and time.monotonic() - failed_at < _EMBEDDING_FAILURE_TTL_SECONDS:
            health["status"] = "search_failed_recent"
            health["error"] = failure
            return [], health

        results = emb_store.search(query, limit=limit)
        nodes_by_qn = store.get_nodes_by_qualified_names([qn for qn, _ in results])
        id_scores: list[tuple[int, float]] = []
        for qn, score in results:
            node = nodes_by_qn.get(qn)
            if node:
                id_scores.append((node.id, score))
        health["status"] = "available"
        _emb_failure_cache.pop(provider_key, None)
        return id_scores, health
    except ValueError as e:
        message = str(e)
        health["status"] = (
            "missing_provider_env"
            if "Missing required environment variable" in message
            else "provider_config_error"
        )
        health["error"] = message
        logger.warning("Embedding search failed: %s", e)
        return [], health
    except _EMBEDDING_SEARCH_ERRORS as e:
        health["status"] = "search_failed"
        health["error"] = str(e)
        provider_name = health.get("resolved_provider")
        provider_key = health.get("resolved_provider_key") or provider_name
        if isinstance(provider_key, str) and provider_key:
            _emb_failure_cache[provider_key] = (time.monotonic(), str(e))
        logger.warning("Embedding search failed: %s", e)
        return [], health


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
    query_tokens = _query_tokens(query)
    rerank_intent = _query_rerank_intent(query, query_tokens)
    embedding_text_mode = _embedding_text_mode_for_intent(rerank_intent)

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
            except _FTS_QUERY_ERRORS as e:
                logger.debug("FTS5 sub-query failed for %r: %s", ident, e)
                continue
            if extra:
                fts_results.extend(extra)
    except _FTS_QUERY_ERRORS as e:
        logger.warning("FTS5 unavailable, will use fallback: %s", e)

    # Try embedding search
    emb_results, embedding_health = _embedding_search_with_health(
        store,
        query,
        limit=fetch_limit,
        model=model,
        provider=provider,
        text_mode=embedding_text_mode,
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
            return {"mode": "empty", "results": [], "embedding_health": embedding_health}
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
    fts_rank_by_id: dict[int, int] = {}
    emb_rank_by_id: dict[int, int] = {}
    for rank, (nid, _score) in enumerate(fts_results, start=1):
        fts_rank_by_id.setdefault(nid, rank)
    for rank, (nid, _score) in enumerate(emb_results, start=1):
        emb_rank_by_id.setdefault(nid, rank)

    # ------ Phase 3+4: Batch-fetch nodes, apply boosting and kind filter ------
    kind_boosts = detect_query_kind_boost(query)
    context_set = set(context_files) if context_files else set()
    hybrid_mode = bool(fts_results and emb_results)

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
        boost *= _intent_boost(
            query_tokens,
            node,
            fts_rank_by_id.get(node_id),
            emb_rank_by_id.get(node_id),
            hybrid_mode=hybrid_mode,
            rerank_intent=rerank_intent,
        )
        if node.is_test:
            # Tests whose names/docstrings mirror the function under test
            # cluster next to that function in embedding space and crowd
            # out the source — deboost so the source wins on semantic
            # queries.  Tests are still returned (not filtered).
            if not (query_tokens & {"test", "tests", "coverage", "proves"}):
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

    return {
        "mode": mode,
        "results": results,
        "embedding_health": embedding_health,
        "rerank_intent": rerank_intent,
    }
