"""Command handlers for ``dagayn.cli.commands.build.handle``."""

from __future__ import annotations

import argparse
import json
import logging
import sys
from contextlib import AbstractContextManager
from pathlib import Path
from typing import Any

from ...incremental import (
    find_project_root,
    find_repo_root,
    get_db_path,
    watch,
)
from ...local_embeddings import DEFAULT_LOCAL_EMBEDDING_BIN
from .build import (
    _print_embedding_status,
    _print_local_embedding_summary,
    _print_postprocess_summary,
    _print_sync_state,
    _print_vcs_status,
    _remove_existing_graph_database,
)


def _postprocess_level(args: argparse.Namespace) -> str:
    if getattr(args, "skip_postprocess", False):
        return "none"
    if getattr(args, "skip_flows", False):
        return "minimal"
    return "full"


def _local_embedding_kwargs(args: argparse.Namespace) -> dict[str, Any]:
    return {
        "local_embedding": getattr(args, "local_embedding", "none"),
        "local_embedding_mode": getattr(args, "local_embedding_mode", None),
        "local_embedding_port": getattr(args, "local_embedding_port", None),
        "local_embedding_bin": getattr(
            args,
            "local_embedding_bin",
            DEFAULT_LOCAL_EMBEDDING_BIN,
        ),
        "keep_local_embedding_server": getattr(args, "keep_local_embedding_server", False),
        "local_embedding_timeout": getattr(args, "local_embedding_timeout", 300),
        "local_embedding_request_timeout": getattr(
            args,
            "local_embedding_request_timeout",
            60,
        ),
        "local_embedding_batch_size": getattr(args, "local_embedding_batch_size", 1),
    }


def handle_postprocess_command(args: argparse.Namespace) -> None:
    repo_root = Path(args.repo) if args.repo else find_project_root()
    from ...tools.build import run_postprocess

    result = run_postprocess(
        flows=not getattr(args, "no_flows", False),
        communities=not getattr(args, "no_communities", False),
        fts=not getattr(args, "no_fts", False),
        repo_root=str(repo_root),
    )
    parts = []
    if result.get("flows_detected"):
        parts.append(f"{result['flows_detected']} flows")
    if result.get("communities_detected"):
        parts.append(f"{result['communities_detected']} communities")
    if result.get("fts_indexed"):
        parts.append(f"{result['fts_indexed']} FTS entries")
    print(f"Post-processing: {', '.join(parts) or 'done'}")


def resolve_repo_root(args: argparse.Namespace) -> Path:
    if args.command == "update":
        repo_root = Path(args.repo) if args.repo else find_repo_root()
        if not repo_root:
            logging.error(
                "Not in a git repository. 'update' requires git for diffing.",
            )
            logging.error("Use 'build' for a full parse, or run 'git init' first.")
            sys.exit(1)
        return repo_root
    return Path(args.repo) if args.repo else find_project_root()


def ensure_worktree_graph_if_needed(args: argparse.Namespace, repo_root: Path) -> None:
    if args.command not in ("update", "status"):
        return
    from ...worktree import ensure_worktree_graph

    seed = ensure_worktree_graph(repo_root)
    if seed.seeded:
        print(f"Inherited graph from {seed.source} (this worktree had none)")
        if seed.base_sha and getattr(args, "base", None) is None:
            args.base = seed.base_sha


def prepare_force_full_build(args: argparse.Namespace, repo_root: Path) -> Path:
    """Delete the graph for a forced rebuild, but only under the write lock.

    Deleting first and locking later loses the graph outright whenever the lock
    turns out to be held: the build that was supposed to replace it fails to
    acquire, and the old graph is already gone. Taking the lock for the delete
    makes an unavailable lock a no-op instead of data loss.

    The lock is released again before the build, which takes it itself: holding
    it across the whole command would keep it held through the embedding pass
    too (the acquisition is reentrant, so the pass's own release would not free
    it), and locking every reader out for that is what the sliced embedding pass
    exists to avoid.
    """
    db_path = get_db_path(repo_root)
    if not (args.command == "build" and getattr(args, "force_full_build", False)):
        return db_path

    from ...tools._common import _evict_store_cache
    from ...write_lock import WriteLockUnavailableError, graph_write_lock, lock_holder_pid

    try:
        with graph_write_lock(db_path):
            _evict_store_cache(db_path)
            _remove_existing_graph_database(db_path)
    except WriteLockUnavailableError as exc:
        holder = lock_holder_pid(db_path)
        logging.error(
            "Not deleting %s: %s%s",
            db_path,
            exc,
            f" (held by pid {holder})" if holder else "",
        )
        sys.exit(1)
    return db_path


def handle_build_command(args: argparse.Namespace, repo_root: Path) -> None:
    pp = _postprocess_level(args)
    from ...tools.build import build_or_update_graph

    result = build_or_update_graph(
        full_rebuild=True,
        repo_root=str(repo_root),
        postprocess=pp,
        **_local_embedding_kwargs(args),
    )
    parsed = result.get("files_parsed", 0)
    nodes = result.get("total_nodes", 0)
    edges = result.get("total_edges", 0)
    print(f"Full build: {parsed} files, {nodes} nodes, {edges} edges (postprocess={pp})")
    if result.get("errors"):
        print(f"Errors: {len(result['errors'])}")
    _print_local_embedding_summary(result)
    if pp != "none":
        _print_postprocess_summary(result)
    if result.get("status") == "error":
        # A build that could not run is not a success. Reporting "0 files" and
        # exiting 0 made a failed forced rebuild look like an empty repository,
        # which is indistinguishable from the case where the graph was deleted
        # and nothing replaced it.
        logging.error("%s", result.get("summary") or "build failed")
        sys.exit(1)


def _hook_update_budget(args: argparse.Namespace) -> float | None:
    """Wall-clock budget for this ``update``, or ``None`` for unbounded.

    An explicit ``--budget-seconds`` always wins (``0`` disables the guard);
    otherwise only hook-triggered runs are bounded, since those are the ones
    nobody is watching.
    """
    from ...hook_guard import DEFAULT_HOOK_BUDGET_SECONDS, running_from_hook

    explicit = getattr(args, "budget_seconds", None)
    if explicit is not None:
        return None if explicit <= 0 else float(explicit)
    return float(DEFAULT_HOOK_BUDGET_SECONDS) if running_from_hook() else None


def handle_update_command(
    args: argparse.Namespace,
    repo_root: Path,
    db_path: Path,
) -> None:
    from ...hook_guard import (
        HOOK_SKIP_MARKER,
        hook_updates_disabled,
        running_from_hook,
        start_budget_watchdog,
    )

    if running_from_hook() and hook_updates_disabled(repo_root):
        print(f"Skipped: .dagayn/{HOOK_SKIP_MARKER} disables hook-triggered updates here")
        return

    watchdog = start_budget_watchdog(_hook_update_budget(args))
    try:
        _run_update_command(args, repo_root, db_path)
    finally:
        if watchdog is not None:
            watchdog.cancel()


def _run_update_command(
    args: argparse.Namespace,
    repo_root: Path,
    db_path: Path,
) -> None:
    pp = _postprocess_level(args)
    from ...tools.build import build_or_update_graph

    base = args.base
    if base is None:
        from ...graph import GraphStore
        from ...write_lock import graph_read_lock

        with graph_read_lock(db_path):
            peek = GraphStore(db_path)
            try:
                base = peek.get_metadata("git_head_sha") or "HEAD~1"
            finally:
                peek.close()

    result = build_or_update_graph(
        full_rebuild=False,
        repo_root=str(repo_root),
        base=base,
        postprocess=pp,
        **_local_embedding_kwargs(args),
    )
    updated = result.get("files_updated", 0)
    nodes = result.get("total_nodes", 0)
    edges = result.get("total_edges", 0)
    print(f"Incremental: {updated} files updated, {nodes} nodes, {edges} edges (postprocess={pp})")
    _print_local_embedding_summary(result)
    if pp != "none" and result.get("files_updated", 0) > 0:
        _print_postprocess_summary(result)


def _run_with_graph_store(
    args: argparse.Namespace,
    repo_root: Path,
    db_path: Path,
    *,
    handler: Any,
) -> None:
    from ...graph import GraphStore
    from ...write_lock import graph_read_lock

    read_lock: AbstractContextManager[None] | None = (
        graph_read_lock(db_path) if args.command != "watch" else None
    )
    if read_lock is not None:
        read_lock.__enter__()
    store = GraphStore(db_path)
    try:
        handler(args, repo_root, store, db_path)
    finally:
        store.close()
        if read_lock is not None:
            read_lock.__exit__(None, None, None)


def handle_status_command(
    _args: argparse.Namespace,
    repo_root: Path,
    store: Any,
    db_path: Path,
) -> None:
    stats = store.get_stats()
    print(f"Nodes: {stats.total_nodes}")
    print(f"Edges: {stats.total_edges}")
    print(f"Files: {stats.files_count}")
    print(f"Languages: {', '.join(stats.languages)}")
    print(f"Last updated: {stats.last_updated or 'never'}")
    _print_embedding_status(db_path)
    _print_vcs_status(repo_root, store)
    _print_sync_state(repo_root, store)


def handle_watch_command(
    _args: argparse.Namespace,
    repo_root: Path,
    store: Any,
    _db_path: Path,
) -> None:
    from ...postprocessing import run_post_processing

    watch(repo_root, store, on_files_updated=run_post_processing)


def handle_visualize_command(
    args: argparse.Namespace,
    repo_root: Path,
    store: Any,
    _db_path: Path,
) -> None:
    from ...incremental import get_data_dir

    data_dir = get_data_dir(repo_root)
    fmt = args.format

    if fmt == "graphml":
        from ...exports import export_graphml

        out = data_dir / "graph.graphml"
        export_graphml(store, out)
        print(f"GraphML exported: {out}")
    elif fmt == "mermaid-c4":
        from ...exports import export_mermaid_c4

        out = data_dir / "graph.mmd"
        export_mermaid_c4(store, out)
        print(f"Mermaid C4 exported: {out}")
    elif fmt == "cypher":
        from ...exports import export_neo4j_cypher

        out = data_dir / "graph.cypher"
        export_neo4j_cypher(store, out)
        print(f"Neo4j Cypher exported: {out}")
    elif fmt == "obsidian":
        from ...exports import export_obsidian_vault

        out = data_dir / "obsidian"
        export_obsidian_vault(store, out)
        print(f"Obsidian vault exported: {out}")
    elif fmt == "svg":
        from ...exports import export_svg

        out = data_dir / "graph.svg"
        export_svg(store, out)
        print(f"SVG exported: {out}")


def handle_detect_adp_command(
    args: argparse.Namespace,
    _repo_root: Path,
    store: Any,
    _db_path: Path,
) -> None:
    from ...architecture import find_adp_violations

    violations = find_adp_violations(
        store,
        granularity=args.granularity,
        artifact_scope=args.artifact_scope,
        min_cycle_size=args.min_cycle_size,
        max_cycle_length=args.max_cycle_length,
    )
    if args.format == "text":
        if not violations:
            print("No ADP violations found.")
        else:
            print(
                f"ADP violations ({len(violations)} cycles, artifact_scope={args.artifact_scope}):"
            )
            for violation in violations:
                nodes = " -> ".join(violation["nodes"]) + f" -> {violation['nodes'][0]}"
                print(f"  [{violation['length']}-cycle, severity={violation['severity']}] {nodes}")
    else:
        print(
            json.dumps(
                {
                    "violations": violations,
                    "count": len(violations),
                    "artifact_scope": args.artifact_scope,
                },
                indent=2,
            )
        )


def handle_sdp_metrics_command(
    args: argparse.Namespace,
    _repo_root: Path,
    store: Any,
    _db_path: Path,
) -> None:
    from ...architecture import compute_sdp_metrics

    metrics = compute_sdp_metrics(
        store,
        granularity=args.granularity,
        artifact_scope=args.artifact_scope,
    )
    top = metrics[: args.top_n]
    if args.format == "text":
        if not top:
            print("No dependency data found.")
        else:
            print(
                f"SDP instability ({args.granularity}-level, "
                f"artifact_scope={args.artifact_scope}, top {len(top)}):"
            )
            for metric in top:
                print(
                    f"  {metric['name']:<50} I={metric['instability']:.4f}  "
                    f"Ca={metric['ca']} Ce={metric['ce']}"
                )
    else:
        print(
            json.dumps(
                {
                    "metrics": top,
                    "total": len(metrics),
                    "artifact_scope": args.artifact_scope,
                },
                indent=2,
            )
        )


def handle_detect_sdp_command(
    args: argparse.Namespace,
    _repo_root: Path,
    store: Any,
    _db_path: Path,
) -> None:
    from ...architecture import find_sdp_violations

    violations = find_sdp_violations(
        store,
        granularity=args.granularity,
        artifact_scope=args.artifact_scope,
        min_delta=args.min_delta,
    )
    if args.format == "text":
        if not violations:
            print("No SDP violations found.")
        else:
            print(f"SDP violations ({len(violations)}, artifact_scope={args.artifact_scope}):")
            for violation in violations:
                print(
                    f"  {violation['source']:<40} -> {violation['target']:<40}"
                    f"  delta={violation['delta']:.4f}"
                    f"  (I_src={violation['source_instability']:.4f}"
                    f", I_tgt={violation['target_instability']:.4f})"
                )
    else:
        print(
            json.dumps(
                {
                    "violations": violations,
                    "count": len(violations),
                    "artifact_scope": args.artifact_scope,
                },
                indent=2,
            )
        )


def handle_sap_metrics_command(
    args: argparse.Namespace,
    _repo_root: Path,
    store: Any,
    _db_path: Path,
) -> None:
    from ...sap import compute_sap_metrics

    unit_filter = (
        [part.strip() for part in args.unit_filter.split(",")] if args.unit_filter else None
    )
    metrics = compute_sap_metrics(
        store,
        scope_kind=args.scope_kind,
        unit_filter=unit_filter,
        artifact_scope=args.artifact_scope,
    )
    top = metrics[: args.top_n]
    if args.format == "text":
        if not top:
            print("No scope data found.")
        else:
            print(
                f"SAP metrics ({args.scope_kind}-level, "
                f"artifact_scope={args.artifact_scope}, top {len(top)}):"
            )
            for metric in top:
                print(
                    f"  {metric['scope_key']:<50}"
                    f"  A={metric['abstractness']:.4f}"
                    f"  I={metric['instability']:.4f}"
                    f"  D={metric['distance']:.4f}"
                )
    else:
        print(
            json.dumps(
                {
                    "metrics": top,
                    "total": len(metrics),
                    "artifact_scope": args.artifact_scope,
                },
                indent=2,
            )
        )


def handle_detect_sap_command(
    args: argparse.Namespace,
    _repo_root: Path,
    store: Any,
    _db_path: Path,
) -> None:
    from ...sap import find_sap_violations

    violations = find_sap_violations(
        store,
        scope_kind=args.scope_kind,
        artifact_scope=args.artifact_scope,
        min_distance=args.min_distance,
    )
    if args.format == "text":
        if not violations:
            print("No SAP violations found.")
        else:
            print(f"SAP violations ({len(violations)}, artifact_scope={args.artifact_scope}):")
            for violation in violations:
                print(
                    f"  {violation['scope_key']:<50}"
                    f"  D={violation['distance']:.4f}"
                    f"  (A={violation['abstractness']:.4f}"
                    f", I={violation['instability']:.4f})"
                )
    else:
        print(
            json.dumps(
                {
                    "violations": violations,
                    "count": len(violations),
                    "artifact_scope": args.artifact_scope,
                },
                indent=2,
            )
        )


_STORE_COMMAND_HANDLERS = {
    "status": handle_status_command,
    "watch": handle_watch_command,
    "visualize": handle_visualize_command,
    "detect-adp": handle_detect_adp_command,
    "sdp-metrics": handle_sdp_metrics_command,
    "detect-sdp": handle_detect_sdp_command,
    "sap-metrics": handle_sap_metrics_command,
    "detect-sap": handle_detect_sap_command,
}


def execute_build_command(args: argparse.Namespace) -> None:
    """Dispatch build/update/postprocess/watch/status/visualize/detect-adp/sdp/sap commands."""
    logging.basicConfig(level=logging.INFO, format="%(levelname)s: %(message)s")

    if args.command == "postprocess":
        handle_postprocess_command(args)
        return

    repo_root = resolve_repo_root(args)
    ensure_worktree_graph_if_needed(args, repo_root)
    db_path = prepare_force_full_build(args, repo_root)

    if args.command == "build":
        handle_build_command(args, repo_root)
        return
    if args.command == "update":
        handle_update_command(args, repo_root, db_path)
        return

    handler = _STORE_COMMAND_HANDLERS.get(args.command)
    if handler is None:
        logging.error("Unknown command: %s", args.command)
        sys.exit(2)
    _run_with_graph_store(args, repo_root, db_path, handler=handler)
