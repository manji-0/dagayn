#!/usr/bin/env python3
"""Search-ranking eval: hybrid (FTS + embeddings) against FTS alone.

Each query names the repository whose graph it searches (``repo:`` in
``search_queries.yaml``, default ``dagayn``); ``--repo NAME=PATH`` maps that
name to a checkout with a built graph. The ``fts`` arm runs the same
``hybrid_search`` with the embedding arm switched off, so every other step
(FTS sub-queries, keyword fallback, reranking) is shared. Arms alternate per
query and pass so caches and drift favor neither; latency is the median over
``--passes``.

Every judged target must exist in its repository's graph, or the eval refuses
to run: a typo would otherwise score as a miss. ``--pool-out`` writes the
unjudged hits in each arm's top 5 so they can be judged by hand.
"""

from __future__ import annotations

import argparse
import json
import math
import sqlite3
import statistics
import time
from collections import Counter, defaultdict
from pathlib import Path
from typing import Any
from unittest import mock

import yaml

from dagayn import search
from dagayn.embeddings_text import _embed_query_cached
from dagayn.graph import GraphStore
from dagayn.paths import get_db_path

ARMS = ("hybrid", "fts")


def _load_yaml(path: Path) -> dict[str, Any]:
    with path.open("r", encoding="utf-8") as fh:
        return yaml.safe_load(fh) or {}


def _target(result: dict[str, Any]) -> str:
    return str(result.get("qualified_name") or result.get("file_path") or result.get("name") or "")


def _dcg(grades: list[int]) -> float:
    return sum((2**grade - 1) / math.log2(idx + 2) for idx, grade in enumerate(grades))


def score(targets: list[str], relevant: dict[str, int]) -> dict[str, float]:
    rank = next((i for i, t in enumerate(targets, start=1) if t in relevant), 0)
    ideal = _dcg(sorted(relevant.values(), reverse=True)[:10])
    observed = _dcg([relevant.get(t, 0) for t in targets[:10]])
    return {
        "rank": rank,
        "rr_at_10": 1.0 / rank if 0 < rank <= 10 else 0.0,
        "ndcg_at_10": observed / ideal if ideal else 0.0,
        "p_at_5": sum(1 for t in targets[:5] if relevant.get(t, 0) > 0) / 5,
        "recall_at_20": len(set(targets[:20]) & set(relevant)) / len(relevant) if relevant else 0.0,
        "success_at_5": float(0 < rank <= 5),
    }


def missing_targets(db_path: Path, targets: set[str]) -> set[str]:
    conn = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
    try:
        known = {
            row[0] for row in conn.execute("SELECT qualified_name FROM nodes") if row[0] in targets
        }
    finally:
        conn.close()
    return targets - known


def _no_embeddings(*_args: Any, **_kwargs: Any) -> tuple[list[Any], dict[str, Any]]:
    return [], {"status": "disabled_by_eval", "matching_vector_count": 0}


def run_query(store: GraphStore, query: str, arm: str) -> tuple[dict[str, Any], float]:
    # An agent rarely repeats a query, so every timed pass pays the query embedding.
    _embed_query_cached.cache_clear()
    started = time.perf_counter()
    if arm == "fts":
        with mock.patch.object(search, "_embedding_search_with_health", _no_embeddings):
            payload = search.hybrid_search(store, query, limit=20)
    else:
        payload = search.hybrid_search(store, query, limit=20)
    return payload, (time.perf_counter() - started) * 1000


def _mean(values: list[float]) -> float:
    return round(sum(values) / len(values), 4) if values else 0.0


def summarize(rows: list[dict[str, Any]]) -> dict[str, Any]:
    metrics = ("rr_at_10", "ndcg_at_10", "p_at_5", "recall_at_20", "success_at_5")
    out: dict[str, Any] = {}
    for arm in ARMS:
        arm_rows = [r for r in rows if arm in r["arms"]]
        if not arm_rows:
            continue
        groups: dict[str, list[dict[str, Any]]] = defaultdict(list)
        for r in arm_rows:
            groups["all"].append(r)
            groups[f"intent:{r['intent']}"].append(r)
            groups[f"repo:{r['repo']}"].append(r)
        out[arm] = {
            name: {
                "n": len(group),
                **{m: _mean([r["arms"][arm][m] for r in group]) for m in metrics},
                "latency_ms_median": round(
                    statistics.median(r["arms"][arm]["latency_ms"] for r in group), 2
                ),
                "mode_counts": dict(Counter(str(r["arms"][arm]["mode"]) for r in group)),
            }
            for name, group in sorted(groups.items())
        }
    return out


def run_eval(
    repos: dict[str, Path],
    queries_path: Path,
    judgments_path: Path,
    arms: tuple[str, ...],
    passes: int,
) -> dict[str, Any]:
    queries = _load_yaml(queries_path).get("queries", [])
    judgments = _load_yaml(judgments_path).get("judgments", {})
    by_repo: dict[str, list[dict[str, Any]]] = defaultdict(list)
    for query in queries:
        by_repo[str(query.get("repo", "dagayn"))].append(query)

    unknown_repos = set(by_repo) - set(repos)
    if unknown_repos:
        raise SystemExit(f"queries name repos without --repo: {sorted(unknown_repos)}")
    unjudged = [q["id"] for q in queries if not judgments.get(q["id"], {}).get("relevant")]
    if unjudged:
        raise SystemExit(f"queries without judgments: {unjudged}")
    for name, repo_queries in by_repo.items():
        targets = {
            str(item["target"]) for q in repo_queries for item in judgments[q["id"]]["relevant"]
        }
        missing = missing_targets(get_db_path(repos[name]), targets)
        if missing:
            raise SystemExit(f"judged targets absent from the {name} graph: {sorted(missing)}")

    rows: list[dict[str, Any]] = []
    for name, repo_queries in sorted(by_repo.items()):
        store = GraphStore(get_db_path(repos[name]))
        try:
            for arm in arms:  # load the vector matrix and FTS pages before timing
                run_query(store, str(repo_queries[0]["query"]), arm)
            for query in repo_queries:
                relevant = {
                    str(item["target"]): int(item.get("grade", 1))
                    for item in judgments[query["id"]]["relevant"]
                }
                row: dict[str, Any] = {
                    "id": query["id"],
                    "repo": name,
                    "intent": query.get("intent", "unknown"),
                    "query": query["query"],
                    "arms": {},
                }
                latencies: dict[str, list[float]] = defaultdict(list)
                payloads: dict[str, dict[str, Any]] = {}
                for p in range(passes):
                    order = arms if p % 2 == 0 else tuple(reversed(arms))
                    for arm in order:
                        payload, ms = run_query(store, str(query["query"]), arm)
                        latencies[arm].append(ms)
                        payloads[arm] = payload
                for arm in arms:
                    payload = payloads[arm]
                    targets = [_target(r) for r in payload.get("results", [])]
                    health = payload.get("embedding_health") or {}
                    row["arms"][arm] = {
                        **score(targets, relevant),
                        "mode": payload.get("mode"),
                        "embedding_status": health.get("status"),
                        "matching_vector_count": health.get("matching_vector_count"),
                        "text_mode": health.get("requested_text_mode"),
                        "rerank_intent": payload.get("rerank_intent"),
                        "latency_ms": round(statistics.median(latencies[arm]), 2),
                        "latency_ms_range": [
                            round(min(latencies[arm]), 2),
                            round(max(latencies[arm]), 2),
                        ],
                        "top10": targets[:10],
                        "unjudged_top5": [t for t in targets[:5] if t not in relevant],
                    }
                rows.append(row)
        finally:
            store.close()
    return {"passes": passes, "summary": summarize(rows), "rows": rows}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--repo",
        action="append",
        default=[],
        metavar="NAME=PATH",
        help="map a query's repo name to a checkout with a built graph (default dagayn=.)",
    )
    parser.add_argument("--queries", default="eval/search_queries.yaml")
    parser.add_argument("--judgments", default="eval/search_judgments.yaml")
    parser.add_argument("--arms", default="hybrid,fts", help="comma list of: hybrid, fts")
    parser.add_argument("--passes", type=int, default=3)
    parser.add_argument("--pool-out", help="write unjudged top-5 hits per query here (YAML)")
    args = parser.parse_args()

    repos = {"dagayn": Path(".")}
    for spec in args.repo:
        name, _, path = spec.partition("=")
        repos[name] = Path(path)
    arms = tuple(a for a in args.arms.split(",") if a)
    if not arms or set(arms) - set(ARMS):
        raise SystemExit(f"--arms takes a comma list of {ARMS}")

    result = run_eval(repos, Path(args.queries), Path(args.judgments), arms, args.passes)
    if args.pool_out:
        pool = {
            row["id"]: sorted({t for arm in row["arms"].values() for t in arm["unjudged_top5"]})
            for row in result["rows"]
        }
        Path(args.pool_out).write_text(
            yaml.safe_dump({k: v for k, v in pool.items() if v}, allow_unicode=True),
            encoding="utf-8",
        )
    print(json.dumps(result, indent=2, sort_keys=True, ensure_ascii=False))


if __name__ == "__main__":
    main()
