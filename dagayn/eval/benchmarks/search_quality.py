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

logger = logging.getLogger(__name__)


def _result_qn(result: Any) -> str:
    if isinstance(result, dict):
        return str(result.get("qualified_name", ""))
    if hasattr(result, "qualified_name"):
        return str(result.qualified_name)
    return ""


def _matches_expected(result: Any, expected: str) -> bool:
    qn = _result_qn(result)
    qn_lower = qn.lower()
    exp_lower = expected.lower()
    exp_name = expected.rsplit("::", 1)[-1] if "::" in expected else expected
    qn_name = qn.rsplit("::", 1)[-1] if "::" in qn else qn
    return exp_lower in qn_lower or qn_lower in exp_lower or exp_name.lower() == qn_name.lower()


def _ndcg_at(rank: int, k: int) -> float:
    if rank <= 0 or rank > k:
        return 0.0
    # Single relevant target: ideal DCG is 1.0 at rank 1.
    return round(1.0 / math.log2(rank + 1), 4)


def run(repo_path: Path, store, config: dict) -> list[dict]:
    """Run search quality benchmark."""
    results = []
    for sq in config.get("search_queries", []):
        query = sq["query"]
        expected = sq["expected"]
        label = sq.get("label", "")

        try:
            from dagayn.search import hybrid_search

            started = time.perf_counter()
            hs = hybrid_search(store, query, limit=20)
            elapsed_ms = (time.perf_counter() - started) * 1000.0
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
            search_mode = "store_search_fallback"
            embedding_health = {}

        rank = 0
        for i, r in enumerate(search_results):
            if _matches_expected(r, expected):
                rank = i + 1
                break

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
                "latency_ms": round(elapsed_ms, 3),
                "result_count": len(search_results),
                "source_counts": json.dumps(dict(sources), sort_keys=True),
                "rank": rank,
                "reciprocal_rank": round(1.0 / rank if rank > 0 else 0.0, 4),
                "hit_at_5": int(0 < rank <= 5),
                "hit_at_20": int(0 < rank <= 20),
                "ndcg_at_20": _ndcg_at(rank, 20),
            }
        )
    return results
