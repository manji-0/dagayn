"""Precision@k benchmark for review and refactor guidance."""

from __future__ import annotations

from pathlib import Path
from typing import Any

from dagayn.eval.scorer import compute_precision_at_k, compute_precision_recall

type BenchmarkValue = Any
type BenchmarkPayload = dict[str, BenchmarkValue]


def _ids_from_items(items: list[BenchmarkPayload], *keys: str) -> list[str]:
    ids: list[str] = []
    for item in items:
        for key in keys:
            value = item.get(key)
            if isinstance(value, str) and value:
                ids.append(value)
                break
    return ids


_GUIDANCE_FIELDS = {
    "claim",
    "evidence",
    "confidence",
    "missingness",
    "action",
    "reason_codes",
    "counts",
}


def _guidance_ids(items: list[BenchmarkPayload]) -> list[str]:
    ids: list[str] = []
    for item in items:
        ids.extend(str(code) for code in item.get("reason_codes", []) if code)
        claim = item.get("claim")
        if isinstance(claim, str) and claim:
            ids.append(claim)
    return ids


def _field_coverage(items: list[BenchmarkPayload]) -> float:
    if not items:
        return 0.0
    covered = sum(1 for item in items if _GUIDANCE_FIELDS.issubset(item.keys()))
    return covered / len(items)


def _review_cache_key(case: BenchmarkPayload) -> tuple[tuple[str, ...], str, str]:
    return (
        tuple(sorted(str(path) for path in case.get("changed_files", []))),
        str(case.get("base", "HEAD~1")),
        str(case.get("detail_level", "standard")),
    )


def _review_predictions(repo_path: Path, case: BenchmarkPayload) -> dict[str, list[str]]:
    from dagayn.tools.review import detect_changes_func

    result = detect_changes_func(
        repo_root=str(repo_path),
        changed_files=list(case.get("changed_files", [])),
        base=str(case.get("base", "HEAD~1")),
        detail_level="standard",
    )
    summary: BenchmarkPayload = (
        result.get("analysis_summary", {}) if isinstance(result, dict) else {}
    )
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
        "guidance_items": _guidance_ids(list(summary.get("guidance", []))),
        "stable_contract_warnings": _ids_from_items(
            [
                item
                for item in list(summary.get("stability_contracts", []))
                if item.get("status") == "warn"
            ],
            "scope_key",
        ),
        "architecture_leads": [
            key
            for key, value in dict(
                summary.get("architecture_delta", {}).get("counts", {}) or {}
            ).items()
            if value
        ],
        "answerability_warnings": _ids_from_items(
            list(result.get("missingness", [])) if isinstance(result, dict) else [],
            "reason_code",
        ),
        "_guidance_field_coverage": [str(_field_coverage(list(summary.get("guidance", []))))],
    }


def _refactor_predictions(repo_path: Path, case: BenchmarkPayload) -> list[str]:
    from dagayn.tools.refactor_tools import refactor_func

    result = refactor_func(
        mode="suggest",
        repo_root=str(repo_path),
        limit=int(case.get("limit") or case.get("k") or 5),
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
    ``refactor_suggestions``. It also accepts calibrated guidance kinds:
    ``guidance_items``, ``stable_contract_warnings``, ``architecture_leads``,
    ``answerability_warnings``, and ``guidance_field_coverage``. Each case
    supplies ``expected`` identifiers and optional ``k``. Review cases also
    supply ``changed_files``.
    """
    del store  # tool calls open their own short-lived store connections
    cases = list(config.get("guidance_precision_cases", []))
    if not cases:
        return [
            {
                "benchmark": "guidance_precision",
                "case": "no_cases",
                "kind": "none",
                "status": "skipped",
            }
        ]

    rows: list[dict] = []
    review_cache: dict[tuple[tuple[str, ...], str, str], dict[str, list[str]]] = {}
    for idx, case in enumerate(cases):
        kind = str(case.get("kind", "recommended_tests"))
        k = int(case.get("k", 5))
        expected = {str(item) for item in case.get("expected", [])}
        if kind in {
            "recommended_tests",
            "documentation_update_candidates",
            "guidance_items",
            "stable_contract_warnings",
            "architecture_leads",
            "answerability_warnings",
        }:
            cache_key = _review_cache_key(case)
            review_cache.setdefault(cache_key, _review_predictions(repo_path, case))
            predicted = review_cache[cache_key].get(kind, [])
        elif kind == "guidance_field_coverage":
            cache_key = _review_cache_key(case)
            review_cache.setdefault(cache_key, _review_predictions(repo_path, case))
            predicted = review_cache[cache_key].get("_guidance_field_coverage", [])
        elif kind == "refactor_suggestions":
            predicted = _refactor_predictions(repo_path, case)
        else:
            predicted = []

        field_coverage = None
        if kind == "guidance_field_coverage" and predicted:
            try:
                field_coverage = float(predicted[0])
            except ValueError:
                field_coverage = None
            score = {
                "precision_at_k": None,
                "hits": None,
                "k": k,
                "returned": len(predicted),
                "relevant": len(expected),
                "status": "ok",
            }
        else:
            score = compute_precision_at_k(predicted, expected, k=k)
        recall_f1 = {}
        if expected:
            recall_f1 = {
                key: value
                for key, value in compute_precision_recall(set(predicted), expected).items()
                if key in {"recall", "f1"}
            }
        rows.append(
            {
                "benchmark": "guidance_precision",
                "case": str(case.get("name", f"case_{idx + 1}")),
                "kind": kind,
                "field_coverage": field_coverage,
                **score,
                **recall_f1,
            }
        )
    return rows
