<!-- derived-from ../dagayn/eval/semantics.py -->
<!-- derived-from ../dagayn/eval/aggregate.py -->

# Evaluation Semantics

dagayn evaluation measures codebase structural memory: whether agents can
retrieve relevant repository knowledge, reason about change impact, produce
actionable review guidance, and do that within practical token, latency, and
build budgets.

There is no single universal score for that value. A low-latency result is not
correctness, a proxy graph-consistency result is not an explicit oracle, and a
token-compression result is valuable only after quality gates pass.

## Metric Roles

| Role | Meaning |
| --- | --- |
| gate | A pass/fail or threshold signal that can block trust in a profile. |
| score | A capability measurement that may contribute to a profile score. |
| cost | A resource or runtime measurement such as tokens, latency, or build time. |
| diagnostic | Context that explains behavior but should not be averaged into capability. |
| metadata | A descriptive field that helps interpret a row. |

## Oracle Types

| Oracle | Meaning |
| --- | --- |
| explicit | Human-authored or configured expected output. |
| proxy | A graph-derived approximation of expected behavior. |
| synthetic | A generated or experiment-only target used for calibration. |
| none | No correctness oracle is attached to the metric. |

Explicit oracle metrics can contribute to profile capability scores when their
metric spec permits it. Proxy metrics and synthetic representation experiments
are excluded from headline capability scores by default.

## Why Costs Stay Separate

Token efficiency, build time, SQL count, and query latency are cost or
operability metrics. They matter because dagayn should fit inside practical
agent budgets, but they do not prove retrieval quality or review correctness.
Token efficiency is therefore reported as efficiency/cost and becomes valuable
after quality gates are acceptable.

`impact_accuracy.graph_proxy_*` metrics are similarly conservative. They compare
one graph-derived view against another graph-derived view, so they are useful
for internal consistency diagnostics, but they are excluded from headline review
capability.

## Profile Summaries

Profile summaries combine only semantically compatible metrics:

| Profile | Capability Inputs | Cost Inputs | Conservative Status |
| --- | --- | --- | --- |
| search | explicit `search_quality` MRR, hit@5, nDCG@20 | separate latency diagnostics | `insufficient_oracle` when no explicit search rows exist |
| review | explicit impact and guidance metrics | none | excludes `graph_proxy_*` rows |
| architecture | explicit flow entry-point recall | none | flow counts alone are diagnostic |
| operability | none | build, latency, SQL cost normalization | reports `efficiency_score`, not capability |
| regression | future baseline comparison | future delta costs | `baseline_missing` without a baseline |

Cost normalization is used only for profile efficiency summaries. Raw benchmark
rows keep their original values.

## Example Metric Semantics

```text
search_quality.reciprocal_rank => retrieval / score / explicit / locator_quality
impact_accuracy.graph_proxy_recall => impact / diagnostic / proxy / graph_internal_consistency
token_efficiency.diff_to_graph_ratio => efficiency / cost / none / compression_efficiency
guidance_precision.field_coverage => guidance / score / none / schema_completeness
```

## Adding A Benchmark Metric

1. Add the raw metric to the benchmark row without changing existing fields.
2. Register a `MetricSpec` in `dagayn/eval/semantics.py`.
3. Choose the metric family, role, oracle type, construct, aggregation policy,
   direction, and headline validity conservatively.
4. If the metric is a gate, set `quality_gate=True`.
5. Update profile aggregation in `dagayn/eval/aggregate.py` only if the metric
   is semantically compatible with that profile.
6. Add tests for row decoration, profile behavior, and report output.
