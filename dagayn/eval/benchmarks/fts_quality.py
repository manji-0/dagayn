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
import sqlite3
from pathlib import Path

logger = logging.getLogger(__name__)

_PRECISION_K = 5


def _matches(qualified_name: str, expected: str) -> bool:
    """Return True when *qualified_name* satisfies the *expected* identifier.

    Mirrors the flexible matching used in search_quality.run so that results
    are comparable across benchmarks.
    """
    qn = qualified_name.lower()
    exp = expected.lower()
    exp_name = expected.rsplit("::", 1)[-1].lower() if "::" in expected else exp
    qn_name = qualified_name.rsplit("::", 1)[-1].lower() if "::" in qualified_name else qn
    return exp in qn or qn in exp or exp_name == qn_name


def run(repo_path: Path, store, config: dict) -> list[dict]:
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

    for sq in queries:
        query: str = sq["query"]
        expected: str = sq["expected"]
        label: str = sq.get("label", "")

        try:
            from dagayn.search import hybrid_search

            result = hybrid_search(store, query, limit=20)
            search_results: list[dict] = result["results"]
            actual_mode: str = result["mode"]
        except (ImportError, sqlite3.OperationalError) as exc:
            logger.debug("hybrid_search unavailable: %s", exc)
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
                }
            )
            reciprocal_ranks.append(0.0)
            hits_at_1.append(0)
            hits_at_k.append(0)
            continue

        rank = 0
        for i, r in enumerate(search_results):
            qn = r.get("qualified_name", "") if isinstance(r, dict) else getattr(r, "qualified_name", "")
            if _matches(qn, expected):
                rank = i + 1
                break

        rr = round(1.0 / rank if rank > 0 else 0.0, 4)
        h1 = 1 if rank == 1 else 0
        hk = 1 if 0 < rank <= _PRECISION_K else 0

        reciprocal_ranks.append(rr)
        hits_at_1.append(h1)
        hits_at_k.append(hk)

        rows.append(
            {
                "repo": config["name"],
                "query": query,
                "expected": expected,
                "label": label,
                "rank": rank,
                "reciprocal_rank": rr,
                "hit_at_1": h1,
                "hit_at_5": hk,
                "search_mode": actual_mode,
            }
        )

    n = len(reciprocal_ranks)
    if n == 0:
        return rows

    rows.append(
        {
            "repo": config["name"],
            "query": "__aggregate__",
            "expected": "",
            "label": "aggregate",
            "rank": 0,
            "reciprocal_rank": 0.0,
            "hit_at_1": 0,
            "hit_at_5": 0,
            "search_mode": "aggregate",
            "mean_mrr": round(sum(reciprocal_ranks) / n, 4),
            "precision_at_1": round(sum(hits_at_1) / n, 4),
            "precision_at_5": round(sum(hits_at_k) / n, 4),
            "query_count": n,
        }
    )

    return rows
