"""Ranking helpers shared by the embedding benchmarks."""

from __future__ import annotations

import math


def _cosine(left: list[float], right: list[float]) -> float:
    if not left or not right:
        return 0.0
    dot = sum(a * b for a, b in zip(left, right))
    left_norm = math.sqrt(sum(a * a for a in left))
    right_norm = math.sqrt(sum(b * b for b in right))
    if left_norm == 0.0 or right_norm == 0.0:
        return 0.0
    return dot / (left_norm * right_norm)


def _matches_expected(qualified_name: str, expected: str) -> bool:
    qn_lower = qualified_name.lower()
    exp_lower = expected.lower()
    exp_name = expected.rsplit("::", 1)[-1] if "::" in expected else expected
    qn_name = qualified_name.rsplit("::", 1)[-1] if "::" in qualified_name else qualified_name
    return exp_lower in qn_lower or qn_lower in exp_lower or exp_name.lower() == qn_name.lower()
