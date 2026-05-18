"""Fuzzy documentation search benchmark comparing FTS and embeddings.

The benchmark focuses on natural-language queries whose wording differs from
the target Markdown section.  It uses a deterministic synonym-aware embedding
provider so CI can measure the retrieval shape without external model access.
"""

from __future__ import annotations

import hashlib
import math
import re
import time
from collections import Counter, defaultdict
from fnmatch import fnmatch
from pathlib import Path
from typing import Any

from dagayn.embeddings import EmbeddingProvider

_TOKEN_RE = re.compile(r"[A-Za-z][A-Za-z0-9_-]*")
_IDENT_BOUNDARY_RE = re.compile(r"(?<=[a-z0-9])(?=[A-Z])|(?<=[A-Z])(?=[A-Z][a-z])")
_MARKDOWN_HEADING_RE = re.compile(r"^(#{1,6})\s+")
_DOC_KINDS = {"DocSection", "DocBody"}
_DEFAULT_LIMIT = 20
_DEFAULT_RELEVANCE_GRADE = 3

_SYNONYM_GROUPS: dict[str, tuple[str, ...]] = {
    "semantic": ("semantic", "meaning", "concept", "conceptual", "fuzzy", "vague"),
    "search": ("search", "find", "lookup", "retrieve", "retrieval", "discover"),
    "documentation": ("documentation", "document", "documents", "docs", "guide", "readme"),
    "embedding": ("embedding", "embeddings", "vector", "vectors", "similarity"),
    "identifier": ("identifier", "identifiers", "symbol", "symbols", "name", "names", "exact"),
    "refresh": ("refresh", "update", "updates", "build", "rebuild", "stale", "current"),
    "graph": ("graph", "repository", "repo", "knowledge"),
    "retry": ("retry", "retries", "retrying", "recover", "recovery", "again"),
    "failure": ("failure", "failures", "error", "errors", "transient", "temporary"),
    "overload": ("overload", "overwhelming", "hammering", "herd", "thundering"),
    "dependency": ("dependency", "dependencies", "upstream", "service", "services", "backend"),
    "secret": ("secret", "secrets", "credential", "credentials", "token", "tokens", "key"),
    "schema": ("schema", "database", "migration", "migrations", "version", "versions"),
}

_SYNONYM_BY_TOKEN = {
    token: canonical for canonical, tokens in _SYNONYM_GROUPS.items() for token in tokens
}


class _DocFuzzyEmbeddingProvider(EmbeddingProvider):
    preferred_batch_size = 128

    def __init__(self, dimension: int = 256) -> None:
        self._dimension = dimension

    @property
    def name(self) -> str:
        return "eval-doc-fuzzy-hash"

    @property
    def dimension(self) -> int:
        return self._dimension

    def embed(self, texts: list[str]) -> list[list[float]]:
        return [self._vectorize(text) for text in texts]

    def embed_query(self, text: str) -> list[float]:
        return self._vectorize(text)

    def _vectorize(self, text: str) -> list[float]:
        counts: Counter[int] = Counter()
        for token in _semantic_tokens(text):
            digest = hashlib.blake2b(token.encode("utf-8"), digest_size=4).digest()
            counts[int.from_bytes(digest, "little") % self._dimension] += 1
        if not counts:
            return [0.0] * self._dimension
        norm = math.sqrt(sum(value * value for value in counts.values()))
        vector = [0.0] * self._dimension
        for idx, value in counts.items():
            vector[idx] = value / norm
        return vector


def _semantic_tokens(text: str) -> list[str]:
    tokens: list[str] = []
    for raw in _TOKEN_RE.findall(text):
        normalized = raw.replace("-", "_")
        for part in _IDENT_BOUNDARY_RE.sub(" ", normalized).replace("_", " ").lower().split():
            tokens.append(part)
            canonical = _SYNONYM_BY_TOKEN.get(part)
            if canonical:
                tokens.append(canonical)
    return tokens


def _cosine(left: list[float], right: list[float]) -> float:
    if not left or not right:
        return 0.0
    dot = sum(a * b for a, b in zip(left, right))
    left_norm = math.sqrt(sum(a * a for a in left))
    right_norm = math.sqrt(sum(b * b for b in right))
    if left_norm == 0.0 or right_norm == 0.0:
        return 0.0
    return dot / (left_norm * right_norm)


def _read_line_span(repo_path: Path, node: Any, max_chars: int = 4096) -> str:
    file_path = Path(node.file_path)
    if not file_path.is_absolute():
        file_path = repo_path / file_path
    try:
        lines = file_path.read_text(encoding="utf-8", errors="replace").splitlines()
    except OSError:
        return ""

    start = max(int(node.line_start or 1) - 1, 0)
    if start >= len(lines):
        return ""
    line_start = int(node.line_start or 1)
    line_end = int(node.line_end or line_start)
    end = min(max(line_end, line_start), len(lines))
    return "\n".join(lines[start:end])[:max_chars]


def _read_doc_section(repo_path: Path, node: Any, max_chars: int = 4096) -> str:
    if node.kind == "DocBody":
        return _read_line_span(repo_path, node, max_chars=max_chars)

    file_path = Path(node.file_path)
    if not file_path.is_absolute():
        file_path = repo_path / file_path
    try:
        lines = file_path.read_text(encoding="utf-8", errors="replace").splitlines()
    except OSError:
        return ""

    start = max(int(node.line_start or 1) - 1, 0)
    if start >= len(lines):
        return ""

    level = None
    match = _MARKDOWN_HEADING_RE.match(lines[start])
    if match:
        level = len(match.group(1))

    end = len(lines)
    for idx in range(start + 1, len(lines)):
        match = _MARKDOWN_HEADING_RE.match(lines[idx])
        if match and (level is None or len(match.group(1)) <= level):
            end = idx
            break

    return "\n".join(lines[start:end])[:max_chars]


def _doc_text(repo_path: Path, node: Any) -> str:
    parts = [
        str(node.name),
        str(node.qualified_name),
        str(node.file_path).replace("/", " "),
        str(node.language or ""),
    ]
    if isinstance(getattr(node, "extra", None), dict):
        display_name = node.extra.get("display_name")
        if display_name:
            parts.append(str(display_name))
    body = _read_doc_section(repo_path, node)
    if body:
        parts.append(body)
    return " ".join(part for part in parts if part)


def _path_matches(path: str, pattern: str) -> bool:
    normalized = path.replace("\\", "/")
    normalized_pattern = pattern.replace("\\", "/")
    return (
        fnmatch(normalized, normalized_pattern)
        or (normalized_pattern.endswith("/") and normalized.startswith(normalized_pattern))
        or normalized == normalized_pattern
    )


def _path_allowed(path: str, include_paths: list[str], exclude_paths: list[str]) -> bool:
    if include_paths and not any(_path_matches(path, pattern) for pattern in include_paths):
        return False
    return not any(_path_matches(path, pattern) for pattern in exclude_paths)


def _doc_nodes(
    store: Any,
    *,
    include_paths: list[str] | None = None,
    exclude_paths: list[str] | None = None,
) -> list[Any]:
    getter = getattr(store, "get_nodes_by_kind", None)
    if callable(getter):
        nodes = list(getter(sorted(_DOC_KINDS)))
    else:
        nodes = [node for node in store.get_all_nodes() if node.kind in _DOC_KINDS]
    include_paths = include_paths or []
    exclude_paths = exclude_paths or []
    return [
        node for node in nodes if _path_allowed(str(node.file_path), include_paths, exclude_paths)
    ]


def _matches(qualified_name: str, expected: str) -> bool:
    qn = qualified_name.lower()
    exp = expected.lower()
    exp_name = expected.rsplit("::", 1)[-1].lower() if "::" in expected else exp
    qn_name = qualified_name.rsplit("::", 1)[-1].lower() if "::" in qualified_name else qn
    return exp in qn or qn in exp or exp_name == qn_name


def _relevance_targets(search_query: dict[str, Any]) -> dict[str, int]:
    targets: dict[str, int] = {}
    expected = str(search_query["expected"])
    targets[expected] = _DEFAULT_RELEVANCE_GRADE

    for item in search_query.get("relevant") or []:
        if isinstance(item, str):
            targets[item] = max(targets.get(item, 0), 1)
            continue
        if not isinstance(item, dict):
            continue
        target = item.get("target") or item.get("qualified_name") or item.get("expected")
        if not target:
            continue
        grade = int(item.get("grade", 1))
        targets[str(target)] = max(targets.get(str(target), 0), grade)
    return targets


def _relevance_grade(qualified_name: str, relevance: dict[str, int]) -> int:
    grade = 0
    for target, target_grade in relevance.items():
        if _matches(qualified_name, target):
            grade = max(grade, int(target_grade))
    return grade


def _rank_relevant(ranked: list[tuple[str, float]], relevance: dict[str, int]) -> tuple[int, int]:
    for rank, (qualified_name, _score) in enumerate(ranked, start=1):
        grade = _relevance_grade(qualified_name, relevance)
        if grade > 0:
            return rank, grade
    return 0, 0


def _dcg_at(ranked: list[tuple[str, float]], relevance: dict[str, int], k: int) -> float:
    score = 0.0
    for rank, (qualified_name, _score) in enumerate(ranked[:k], start=1):
        grade = _relevance_grade(qualified_name, relevance)
        if grade <= 0:
            continue
        score += (2**grade - 1) / math.log2(rank + 1)
    return score


def _ideal_dcg_at(relevance: dict[str, int], k: int) -> float:
    grades = sorted((grade for grade in relevance.values() if grade > 0), reverse=True)
    score = 0.0
    for rank, grade in enumerate(grades[:k], start=1):
        score += (2**grade - 1) / math.log2(rank + 1)
    return score


def _ndcg_at(ranked: list[tuple[str, float]], relevance: dict[str, int], k: int) -> float:
    ideal = _ideal_dcg_at(relevance, k)
    if ideal == 0.0:
        return 0.0
    return _dcg_at(ranked, relevance, k) / ideal


def _fts_ranked_docs(
    store: Any,
    query: str,
    limit: int,
    *,
    include_paths: list[str],
    exclude_paths: list[str],
) -> list[tuple[str, float]]:
    fetch_limit = max(limit * 25, 500)
    pairs = store.fts_query(query, limit=fetch_limit)
    nodes_by_id = store.get_nodes_by_ids([node_id for node_id, _score in pairs])
    ranked: list[tuple[str, float]] = []
    for node_id, score in pairs:
        node = nodes_by_id.get(node_id)
        if not node or node.kind not in _DOC_KINDS:
            continue
        if not _path_allowed(str(node.file_path), include_paths, exclude_paths):
            continue
        ranked.append((node.qualified_name, score))
        if len(ranked) >= limit:
            break
    return ranked


def _embedding_ranked_docs(
    nodes: list[Any],
    vectors: list[list[float]],
    provider: _DocFuzzyEmbeddingProvider,
    query: str,
    limit: int,
) -> list[tuple[str, float]]:
    query_vector = provider.embed_query(query)
    scored = [
        (node.qualified_name, _cosine(query_vector, vector)) for node, vector in zip(nodes, vectors)
    ]
    scored.sort(key=lambda item: item[1], reverse=True)
    return scored[:limit]


def _metric_row(
    *,
    repo: str,
    mode: str,
    label: str,
    query: str,
    expected: str,
    relevance: dict[str, int],
    ranked: list[tuple[str, float]],
    latency_ms: float,
    index_ms: float,
    provider: str,
    query_variant: str,
    effective_query: str,
) -> dict[str, Any]:
    rank, best_grade = _rank_relevant(ranked, relevance)
    ndcg_5 = _ndcg_at(ranked, relevance, 5)
    ndcg_20 = _ndcg_at(ranked, relevance, 20)
    return {
        "benchmark": "doc_fuzzy_search",
        "repo": repo,
        "mode": mode,
        "label": label,
        "query_variant": query_variant,
        "query": query,
        "effective_query": effective_query,
        "expected": expected,
        "relevant": ";".join(f"{target}:{grade}" for target, grade in sorted(relevance.items())),
        "relevant_count": len(relevance),
        "rank": rank,
        "reciprocal_rank": round(1.0 / rank if rank > 0 else 0.0, 4),
        "hit_at_1": int(rank == 1),
        "hit_at_5": int(0 < rank <= 5),
        "hit_at_20": int(0 < rank <= 20),
        "best_relevance_grade": best_grade,
        "ndcg_at_5": round(ndcg_5, 4),
        "ndcg_at_20": round(ndcg_20, 4),
        "result_count": len(ranked),
        "latency_ms": round(latency_ms, 3),
        "index_ms": round(index_ms, 3),
        "provider": provider,
        "top_result": ranked[0][0] if ranked else "",
        "mean_mrr": "",
        "precision_at_1": "",
        "precision_at_5": "",
        "precision_at_20": "",
        "mean_ndcg_at_5": "",
        "mean_ndcg_at_20": "",
        "query_count": "",
    }


def _aggregate_rows(repo: str, rows: list[dict[str, Any]]) -> list[dict[str, Any]]:
    grouped: dict[str, list[dict[str, Any]]] = defaultdict(list)
    for row in rows:
        grouped[str(row["mode"])].append(row)

    aggregates: list[dict[str, Any]] = []
    for mode, mode_rows in sorted(grouped.items()):
        query_count = len(mode_rows)
        if query_count == 0:
            continue
        aggregates.append(
            {
                "benchmark": "doc_fuzzy_search",
                "repo": repo,
                "mode": mode,
                "label": "aggregate",
                "query_variant": "",
                "query": "__aggregate__",
                "effective_query": "",
                "expected": "",
                "relevant": "",
                "relevant_count": "",
                "rank": 0,
                "reciprocal_rank": 0.0,
                "hit_at_1": 0,
                "hit_at_5": 0,
                "hit_at_20": 0,
                "best_relevance_grade": "",
                "ndcg_at_5": "",
                "ndcg_at_20": "",
                "result_count": "",
                "latency_ms": "",
                "index_ms": "",
                "provider": mode_rows[0].get("provider", ""),
                "top_result": "",
                "mean_mrr": round(
                    sum(float(row["reciprocal_rank"]) for row in mode_rows) / query_count,
                    4,
                ),
                "precision_at_1": round(
                    sum(int(row["hit_at_1"]) for row in mode_rows) / query_count,
                    4,
                ),
                "precision_at_5": round(
                    sum(int(row["hit_at_5"]) for row in mode_rows) / query_count,
                    4,
                ),
                "precision_at_20": round(
                    sum(int(row["hit_at_20"]) for row in mode_rows) / query_count,
                    4,
                ),
                "mean_ndcg_at_5": round(
                    sum(float(row["ndcg_at_5"]) for row in mode_rows) / query_count,
                    4,
                ),
                "mean_ndcg_at_20": round(
                    sum(float(row["ndcg_at_20"]) for row in mode_rows) / query_count,
                    4,
                ),
                "query_count": query_count,
            }
        )
    return aggregates


def _embedding_query_variants(config: dict[str, Any]) -> list[tuple[str, str]]:
    variants = [("embedding", "")]
    for item in config.get("doc_fuzzy_search_query_variants") or []:
        if isinstance(item, str):
            variants.append((f"embedding_{item}", item))
            continue
        if not isinstance(item, dict):
            continue
        name = str(item.get("name") or "variant").strip().lower().replace("-", "_")
        prefix = str(item.get("prefix") or "")
        variants.append((f"embedding_{name}", prefix))
    return variants


def run(repo_path: Path, store: Any, config: dict[str, Any]) -> list[dict[str, Any]]:
    """Run fuzzy documentation retrieval for FTS and embedding-only modes."""
    queries = config.get("doc_fuzzy_search_queries") or []
    if not queries:
        return []

    limit = int(config.get("doc_fuzzy_search_limit", _DEFAULT_LIMIT))
    include_paths = [str(path) for path in config.get("doc_fuzzy_search_include_paths") or []]
    exclude_paths = [str(path) for path in config.get("doc_fuzzy_search_exclude_paths") or []]
    provider = _DocFuzzyEmbeddingProvider(
        dimension=int(config.get("doc_fuzzy_search_dimension", 256))
    )

    index_started = time.perf_counter()
    nodes = _doc_nodes(store, include_paths=include_paths, exclude_paths=exclude_paths)
    vectors = provider.embed([_doc_text(repo_path, node) for node in nodes])
    embedding_index_ms = (time.perf_counter() - index_started) * 1000.0

    rows: list[dict[str, Any]] = []
    repo_name = str(config["name"])
    for sq in queries:
        query = str(sq["query"])
        expected = str(sq["expected"])
        relevance = _relevance_targets(sq)
        label = str(sq.get("label", ""))

        started = time.perf_counter()
        fts_ranked = _fts_ranked_docs(
            store,
            query,
            limit,
            include_paths=include_paths,
            exclude_paths=exclude_paths,
        )
        fts_latency_ms = (time.perf_counter() - started) * 1000.0
        rows.append(
            _metric_row(
                repo=repo_name,
                mode="fts",
                label=label,
                query=query,
                expected=expected,
                relevance=relevance,
                ranked=fts_ranked,
                latency_ms=fts_latency_ms,
                index_ms=0.0,
                provider="fts5",
                query_variant="raw",
                effective_query=query,
            )
        )

        for mode, prefix in _embedding_query_variants(config):
            effective_query = f"{prefix}{query}" if prefix else query
            started = time.perf_counter()
            embedding_ranked = _embedding_ranked_docs(
                nodes,
                vectors,
                provider,
                effective_query,
                limit,
            )
            embedding_latency_ms = (time.perf_counter() - started) * 1000.0
            rows.append(
                _metric_row(
                    repo=repo_name,
                    mode=mode,
                    label=label,
                    query=query,
                    expected=expected,
                    relevance=relevance,
                    ranked=embedding_ranked,
                    latency_ms=embedding_latency_ms,
                    index_ms=embedding_index_ms,
                    provider=provider.name,
                    query_variant=mode.removeprefix("embedding_") if mode != "embedding" else "raw",
                    effective_query=effective_query,
                )
            )

    rows.extend(_aggregate_rows(repo_name, rows))
    return rows
