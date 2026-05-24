"""Scoring metrics for evaluating graph-based code review quality.

Provides:
- Token efficiency: measures how many tokens the graph saves vs raw context.
- Mean Reciprocal Rank (MRR): evaluates ranking quality for search results.
- Precision / Recall / F1: evaluates set-based retrieval accuracy.
"""

from __future__ import annotations


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


def compute_precision_recall(predicted: set, actual: set) -> dict:
    """Compute precision, recall, and F1 score.

    Args:
        predicted: Set of predicted/returned items.
        actual: Set of ground-truth items.

    Returns:
        Dict with keys: precision, recall, f1.
    """
    if not predicted and not actual:
        return {"precision": 1.0, "recall": 1.0, "f1": 1.0}

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


def compute_precision_at_k(predicted: list[str], actual: set[str], k: int = 5) -> dict:
    """Compute precision@k for ranked guidance outputs.

    Args:
        predicted: Ranked identifiers returned by a tool.
        actual: Ground-truth relevant identifiers.
        k: Cutoff rank. Values less than 1 are treated as 1.

    Returns:
        Dict with precision_at_k, hits, k, returned, and relevant.
    """
    cutoff = max(1, int(k))
    returned = [item for item in predicted[:cutoff] if item]
    if not returned and not actual:
        precision = 1.0
        hits = 0
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
