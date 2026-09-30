"""Command handlers for ``dagayn.cli.commands.build.handle``."""

from __future__ import annotations

import argparse
import json
import logging
import sys
from collections.abc import Mapping
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

#: What to do about each graph sync state, for ``dagayn status`` readers.
_SYNC_STATE_HINTS = {
    "unbuilt": "no graph yet — run 'dagayn build'",
    "commit_drift": "graph describes another commit — run 'dagayn update'",
    "commit_synced": "graph matches HEAD",
    "worktree_behind": "uncommitted or reverted edits are not in the graph — run 'dagayn update'",
    "worktree_ahead": "graph already includes the uncommitted edits",
}


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
    from ...incremental import GIT_BACKED_VCS, _git_branch_info, _svn_revision_info, detect_vcs

    get_metadata = getattr(store, "get_metadata")
    stored_branch = get_metadata("git_branch")
    stored_sha = get_metadata("git_head_sha")

    vcs = detect_vcs(repo_root)
    label: str | None = None
    if vcs in GIT_BACKED_VCS:
        from ...worktree import main_worktree_root, worktree_label

        label = worktree_label(repo_root)
        if label:
            print(f"Linked worktree: {label} (main checkout: {main_worktree_root(repo_root)})")

    if stored_branch:
        print(f"Built on branch: {stored_branch}")
    if stored_sha:
        print(f"Built at commit: {stored_sha[:12]}")

    if vcs in GIT_BACKED_VCS:
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
    extractor_drift = sync.get("extractor_drift") or []
    if extractor_drift:
        print(
            "  Parsed by an older extractor: "
            + ", ".join(str(name) for name in extractor_drift)
            + " — 'dagayn update' re-parses those files"
        )
    pending = sync.get("pending_files") or []
    if pending:
        shown = ", ".join(pending[:5])
        more = f" (+{len(pending) - 5} more)" if len(pending) > 5 else ""
        print(f"  Needs re-indexing: {shown}{more}")


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
        return _reject_wide_root(repo_root, explicit=bool(args.repo))
    if args.repo:
        return Path(args.repo)
    return _reject_wide_root(find_project_root(), explicit=False)


def _reject_wide_root(repo_root: Path, *, explicit: bool) -> Path:
    """Exit rather than index the home directory that was merely the cwd."""
    if explicit:
        return repo_root
    from ...paths import ALLOW_WIDE_ROOT_ENV, unsafe_root_reason

    reason = unsafe_root_reason(repo_root)
    if reason is None:
        return repo_root
    logging.error("Refusing to use %s (%s) as the repository root.", reason, repo_root)
    logging.error("Run dagayn from inside the project, pass --repo, or set CRG_REPO_ROOT.")
    logging.error("Set %s=1 to index it anyway.", ALLOW_WIDE_ROOT_ENV)
    sys.exit(1)


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
        from ...hook_guard import running_from_hook
        from ...write_lock import WriteLockUnavailableError, graph_read_lock

        # The base peek needs a live connection, so it must hold the shared
        # lock. A hook run must not spend that lock's 120 s budget waiting for
        # a writer: it is going to skip anyway (its write lock is non-blocking),
        # so a blocking peek would turn a would-be skip into a silent hang and
        # make the editor wait for nothing. Manual runs keep the blocking
        # behaviour and benefit from the wait.
        try:
            with graph_read_lock(db_path, blocking=not running_from_hook()):
                peek = GraphStore(db_path)
                try:
                    base = peek.get_metadata("git_head_sha") or "HEAD~1"
                finally:
                    peek.close()
        except WriteLockUnavailableError:
            if running_from_hook():
                print(
                    "Skipped: another process is writing the graph "
                    "(hook update must not queue behind it)"
                )
                return
            raise

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
