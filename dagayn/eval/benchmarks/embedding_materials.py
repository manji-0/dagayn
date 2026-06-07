"""Embedding material strategy benchmark.

This benchmark compares what text should be embedded for a graph node before
spending time on a larger embedding model.  It uses a deterministic token-hash
provider so CI and local runs can compare material strategies without an API.
Each embedded material points back to one graph node, which lets us measure both
"combined" and "split" representations against the same retrieval targets.
"""

from __future__ import annotations

import hashlib
import math
import re
import time
from collections import Counter, defaultdict
from dataclasses import dataclass
from itertools import product
from pathlib import Path
from typing import Any

from dagayn.embeddings import EmbeddingProvider

np: Any
try:
    import numpy as _np
except ImportError:  # pragma: no cover - exercised only without numpy installed
    np = None
else:
    np = _np

_TOKEN_RE = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")
_IDENT_BOUNDARY_RE = re.compile(r"(?<=[a-z0-9])(?=[A-Z])|(?<=[A-Z])(?=[A-Z][a-z])")
_SENTENCE_SPLIT_RE = re.compile(r"(?<=[.!?。！？])\s+|\n+")
_MARKDOWN_HEADING_RE = re.compile(r"^(#{1,6})\s+")
_CODE_KINDS = {"Class", "Function", "Method"}
_DOC_KINDS = {"DocSection", "DocBody"}
_DEFAULT_LIMIT = 20
_DEFAULT_RELEVANCE_GRADE = 3


class _TokenHashEmbeddingProvider(EmbeddingProvider):
    preferred_batch_size = 512

    def __init__(self, dimension: int = 256) -> None:
        self._dimension = dimension

    @property
    def name(self) -> str:
        return "eval-material-token-hash"

    @property
    def dimension(self) -> int:
        return self._dimension

    def embed(self, texts: list[str]) -> list[list[float]]:
        return [self._vectorize(text) for text in texts]

    def embed_query(self, text: str) -> list[float]:
        return self._vectorize(text)

    def _vectorize(self, text: str) -> list[float]:
        counts: Counter[int] = Counter()
        for raw_token in _TOKEN_RE.findall(text):
            normalized = raw_token.replace("_", " ")
            for token in _IDENT_BOUNDARY_RE.sub(" ", normalized).lower().split():
                digest = hashlib.blake2b(token.encode("utf-8"), digest_size=4).digest()
                counts[int.from_bytes(digest, "little") % self._dimension] += 1
        if not counts:
            return [0.0] * self._dimension
        norm = math.sqrt(sum(value * value for value in counts.values()))
        vector = [0.0] * self._dimension
        for idx, value in counts.items():
            vector[idx] = value / norm
        return vector


@dataclass(frozen=True)
class _Strategy:
    name: str
    doc_granularity: str
    code_symbol: str
    comment_granularity: str
    symbol_comment: str


@dataclass(frozen=True)
class _Material:
    ref: str
    text: str
    kind: str


def _cosine(left: list[float], right: list[float]) -> float:
    if not left or not right:
        return 0.0
    dot = sum(a * b for a, b in zip(left, right))
    left_norm = math.sqrt(sum(a * a for a in left))
    right_norm = math.sqrt(sum(b * b for b in right))
    if left_norm == 0.0 or right_norm == 0.0:
        return 0.0
    return dot / (left_norm * right_norm)


def _read_lines(repo_path: Path, node: Any) -> list[str]:
    file_path = Path(node.file_path)
    if not file_path.is_absolute():
        file_path = repo_path / file_path
    try:
        return file_path.read_text(encoding="utf-8", errors="replace").splitlines()
    except OSError:
        return []


def _line_span(lines: list[str], node: Any) -> tuple[int, int]:
    if not lines:
        return 0, 0
    start = max(int(getattr(node, "line_start", 1) or 1) - 1, 0)
    line_start = int(getattr(node, "line_start", 1) or 1)
    line_end = int(getattr(node, "line_end", line_start) or line_start)
    end = min(max(line_end, line_start), len(lines))
    return start, end


def _doc_section_text(repo_path: Path, node: Any, *, max_chars: int) -> str:
    lines = _read_lines(repo_path, node)
    if not lines:
        return ""
    start, end = _line_span(lines, node)
    if getattr(node, "kind", "") == "DocSection":
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
    return "\n".join(lines[start:end])[:max_chars]


def _paragraphs(text: str) -> list[str]:
    return [part.strip() for part in re.split(r"\n\s*\n+", text) if part.strip()]


def _sentences(text: str) -> list[str]:
    chunks = [part.strip() for part in _SENTENCE_SPLIT_RE.split(text) if part.strip()]
    return chunks or ([text.strip()] if text.strip() else [])


def _display_name(node: Any) -> str:
    extra = getattr(node, "extra", None)
    if isinstance(extra, dict) and extra.get("display_name"):
        return str(extra["display_name"])
    return ""


def _base_node_text(node: Any) -> str:
    parts = [
        str(getattr(node, "name", "")),
        str(getattr(node, "qualified_name", "")),
        str(getattr(node, "file_path", "")).replace("/", " "),
        str(getattr(node, "language", "") or ""),
        _display_name(node),
    ]
    parent = getattr(node, "parent_name", None)
    if parent:
        parts.append(f"in {parent}")
    return " ".join(part for part in parts if part)


def _signature_text(node: Any) -> str:
    parts = [_base_node_text(node)]
    signature = getattr(node, "signature", None)
    if signature:
        parts.append(str(signature))
    params = getattr(node, "params", None)
    if params:
        parts.append(f"parameters {params}")
    return_type = getattr(node, "return_type", None)
    if return_type:
        parts.append(f"returns {return_type}")
    return " ".join(parts)


def _param_names(params: str | None) -> list[str]:
    if not params:
        return []
    names: list[str] = []
    for raw in _TOKEN_RE.findall(params):
        if raw in {"self", "cls", "int", "str", "bool", "float", "None"}:
            continue
        if raw and raw[0].islower() and raw not in names:
            names.append(raw)
    return names


def _predicate_text(node: Any) -> str:
    name = str(getattr(node, "name", ""))
    kind = str(getattr(node, "kind", "node")).lower()
    params = _param_names(getattr(node, "params", None))
    words = _IDENT_BOUNDARY_RE.sub(" ", name.replace("_", " ")).lower()
    parts = [_signature_text(node), f"{kind} {words}"]
    if params:
        parts.append(f"{kind} {words} uses inputs " + ", ".join(params))
        parts.append("given " + " and ".join(params) + f", {words}")
    return " ".join(parts)


def _comment_lines(repo_path: Path, node: Any) -> list[str]:
    lines = _read_lines(repo_path, node)
    if not lines:
        return []
    start, end = _line_span(lines, node)
    comments: list[str] = []

    # Include immediately adjacent leading comments/doc comments.
    idx = start - 1
    while idx >= 0:
        stripped = lines[idx].strip()
        if not stripped:
            idx -= 1
            continue
        if _looks_like_comment(stripped):
            comments.insert(0, _clean_comment(stripped))
            idx -= 1
            continue
        break

    for line in lines[start:end]:
        stripped = line.strip()
        if _looks_like_comment(stripped):
            comments.append(_clean_comment(stripped))
    return [comment for comment in comments if comment]


def _looks_like_comment(stripped: str) -> bool:
    prefixes = ("#", "//", "///", "/*", "*", "--", '"""', "'''")
    return stripped.startswith(prefixes)


def _clean_comment(stripped: str) -> str:
    cleaned = stripped
    for prefix in ("///", "//", "#", "/*", "*/", "*", "--", '"""', "'''"):
        if cleaned.startswith(prefix):
            cleaned = cleaned[len(prefix) :]
    return cleaned.strip(" */'\"")


def _comment_texts(repo_path: Path, node: Any, granularity: str) -> list[str]:
    if granularity == "none":
        return []
    whole = "\n".join(_comment_lines(repo_path, node)).strip()
    if not whole:
        return []
    if granularity == "sentence":
        return _sentences(whole)
    return [whole]


def _doc_materials(
    repo_path: Path,
    node: Any,
    strategy: _Strategy,
    *,
    max_chars: int,
) -> list[_Material]:
    text = _doc_section_text(repo_path, node, max_chars=max_chars)
    if not text:
        text = _base_node_text(node)
    base = _base_node_text(node)
    if strategy.doc_granularity == "section":
        chunks = [text]
    elif strategy.doc_granularity == "paragraph":
        chunks = _paragraphs(text)
    else:
        chunks = _sentences(text)
    return [
        _Material(str(node.qualified_name), f"{base} {chunk}", f"doc_{strategy.doc_granularity}")
        for chunk in chunks
        if chunk
    ]


def _code_materials(repo_path: Path, node: Any, strategy: _Strategy) -> list[_Material]:
    if strategy.code_symbol == "name":
        symbol_text = _base_node_text(node)
    elif strategy.code_symbol == "signature":
        symbol_text = _signature_text(node)
    else:
        symbol_text = _predicate_text(node)

    comments = _comment_texts(repo_path, node, strategy.comment_granularity)
    qn = str(node.qualified_name)
    if not comments:
        return [_Material(qn, symbol_text, f"code_{strategy.code_symbol}")]

    if strategy.symbol_comment == "combined":
        return [
            _Material(
                qn,
                f"{symbol_text} {comment}",
                f"code_{strategy.code_symbol}_comment_{strategy.comment_granularity}_combined",
            )
            for comment in comments
        ]

    materials = [_Material(qn, symbol_text, f"code_{strategy.code_symbol}")]
    materials.extend(
        _Material(qn, comment, f"comment_{strategy.comment_granularity}") for comment in comments
    )
    return materials


def _materials_for_strategy(
    repo_path: Path,
    nodes: list[Any],
    strategy: _Strategy,
    *,
    max_chars: int,
) -> list[_Material]:
    materials: list[_Material] = []
    for node in nodes:
        kind = str(getattr(node, "kind", ""))
        if kind in _DOC_KINDS:
            materials.extend(_doc_materials(repo_path, node, strategy, max_chars=max_chars))
        elif kind in _CODE_KINDS:
            materials.extend(_code_materials(repo_path, node, strategy))
        else:
            materials.append(_Material(str(node.qualified_name), _signature_text(node), "metadata"))
    return [material for material in materials if material.text.strip()]


def _strategies(config: dict[str, Any]) -> list[_Strategy]:
    configured = config.get("embedding_material_strategies")
    if configured:
        strategies: list[_Strategy] = []
        for item in configured:
            if isinstance(item, str):
                strategies.append(_parse_strategy_name(item))
                continue
            if not isinstance(item, dict):
                continue
            strategy = _Strategy(
                name=str(item.get("name") or ""),
                doc_granularity=str(item.get("doc_granularity") or "section"),
                code_symbol=str(item.get("code_symbol") or "signature"),
                comment_granularity=str(item.get("comment_granularity") or "whole"),
                symbol_comment=str(item.get("symbol_comment") or "combined"),
            )
            strategies.append(strategy if strategy.name else _with_generated_name(strategy))
        return strategies

    doc_granularities = config.get("embedding_material_doc_granularities") or [
        "section",
        "paragraph",
        "sentence",
    ]
    code_symbols = config.get("embedding_material_code_symbols") or [
        "name",
        "signature",
        "predicate",
    ]
    comment_granularities = config.get("embedding_material_comment_granularities") or [
        "none",
        "whole",
        "sentence",
    ]
    symbol_comments = config.get("embedding_material_symbol_comments") or ["combined", "split"]

    strategies = []
    for doc_granularity, code_symbol, comment_granularity, symbol_comment in product(
        doc_granularities,
        code_symbols,
        comment_granularities,
        symbol_comments,
    ):
        if comment_granularity == "none" and symbol_comment == "split":
            continue
        strategies.append(
            _with_generated_name(
                _Strategy("", doc_granularity, code_symbol, comment_granularity, symbol_comment)
            )
        )
    return strategies


def _with_generated_name(strategy: _Strategy) -> _Strategy:
    name = (
        f"doc={strategy.doc_granularity}|code={strategy.code_symbol}|"
        f"comment={strategy.comment_granularity}|join={strategy.symbol_comment}"
    )
    return _Strategy(
        name,
        strategy.doc_granularity,
        strategy.code_symbol,
        strategy.comment_granularity,
        strategy.symbol_comment,
    )


def _parse_strategy_name(name: str) -> _Strategy:
    parts = dict(part.split("=", 1) for part in name.split("|") if "=" in part)
    return _Strategy(
        name=name,
        doc_granularity=parts.get("doc", "section"),
        code_symbol=parts.get("code", "signature"),
        comment_granularity=parts.get("comment", "whole"),
        symbol_comment=parts.get("join", "combined"),
    )


def _matches_expected(qualified_name: str, expected: str) -> bool:
    qn_lower = qualified_name.lower()
    exp_lower = expected.lower()
    exp_name = expected.rsplit("::", 1)[-1] if "::" in expected else expected
    qn_name = qualified_name.rsplit("::", 1)[-1] if "::" in qualified_name else qualified_name
    return exp_lower in qn_lower or qn_lower in exp_lower or exp_name.lower() == qn_name.lower()


def _relevance_targets(search_query: dict[str, Any]) -> dict[str, int]:
    expected = str(search_query.get("expected") or "")
    targets = {expected: _DEFAULT_RELEVANCE_GRADE} if expected else {}
    for item in search_query.get("relevant") or []:
        if isinstance(item, str):
            targets[item] = max(targets.get(item, 0), 1)
            continue
        if not isinstance(item, dict):
            continue
        target = item.get("target") or item.get("qualified_name") or item.get("expected")
        if target:
            targets[str(target)] = max(targets.get(str(target), 0), int(item.get("grade", 1)))
    return targets


def _relevance_grade(qualified_name: str, relevance: dict[str, int]) -> int:
    grade = 0
    for target, target_grade in relevance.items():
        if _matches_expected(qualified_name, target):
            grade = max(grade, int(target_grade))
    return grade


def _best_unseen_relevance_target(
    qualified_name: str,
    relevance: dict[str, int],
    seen_targets: set[str],
) -> tuple[str | None, int]:
    best_target = None
    best_grade = 0
    for target, target_grade in relevance.items():
        if target in seen_targets:
            continue
        if _matches_expected(qualified_name, target) and int(target_grade) > best_grade:
            best_target = target
            best_grade = int(target_grade)
    return best_target, best_grade


def _rank_relevant(ranked: list[tuple[str, float]], relevance: dict[str, int]) -> tuple[int, int]:
    for rank, (qualified_name, _score) in enumerate(ranked, start=1):
        grade = _relevance_grade(qualified_name, relevance)
        if grade > 0:
            return rank, grade
    return 0, 0


def _dcg_at(ranked: list[tuple[str, float]], relevance: dict[str, int], k: int) -> float:
    score = 0.0
    seen_targets: set[str] = set()
    for rank, (qualified_name, _score) in enumerate(ranked[:k], start=1):
        target, grade = _best_unseen_relevance_target(qualified_name, relevance, seen_targets)
        if grade > 0:
            if target is not None:
                seen_targets.add(target)
            score += (2**grade - 1) / math.log2(rank + 1)
    return score


def _ndcg_at(ranked: list[tuple[str, float]], relevance: dict[str, int], k: int) -> float:
    ideal_grades = sorted((grade for grade in relevance.values() if grade > 0), reverse=True)
    ideal = sum(
        (2**grade - 1) / math.log2(rank + 1)
        for rank, grade in enumerate(ideal_grades[:k], 1)
    )
    if ideal == 0.0:
        return 0.0
    return _dcg_at(ranked, relevance, k) / ideal


def _rank_materials(
    provider: _TokenHashEmbeddingProvider,
    materials: list[_Material],
    vectors: Any,
    query: str,
    limit: int,
) -> list[tuple[str, float]]:
    query_vector = provider.embed_query(query)
    best_by_ref: dict[str, float] = {}
    if np is not None:
        query_array = np.array(query_vector, dtype=np.float32)
        query_norm = float(np.linalg.norm(query_array))
        if query_norm == 0.0:
            return []
        scores = vectors @ (query_array / query_norm)
        for material, score in zip(materials, scores):
            score_float = float(score)
            if score_float > best_by_ref.get(material.ref, -1.0):
                best_by_ref[material.ref] = score_float
    else:
        for material, vector in zip(materials, vectors):
            score = _cosine(query_vector, vector)
            if score > best_by_ref.get(material.ref, -1.0):
                best_by_ref[material.ref] = score
    ranked = sorted(best_by_ref.items(), key=lambda item: item[1], reverse=True)
    return ranked[:limit]


def _metric_row(
    *,
    repo: str,
    strategy: _Strategy,
    query: str,
    expected: str,
    label: str,
    relevance: dict[str, int],
    ranked: list[tuple[str, float]],
    latency_ms: float,
    index_ms: float,
    material_count: int,
    ref_count: int,
    provider: str,
) -> dict[str, Any]:
    rank, best_grade = _rank_relevant(ranked, relevance)
    top_score = ranked[0][1] if ranked else 0.0
    mean_top_5_score = (
        sum(score for _qualified_name, score in ranked[:5]) / min(len(ranked), 5) if ranked else 0.0
    )
    query_type = "positive" if relevance else "negative"
    return {
        "benchmark": "embedding_materials",
        "repo": repo,
        "strategy": strategy.name,
        "doc_granularity": strategy.doc_granularity,
        "code_symbol": strategy.code_symbol,
        "comment_granularity": strategy.comment_granularity,
        "symbol_comment": strategy.symbol_comment,
        "query_type": query_type,
        "label": label,
        "query": query,
        "expected": expected,
        "rank": rank,
        "reciprocal_rank": round(1.0 / rank if rank > 0 else 0.0, 4),
        "hit_at_1": int(rank == 1),
        "hit_at_5": int(0 < rank <= 5),
        "hit_at_20": int(0 < rank <= 20),
        "best_relevance_grade": best_grade,
        "ndcg_at_5": round(_ndcg_at(ranked, relevance, 5), 4),
        "ndcg_at_20": round(_ndcg_at(ranked, relevance, 20), 4),
        "result_count": len(ranked),
        "material_count": material_count,
        "ref_count": ref_count,
        "latency_ms": round(latency_ms, 3),
        "index_ms": round(index_ms, 3),
        "provider": provider,
        "top_result": ranked[0][0] if ranked else "",
        "top_score": round(top_score, 6),
        "mean_top_5_score": round(mean_top_5_score, 6),
        "mean_mrr": "",
        "precision_at_1": "",
        "precision_at_5": "",
        "precision_at_20": "",
        "mean_ndcg_at_5": "",
        "mean_ndcg_at_20": "",
        "mean_top_score": "",
        "mean_top_5_score_aggregate": "",
        "query_count": "",
    }


def _aggregate_rows(repo: str, rows: list[dict[str, Any]]) -> list[dict[str, Any]]:
    grouped: dict[tuple[str, str], list[dict[str, Any]]] = defaultdict(list)
    for row in rows:
        grouped[(str(row["strategy"]), "aggregate_all")].append(row)
        grouped[(str(row["strategy"]), f"aggregate_{row['query_type']}")].append(row)
        grouped[(str(row["strategy"]), str(row["label"]))].append(row)

    aggregates: list[dict[str, Any]] = []
    for (strategy_name, label), group_rows in sorted(grouped.items()):
        if not group_rows:
            continue
        first = group_rows[0]
        count = len(group_rows)
        aggregates.append(
            {
                **{key: first.get(key, "") for key in first.keys()},
                "repo": repo,
                "label": label,
                "query": "__aggregate__",
                "expected": "",
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
                "top_result": "",
                "mean_mrr": round(
                    sum(float(row["reciprocal_rank"]) for row in group_rows) / count,
                    4,
                ),
                "precision_at_1": round(sum(int(row["hit_at_1"]) for row in group_rows) / count, 4),
                "precision_at_5": round(sum(int(row["hit_at_5"]) for row in group_rows) / count, 4),
                "precision_at_20": round(
                    sum(int(row["hit_at_20"]) for row in group_rows) / count,
                    4,
                ),
                "mean_ndcg_at_5": round(
                    sum(float(row["ndcg_at_5"]) for row in group_rows) / count,
                    4,
                ),
                "mean_ndcg_at_20": round(
                    sum(float(row["ndcg_at_20"]) for row in group_rows) / count,
                    4,
                ),
                "mean_top_score": round(
                    sum(float(row["top_score"]) for row in group_rows) / count,
                    6,
                ),
                "mean_top_5_score_aggregate": round(
                    sum(float(row["mean_top_5_score"]) for row in group_rows) / count,
                    6,
                ),
                "query_count": count,
                "strategy": strategy_name,
            }
        )
    return aggregates


def _queries(config: dict[str, Any]) -> list[dict[str, Any]]:
    rows: list[dict[str, Any]] = []
    rows.extend(config.get("search_queries") or [])
    rows.extend(config.get("doc_fuzzy_search_queries") or [])
    for item in config.get("embedding_material_negative_queries") or []:
        if not isinstance(item, dict):
            continue
        rows.append(
            {
                "query": str(item.get("query") or ""),
                "expected": "",
                "label": str(item.get("label") or "negative"),
            }
        )
    return rows


def run(repo_path: Path, store: Any, config: dict[str, Any]) -> list[dict[str, Any]]:
    """Run retrieval quality across generated embedding material strategies."""
    queries = _queries(config)
    if not queries:
        return []

    provider = _TokenHashEmbeddingProvider(
        dimension=int(config.get("embedding_material_dimension", 256))
    )
    limit = int(config.get("embedding_material_limit", _DEFAULT_LIMIT))
    max_chars = int(config.get("embedding_material_max_chars", 4096))
    nodes = store.get_all_nodes(exclude_files=True)
    repo_name = str(config["name"])

    rows: list[dict[str, Any]] = []
    for strategy in _strategies(config):
        index_started = time.perf_counter()
        materials = _materials_for_strategy(repo_path, nodes, strategy, max_chars=max_chars)
        vectors = provider.embed([material.text for material in materials])
        if np is not None and vectors:
            matrix = np.array(vectors, dtype=np.float32)
            norms = np.linalg.norm(matrix, axis=1)
            safe_norms = np.where(norms > 0, norms, 1.0)
            vectors_for_search: Any = matrix / safe_norms[:, None]
        else:
            vectors_for_search = vectors
        index_ms = (time.perf_counter() - index_started) * 1000.0
        ref_count = len({material.ref for material in materials})

        for sq in queries:
            query = str(sq["query"])
            expected = str(sq.get("expected") or "")
            relevance = _relevance_targets(sq)
            started = time.perf_counter()
            ranked = _rank_materials(provider, materials, vectors_for_search, query, limit)
            latency_ms = (time.perf_counter() - started) * 1000.0
            rows.append(
                _metric_row(
                    repo=repo_name,
                    strategy=strategy,
                    query=query,
                    expected=expected,
                    label=str(sq.get("label", "")),
                    relevance=relevance,
                    ranked=ranked,
                    latency_ms=latency_ms,
                    index_ms=index_ms,
                    material_count=len(materials),
                    ref_count=ref_count,
                    provider=provider.name,
                )
            )

    rows.extend(_aggregate_rows(repo_name, rows))
    return rows
