"""Precision@k benchmark for review and refactor guidance."""

from __future__ import annotations

from pathlib import Path
from typing import Any

from dagayn.eval.scorer import compute_precision_at_k


def _ids_from_items(items: list[dict[str, Any]], *keys: str) -> list[str]:
    ids: list[str] = []
    for item in items:
        for key in keys:
            value = item.get(key)
            if isinstance(value, str) and value:
                ids.append(value)
                break
    return ids


def _review_predictions(repo_path: Path, case: dict[str, Any]) -> dict[str, list[str]]:
    from dagayn.tools.review import detect_changes_func

    result = detect_changes_func(
        repo_root=str(repo_path),
        changed_files=list(case.get("changed_files", [])),
        base=str(case.get("base", "HEAD~1")),
        detail_level="standard",
    )
    summary = result.get("analysis_summary", {}) if isinstance(result, dict) else {}
    return {
        "recommended_tests": _ids_from_items(
            list(summary.get("recommended_tests", [])),
            "qualified_name",
            "name",
        ),
        "documentation_update_candidates": _ids_from_items(
            list(summary.get("documentation_update_candidates", [])),
            "qualified_name",
            "file",
        ),
    }


def _refactor_predictions(repo_path: Path, case: dict[str, Any]) -> list[str]:
    from dagayn.tools.refactor_tools import refactor_func

    result = refactor_func(
        mode="suggest",
        repo_root=str(repo_path),
        limit=int(case.get("limit", case.get("k", 5))),
    )
    suggestions = list(result.get("suggestions", [])) if isinstance(result, dict) else []
    out: list[str] = []
    for suggestion in suggestions:
        symbols = suggestion.get("symbols", [])
        if isinstance(symbols, list) and symbols:
            out.append(str(symbols[0]))
    return out


def run(repo_path: Path, store: Any, config: dict) -> list[dict]:
    """Run configured guidance precision cases.

    Config shape:

    ``guidance_precision_cases`` is a list of cases with ``kind`` set to
    ``recommended_tests``, ``documentation_update_candidates``, or
    ``refactor_suggestions``. Each case supplies ``expected`` identifiers and
    optional ``k``. Review cases also supply ``changed_files``.
    """
    del store  # tool calls open their own short-lived store connections
    cases = list(config.get("guidance_precision_cases", []))
    if not cases:
        return [
            {
                "benchmark": "guidance_precision",
                "case": "no_cases",
                "kind": "none",
                "precision_at_k": 1.0,
                "hits": 0,
                "k": 5,
                "returned": 0,
                "relevant": 0,
            }
        ]

    rows: list[dict] = []
    review_cache: dict[int, dict[str, list[str]]] = {}
    for idx, case in enumerate(cases):
        kind = str(case.get("kind", "recommended_tests"))
        k = int(case.get("k", 5))
        expected = {str(item) for item in case.get("expected", [])}
        if kind in {"recommended_tests", "documentation_update_candidates"}:
            review_cache.setdefault(idx, _review_predictions(repo_path, case))
            predicted = review_cache[idx].get(kind, [])
        elif kind == "refactor_suggestions":
            predicted = _refactor_predictions(repo_path, case)
        else:
            predicted = []

        score = compute_precision_at_k(predicted, expected, k=k)
        rows.append(
            {
                "benchmark": "guidance_precision",
                "case": str(case.get("name", f"case_{idx + 1}")),
                "kind": kind,
                **score,
            }
        )
    return rows
