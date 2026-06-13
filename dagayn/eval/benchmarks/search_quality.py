"""Search quality benchmark: measures ranking, recall, source mix, and latency."""

from __future__ import annotations

import json
import logging
import math
import sqlite3
import time
from collections import Counter
from pathlib import Path
from typing import Any

from dagayn.eval.scorer import IdentifierMatcher

logger = logging.getLogger(__name__)


def _result_qn(result: Any) -> str:
    if isinstance(result, dict):
        return str(result.get("qualified_name", ""))
    if hasattr(result, "qualified_name"):
        return str(result.qualified_name)
    return ""


def _matches_expected(result: Any, expected: str, matcher: IdentifierMatcher | None = None) -> bool:
    return (matcher or IdentifierMatcher()).matches(_result_qn(result), expected)


def _ndcg_at(rank: int, k: int) -> float:
    if rank <= 0 or rank > k:
        return 0.0
    # Single relevant target: ideal DCG is 1.0 at rank 1.
    return round(1.0 / math.log2(rank + 1), 4)


def run(repo_path: Path, store, config: dict) -> list[dict]:
    """Run search quality benchmark."""
    results = []
    matcher = IdentifierMatcher.from_config(config)
    repeat = int(config.get("latency_repeat", 5))
    warmup = int(config.get("latency_warmup", 1))
    for sq in config.get("search_queries", []):
        query = sq["query"]
        expected = sq["expected"]
        label = sq.get("label", "")
        relevant_items = sq.get("relevant") or []
        relevant = {expected: 1}
        for item in relevant_items:
            if isinstance(item, dict):
                target = item.get("target") or item.get("qualified_name") or item.get("expected")
                if target:
                    relevant[str(target)] = int(item.get("grade", 1))
            elif item:
                relevant[str(item)] = 1

        try:
            from dagayn.search import hybrid_search

            timings: list[float] = []
            hs = {"results": [], "mode": "unknown", "embedding_health": {}}
            for idx in range(max(1, warmup + repeat)):
                started = time.perf_counter()
                hs = hybrid_search(store, query, limit=20)
                elapsed_ms = (time.perf_counter() - started) * 1000.0
                if idx >= warmup:
                    timings.append(elapsed_ms)
            search_results = hs["results"]
            search_mode = hs.get("mode", "unknown")
            embedding_health = hs.get("embedding_health", {})
        except (ImportError, sqlite3.OperationalError) as exc:
            logger.debug("hybrid_search unavailable, using fallback: %s", exc)
            started = time.perf_counter()
            # Fallback to basic search
            search_results = [
                {"qualified_name": n.qualified_name} for n in store.search_nodes(query, limit=20)
            ]
            elapsed_ms = (time.perf_counter() - started) * 1000.0
            timings = [elapsed_ms]
            search_mode = "store_search_fallback"
            embedding_health = {}
        timings = sorted(timings)

        rank = 0
        for i, r in enumerate(search_results):
            if _matches_expected(r, expected, matcher):
                rank = i + 1
                break
        ranked_names = [_result_qn(r) for r in search_results]
        gains = []
        for name in ranked_names[:20]:
            gains.append(
                max(
                    (
                        grade
                        for target, grade in relevant.items()
                        if matcher.matches(name, target)
                    ),
                    default=0,
                )
            )
        ideal = sorted(relevant.values(), reverse=True)
        dcg = sum((2**gain - 1) / math.log2(idx + 2) for idx, gain in enumerate(gains))
        idcg = sum((2**gain - 1) / math.log2(idx + 2) for idx, gain in enumerate(ideal[:20]))
        ndcg_at_20 = round(dcg / idcg, 4) if idcg else 0.0

        sources = Counter(
            str(r.get("source", "unknown")) if isinstance(r, dict) else "unknown"
            for r in search_results
        )
        embedding_status = (
            embedding_health.get("status", "unknown")
            if isinstance(embedding_health, dict)
            else "unknown"
        )

        results.append(
            {
                "repo": config["name"],
                "label": label,
                "query": query,
                "expected": expected,
                "search_mode": search_mode,
                "embedding_status": embedding_status,
                "latency_ms": round(timings[len(timings) // 2], 3),
                "best_ms": round(timings[0], 3),
                "median_ms": round(timings[len(timings) // 2], 3),
                "p95_ms": round(timings[min(len(timings) - 1, int(len(timings) * 0.95))], 3),
                "result_count": len(search_results),
                "source_counts": json.dumps(dict(sources), sort_keys=True),
                "rank": rank,
                "reciprocal_rank": round(1.0 / rank if rank > 0 else 0.0, 4),
                "hit_at_5" if len(relevant) <= 1 else "precision_at_5": int(0 < rank <= 5),
                "hit_at_20": int(0 < rank <= 20),
                "ndcg_at_20": ndcg_at_20 if len(relevant) > 1 else _ndcg_at(rank, 20),
            }
        )
    return results
