#!/usr/bin/env python3
"""Micro-benchmark for the native Rust embedding search."""

from __future__ import annotations

import argparse
import math
import random
import sqlite3
import statistics
import struct
import tempfile
import time
from pathlib import Path
from typing import Any


def _encode_vector(vec: list[float]) -> bytes:
    return struct.pack(f"{len(vec)}f", *vec)


def _normalize(vec: list[float]) -> list[float]:
    norm = math.sqrt(sum(v * v for v in vec))
    if norm == 0.0:
        return vec
    inv = 1.0 / norm
    return [v * inv for v in vec]


def _build_db(db_path: Path, rows: int, dim: int, provider: str) -> None:
    db_path.parent.mkdir(parents=True, exist_ok=True)
    if db_path.exists():
        db_path.unlink()
    conn = sqlite3.connect(str(db_path))
    try:
        conn.executescript(
            """
            CREATE TABLE embeddings (
                qualified_name TEXT NOT NULL,
                vector BLOB NOT NULL,
                text_hash TEXT NOT NULL,
                provider TEXT NOT NULL,
                PRIMARY KEY (qualified_name, provider)
            );
            """
        )
        rng = random.Random(42)
        records = []
        for i in range(rows):
            vec = _normalize([rng.random() for _ in range(dim)])
            records.append((f"node::{i}", _encode_vector(vec), "deadbeef", provider))
        conn.executemany(
            "INSERT INTO embeddings "
            "(qualified_name, vector, text_hash, provider) "
            "VALUES (?, ?, ?, ?)",
            records,
        )
        conn.commit()
    finally:
        conn.close()


def _summarize(times: list[float], loaded_rows: int) -> dict[str, Any]:
    return {
        "loaded_rows": loaded_rows,
        "mean_ms": statistics.mean(times) * 1000,
        "best_ms": min(times) * 1000,
        "std_ms": (statistics.stdev(times) * 1000) if len(times) > 1 else 0.0,
    }


def _benchmark_native(
    db_path: Path, provider: str, query: list[float], limit: int, iterations: int, warmup: int
) -> dict[str, Any]:
    from dagayn import _core

    # Load matrix into the process cache.
    loaded = _core.embedding_search_prewarm(str(db_path), provider)
    for _ in range(warmup):
        _core.embedding_search(str(db_path), provider, query, limit)

    times: list[float] = []
    for _ in range(iterations):
        start = time.perf_counter()
        result = _core.embedding_search(str(db_path), provider, query, limit)
        elapsed = time.perf_counter() - start
        times.append(elapsed)
        assert len(result) == limit, f"expected {limit} results, got {len(result)}"

    return _summarize(times, loaded)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rows", type=int, default=10000)
    parser.add_argument("--dim", type=int, default=384)
    parser.add_argument("--limit", type=int, default=20)
    parser.add_argument("--iterations", type=int, default=20)
    parser.add_argument("--warmup", type=int, default=3)
    parser.add_argument("--db", type=Path, default=None)
    args = parser.parse_args()

    db_path = args.db or Path(tempfile.mkdtemp(prefix="dagayn-embed-bench-")) / "graph.db"
    provider = "bench-provider"
    rng = random.Random(7)
    query = _normalize([rng.random() for _ in range(args.dim)])

    print(f"Building benchmark DB: {db_path}")
    print(f"  rows={args.rows}, dim={args.dim}, limit={args.limit}")
    _build_db(db_path, args.rows, args.dim, provider)

    r = _benchmark_native(db_path, provider, query, args.limit, args.iterations, args.warmup)
    mean_ms = r["mean_ms"]
    rows = r["loaded_rows"]
    throughput = rows / (mean_ms / 1000.0)
    per_row_ns = mean_ms * 1_000_000 / rows
    print("\nResults:")
    print(
        f"  mean={mean_ms:8.4f} ms  best={r['best_ms']:8.4f} ms  "
        f"std={r['std_ms']:8.4f} ms  rows={rows}"
    )
    print(f"  throughput={throughput:,.0f} rows/s  per-row={per_row_ns:.2f} ns")


if __name__ == "__main__":
    main()
