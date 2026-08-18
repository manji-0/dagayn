"""Compare metadata-only, body, structured, and narrative embedding text modes.

This benchmark deliberately uses a deterministic local token-hash provider so
it can run in CI or on a developer laptop without contacting an embedding API.
It measures the ranking effect of the text fed into the embedding provider,
not the quality of any particular external model.
"""

from __future__ import annotations

import hashlib
import math
import re
import tempfile
import time
from collections import Counter
from pathlib import Path
from typing import Any

from dagayn.embeddings import EmbeddingProvider, EmbeddingStore, embed_all_nodes

type BenchmarkValue = Any
type BenchmarkPayload = dict[str, BenchmarkValue]

_TOKEN_RE = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")
_IDENT_BOUNDARY_RE = re.compile(r"(?<=[a-z0-9])(?=[A-Z])|(?<=[A-Z])(?=[A-Z][a-z])")


class _TokenHashEmbeddingProvider(EmbeddingProvider):
    preferred_batch_size = 128

    def __init__(self, dimension: int = 256) -> None:
        self._dimension = dimension

    @property
    def name(self) -> str:
        return "eval-token-hash"

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
            for token in _IDENT_BOUNDARY_RE.sub(" ", raw_token.replace("_", " ")).lower().split():
                digest = hashlib.blake2b(token.encode("utf-8"), digest_size=4).digest()
                idx = int.from_bytes(digest, "little") % self._dimension
                counts[idx] += 1
        if not counts:
            return [0.0] * self._dimension
        norm = math.sqrt(sum(value * value for value in counts.values()))
        vec = [0.0] * self._dimension
        for idx, value in counts.items():
            vec[idx] = value / norm
        return vec


def _matches_expected(qualified_name: str, expected: str) -> bool:
    qn_lower = qualified_name.lower()
    exp_lower = expected.lower()
    exp_name = expected.rsplit("::", 1)[-1] if "::" in expected else expected
    qn_name = qualified_name.rsplit("::", 1)[-1] if "::" in qualified_name else qualified_name
    return exp_lower in qn_lower or qn_lower in exp_lower or exp_name.lower() == qn_name.lower()


def run(repo_path: Path, store, config: BenchmarkPayload) -> list[BenchmarkPayload]:
    """Run embedding-only search quality for configured node text modes."""
    queries = config.get("embedding_text_mode_queries") or config.get("search_queries", [])
    if not queries:
        return []

    results: list[BenchmarkPayload] = []
    modes = config.get("embedding_text_modes") or ["metadata", "body", "structured", "narrative"]
    allowed_kinds = set(config.get("embedding_text_mode_result_kinds") or [])
    kind_by_qualified_name: dict[str, str] = {}
    if allowed_kinds:
        for node in store.get_all_nodes():
            kind_by_qualified_name[node.qualified_name] = node.kind
    for mode in modes:
        provider = _TokenHashEmbeddingProvider(
            dimension=int(config.get("embedding_text_mode_dimension", 256))
        )
        with tempfile.TemporaryDirectory(prefix=f"dagayn-embedding-{mode}-") as tmpdir:
            emb_store = EmbeddingStore(
                Path(tmpdir) / "embeddings.db",
                provider_instance=provider,
                text_mode=str(mode),
                source_root=repo_path,
            )
            try:
                embed_started = time.perf_counter()
                embedded = embed_all_nodes(store, emb_store)
                embed_ms = (time.perf_counter() - embed_started) * 1000.0

                for sq in queries:
                    query = sq["query"]
                    expected = sq["expected"]
                    label = sq.get("label", "")
                    started = time.perf_counter()
                    raw_limit = int(config.get("embedding_text_mode_search_limit") or 100)
                    limit = raw_limit if allowed_kinds else 20
                    search_results = emb_store.search(query, limit=limit)
                    if allowed_kinds:
                        search_results = [
                            (qualified_name, score)
                            for qualified_name, score in search_results
                            if kind_by_qualified_name.get(qualified_name) in allowed_kinds
                        ][:20]
                    latency_ms = (time.perf_counter() - started) * 1000.0
                    rank = 0
                    for idx, (qualified_name, _score) in enumerate(search_results, start=1):
                        if _matches_expected(qualified_name, expected):
                            rank = idx
                            break
                    results.append(
                        {
                            "repo": config["name"],
                            "text_mode": str(mode),
                            "provider": provider.name,
                            "label": label,
                            "query": query,
                            "expected": expected,
                            "embedded_nodes": embedded,
                            "embedding_build_ms": round(embed_ms, 3),
                            "latency_ms": round(latency_ms, 3),
                            "result_count": len(search_results),
                            "rank": rank,
                            "reciprocal_rank": round(1.0 / rank if rank > 0 else 0.0, 4),
                            "hit_at_5": int(0 < rank <= 5),
                            "hit_at_20": int(0 < rank <= 20),
                        }
                    )
            finally:
                emb_store.close()
    return results
