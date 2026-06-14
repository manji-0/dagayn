"""Profile-level aggregation for semantically annotated eval rows."""

from __future__ import annotations

from dataclasses import asdict, dataclass, field
from typing import Any

from dagayn.eval.semantics import (
    MetricRole,
    OracleType,
    get_metric_spec,
    metric_specs_for_row,
)

PROFILES = {"search", "review", "architecture", "operability", "regression"}
ERROR_STATUSES = {"error", "skipped"}


@dataclass
class ProfileSummary:
    profile: str
    status: str
    score: float | None
    capability_score: float | None = None
    efficiency_score: float | None = None
    notes: list[str] = field(default_factory=list)
    gates: dict[str, str] = field(default_factory=dict)
    components: dict[str, float] = field(default_factory=dict)


def _to_float(value: Any) -> float | None:
    try:
        if value in (None, ""):
            return None
        return float(value)
    except (TypeError, ValueError):
        return None


def _clamp(value: float) -> float:
    return min(1.0, max(0.0, value))


def normalize_higher_better(value: Any, target: float, floor: float = 0.0) -> float | None:
    """Normalize a higher-is-better metric to [0, 1]."""
    numeric = _to_float(value)
    if numeric is None:
        return None
    if target <= floor:
        return None
    return _clamp((numeric - floor) / (target - floor))


def normalize_lower_better(value: Any, budget: float, worst: float | None = None) -> float | None:
    """Normalize a lower-is-better cost metric to [0, 1]."""
    numeric = _to_float(value)
    if numeric is None:
        return None
    if budget <= 0:
        return None
    if numeric <= budget:
        return 1.0
    worst = worst if worst is not None else budget * 4
    if worst <= budget:
        return 0.0
    return _clamp(1.0 - ((numeric - budget) / (worst - budget)))


def _mean(values: list[float]) -> float | None:
    if not values:
        return None
    return round(sum(values) / len(values), 4)


def _weighted_mean(components: dict[str, float], weights: dict[str, float]) -> float | None:
    total = 0.0
    denom = 0.0
    for key, value in components.items():
        weight = weights.get(key, 0.0)
        if weight <= 0:
            continue
        total += value * weight
        denom += weight
    if denom == 0:
        return None
    return round(total / denom, 4)


def _eligible_rows(rows: list[dict[str, Any]]) -> list[dict[str, Any]]:
    return [row for row in rows if str(row.get("status", "ok")) not in ERROR_STATUSES]


def _add_error_notes(summary: ProfileSummary, rows: list[dict[str, Any]]) -> None:
    for row in rows:
        status = str(row.get("status", "ok"))
        if status == "error":
            benchmark = row.get("benchmark", "unknown")
            error = row.get("error", "")
            summary.notes.append(f"{benchmark} error: {error}".strip())
            summary.gates[str(benchmark)] = "error"
        elif status == "skipped":
            benchmark = row.get("benchmark", "unknown")
            summary.notes.append(f"{benchmark} skipped")


def _explicit_metric_values(
    rows: list[dict[str, Any]],
    benchmark: str,
    metric: str,
) -> list[float]:
    values: list[float] = []
    spec = get_metric_spec(benchmark, metric)
    if spec is None or spec.oracle_type != OracleType.EXPLICIT:
        return values
    for row in _eligible_rows(rows):
        if row.get("benchmark") != benchmark:
            continue
        if str(row.get("status", "ok")) == "proxy":
            continue
        value = _to_float(row.get(metric))
        if value is not None:
            values.append(value)
    return values


def _search_profile(rows: list[dict[str, Any]]) -> ProfileSummary:
    summary = ProfileSummary(profile="search", status="ok", score=None)
    _add_error_notes(summary, rows)
    components = {
        "mrr": _mean(_explicit_metric_values(rows, "search_quality", "reciprocal_rank")),
        "hit_at_5": _mean(_explicit_metric_values(rows, "search_quality", "hit_at_5")),
        "ndcg_at_20": _mean(_explicit_metric_values(rows, "search_quality", "ndcg_at_20")),
    }
    summary.components = {key: value for key, value in components.items() if value is not None}
    if not summary.components:
        summary.status = "insufficient_oracle"
        summary.notes.append("No explicit search_quality oracle rows found.")
        return summary
    score = _weighted_mean(summary.components, {"mrr": 0.45, "hit_at_5": 0.35, "ndcg_at_20": 0.20})
    summary.score = score
    summary.capability_score = score
    if summary.gates:
        summary.status = "fail"
    return summary


def _review_profile(rows: list[dict[str, Any]]) -> ProfileSummary:
    summary = ProfileSummary(profile="review", status="ok", score=None)
    _add_error_notes(summary, rows)
    explicit_components = {
        "impact_f1": _mean(_explicit_metric_values(rows, "impact_accuracy", "f1")),
        "impact_recall": _mean(_explicit_metric_values(rows, "impact_accuracy", "recall")),
        "guidance_precision": _mean(
            _explicit_metric_values(rows, "guidance_precision", "precision_at_k")
        ),
        "guidance_f1": _mean(_explicit_metric_values(rows, "guidance_precision", "f1")),
    }
    summary.components = {
        key: value for key, value in explicit_components.items() if value is not None
    }
    field_coverage = _mean(
        [
            value
            for row in _eligible_rows(rows)
            if row.get("benchmark") == "guidance_precision"
            for value in [_to_float(row.get("field_coverage"))]
            if value is not None
        ]
    )
    if field_coverage is not None:
        summary.components["field_coverage"] = field_coverage
        summary.notes.append("field_coverage is reported separately from core correctness.")

    proxy_rows = [
        row
        for row in rows
        if row.get("benchmark") == "impact_accuracy"
        and any(spec.oracle_type == OracleType.PROXY for spec in metric_specs_for_row(row))
    ]
    if proxy_rows:
        summary.notes.append("Proxy impact metrics are excluded from review capability score.")

    scoring_components = {
        key: value
        for key, value in summary.components.items()
        if key in {"impact_f1", "impact_recall", "guidance_precision", "guidance_f1"}
    }
    if not scoring_components:
        summary.status = "insufficient_oracle"
        summary.notes.append("No explicit impact or guidance oracle rows found.")
        return summary

    impact_recall = scoring_components.get("impact_recall")
    if impact_recall is not None and impact_recall < 0.6:
        summary.gates["impact_recall"] = f"fail ({impact_recall} < 0.6)"
    guidance_precision = scoring_components.get("guidance_precision")
    if guidance_precision is not None and guidance_precision < 0.5:
        summary.gates["guidance_precision"] = f"fail ({guidance_precision} < 0.5)"

    score = _weighted_mean(
        scoring_components,
        {
            "impact_f1": 0.45,
            "impact_recall": 0.25,
            "guidance_precision": 0.20,
            "guidance_f1": 0.10,
        },
    )
    summary.score = score
    summary.capability_score = score
    if summary.gates:
        summary.status = "fail"
    return summary


def _architecture_profile(rows: list[dict[str, Any]]) -> ProfileSummary:
    summary = ProfileSummary(profile="architecture", status="ok", score=None)
    _add_error_notes(summary, rows)
    recall = _mean(_explicit_metric_values(rows, "flow_completeness", "recall"))
    if recall is None:
        summary.status = "insufficient_oracle"
        summary.notes.append("No explicit flow_completeness entry-point oracle rows found.")
        diagnostic_rows = [
            row
            for row in rows
            if row.get("benchmark") == "flow_completeness"
            and any(spec.role == MetricRole.DIAGNOSTIC for spec in metric_specs_for_row(row))
        ]
        if diagnostic_rows:
            summary.notes.append("Flow counts are diagnostic and not treated as higher-is-better.")
        return summary
    summary.components = {"entry_point_recall": recall}
    summary.score = recall
    summary.capability_score = recall
    if summary.gates:
        summary.status = "fail"
    return summary


def _operability_profile(rows: list[dict[str, Any]]) -> ProfileSummary:
    summary = ProfileSummary(profile="operability", status="ok", score=None)
    _add_error_notes(summary, rows)
    components: dict[str, float] = {}
    exceeded: list[str] = []

    build_values = [
        value
        for row in _eligible_rows(rows)
        if row.get("benchmark") == "build_performance"
        for value in (
            _to_float(row.get("median_build_total_ms")),
            _to_float(row.get("build_total_ms")),
        )
        if value is not None
    ]
    if build_values:
        build_ms = min(build_values)
        score = normalize_lower_better(build_ms, budget=60_000)
        if score is not None:
            components["build_cost"] = score
        if build_ms > 60_000:
            exceeded.append("build_total_ms")

    latency_values = [
        value
        for row in _eligible_rows(rows)
        if row.get("benchmark") in {"search_quality", "mcp_latency"}
        for value in (
            _to_float(row.get("p95_ms")),
            _to_float(row.get("median_ms")),
            _to_float(row.get("latency_ms")),
        )
        if value is not None
    ]
    if latency_values:
        latency_ms = max(latency_values)
        score = normalize_lower_better(latency_ms, budget=250)
        if score is not None:
            components["query_latency_cost"] = score
        if latency_ms > 250:
            exceeded.append("query_latency")

    sql_values = [
        value
        for row in _eligible_rows(rows)
        if row.get("benchmark") == "nplusone_count"
        for value in [_to_float(row.get("sql_count"))]
        if value is not None
    ]
    if sql_values:
        sql_count = max(sql_values)
        score = normalize_lower_better(sql_count, budget=100)
        if score is not None:
            components["sql_scalability"] = score
        if sql_count > 100:
            exceeded.append("sql_count")

    summary.components = components
    summary.efficiency_score = _mean(list(components.values()))
    summary.score = summary.efficiency_score
    summary.capability_score = None
    if summary.gates:
        summary.status = "fail"
    elif exceeded:
        summary.status = "warn"
        for metric in exceeded:
            summary.gates[metric] = "budget_exceeded"
    elif not components:
        summary.status = "diagnostic_only"
        summary.notes.append("No cost metrics were available for efficiency scoring.")
    return summary


def _regression_profile(rows: list[dict[str, Any]]) -> ProfileSummary:
    del rows
    return ProfileSummary(
        profile="regression",
        status="baseline_missing",
        score=None,
        notes=["No baseline rows were supplied; regression comparison is not computed yet."],
    )


def summarize_profile(rows: list[dict[str, Any]], profile: str) -> ProfileSummary:
    """Summarize benchmark rows for one semantic profile."""
    if profile == "search":
        return _search_profile(rows)
    if profile == "review":
        return _review_profile(rows)
    if profile == "architecture":
        return _architecture_profile(rows)
    if profile == "operability":
        return _operability_profile(rows)
    if profile == "regression":
        return _regression_profile(rows)
    raise ValueError(f"Unknown evaluation profile: {profile}")


def summarize_all_profiles(rows: list[dict[str, Any]]) -> list[ProfileSummary]:
    """Summarize all built-in profiles in stable order."""
    return [summarize_profile(rows, profile) for profile in sorted(PROFILES)]


def profile_summary_as_dict(summary: ProfileSummary) -> dict[str, Any]:
    """Convert a profile summary to a JSON-ready dictionary."""
    return asdict(summary)


def profile_summaries_as_dicts(summaries: list[ProfileSummary]) -> list[dict[str, Any]]:
    """Convert profile summaries to JSON-ready dictionaries."""
    return [profile_summary_as_dict(summary) for summary in summaries]
