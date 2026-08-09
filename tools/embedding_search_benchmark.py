#!/usr/bin/env python3
"""Micro-benchmark for embedding search backends (Rust / numpy / pure-Python)."""

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


def _decode_vector(blob: bytes) -> list[float]:
    n = len(blob) // 4
    return list(struct.unpack(f"{n}f", blob))


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


def _summarize(backend: str, times: list[float], loaded_rows: int) -> dict[str, Any]:
    return {
        "backend": backend,
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

    return _summarize("native-rust", times, loaded)


def _benchmark_python(
    db_path: Path, provider: str, query: list[float], limit: int, iterations: int, warmup: int
) -> dict[str, Any]:
    """Full pure-Python cosine scan (decode + score + sort) each iteration."""
    conn = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
    try:
        rows = conn.execute(
            "SELECT qualified_name, vector FROM embeddings WHERE provider = ?",
            (provider,),
        ).fetchall()
    finally:
        conn.close()

    decoded = [(qn, _decode_vector(blob)) for qn, blob in rows]
    query_norm = math.sqrt(sum(v * v for v in query))

    def _run() -> list[tuple[str, float]]:
        scored = []
        for qn, vec in decoded:
            dot = sum(a * b for a, b in zip(query, vec))
            scored.append((qn, dot / query_norm))
        scored.sort(key=lambda x: x[1], reverse=True)
        return scored[:limit]

    for _ in range(warmup):
        _run()

    times: list[float] = []
    for _ in range(iterations):
        start = time.perf_counter()
        result = _run()
        elapsed = time.perf_counter() - start
        times.append(elapsed)
        assert len(result) == limit

    return _summarize("pure-python", times, len(rows))


def _benchmark_numpy(
    db_path: Path, provider: str, query: list[float], limit: int, iterations: int, warmup: int
) -> dict[str, Any] | None:
    try:
        import numpy as np
    except ImportError:
        return None

    from dagayn.embeddings_store import _load_vec_matrix, _np_vec_cache

    conn = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
    conn.row_factory = sqlite3.Row
    try:
        matrix, names, row_norms = _load_vec_matrix(conn, provider)
    finally:
        conn.close()

    if not names:
        return None

    # Seed the process cache the same way EmbeddingStore.search does.
    _np_vec_cache.clear()
    stamp = int(db_path.stat().st_mtime_ns)
    _np_vec_cache[(str(db_path), provider, stamp)] = (matrix, names, row_norms)

    q = np.array(query, dtype=np.float32)
    q = q / float(np.linalg.norm(q))
    safe_norms = np.where(row_norms > 0, row_norms, 1.0)

    def _run() -> list[tuple[str, float]]:
        sims = (matrix @ q / safe_norms).astype(np.float32)
        k = min(limit, len(names))
        if k == len(names):
            top_idx = np.argsort(-sims)
        else:
            # argpartition kth is 0-based: k-1 places the k-th largest on the boundary.
            top_idx = np.argpartition(-sims, k - 1)[:k]
            top_idx = top_idx[np.argsort(-sims[top_idx])]
        return [(names[int(i)], float(sims[i])) for i in top_idx]

    for _ in range(warmup):
        _run()

    times: list[float] = []
    for _ in range(iterations):
        start = time.perf_counter()
        result = _run()
        elapsed = time.perf_counter() - start
        times.append(elapsed)
        assert len(result) == limit

    return _summarize("numpy-matmul", times, len(names))


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rows", type=int, default=10000)
    parser.add_argument("--dim", type=int, default=384)
    parser.add_argument("--limit", type=int, default=20)
    parser.add_argument("--iterations", type=int, default=20)
    parser.add_argument("--warmup", type=int, default=3)
    parser.add_argument("--db", type=Path, default=None)
    parser.add_argument("--compare-python", action="store_true")
    parser.add_argument(
        "--compare-numpy",
        action="store_true",
        help="Also time the optional numpy BLAS matmul path (requires numpy).",
    )
    args = parser.parse_args()

    db_path = args.db or Path(tempfile.mkdtemp(prefix="dagayn-embed-bench-")) / "graph.db"
    provider = "bench-provider"
    rng = random.Random(7)
    query = _normalize([rng.random() for _ in range(args.dim)])

    print(f"Building benchmark DB: {db_path}")
    print(f"  rows={args.rows}, dim={args.dim}, limit={args.limit}")
    _build_db(db_path, args.rows, args.dim, provider)

    results: list[dict[str, Any]] = []

    native = _benchmark_native(db_path, provider, query, args.limit, args.iterations, args.warmup)
    results.append(native)

    if args.compare_numpy:
        numpy_result = _benchmark_numpy(
            db_path, provider, query, args.limit, args.iterations, args.warmup
        )
        if numpy_result is None:
            print(
                "\nnumpy unavailable — skip --compare-numpy "
                "(install with: pip install 'dagayn[numpy]')"
            )
        else:
            results.append(numpy_result)

    if args.compare_python:
        py = _benchmark_python(db_path, provider, query, args.limit, args.iterations, args.warmup)
        results.append(py)

    print("\nResults:")
    for r in results:
        backend = r["backend"]
        mean_ms = r["mean_ms"]
        best_ms = r["best_ms"]
        std_ms = r["std_ms"]
        rows = r["loaded_rows"]
        throughput = rows / (mean_ms / 1000.0)
        per_row_ns = mean_ms * 1_000_000 / rows
        print(
            f"  {backend:20s}  mean={mean_ms:8.4f} ms  "
            f"best={best_ms:8.4f} ms  std={std_ms:8.4f} ms  "
            f"rows={rows}"
        )
        print(
            f"                        throughput={throughput:,.0f} rows/s  "
            f"per-row={per_row_ns:.2f} ns"
        )

    by_name = {r["backend"]: r for r in results}
    if "pure-python" in by_name and "native-rust" in by_name:
        speedup = by_name["pure-python"]["mean_ms"] / by_name["native-rust"]["mean_ms"]
        print(f"\nNative is {speedup:.2f}x faster than pure-Python (decode once, score each iter).")
    if "pure-python" in by_name and "numpy-matmul" in by_name:
        speedup = by_name["pure-python"]["mean_ms"] / by_name["numpy-matmul"]["mean_ms"]
        print(f"numpy matmul is {speedup:.2f}x faster than pure-Python (cached matrix vs loop).")


if __name__ == "__main__":
    main()
