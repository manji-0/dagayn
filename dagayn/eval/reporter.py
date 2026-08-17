"""Markdown report generator for evaluation benchmark results.

Takes a list of benchmark result dicts and produces a formatted markdown table
suitable for inclusion in documentation or CI output.
"""

from __future__ import annotations

import csv
import json
from pathlib import Path
from typing import Any

from dagayn.eval.aggregate import (
    PROFILES,
    profile_summaries_as_dicts,
    summarize_all_profiles,
    summarize_profile,
)
from dagayn.eval.runner import BENCHMARK_REGISTRY
from dagayn.eval.semantics import decorate_rows, metric_specs_as_dicts

type ReportValue = Any
type ReportPayload = dict[str, ReportValue]


def _escape_md_cell(value: Any) -> str:
    text = str(value)
    return (
        text.replace("\\", "\\\\")
        .replace("|", "\\|")
        .replace("`", "\\`")
        .replace("\r\n", "<br>")
        .replace("\n", "<br>")
    )


def generate_markdown_report(results: list[ReportPayload]) -> str:
    """Generate a markdown report from benchmark results.

    Each result dict should contain at minimum a ``benchmark`` key identifying
    the benchmark name, plus any metric keys (e.g. ``ratio``,
    ``reduction_percent``, ``mrr``, ``precision``, ``recall``, ``f1``).

    Args:
        results: List of result dicts from benchmark runs.

    Returns:
        A markdown string containing a summary table and per-benchmark details.
    """
    if not results:
        return "# Evaluation Report\n\nNo benchmark results to report.\n"

    lines: list[str] = []
    lines.append("# Evaluation Report")
    lines.append("")

    # Collect all metric keys across results (excluding 'benchmark')
    all_keys: list[str] = []
    seen: set[str] = set()
    for r in results:
        for k in r:
            if k != "benchmark" and k not in seen:
                all_keys.append(k)
                seen.add(k)

    # Summary table
    lines.append("## Summary")
    lines.append("")

    header = "| Benchmark | " + " | ".join(all_keys) + " |"
    separator = "| --- | " + " | ".join("---" for _ in all_keys) + " |"
    lines.append(header)
    lines.append(separator)

    for r in results:
        name = r.get("benchmark", "unknown")
        values = [_escape_md_cell(r.get(k, "-")) for k in all_keys]
        lines.append(f"| {_escape_md_cell(name)} | " + " | ".join(values) + " |")

    lines.append("")

    # Per-benchmark detail sections
    lines.append("## Details")
    lines.append("")
    for r in results:
        name = r.get("benchmark", "unknown")
        lines.append(f"### {name}")
        lines.append("")
        for k in all_keys:
            v = r.get(k, "-")
            lines.append(f"- **{k}**: {_escape_md_cell(v)}")
        lines.append("")

    return "\n".join(lines)


def _read_csvs(results_dir: Path, prefix: str) -> list[dict[str, str]]:
    """Read all CSV files matching a prefix from the results directory."""
    rows: list[dict[str, str]] = []
    for p in sorted(results_dir.glob(f"*_{prefix}_*.csv")):
        with open(p, newline="") as f:
            reader = csv.DictReader(f)
            rows.extend(reader)
    return rows


def _read_all_csvs(results_dir: Path) -> list[dict[str, str]]:
    """Read all benchmark CSV files from the results directory."""
    rows: list[dict[str, str]] = []
    for benchmark in BENCHMARK_REGISTRY:
        rows.extend(_read_csvs(results_dir, benchmark))
    return decorate_rows(rows)  # type: ignore[return-value]


def _md_table(headers: list[str], rows: list[list[str]]) -> str:
    """Build a markdown table from headers and rows."""
    lines = []
    lines.append("| " + " | ".join(_escape_md_cell(h) for h in headers) + " |")
    lines.append("| " + " | ".join("---" for _ in headers) + " |")
    for row in rows:
        lines.append("| " + " | ".join(_escape_md_cell(cell) for cell in row) + " |")
    return "\n".join(lines)


def _format_score(value: float | None) -> str:
    if value is None:
        return "-"
    return f"{value:.4f}".rstrip("0").rstrip(".")


def _reports_dir_for(results_dir: Path, reports_dir: str | Path | None) -> Path:
    if reports_dir is not None:
        return Path(reports_dir)
    if results_dir.name == "results":
        return results_dir.parent / "reports"
    return results_dir / "reports"


def _write_semantic_json(
    summaries: list[Any],
    reports_dir: Path,
) -> None:
    reports_dir.mkdir(parents=True, exist_ok=True)
    (reports_dir / "profile_summary.json").write_text(
        json.dumps(profile_summaries_as_dicts(summaries), indent=2, sort_keys=True),
        encoding="utf-8",
    )
    (reports_dir / "metric_semantics.json").write_text(
        json.dumps(metric_specs_as_dicts(), indent=2, sort_keys=True),
        encoding="utf-8",
    )


def generate_full_report(
    results_dir: str | Path,
    *,
    profile: str = "all",
    semantic_report: bool = True,
    reports_dir: str | Path | None = None,
) -> str:
    """Generate a full markdown evaluation report from CSV result files.

    Reads all CSV files in *results_dir*, groups them by benchmark type,
    and produces a markdown report with methodology notes and per-benchmark
    result tables.

    Args:
        results_dir: Directory containing CSV result files.

    Returns:
        Markdown string with the full report.
    """
    results_dir = Path(results_dir)
    if profile != "all" and profile not in PROFILES:
        raise ValueError(f"Unknown evaluation profile: {profile}")

    all_rows = _read_all_csvs(results_dir)
    summaries = (
        summarize_all_profiles(all_rows)
        if profile == "all"
        else [summarize_profile(all_rows, profile)]
    )
    if semantic_report:
        _write_semantic_json(summaries, _reports_dir_for(results_dir, reports_dir))

    lines: list[str] = []
    lines.append("# Evaluation Report")
    lines.append("")
    lines.append("## Methodology")
    lines.append("")
    lines.append("Benchmarks are run against real open-source repositories.")
    lines.append("Token counts record the active counter in `token_counter`.")
    lines.append(
        "Impact accuracy uses explicit oracle labels when present; "
        "graph-derived rows are marked `status=proxy`."
    )
    lines.append("")

    if semantic_report:
        lines.append("## Evaluation Semantics")
        lines.append("")
        lines.append("This report separates:")
        lines.append("- capability scores")
        lines.append("- efficiency/cost metrics")
        lines.append("- gates")
        lines.append("- diagnostics")
        lines.append("- proxy/synthetic metrics")
        lines.append("")
        lines.append(
            "Proxy metrics and synthetic representation experiments are excluded from "
            "headline capability scores unless a profile explicitly includes them."
        )
        lines.append("")

        lines.append("## Profile Summary")
        lines.append("")
        profile_rows = []
        for summary in summaries:
            gates = "; ".join(f"{key}={value}" for key, value in sorted(summary.gates.items()))
            notes = "; ".join(summary.notes)
            profile_rows.append(
                [
                    summary.profile,
                    summary.status,
                    _format_score(summary.capability_score),
                    _format_score(summary.efficiency_score),
                    gates or "-",
                    notes or "-",
                ]
            )
        lines.append(
            _md_table(
                [
                    "Profile",
                    "Status",
                    "Capability Score",
                    "Efficiency Score",
                    "Gates",
                    "Notes",
                ],
                profile_rows,
            )
        )
        lines.append("")

        lines.append("## Metric Semantics")
        lines.append("")
        spec_rows = []
        for spec in metric_specs_as_dicts():
            spec_rows.append(
                [
                    str(spec["benchmark"]),
                    str(spec["metric"]),
                    str(spec["family"]),
                    str(spec["role"]),
                    str(spec["oracle_type"]),
                    str(spec["construct"]),
                    str(spec["direction"]),
                    "yes" if spec["valid_for_headline"] else "no",
                ]
            )
        lines.append(
            _md_table(
                [
                    "Benchmark",
                    "Metric",
                    "Family",
                    "Role",
                    "Oracle",
                    "Construct",
                    "Direction",
                    "Headline?",
                ],
                spec_rows,
            )
        )
        lines.append("")

    benchmark_types = list(BENCHMARK_REGISTRY.keys())

    for btype in benchmark_types:
        rows = _read_csvs(results_dir, btype)
        if not rows:
            continue

        title = btype.replace("_", " ").title()
        lines.append(f"## {title}")
        lines.append("")

        headers = list(rows[0].keys())
        table_rows = [[r.get(h, "-") for h in headers] for r in rows]
        lines.append(_md_table(headers, table_rows))
        lines.append("")

    if not all_rows:
        lines.append("No benchmark results found.")
        lines.append("")

    return "\n".join(lines)


def generate_readme_tables(results_dir: str | Path) -> str:
    """Generate concise README-ready tables from CSV result files.

    Produces concise profile-oriented tables:
    - Search Capability
    - Review Capability
    - Operability
    - Diagnostics

    Args:
        results_dir: Directory containing CSV result files.

    Returns:
        Markdown string with the three tables.
    """
    results_dir = Path(results_dir)
    lines: list[str] = []

    # Operability: costs and budgets, not correctness.
    te_rows = _read_csvs(results_dir, "token_efficiency")
    if te_rows:
        lines.append("### Operability")
        lines.append("")
        headers = [
            "Repo",
            "Files",
            "Naive Tokens",
            "Standard Tokens",
            "Graph Tokens",
            "Naive/Graph",
            "Std/Graph",
        ]
        table_rows = []
        for r in te_rows:
            table_rows.append(
                [
                    r.get("repo", "-"),
                    r.get("changed_files", "-"),
                    r.get("naive_changed_file_tokens", r.get("naive_tokens", "-")),
                    r.get("diff_tokens", r.get("standard_tokens", "-")),
                    r.get("graph_context_tokens", r.get("graph_tokens", "-")),
                    r.get("changed_file_to_graph_ratio", r.get("naive_to_graph_ratio", "-")),
                    r.get("diff_to_graph_ratio", r.get("standard_to_graph_ratio", "-")),
                ]
            )
        lines.append(_md_table(headers, table_rows))
        lines.append("")

    # Capability and diagnostic slices should not imply one global score.
    ia_rows = _read_csvs(results_dir, "impact_accuracy")
    fc_rows = _read_csvs(results_dir, "flow_completeness")
    sq_rows = _read_csvs(results_dir, "search_quality")
    fts_rows = _read_csvs(results_dir, "fts_quality")

    if sq_rows or fts_rows:
        lines.append("### Search Capability")
        lines.append("")
        headers = ["Repo", "Search MRR", "FTS MRR"]
        repo_data: dict[str, dict[str, object]] = {}
        mrr_accum: dict[str, list[float]] = {}
        for r in sq_rows:
            repo = r.get("repo", "?")
            repo_data.setdefault(repo, {})
            try:
                mrr_accum.setdefault(repo, []).append(float(r.get("reciprocal_rank", 0)))
            except (ValueError, TypeError):
                pass
        fts_accum: dict[str, list[float]] = {}
        for r in fts_rows:
            repo = r.get("repo", "?")
            repo_data.setdefault(repo, {})
            try:
                fts_accum.setdefault(repo, []).append(float(r.get("reciprocal_rank", 0)))
            except (ValueError, TypeError):
                pass
        table_rows = []
        for repo in sorted(repo_data):
            mrr_vals = mrr_accum.get(repo, [])
            mrr = str(round(sum(mrr_vals) / len(mrr_vals), 3)) if mrr_vals else "-"
            fts_vals = fts_accum.get(repo, [])
            fts = str(round(sum(fts_vals) / len(fts_vals), 3)) if fts_vals else "-"
            table_rows.append([repo, mrr, fts])
        lines.append(_md_table(headers, table_rows))
        lines.append("")

    if ia_rows or fc_rows:
        lines.append("### Review Capability")
        lines.append("")
        headers = ["Repo", "Impact F1", "Flow Recall"]
        repo_data: dict[str, dict[str, object]] = {}
        for r in ia_rows:
            if r.get("status") == "proxy":
                continue
            repo_data.setdefault(r.get("repo", "?"), {})["f1"] = r.get("f1", "-")
        for r in fc_rows:
            repo_data.setdefault(r.get("repo", "?"), {})["recall"] = r.get("recall", "-")
        table_rows = []
        for repo, d in sorted(repo_data.items()):
            table_rows.append([repo, str(d.get("f1", "-")), str(d.get("recall", "-"))])
        lines.append(_md_table(headers, table_rows))
        lines.append("")

    # Diagnostics: useful operational detail, not headline capability.
    bp_rows = _read_csvs(results_dir, "build_performance")
    if bp_rows:
        lines.append("### Diagnostics")
        lines.append("")
        headers = ["Repo", "Files", "Nodes", "Build (ms)", "Nodes/s"]
        table_rows = []
        for r in bp_rows:
            table_rows.append(
                [
                    r.get("repo", "-"),
                    r.get("files_parsed", r.get("file_count", "-")),
                    r.get("nodes", r.get("node_count", "-")),
                    r.get("build_total_ms", "-"),
                    r.get("nodes_per_second", "-"),
                ]
            )
        lines.append(_md_table(headers, table_rows))
        lines.append("")

    if not lines:
        return "No benchmark results found.\n"

    return "\n".join(lines)
