"""build / update / postprocess / watch / status / visualize commands."""

from __future__ import annotations

import argparse
import logging
from collections.abc import Mapping
from pathlib import Path
from typing import Any

from ...hook_guard import DEFAULT_HOOK_BUDGET_SECONDS
from ._shared import CommandRegistry, _add_local_embedding_args


def _remove_existing_graph_database(db_path: Path) -> list[Path]:
    """Remove the graph database and SQLite sidecar files before a forced build."""
    removed: list[Path] = []
    sidecars = [
        db_path.with_name(f"{db_path.name}{suffix}") for suffix in ("-wal", "-shm", "-journal")
    ]
    candidates = [db_path] + sidecars
    for path in candidates:
        try:
            path.unlink()
        except FileNotFoundError:
            continue
        removed.append(path)
    return removed


def _print_local_embedding_summary(result: Mapping[str, Any]) -> None:
    emb = result.get("local_embedding")
    if not emb:
        return
    runtime = "started server" if emb.get("server_started") else "reused server"
    preset = emb.get("preset")
    text_mode = emb.get("text_mode")
    preset_label = f"{preset}/{text_mode}" if text_mode else preset
    print(
        "Local embeddings "
        f"({preset_label}, {runtime}): "
        f"{emb.get('newly_embedded', 0)} new, "
        f"{emb.get('orphans_removed', 0)} orphan removed, "
        f"{emb.get('total_embeddings', 0)} total"
    )


def _print_embedding_status(db_path: Path) -> None:
    from ...embeddings import get_embedding_status

    status = get_embedding_status(db_path)
    state = status.get("status", "unknown")
    total = int(status.get("total_embeddings") or 0)
    providers = status.get("provider_counts") or {}
    if state == "not_indexed":
        print("Embeddings: not indexed")
        return
    if state == "unavailable":
        message = status.get("error") or "unavailable"
        print(f"Embeddings: unavailable ({message})")
        return

    provider_count = len(providers)
    print(f"Embeddings: {state} ({total} vectors, {provider_count} provider(s))")

    embeddable = status.get("embeddable_nodes")
    indexed = status.get("indexed_embeddings", total)
    missing = status.get("missing_embeddings")
    orphan = status.get("orphan_embeddings")
    if embeddable is not None and missing is not None and orphan is not None:
        print(f"  Coverage: {indexed}/{embeddable} embeddable nodes ({missing} missing)")
        if orphan:
            print(f"  Orphans: {orphan}")
    for provider, count in sorted(providers.items()):
        print(f"  Provider: {provider} ({count})")


def _print_vcs_status(repo_root: Path, store: object) -> None:
    """Print stored VCS metadata and warn when the working copy has drifted."""
    from ...incremental import _git_branch_info, _svn_revision_info, detect_vcs

    get_metadata = getattr(store, "get_metadata")
    stored_branch = get_metadata("git_branch")
    stored_sha = get_metadata("git_head_sha")

    vcs = detect_vcs(repo_root)
    label: str | None = None
    if vcs == "git":
        from ...worktree import main_worktree_root, worktree_label

        label = worktree_label(repo_root)
        if label:
            print(f"Linked worktree: {label} (main checkout: {main_worktree_root(repo_root)})")

    if stored_branch:
        print(f"Built on branch: {stored_branch}")
    if stored_sha:
        print(f"Built at commit: {stored_sha[:12]}")

    if vcs == "git":
        current_branch, current_sha = _git_branch_info(repo_root)
        # Same commit means the parsed tree matches HEAD, so a different branch
        # name is not staleness — that is the normal state in a worktree that
        # inherited the main checkout's graph.
        same_commit = bool(stored_sha) and bool(current_sha) and stored_sha == current_sha
        refresh_hint = (
            "Run 'dagayn worktree sync' to catch up." if label else "Run 'dagayn build' to rebuild."
        )
        if same_commit:
            pass
        elif stored_branch and current_branch and stored_branch != current_branch:
            print(
                f"WARNING: Graph was built on '{stored_branch}' "
                f"but you are now on '{current_branch}'. {refresh_hint}"
            )
        elif stored_sha and current_sha:
            print(
                f"WARNING: Graph was built at commit '{stored_sha[:12]}' "
                f"but HEAD is now '{current_sha[:12]}'. "
                f"Run 'dagayn update' or 'dagayn build' to refresh."
            )
    elif vcs == "svn":
        stored_rev = get_metadata("svn_revision")
        stored_svn_branch = get_metadata("svn_branch")
        if stored_svn_branch:
            print(f"SVN branch: {stored_svn_branch}")
        if stored_rev:
            print(f"SVN revision at build: {stored_rev}")
        current_branch, current_rev = _svn_revision_info(repo_root)
        if stored_svn_branch and current_branch and stored_svn_branch != current_branch:
            print(
                f"WARNING: Graph was built on SVN path '{stored_svn_branch}' "
                f"but the working copy is now '{current_branch}'. "
                f"Run 'dagayn build' to rebuild."
            )
        elif stored_rev and current_rev and stored_rev != current_rev:
            print(
                f"WARNING: Graph was built at SVN revision '{stored_rev}' "
                f"but the working copy is now '{current_rev}'. "
                f"Run 'dagayn update' or 'dagayn build' to refresh."
            )


#: What to do about each graph sync state, for ``dagayn status`` readers.
_SYNC_STATE_HINTS = {
    "unbuilt": "no graph yet — run 'dagayn build'",
    "commit_drift": "graph describes another commit — run 'dagayn update'",
    "commit_synced": "graph matches HEAD",
    "worktree_behind": "uncommitted or reverted edits are not in the graph — run 'dagayn update'",
    "worktree_ahead": "graph already includes the uncommitted edits",
}


def _print_sync_state(repo_root: Path, store: object) -> None:
    """Print the graph's freshness state from the single authority for it.

    ``_print_vcs_status`` only compares commits, so it stays silent when the
    graph holds content the tree no longer has (an indexed edit that was later
    discarded). Report the assessed state so status cannot disagree with what
    ``session prepare`` and the MCP tools act on.
    """
    from ...tools.sync_status import assess_graph_sync

    try:
        sync = assess_graph_sync(store, repo_root)
    except Exception as exc:  # noqa: BLE001 — status must never fail on this
        logging.debug("Could not assess graph sync state: %s", exc)
        return

    state = str(sync.get("state") or "")
    hint = _SYNC_STATE_HINTS.get(state)
    print(f"Graph state: {state}" + (f" — {hint}" if hint else ""))
    pending = sync.get("pending_files") or []
    if pending:
        shown = ", ".join(pending[:5])
        more = f" (+{len(pending) - 5} more)" if len(pending) > 5 else ""
        print(f"  Needs re-indexing: {shown}{more}")


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


def _print_postprocess_summary(result: Mapping[str, Any]) -> None:
    """Print postprocess counts already returned by the build tool."""
    if result.get("signatures_computed"):
        print(f"Signatures: {result['signatures_computed']} nodes")
    if result.get("fts_indexed"):
        print(f"FTS indexed: {result['fts_indexed']} nodes")
    if result.get("flows_detected") is not None:
        print(f"Flows: {result['flows_detected']}")
    if result.get("communities_detected") is not None:
        print(f"Communities: {result['communities_detected']}")


def handle(args: argparse.Namespace) -> None:
    """Dispatch build/update/postprocess/watch/status/visualize/detect-adp/sdp/sap commands."""
    from .build_handlers import execute_build_command

    execute_build_command(args)
