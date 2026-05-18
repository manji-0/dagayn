"""eval command — argument registration and handler."""

from __future__ import annotations

import argparse
from pathlib import Path


def register_command(sub: argparse._SubParsersAction) -> argparse.ArgumentParser:
    """Register the eval subcommand. Returns the subparser."""
    eval_parser = sub.add_parser("eval", help="Run evaluation benchmarks")
    eval_parser.add_argument(
        "--benchmark",
        default=None,
        help="Comma-separated benchmarks to run (token_efficiency, impact_accuracy, "
        "flow_completeness, search_quality, build_performance, nplusone_count, mcp_latency)",
    )
    eval_parser.add_argument("--repo", default=None, help="Comma-separated repo config names")
    eval_parser.add_argument(
        "--all", action="store_true", dest="run_all", help="Run all benchmarks"
    )
    eval_parser.add_argument("--report", action="store_true", help="Generate report from results")
    eval_parser.add_argument("--output-dir", default=None, help="Output directory for results")
    return eval_parser


def handle(args: argparse.Namespace) -> None:
    """Run evaluation benchmarks."""
    from ...eval.reporter import generate_full_report, generate_readme_tables
    from ...eval.runner import run_eval

    if getattr(args, "report", False):
        output_dir = Path(getattr(args, "output_dir", None) or "evaluate/results")
        report = generate_full_report(output_dir)
        report_path = Path("evaluate/reports/summary.md")
        report_path.parent.mkdir(parents=True, exist_ok=True)
        report_path.write_text(report, encoding="utf-8")
        print(f"Report written to {report_path}")

        tables = generate_readme_tables(output_dir)
        print("\n--- README Tables (copy-paste) ---\n")
        print(tables)
    else:
        repos = [r.strip() for r in args.repo.split(",")] if getattr(args, "repo", None) else None
        benchmarks = (
            [b.strip() for b in args.benchmark.split(",")]
            if getattr(args, "benchmark", None)
            else None
        )

        if not repos and not benchmarks and not getattr(args, "run_all", False):
            print("Specify --all, --repo, or --benchmark. See --help.")
            return

        results = run_eval(
            repos=repos,
            benchmarks=benchmarks,
            output_dir=getattr(args, "output_dir", None),
        )
        print(f"\nCompleted {len(results)} benchmark(s).")
        print("Run 'dagayn eval --report' to generate tables.")
