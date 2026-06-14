"""Semantic metadata for evaluation benchmark metrics."""

from __future__ import annotations

import json
from dataclasses import asdict, dataclass
from enum import Enum
from typing import Any

try:
    from enum import StrEnum
except ImportError:  # pragma: no cover - Python < 3.11 compatibility

    class StrEnum(str, Enum):
        """Small fallback with the same string behavior as enum.StrEnum."""


class MetricFamily(StrEnum):
    RETRIEVAL = "retrieval"
    IMPACT = "impact"
    GUIDANCE = "guidance"
    COVERAGE = "coverage"
    EFFICIENCY = "efficiency"
    RELIABILITY = "reliability"
    DIAGNOSTIC = "diagnostic"


class MetricRole(StrEnum):
    GATE = "gate"
    SCORE = "score"
    COST = "cost"
    DIAGNOSTIC = "diagnostic"
    METADATA = "metadata"


class OracleType(StrEnum):
    EXPLICIT = "explicit"
    PROXY = "proxy"
    SYNTHETIC = "synthetic"
    NONE = "none"


class Construct(StrEnum):
    LOCATOR_QUALITY = "locator_quality"
    CHANGE_IMPACT_FIDELITY = "change_impact_fidelity"
    GRAPH_INTERNAL_CONSISTENCY = "graph_internal_consistency"
    AGENT_GUIDANCE_ACTIONABILITY = "agent_guidance_actionability"
    SCHEMA_COMPLETENESS = "schema_completeness"
    STRUCTURAL_COVERAGE = "structural_coverage"
    COMPRESSION_EFFICIENCY = "compression_efficiency"
    BUILD_OPERABILITY = "build_operability"
    QUERY_LATENCY = "query_latency"
    SQL_SCALABILITY = "sql_scalability"
    RUN_VALIDITY = "run_validity"
    REPRESENTATION_EXPERIMENT = "representation_experiment"


class AggregationPolicy(StrEnum):
    EXCLUDED = "excluded"
    MEAN = "mean"
    MEDIAN = "median"
    MIN = "min"
    MAX = "max"
    THRESHOLD_GATE = "threshold_gate"
    WEIGHTED_MEAN = "weighted_mean"
    LATEST = "latest"


class MetricDirection(StrEnum):
    HIGHER_IS_BETTER = "higher_is_better"
    LOWER_IS_BETTER = "lower_is_better"
    NOMINAL = "nominal"


@dataclass(frozen=True)
class MetricSpec:
    benchmark: str
    metric: str
    family: MetricFamily
    role: MetricRole
    oracle_type: OracleType
    construct: Construct
    aggregation_policy: AggregationPolicy
    direction: MetricDirection
    valid_for_headline: bool = False
    weight: float = 1.0
    description: str = ""
    quality_gate: bool = False


def _spec(
    benchmark: str,
    metric: str,
    family: MetricFamily,
    role: MetricRole,
    oracle_type: OracleType,
    construct: Construct,
    aggregation_policy: AggregationPolicy,
    direction: MetricDirection,
    *,
    valid_for_headline: bool = False,
    weight: float = 1.0,
    description: str = "",
    quality_gate: bool = False,
) -> MetricSpec:
    return MetricSpec(
        benchmark=benchmark,
        metric=metric,
        family=family,
        role=role,
        oracle_type=oracle_type,
        construct=construct,
        aggregation_policy=aggregation_policy,
        direction=direction,
        valid_for_headline=valid_for_headline,
        weight=weight,
        description=description,
        quality_gate=quality_gate,
    )


def _retrieval_specs(
    benchmark: str,
    *,
    oracle_type: OracleType,
    valid_for_headline: bool,
    construct: Construct = Construct.LOCATOR_QUALITY,
) -> list[MetricSpec]:
    specs: list[MetricSpec] = []
    for metric in ("reciprocal_rank", "mean_mrr"):
        specs.append(
            _spec(
                benchmark,
                metric,
                MetricFamily.RETRIEVAL,
                MetricRole.SCORE,
                oracle_type,
                construct,
                AggregationPolicy.MEAN,
                MetricDirection.HIGHER_IS_BETTER,
                valid_for_headline=valid_for_headline,
            )
        )
    for metric in (
        "hit_at_1",
        "hit_at_5",
        "hit_at_20",
        "precision_at_1",
        "precision_at_5",
        "precision_at_20",
        "ndcg_at_5",
        "ndcg_at_20",
        "mean_ndcg_at_5",
        "mean_ndcg_at_20",
    ):
        specs.append(
            _spec(
                benchmark,
                metric,
                MetricFamily.RETRIEVAL,
                MetricRole.SCORE,
                oracle_type,
                construct,
                AggregationPolicy.MEAN,
                MetricDirection.HIGHER_IS_BETTER,
                valid_for_headline=valid_for_headline,
            )
        )
    for metric in ("latency_ms", "median_ms", "p95_ms", "best_ms", "worst_ms"):
        specs.append(
            _spec(
                benchmark,
                metric,
                MetricFamily.EFFICIENCY,
                MetricRole.COST,
                OracleType.NONE,
                Construct.QUERY_LATENCY,
                AggregationPolicy.MEDIAN,
                MetricDirection.LOWER_IS_BETTER,
                valid_for_headline=False,
            )
        )
    return specs


def _registry() -> dict[tuple[str, str], MetricSpec]:
    specs: list[MetricSpec] = []

    for metric in (
        "changed_file_to_graph_ratio",
        "diff_to_graph_ratio",
        "graph_context_tokens",
        "naive_to_graph_ratio",
        "standard_to_graph_ratio",
        "graph_tokens",
    ):
        specs.append(
            _spec(
                "token_efficiency",
                metric,
                MetricFamily.EFFICIENCY,
                MetricRole.COST,
                OracleType.NONE,
                Construct.COMPRESSION_EFFICIENCY,
                AggregationPolicy.EXCLUDED,
                MetricDirection.LOWER_IS_BETTER,
                description="Token compression or graph-context cost metric.",
            )
        )

    specs.extend(
        _retrieval_specs(
            "search_quality",
            oracle_type=OracleType.EXPLICIT,
            valid_for_headline=True,
        )
    )
    specs.extend(
        _retrieval_specs(
            "fts_quality",
            oracle_type=OracleType.SYNTHETIC,
            valid_for_headline=False,
        )
    )
    specs.extend(
        _retrieval_specs(
            "doc_fuzzy_search",
            oracle_type=OracleType.SYNTHETIC,
            valid_for_headline=False,
        )
    )
    for benchmark in ("embedding_text_modes", "embedding_materials"):
        specs.extend(
            _retrieval_specs(
                benchmark,
                oracle_type=OracleType.SYNTHETIC,
                valid_for_headline=False,
                construct=Construct.REPRESENTATION_EXPERIMENT,
            )
        )
        for metric in ("negative_top_score", "false_positive", "false_positive_threshold"):
            specs.append(
                _spec(
                    benchmark,
                    metric,
                    MetricFamily.DIAGNOSTIC,
                    MetricRole.DIAGNOSTIC,
                    OracleType.SYNTHETIC,
                    Construct.LOCATOR_QUALITY,
                    AggregationPolicy.EXCLUDED,
                    MetricDirection.NOMINAL,
                    valid_for_headline=False,
                )
            )

    for metric in ("precision", "recall", "f1"):
        specs.append(
            _spec(
                "impact_accuracy",
                metric,
                MetricFamily.IMPACT,
                MetricRole.GATE if metric in {"recall", "f1"} else MetricRole.SCORE,
                OracleType.EXPLICIT,
                Construct.CHANGE_IMPACT_FIDELITY,
                AggregationPolicy.MEAN,
                MetricDirection.HIGHER_IS_BETTER,
                valid_for_headline=True,
                quality_gate=metric == "recall",
            )
        )
    for metric in ("graph_proxy_precision", "graph_proxy_recall", "graph_proxy_f1"):
        specs.append(
            _spec(
                "impact_accuracy",
                metric,
                MetricFamily.IMPACT,
                MetricRole.DIAGNOSTIC,
                OracleType.PROXY,
                Construct.GRAPH_INTERNAL_CONSISTENCY,
                AggregationPolicy.EXCLUDED,
                MetricDirection.HIGHER_IS_BETTER,
                valid_for_headline=False,
            )
        )

    specs.append(
        _spec(
            "flow_completeness",
            "recall",
            MetricFamily.COVERAGE,
            MetricRole.SCORE,
            OracleType.EXPLICIT,
            Construct.STRUCTURAL_COVERAGE,
            AggregationPolicy.MEAN,
            MetricDirection.HIGHER_IS_BETTER,
            valid_for_headline=True,
            description="Architecture-profile entry-point recall.",
        )
    )
    for metric in ("detected_flows", "avg_flow_depth", "max_flow_depth", "detected_entry_points"):
        specs.append(
            _spec(
                "flow_completeness",
                metric,
                MetricFamily.DIAGNOSTIC,
                MetricRole.DIAGNOSTIC,
                OracleType.NONE,
                Construct.STRUCTURAL_COVERAGE,
                AggregationPolicy.EXCLUDED,
                MetricDirection.NOMINAL,
            )
        )

    for metric in ("precision_at_k", "recall", "f1"):
        specs.append(
            _spec(
                "guidance_precision",
                metric,
                MetricFamily.GUIDANCE,
                MetricRole.SCORE,
                OracleType.EXPLICIT,
                Construct.AGENT_GUIDANCE_ACTIONABILITY,
                AggregationPolicy.MEAN,
                MetricDirection.HIGHER_IS_BETTER,
                valid_for_headline=True,
            )
        )
    specs.append(
        _spec(
            "guidance_precision",
            "field_coverage",
            MetricFamily.GUIDANCE,
            MetricRole.SCORE,
            OracleType.NONE,
            Construct.SCHEMA_COMPLETENESS,
            AggregationPolicy.MEAN,
            MetricDirection.HIGHER_IS_BETTER,
            valid_for_headline=False,
        )
    )

    for metric in ("build_total_ms", "median_build_total_ms", "best_build_total_ms"):
        specs.append(
            _spec(
                "build_performance",
                metric,
                MetricFamily.EFFICIENCY,
                MetricRole.COST,
                OracleType.NONE,
                Construct.BUILD_OPERABILITY,
                AggregationPolicy.MEDIAN,
                MetricDirection.LOWER_IS_BETTER,
            )
        )
    for metric in ("files_per_second", "nodes_per_second"):
        specs.append(
            _spec(
                "build_performance",
                metric,
                MetricFamily.EFFICIENCY,
                MetricRole.DIAGNOSTIC,
                OracleType.NONE,
                Construct.BUILD_OPERABILITY,
                AggregationPolicy.EXCLUDED,
                MetricDirection.HIGHER_IS_BETTER,
            )
        )
    specs.append(
        _spec(
            "build_performance",
            "errors_count",
            MetricFamily.RELIABILITY,
            MetricRole.GATE,
            OracleType.NONE,
            Construct.RUN_VALIDITY,
            AggregationPolicy.THRESHOLD_GATE,
            MetricDirection.LOWER_IS_BETTER,
            quality_gate=True,
        )
    )

    specs.append(
        _spec(
            "nplusone_count",
            "sql_count",
            MetricFamily.EFFICIENCY,
            MetricRole.DIAGNOSTIC,
            OracleType.NONE,
            Construct.SQL_SCALABILITY,
            AggregationPolicy.EXCLUDED,
            MetricDirection.LOWER_IS_BETTER,
        )
    )
    specs.append(
        _spec(
            "nplusone_count",
            "status",
            MetricFamily.RELIABILITY,
            MetricRole.GATE,
            OracleType.NONE,
            Construct.RUN_VALIDITY,
            AggregationPolicy.THRESHOLD_GATE,
            MetricDirection.NOMINAL,
            quality_gate=True,
        )
    )

    for metric in ("median_ms", "worst_ms", "best_ms", "p95_ms"):
        specs.append(
            _spec(
                "mcp_latency",
                metric,
                MetricFamily.EFFICIENCY,
                MetricRole.COST,
                OracleType.SYNTHETIC,
                Construct.QUERY_LATENCY,
                AggregationPolicy.MEDIAN,
                MetricDirection.LOWER_IS_BETTER,
            )
        )

    specs.append(
        _spec(
            "recent_changes_effects",
            "speedup",
            MetricFamily.EFFICIENCY,
            MetricRole.DIAGNOSTIC,
            OracleType.SYNTHETIC,
            Construct.BUILD_OPERABILITY,
            AggregationPolicy.EXCLUDED,
            MetricDirection.HIGHER_IS_BETTER,
        )
    )

    return {(spec.benchmark, spec.metric): spec for spec in specs}


METRIC_SPECS: dict[tuple[str, str], MetricSpec] = _registry()


def get_metric_spec(benchmark: str, metric: str) -> MetricSpec | None:
    """Return the registered semantic spec for a benchmark metric."""
    return METRIC_SPECS.get((benchmark, metric))


def metric_specs_for_benchmark(benchmark: str) -> list[MetricSpec]:
    """Return all registered specs for *benchmark* in stable metric order."""
    return sorted(
        (spec for (bench, _), spec in METRIC_SPECS.items() if bench == benchmark),
        key=lambda spec: spec.metric,
    )


def metric_specs_for_row(row: dict[str, Any]) -> list[MetricSpec]:
    """Return specs for recognized metric columns present with non-empty values."""
    benchmark = str(row.get("benchmark", ""))
    specs: list[MetricSpec] = []
    for metric in sorted(row):
        value = row.get(metric)
        if value in (None, ""):
            continue
        spec = get_metric_spec(benchmark, metric)
        if spec is not None:
            specs.append(spec)
    return specs


def _unique_value(specs: list[MetricSpec], attr: str) -> str | bool | None:
    values = {getattr(spec, attr) for spec in specs}
    if len(values) != 1:
        return None
    value = values.pop()
    if isinstance(value, StrEnum):
        return value.value
    return value


def _stable_metric_list(specs: list[MetricSpec]) -> str:
    return ";".join(spec.metric for spec in sorted(specs, key=lambda spec: spec.metric))


def decorate_metric_row(row: dict[str, Any]) -> dict[str, Any]:
    """Add semantic metadata to a benchmark row without changing existing values."""
    out = dict(row)
    specs = metric_specs_for_row(out)
    if not specs:
        return out

    for field, attr in (
        ("metric_family", "family"),
        ("metric_role", "role"),
        ("oracle_type", "oracle_type"),
        ("construct", "construct"),
        ("aggregation_policy", "aggregation_policy"),
        ("valid_for_headline", "valid_for_headline"),
    ):
        value = _unique_value(specs, attr)
        if value is not None:
            out.setdefault(field, value)

    direction = _unique_value(specs, "direction")
    if direction is not None:
        out.setdefault("higher_is_better", direction == MetricDirection.HIGHER_IS_BETTER.value)

    if len(specs) > 1:
        headline = [spec for spec in specs if spec.valid_for_headline]
        diagnostic = [
            spec
            for spec in specs
            if spec.role == MetricRole.DIAGNOSTIC or not spec.valid_for_headline
        ]
        families = sorted({spec.family.value for spec in specs})
        roles = sorted({spec.role.value for spec in specs})
        oracles = sorted({spec.oracle_type.value for spec in specs})
        notes = [
            "multi_metric_row",
            f"families={','.join(families)}",
            f"roles={','.join(roles)}",
            f"oracles={','.join(oracles)}",
        ]
        if out.get("benchmark") == "mcp_latency" and out.get("transport") == "direct_function_call":
            notes.append("direct_function_call_latency_not_mcp_transport")
        out.setdefault("semantic_notes", ";".join(notes))
        out.setdefault("headline_metrics", _stable_metric_list(headline))
        out.setdefault("diagnostic_metrics", _stable_metric_list(diagnostic))
    elif out.get("benchmark") == "mcp_latency" and out.get("transport") == "direct_function_call":
        out.setdefault("semantic_notes", "direct_function_call_latency_not_mcp_transport")

    return out


def decorate_rows(rows: list[dict[str, Any]]) -> list[dict[str, Any]]:
    """Decorate benchmark rows with metric semantics."""
    return [decorate_metric_row(row) for row in rows]


def metric_specs_as_dicts() -> list[dict[str, Any]]:
    """Return the registry in stable, JSON-ready order."""
    records = []
    for spec in sorted(METRIC_SPECS.values(), key=lambda item: (item.benchmark, item.metric)):
        record = asdict(spec)
        for key, value in list(record.items()):
            if isinstance(value, StrEnum):
                record[key] = value.value
        records.append(record)
    return records


def metric_specs_json() -> str:
    """Return the metric registry as stable JSON."""
    return json.dumps(metric_specs_as_dicts(), indent=2, sort_keys=True)
