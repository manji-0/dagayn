"""build / update / postprocess / watch / status / visualize commands."""

from __future__ import annotations

import argparse

from ...hook_guard import DEFAULT_HOOK_BUDGET_SECONDS
from ._shared import CommandRegistry, _add_local_embedding_args


def register_commands(sub: argparse._SubParsersAction) -> CommandRegistry:
    """Register build/update/postprocess/watch/status/visualize subcommands."""

    # build
    build_cmd = sub.add_parser("build", help="Full graph build (re-parse all files)")
    build_cmd.add_argument("--repo", default=None, help="Repository root (auto-detected)")
    build_cmd.add_argument(
        "--force-full-build",
        "--force",
        dest="force_full_build",
        action="store_true",
        help="Delete the existing graph database before rebuilding",
    )
    build_cmd.add_argument(
        "--skip-flows",
        action="store_true",
        help="Skip flow/community detection (signatures + FTS only)",
    )
    build_cmd.add_argument(
        "--skip-postprocess",
        action="store_true",
        help="Skip all post-processing (raw parse only)",
    )
    build_cmd.add_argument(
        "--scip",
        action="store_true",
        help=(
            "Settle call targets with the SCIP indexers found on this machine "
            "(rust-analyzer, scip-typescript) before post-processing"
        ),
    )
    _add_local_embedding_args(build_cmd)

    # update
    update_cmd = sub.add_parser("update", help="Incremental update (only changed files)")
    update_cmd.add_argument(
        "--base",
        default=None,
        help=("Git diff base (default: the commit the graph was built at, falling back to HEAD~1)"),
    )
    update_cmd.add_argument("--repo", default=None, help="Repository root (auto-detected)")
    update_cmd.add_argument(
        "--skip-flows",
        action="store_true",
        help="Skip flow/community detection (signatures + FTS only)",
    )
    update_cmd.add_argument(
        "--skip-postprocess",
        action="store_true",
        help="Skip all post-processing (raw parse only)",
    )
    update_cmd.add_argument(
        "--budget-seconds",
        type=float,
        default=None,
        help=(
            "Stop the update if it outlives this many seconds. Hook-triggered runs"
            f" (DAGAYN_HOOK_UPDATE=1) default to {DEFAULT_HOOK_BUDGET_SECONDS}s;"
            " manual runs are unbounded. Use 0 to disable."
        ),
    )
    _add_local_embedding_args(update_cmd)

    # postprocess
    pp_cmd = sub.add_parser(
        "postprocess",
        help="Run post-processing on existing graph (flows, communities, FTS)",
    )
    pp_cmd.add_argument("--repo", default=None, help="Repository root (auto-detected)")
    pp_cmd.add_argument("--no-flows", action="store_true", help="Skip flow detection")
    pp_cmd.add_argument("--no-communities", action="store_true", help="Skip community detection")
    pp_cmd.add_argument("--no-fts", action="store_true", help="Skip FTS rebuild")

    # watch
    watch_cmd = sub.add_parser("watch", help="Watch for changes and auto-update")
    watch_cmd.add_argument("--repo", default=None, help="Repository root (auto-detected)")

    # status
    status_cmd = sub.add_parser("status", help="Show graph statistics")
    status_cmd.add_argument("--repo", default=None, help="Repository root (auto-detected)")

    # visualize
    vis_cmd = sub.add_parser(
        "visualize",
        help="Export graph artifacts (GraphML, Mermaid C4, Cypher, Obsidian, SVG)",
    )
    vis_cmd.add_argument("--repo", default=None, help="Repository root (auto-detected)")
    vis_cmd.add_argument(
        "--format",
        choices=["graphml", "mermaid-c4", "cypher", "obsidian", "svg"],
        required=True,
        help="Export format: graphml, mermaid-c4, cypher, obsidian, or svg",
    )

    # detect-adp
    adp_cmd = sub.add_parser("detect-adp", help="Detect cyclic dependencies (ADP violations)")
    adp_cmd.add_argument(
        "--granularity",
        choices=["package", "file"],
        default="package",
        help="Aggregation level: 'package' (directory) or 'file' (default: package)",
    )
    adp_cmd.add_argument(
        "--artifact-scope",
        choices=["code", "docs", "all"],
        default="code",
        help="Analyze code, docs, or the legacy mixed graph (default: code)",
    )
    adp_cmd.add_argument(
        "--min-cycle-size", type=int, default=2, help="Minimum cycle length (default: 2)"
    )
    adp_cmd.add_argument(
        "--max-cycle-length", type=int, default=10, help="Upper bound on cycle length (default: 10)"
    )
    adp_cmd.add_argument(
        "--format",
        choices=["json", "text"],
        default="json",
        help="Output format (default: json)",
    )
    adp_cmd.add_argument("--repo", default=None, help="Repository root (auto-detected)")

    # sdp-metrics
    sdp_metrics_cmd = sub.add_parser(
        "sdp-metrics", help="Compute instability scores per module (SDP)"
    )
    sdp_metrics_cmd.add_argument(
        "--granularity",
        choices=["package", "file"],
        default="package",
        help="Aggregation level: 'package' (directory) or 'file' (default: package)",
    )
    sdp_metrics_cmd.add_argument(
        "--artifact-scope",
        choices=["code", "docs", "all"],
        default="code",
        help="Analyze code, docs, or the legacy mixed graph (default: code)",
    )
    sdp_metrics_cmd.add_argument(
        "--top-n", type=int, default=30, help="Number of entries to return (default: 30)"
    )
    sdp_metrics_cmd.add_argument(
        "--format",
        choices=["json", "text"],
        default="json",
        help="Output format (default: json)",
    )
    sdp_metrics_cmd.add_argument("--repo", default=None, help="Repository root (auto-detected)")

    # detect-sdp
    detect_sdp_cmd = sub.add_parser(
        "detect-sdp", help="Detect stability-direction violations (SDP)"
    )
    detect_sdp_cmd.add_argument(
        "--granularity",
        choices=["package", "file"],
        default="package",
        help="Aggregation level: 'package' (directory) or 'file' (default: package)",
    )
    detect_sdp_cmd.add_argument(
        "--artifact-scope",
        choices=["code", "docs", "all"],
        default="code",
        help="Analyze code, docs, or the legacy mixed graph (default: code)",
    )
    detect_sdp_cmd.add_argument(
        "--min-delta",
        type=float,
        default=0.1,
        help="Minimum instability gap to flag (default: 0.1)",
    )
    detect_sdp_cmd.add_argument(
        "--format",
        choices=["json", "text"],
        default="json",
        help="Output format (default: json)",
    )
    detect_sdp_cmd.add_argument("--repo", default=None, help="Repository root (auto-detected)")

    # sap-metrics
    sap_metrics_cmd = sub.add_parser(
        "sap-metrics", help="Compute abstractness/instability/distance scores per scope (SAP)"
    )
    sap_metrics_cmd.add_argument(
        "--scope-kind",
        choices=["package", "file", "directory"],
        default="package",
        help="Aggregation level: 'package' (directory) or 'file' (default: package)",
    )
    sap_metrics_cmd.add_argument(
        "--unit-filter",
        default=None,
        help="Comma-separated scope_key prefixes to restrict output",
    )
    sap_metrics_cmd.add_argument(
        "--artifact-scope",
        choices=["code", "docs", "all"],
        default="code",
        help="Analyze code, docs, or the legacy mixed graph (default: code)",
    )
    sap_metrics_cmd.add_argument(
        "--top-n", type=int, default=30, help="Number of entries to return (default: 30)"
    )
    sap_metrics_cmd.add_argument(
        "--format",
        choices=["json", "text"],
        default="json",
        help="Output format (default: json)",
    )
    sap_metrics_cmd.add_argument("--repo", default=None, help="Repository root (auto-detected)")

    # detect-sap
    detect_sap_cmd = sub.add_parser(
        "detect-sap", help="Detect scopes far from the main sequence (SAP violations)"
    )
    detect_sap_cmd.add_argument(
        "--scope-kind",
        choices=["package", "file", "directory"],
        default="package",
        help="Aggregation level (default: package)",
    )
    detect_sap_cmd.add_argument(
        "--artifact-scope",
        choices=["code", "docs", "all"],
        default="code",
        help="Analyze code, docs, or the legacy mixed graph (default: code)",
    )
    detect_sap_cmd.add_argument(
        "--min-distance",
        type=float,
        default=0.5,
        help="Minimum D value to flag (default: 0.5)",
    )
    detect_sap_cmd.add_argument(
        "--format",
        choices=["json", "text"],
        default="json",
        help="Output format (default: json)",
    )
    detect_sap_cmd.add_argument("--repo", default=None, help="Repository root (auto-detected)")

    return {
        "build": build_cmd,
        "update": update_cmd,
        "postprocess": pp_cmd,
        "watch": watch_cmd,
        "status": status_cmd,
        "visualize": vis_cmd,
        "detect-adp": adp_cmd,
        "sdp-metrics": sdp_metrics_cmd,
        "detect-sdp": detect_sdp_cmd,
        "sap-metrics": sap_metrics_cmd,
        "detect-sap": detect_sap_cmd,
    }


def handle(args: argparse.Namespace) -> None:
    """Dispatch build/update/postprocess/watch/status/visualize/detect-adp/sdp/sap commands."""
    from .build_handlers import execute_build_command

    execute_build_command(args)
