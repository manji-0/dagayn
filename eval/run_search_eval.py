#!/usr/bin/env python3
"""Minimal search-ranking eval scaffold for dagayn hybrid search."""

from __future__ import annotations

import argparse
import json
import math
from pathlib import Path
from typing import Any

import yaml

from dagayn.graph import GraphStore
from dagayn.incremental import get_db_path
from dagayn.search import hybrid_search


def _load_yaml(path: Path) -> dict[str, Any]:
    with path.open("r", encoding="utf-8") as fh:
        return yaml.safe_load(fh) or {}


def _target(result: dict[str, Any]) -> str:
    return str(result.get("qualified_name") or result.get("file_path") or result.get("name") or "")


def _rank(results: list[dict[str, Any]], relevant: dict[str, int]) -> int:
    for idx, result in enumerate(results, start=1):
        if _target(result) in relevant:
            return idx
    return 0


def _dcg(grades: list[int]) -> float:
    return sum((2**grade - 1) / math.log2(idx + 2) for idx, grade in enumerate(grades))


def _ndcg_at(results: list[dict[str, Any]], relevant: dict[str, int], k: int) -> float:
    observed = [relevant.get(_target(result), 0) for result in results[:k]]
    ideal = sorted(relevant.values(), reverse=True)[:k]
    ideal_dcg = _dcg(ideal)
    return round(_dcg(observed) / ideal_dcg, 4) if ideal_dcg else 0.0


def _is_doc(result: dict[str, Any]) -> bool:
    path = str(result.get("file_path") or "")
    kind = str(result.get("kind") or "")
    return kind.startswith("Doc") or path.lower().endswith((".md", ".markdown", ".mdx"))


def _is_test(result: dict[str, Any]) -> bool:
    path = str(result.get("file_path") or "")
    return bool(result.get("is_test")) or "/tests/" in path.replace("\\", "/")


def run_eval(repo_root: Path, queries_path: Path, judgments_path: Path) -> dict[str, Any]:
    queries = _load_yaml(queries_path).get("queries", [])
    judgments = _load_yaml(judgments_path).get("judgments", {})
    store = GraphStore(get_db_path(repo_root))
    rows: list[dict[str, Any]] = []
    try:
        for query in queries:
            query_id = str(query["id"])
            relevant = {
                str(item["target"]): int(item.get("grade", 1))
                for item in judgments.get(query_id, {}).get("relevant", [])
            }
            results = hybrid_search(store, str(query["query"]), limit=20).get("results", [])
            rank = _rank(results, relevant)
            relevant_seen_at_20 = len({_target(result) for result in results[:20]} & set(relevant))
            rows.append(
                {
                    "id": query_id,
                    "intent": query.get("intent", "unknown"),
                    "query": query["query"],
                    "rank": rank,
                    "reciprocal_rank": round(1.0 / rank, 4) if rank else 0.0,
                    "ndcg_at_10": _ndcg_at(results, relevant, 10),
                    "recall_at_20": (
                        round(relevant_seen_at_20 / len(relevant), 4) if relevant else 0.0
                    ),
                    "exact_symbol_success_at_5": int(
                        query.get("intent") == "exact-symbol" and 0 < rank <= 5
                    ),
                    "prose_intent_success_at_10": int(
                        query.get("intent") == "prose-intent" and 0 < rank <= 10
                    ),
                    "doc_vs_code_confusion": int(
                        bool(relevant)
                        and any(
                            target.endswith((".md", ".markdown", ".mdx")) for target in relevant
                        )
                        and any(not _is_doc(result) for result in results[:5])
                    ),
                    "test_crowding_rate": round(
                        sum(1 for result in results[:10] if _is_test(result))
                        / max(len(results[:10]), 1),
                        4,
                    ),
                }
            )
    finally:
        store.close()

    count = len(rows)
    summary = {
        "query_count": count,
        "mrr_at_10": round(
            sum(row["reciprocal_rank"] if 0 < row["rank"] <= 10 else 0.0 for row in rows) / count,
            4,
        )
        if count
        else 0.0,
        "ndcg_at_10": round(sum(row["ndcg_at_10"] for row in rows) / count, 4) if count else 0.0,
        "recall_at_20": round(sum(row["recall_at_20"] for row in rows) / count, 4)
        if count
        else 0.0,
        "exact_symbol_success_at_5": sum(row["exact_symbol_success_at_5"] for row in rows),
        "prose_intent_success_at_10": sum(row["prose_intent_success_at_10"] for row in rows),
        "doc_vs_code_confusion_rate": round(
            sum(row["doc_vs_code_confusion"] for row in rows) / count,
            4,
        )
        if count
        else 0.0,
        "test_crowding_rate": round(sum(row["test_crowding_rate"] for row in rows) / count, 4)
        if count
        else 0.0,
    }
    return {"summary": summary, "rows": rows}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo-root", default=".")
    parser.add_argument("--queries", default="eval/search_queries.yaml")
    parser.add_argument("--judgments", default="eval/search_judgments.yaml")
    args = parser.parse_args()
    result = run_eval(Path(args.repo_root), Path(args.queries), Path(args.judgments))
    print(json.dumps(result, indent=2, sort_keys=True))


if __name__ == "__main__":
    main()
