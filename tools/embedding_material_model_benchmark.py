#!/usr/bin/env python3
"""Compare local embedding models on the best embedding material strategy."""

from __future__ import annotations

import argparse
import csv
import json
import time
from pathlib import Path
from typing import Any

import yaml

from dagayn.embeddings import LocalEmbeddingProvider, OpenAIEmbeddingProvider
from dagayn.eval.benchmarks import embedding_materials
from dagayn.graph import GraphStore
from dagayn.incremental import get_db_path

DEFAULT_STRATEGY = "doc=section|code=name|comment=sentence|join=combined"
DEFAULT_MODELS = [
    "openai:qwen3-embedding-0.6b-gguf-q8_0@http://127.0.0.1:18080/v1",
    "local:BAAI/bge-m3",
    "local:google/embeddinggemma-300m",
    "local:nomic-ai/nomic-embed-code",
    "local:microsoft/Harrier-OSS-v1-0.6B",
]


def _provider(spec: str, *, openai_batch_size: int):
    if spec.startswith("openai:"):
        model, base_url = spec[len("openai:") :].rsplit("@", 1)
        return OpenAIEmbeddingProvider(
            api_key="dagayn-local",
            base_url=base_url,
            model=model,
            batch_size=openai_batch_size,
            timeout=180,
        )
    if spec.startswith("local:"):
        return LocalEmbeddingProvider(spec[len("local:") :])
    raise ValueError(f"Unsupported model spec: {spec}")


def _queries(config: dict[str, Any]) -> list[dict[str, Any]]:
    queries: list[dict[str, Any]] = []
    queries.extend(config.get("search_queries") or [])
    queries.extend(config.get("doc_fuzzy_search_queries") or [])
    for item in config.get("embedding_material_negative_queries") or []:
        if isinstance(item, dict):
            queries.append(
                {
                    "query": str(item.get("query") or ""),
                    "expected": "",
                    "label": str(item.get("label") or "negative"),
                }
            )
    return queries


def _aggregate(rows: list[dict[str, Any]], model_spec: str) -> list[dict[str, Any]]:
    by_label: dict[str, list[dict[str, Any]]] = {}
    for row in rows:
        by_label.setdefault("aggregate_all", []).append(row)
        by_label.setdefault(f"aggregate_{row['query_type']}", []).append(row)
        by_label.setdefault(str(row["label"]), []).append(row)

    aggregates: list[dict[str, Any]] = []
    for label, label_rows in sorted(by_label.items()):
        count = len(label_rows)
        if not count:
            continue
        aggregates.append(
            {
                "benchmark": "embedding_material_model",
                "model": model_spec,
                "label": label,
                "query_type": (
                    label_rows[0]["query_type"] if not label.startswith("aggregate") else ""
                ),
                "mean_mrr": round(
                    sum(float(row["reciprocal_rank"]) for row in label_rows) / count,
                    4,
                ),
                "precision_at_1": round(sum(int(row["hit_at_1"]) for row in label_rows) / count, 4),
                "precision_at_5": round(sum(int(row["hit_at_5"]) for row in label_rows) / count, 4),
                "precision_at_20": round(
                    sum(int(row["hit_at_20"]) for row in label_rows) / count,
                    4,
                ),
                "mean_ndcg_at_5": round(
                    sum(float(row["ndcg_at_5"]) for row in label_rows) / count,
                    4,
                ),
                "mean_ndcg_at_20": round(
                    sum(float(row["ndcg_at_20"]) for row in label_rows) / count,
                    4,
                ),
                "mean_top_score": round(
                    sum(float(row["top_score"]) for row in label_rows) / count,
                    6,
                ),
                "mean_top_5_score": round(
                    sum(float(row["mean_top_5_score"]) for row in label_rows) / count,
                    6,
                ),
                "query_count": count,
            }
        )
    return aggregates


def _model_rows(
    *,
    repo_path: Path,
    store: GraphStore,
    config: dict[str, Any],
    model_spec: str,
    strategy_name: str,
    limit: int,
    max_chars: int,
    openai_batch_size: int,
) -> tuple[list[dict[str, Any]], dict[str, Any]]:
    strategy = embedding_materials._parse_strategy_name(strategy_name)
    nodes = store.get_all_nodes(exclude_files=True)
    material_started = time.perf_counter()
    materials = embedding_materials._materials_for_strategy(
        repo_path,
        nodes,
        strategy,
        max_chars=max_chars,
    )
    material_ms = (time.perf_counter() - material_started) * 1000.0

    provider = _provider(model_spec, openai_batch_size=openai_batch_size)
    load_started = time.perf_counter()
    # Force lazy sentence-transformers providers to load before timing indexing.
    _ = provider.dimension
    load_ms = (time.perf_counter() - load_started) * 1000.0

    texts = [material.text for material in materials]
    embed_started = time.perf_counter()
    raw_vectors = provider.embed(texts)
    embed_ms = (time.perf_counter() - embed_started) * 1000.0
    if embedding_materials.np is not None and raw_vectors:
        matrix = embedding_materials.np.array(raw_vectors, dtype=embedding_materials.np.float32)
        norms = embedding_materials.np.linalg.norm(matrix, axis=1)
        safe_norms = embedding_materials.np.where(norms > 0, norms, 1.0)
        vectors: Any = matrix / safe_norms[:, None]
    else:
        vectors = raw_vectors

    rows: list[dict[str, Any]] = []
    query_latency_total = 0.0
    for sq in _queries(config):
        query = str(sq["query"])
        expected = str(sq.get("expected") or "")
        relevance = embedding_materials._relevance_targets(sq)
        started = time.perf_counter()
        ranked = embedding_materials._rank_materials(provider, materials, vectors, query, limit)
        latency_ms = (time.perf_counter() - started) * 1000.0
        query_latency_total += latency_ms
        row = embedding_materials._metric_row(
            repo=str(config["name"]),
            strategy=strategy,
            query=query,
            expected=expected,
            label=str(sq.get("label", "")),
            relevance=relevance,
            ranked=ranked,
            latency_ms=latency_ms,
            index_ms=embed_ms,
            material_count=len(materials),
            ref_count=len({material.ref for material in materials}),
            provider=provider.name,
        )
        row.update(
            {
                "benchmark": "embedding_material_model",
                "model": model_spec,
                "strategy": strategy_name,
                "model_provider": provider.name,
                "load_ms": round(load_ms, 3),
                "material_ms": round(material_ms, 3),
                "embed_ms": round(embed_ms, 3),
                "nodes_per_second": round(len(materials) / (embed_ms / 1000.0), 3)
                if embed_ms > 0
                else 0.0,
            }
        )
        rows.append(row)

    summary = {
        "model": model_spec,
        "model_provider": provider.name,
        "status": "ok",
        "strategy": strategy_name,
        "material_count": len(materials),
        "ref_count": len({material.ref for material in materials}),
        "load_ms": round(load_ms, 3),
        "material_ms": round(material_ms, 3),
        "embed_ms": round(embed_ms, 3),
        "query_ms": round(query_latency_total, 3),
        "nodes_per_second": round(len(materials) / (embed_ms / 1000.0), 3)
        if embed_ms > 0
        else 0.0,
    }
    for aggregate in _aggregate(rows, model_spec):
        if aggregate["label"] in {"aggregate_positive", "aggregate_negative"}:
            prefix = "positive" if aggregate["label"] == "aggregate_positive" else "negative"
            for key, value in aggregate.items():
                if key in {"benchmark", "model", "label", "query_type", "query_count"}:
                    continue
                summary[f"{prefix}_{key}"] = value
    return rows, summary


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", default=".")
    parser.add_argument("--config", default="dagayn/eval/configs/dagayn.yaml")
    parser.add_argument("--strategy", default=DEFAULT_STRATEGY)
    parser.add_argument("--model", action="append", dest="models")
    parser.add_argument("--output", default="evaluate/results/local_embedding_model_benchmark.csv")
    parser.add_argument(
        "--summary-output",
        default="evaluate/results/local_embedding_model_summary.csv",
    )
    parser.add_argument("--limit", type=int, default=20)
    parser.add_argument("--max-chars", type=int, default=4096)
    parser.add_argument("--openai-batch-size", type=int, default=16)
    args = parser.parse_args()

    repo_path = Path(args.repo).resolve()
    config = yaml.safe_load((repo_path / args.config).read_text(encoding="utf-8"))
    models = args.models or DEFAULT_MODELS
    all_rows: list[dict[str, Any]] = []
    summaries: list[dict[str, Any]] = []
    store = GraphStore(get_db_path(repo_path))
    try:
        for model in models:
            print(f"== {model} ==", flush=True)
            try:
                rows, summary = _model_rows(
                    repo_path=repo_path,
                    store=store,
                    config=config,
                    model_spec=model,
                    strategy_name=args.strategy,
                    limit=args.limit,
                    max_chars=args.max_chars,
                    openai_batch_size=args.openai_batch_size,
                )
            except Exception as exc:
                summary = {"model": model, "status": "error", "error": str(exc)}
                rows = []
                print(json.dumps(summary, ensure_ascii=False), flush=True)
            else:
                print(json.dumps(summary, ensure_ascii=False), flush=True)
            all_rows.extend(rows)
            summaries.append(summary)
    finally:
        store.close()

    output = repo_path / args.output
    output.parent.mkdir(parents=True, exist_ok=True)
    if all_rows:
        with output.open("w", newline="", encoding="utf-8") as fh:
            writer = csv.DictWriter(fh, fieldnames=list(all_rows[0].keys()))
            writer.writeheader()
            writer.writerows(all_rows)

    summary_output = repo_path / args.summary_output
    if summaries:
        fieldnames = sorted({key for row in summaries for key in row})
        with summary_output.open("w", newline="", encoding="utf-8") as fh:
            writer = csv.DictWriter(fh, fieldnames=fieldnames)
            writer.writeheader()
            writer.writerows(summaries)
    print(f"rows={output}")
    print(f"summary={summary_output}")


if __name__ == "__main__":
    main()
