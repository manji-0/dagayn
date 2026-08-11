"""FTS search quality benchmark.

Measures FTS-only search quality by calling hybrid_search without an embedding
provider.  Records per-query metrics (rank, reciprocal rank, hit@1, hit@5) and
appends an aggregate summary row (query == "__aggregate__") containing mean MRR,
Precision@1, and Precision@5 across all queries.

This file produces a stable baseline.  Run the same config with an embedding
provider later and compare mean_mrr to quantify the quality uplift.
"""

from __future__ import annotations

import logging
import math
import sqlite3
import time
from collections.abc import Mapping
from pathlib import Path
from typing import Any, cast

from dagayn.eval.scorer import IdentifierMatcher

logger = logging.getLogger(__name__)

_PRECISION_K = 5


def _matches(qualified_name: str, expected: str) -> bool:
    """Return True when *qualified_name* satisfies the *expected* identifier.

    Mirrors the flexible matching used in search_quality.run so that results
    are comparable across benchmarks.
    """
    return IdentifierMatcher(allow_basename=True).matches(qualified_name, expected)


def _result_qn(result: object) -> str:
    if isinstance(result, Mapping):
        row = cast(Mapping[str, object], result)
        return str(row.get("qualified_name", ""))
    return str(getattr(result, "qualified_name", ""))


def _relevance(sq: dict[str, Any], expected: str) -> dict[str, int]:
    relevant = {expected: 1} if expected else {}
    for item in sq.get("relevant") or []:
        if isinstance(item, dict):
            target = item.get("target") or item.get("qualified_name") or item.get("expected")
            if target:
                relevant[str(target)] = int(item.get("grade", 1))
        elif item:
            relevant[str(item)] = 1
    return relevant


def _ndcg(ranked: list[str], relevant: dict[str, int], matcher: IdentifierMatcher, k: int) -> float:
    gains = [
        max(
            (grade for target, grade in relevant.items() if matcher.matches(name, target)),
            default=0,
        )
        for name in ranked[:k]
    ]
    ideal = sorted(relevant.values(), reverse=True)[:k]
    dcg = sum((2**gain - 1) / math.log2(idx + 2) for idx, gain in enumerate(gains))
    idcg = sum((2**gain - 1) / math.log2(idx + 2) for idx, gain in enumerate(ideal))
    return round(dcg / idcg, 4) if idcg else 0.0


def run(repo_path: Path, store, config: dict[str, Any]) -> list[dict[str, Any]]:
    """Run FTS-only search quality benchmark.

    Args:
        repo_path: Root of the repository (unused, kept for benchmark protocol).
        store: GraphStore instance with a built and FTS-indexed graph.
        config: Eval config dict.  ``search_queries`` entries must have
            ``query`` and ``expected`` keys.  An optional ``label`` key
            groups queries (e.g. ``"exact_name"``, ``"conceptual"``).

    Returns:
        List of per-query dicts followed by a single aggregate summary dict
        whose ``"query"`` key is ``"__aggregate__"``.  Returns ``[]`` when
        no queries are configured.
    """
    queries = config.get("search_queries", [])
    if not queries:
        return []

    rows: list[dict] = []
    reciprocal_ranks: list[float] = []
    hits_at_1: list[int] = []
    hits_at_k: list[int] = []

    matcher = IdentifierMatcher.from_config({**config, "allow_basename_match": True})
    repeat = int(config.get("latency_repeat", 5))
    warmup = int(config.get("latency_warmup", 1))

    for sq in queries:
        query: str = sq["query"]
        expected: str = sq["expected"]
        label: str = sq.get("label", "")

        relevant = _relevance(sq, expected)

        try:
            timings: list[float] = []
            search_results: list[dict] = []
            for idx in range(max(1, warmup + repeat)):
                started = time.perf_counter()
                fts_hits = store.fts_query(query, limit=20)
                node_map = store.get_nodes_by_ids([node_id for node_id, _score in fts_hits.hits])
                search_results = [
                    {
                        "qualified_name": node_map[node_id].qualified_name,
                        "score": score,
                        "source": "fts",
                    }
                    for node_id, score in fts_hits.hits
                    if node_id in node_map
                ]
                elapsed_ms = (time.perf_counter() - started) * 1000.0
                if idx >= warmup:
                    timings.append(elapsed_ms)
            actual_mode = "fts_only"
        except (ImportError, sqlite3.OperationalError, RuntimeError) as exc:
            logger.debug("hybrid_search unavailable: %s", exc)
            timings = []
            rows.append(
                {
                    "repo": config["name"],
                    "query": query,
                    "expected": expected,
                    "label": label,
                    "rank": 0,
                    "reciprocal_rank": 0.0,
                    "hit_at_1": 0,
                    "hit_at_5": 0,
                    "search_mode": "error",
                    "error": str(exc),
                    "status": "error",
                }
            )
            reciprocal_ranks.append(0.0)
            hits_at_1.append(0)
            hits_at_k.append(0)
            continue

        rank = 0
        for i, r in enumerate(search_results):
            if matcher.matches(_result_qn(r), expected):
                rank = i + 1
                break

        rr = round(1.0 / rank if rank > 0 else 0.0, 4)
        h1 = 1 if rank == 1 else 0
        hk = 1 if 0 < rank <= _PRECISION_K else 0

        reciprocal_ranks.append(rr)
        hits_at_1.append(h1)
        hits_at_k.append(hk)

        timings = sorted(timings) or [0.0]
        ranked_names = [_result_qn(r) for r in search_results]
        rows.append(
            {
                "benchmark": "fts_quality",
                "repo": config["name"],
                "query": query,
                "expected": expected,
                "label": label,
                "status": "ok",
                "rank": rank,
                "reciprocal_rank": rr,
                "hit_at_1": h1,
                "hit_at_5" if len(relevant) <= 1 else "precision_at_5": hk,
                "ndcg_at_5": _ndcg(ranked_names, relevant, matcher, 5),
                "search_mode": actual_mode,
                "best_ms": round(timings[0], 3),
                "median_ms": round(timings[len(timings) // 2], 3),
                "p95_ms": round(timings[min(len(timings) - 1, int(len(timings) * 0.95))], 3),
            }
        )

    n = len(reciprocal_ranks)
    if n == 0:
        return rows

    rows.append(
        {
            "repo": config["name"],
            "benchmark": "fts_quality",
            "query": "__aggregate__",
            "expected": "",
            "label": "aggregate",
            "rank": 0,
            "reciprocal_rank": 0.0,
            "hit_at_1": 0,
            "search_mode": "aggregate",
            "status": "ok",
            "mean_mrr": round(sum(reciprocal_ranks) / n, 4),
            "precision_at_1": round(sum(hits_at_1) / n, 4),
            "precision_at_5": round(sum(hits_at_k) / n, 4),
            "hit_at_5": round(sum(hits_at_k) / n, 4),
            "query_count": n,
        }
    )

    return rows
