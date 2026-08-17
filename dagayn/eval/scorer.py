"""Scoring metrics for evaluating graph-based code review quality.

Provides:
- Token efficiency: measures how many tokens the graph saves vs raw context.
- Mean Reciprocal Rank (MRR): evaluates ranking quality for search results.
- Precision / Recall / F1: evaluates set-based retrieval accuracy.
"""

from __future__ import annotations

from collections.abc import Iterable, Mapping
from pathlib import PurePosixPath
from typing import Any

type ScoreValue = Any
type ScorePayload = dict[str, ScoreValue]

def compute_token_efficiency(raw_tokens: int, graph_tokens: int) -> dict:
    """Compute token efficiency metrics.

    Args:
        raw_tokens: Number of tokens when sending raw source code.
        graph_tokens: Number of tokens when using graph-based context.

    Returns:
        Dict with keys:
        - raw_tokens: the raw token count
        - graph_tokens: the graph token count
        - ratio: graph_tokens / raw_tokens (lower is better)
        - reduction_percent: percentage of tokens saved (higher is better)
    """
    if raw_tokens <= 0:
        return {
            "raw_tokens": raw_tokens,
            "graph_tokens": graph_tokens,
            "ratio": 0.0,
            "reduction_percent": 0.0,
        }
    ratio = graph_tokens / raw_tokens
    reduction = (1.0 - ratio) * 100.0
    return {
        "raw_tokens": raw_tokens,
        "graph_tokens": graph_tokens,
        "ratio": round(ratio, 4),
        "reduction_percent": round(reduction, 2),
    }


def compute_mrr(correct: str, results: list[str]) -> float:
    """Compute Mean Reciprocal Rank for a single query.

    Args:
        correct: The correct/expected result identifier.
        results: Ordered list of result identifiers (best first).

    Returns:
        1/rank if *correct* is found in *results*, else 0.0.
    """
    for i, r in enumerate(results, start=1):
        if r == correct:
            return 1.0 / i
    return 0.0


def compute_precision_recall(
    predicted: set,
    actual: set,
    *,
    perfect_empty: bool = False,
) -> dict:
    """Compute precision, recall, and F1 score.

    Args:
        predicted: Set of predicted/returned items.
        actual: Set of ground-truth items.

    Returns:
        Dict with keys: precision, recall, f1.
    """
    if not predicted and not actual and perfect_empty:
        return {"precision": 1.0, "recall": 1.0, "f1": 1.0}
    if not predicted and not actual:
        return {"precision": None, "recall": None, "f1": None, "status": "skipped"}

    true_positive = len(predicted & actual)
    precision = true_positive / len(predicted) if predicted else 0.0
    recall = true_positive / len(actual) if actual else 0.0

    if precision + recall > 0:
        f1 = 2 * precision * recall / (precision + recall)
    else:
        f1 = 0.0

    return {
        "precision": round(precision, 4),
        "recall": round(recall, 4),
        "f1": round(f1, 4),
    }


def compute_precision_at_k(
    predicted: list[str],
    actual: set[str],
    k: int = 5,
    *,
    perfect_empty: bool = False,
) -> dict:
    """Compute precision@k for ranked guidance outputs.

    Args:
        predicted: Ranked identifiers returned by a tool.
        actual: Ground-truth relevant identifiers.
        k: Cutoff rank. Values less than 1 are treated as 1.

    Returns:
        Dict with precision_at_k, hits, k, returned, and relevant.
    """
    cutoff = max(1, k)
    returned = [item for item in predicted[:cutoff] if item]
    if not returned and not actual and perfect_empty:
        precision = 1.0
        hits = 0
    elif not returned and not actual:
        return {
            "precision_at_k": None,
            "hits": 0,
            "k": cutoff,
            "returned": 0,
            "relevant": 0,
            "status": "skipped",
        }
    else:
        hits = len(set(returned) & actual)
        precision = hits / cutoff
    return {
        "precision_at_k": round(precision, 4),
        "hits": hits,
        "k": cutoff,
        "returned": len(returned),
        "relevant": len(actual),
    }


def _aliases_from_config(config: ScorePayload | None) -> dict[str, set[str]]:
    aliases: dict[str, set[str]] = {}
    if not config:
        return aliases
    for item in config.get("aliases", []) or config.get("match_aliases", []) or []:
        if not isinstance(item, dict):
            continue
        target = str(item.get("target") or item.get("canonical") or "")
        values = {str(v) for v in item.get("aliases", []) if v}
        if target and values:
            aliases.setdefault(target, set()).update(values)
            for value in values:
                aliases.setdefault(value, set()).add(target)
    return aliases


class IdentifierMatcher:
    """Exact qualified-name matcher with explicit aliases and opt-in basename matching."""

    def __init__(
        self,
        aliases: Mapping[str, Iterable[str]] | None = None,
        *,
        allow_basename: bool = False,
    ) -> None:
        self.allow_basename = allow_basename
        self.aliases: dict[str, set[str]] = {}
        for target, values in (aliases or {}).items():
            target_s = target
            self.aliases.setdefault(target_s, set()).update(value for value in values)
            for value in values:
                self.aliases.setdefault(value, set()).add(target_s)

    @classmethod
    def from_config(cls, config: ScorePayload | None) -> "IdentifierMatcher":
        return cls(
            _aliases_from_config(config),
            allow_basename=bool((config or {}).get("allow_basename_match", False)),
        )

    def _equivalents(self, value: str) -> set[str]:
        values = {value, *self.aliases.get(value, set())}
        if self.allow_basename:
            values.update(
                PurePosixPath(v.rsplit("::", 1)[0]).name + "::" + v.rsplit("::", 1)[-1]
                for v in list(values)
                if "::" in v
            )
            values.update(v.rsplit("::", 1)[-1] for v in list(values))
        return {v.lower() for v in values if v}

    def matches(self, candidate: str, expected: str) -> bool:
        if not candidate or not expected:
            return False
        return bool(self._equivalents(candidate) & self._equivalents(expected))

    def first_rank(self, candidates: list[str], expected: str) -> int:
        for idx, candidate in enumerate(candidates, start=1):
            if self.matches(candidate, expected):
                return idx
        return 0
