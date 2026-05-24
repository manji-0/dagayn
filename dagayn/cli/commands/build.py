"""build / update / postprocess / watch / status / visualize commands."""

from __future__ import annotations

import argparse
import json
import logging
import sys
from pathlib import Path

from ...local_embeddings import DEFAULT_LOCAL_EMBEDDING_BIN
from ._shared import _add_local_embedding_args


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


def _print_local_embedding_summary(result: dict) -> None:
    emb = result.get("local_embedding")
    if not emb:
        return
    started = "started" if emb.get("server_started") else "reused"
    preset = emb.get("preset")
    text_mode = emb.get("text_mode")
    preset_label = f"{preset}/{text_mode}" if text_mode else preset
    print(
        "Local embeddings "
        f"({preset_label}, {started} server): "
        f"{emb.get('newly_embedded', 0)} new, "
        f"{emb.get('orphans_removed', 0)} orphan removed, "
        f"{emb.get('total_embeddings', 0)} total"
    )


def register_commands(sub: argparse._SubParsersAction) -> dict:
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
    update_cmd.add_argument("--base", default="HEAD~1", help="Git diff base (default: HEAD~1)")
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
        help="Generate graph reports (HTML, GraphML, Mermaid C4, Cypher, Obsidian, SVG)",
    )
    vis_cmd.add_argument("--repo", default=None, help="Repository root (auto-detected)")
    vis_cmd.add_argument(
        "--mode",
        choices=["auto", "full", "community", "file"],
        default="auto",
        help="Rendering mode: auto (default), full, community, or file",
    )
    vis_cmd.add_argument(
        "--serve",
        action="store_true",
        help="Start a local HTTP server to view the visualization (localhost:8765)",
    )
    vis_cmd.add_argument(
        "--format",
        choices=["html", "graphml", "mermaid-c4", "cypher", "obsidian", "svg"],
        default="html",
        help="Export format: html, graphml, mermaid-c4, cypher, obsidian, or svg (default: html)",
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


def _print_postprocess_summary(result: dict) -> None:
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
    logging.basicConfig(level=logging.INFO, format="%(levelname)s: %(message)s")

    from ...graph import GraphStore
    from ...incremental import (
        find_project_root,
        find_repo_root,
        get_db_path,
        watch,
    )

    if args.command == "postprocess":
        repo_root = Path(args.repo) if args.repo else find_project_root()
        db_path = get_db_path(repo_root)
        store = GraphStore(db_path)
        try:
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
        finally:
            store.close()
        return

    if args.command == "update":
        # update requires git for diffing
        repo_root = Path(args.repo) if args.repo else find_repo_root()
        if not repo_root:
            logging.error(
                "Not in a git repository. 'update' requires git for diffing.",
            )
            logging.error("Use 'build' for a full parse, or run 'git init' first.")
            sys.exit(1)
    else:
        repo_root = Path(args.repo) if args.repo else find_project_root()

    db_path = get_db_path(repo_root)
    if args.command == "build" and getattr(args, "force_full_build", False):
        from ...tools._common import _evict_store_cache

        _evict_store_cache(db_path)
        _remove_existing_graph_database(db_path)

    store = GraphStore(db_path)

    try:
        if args.command == "build":
            pp = (
                "none"
                if getattr(args, "skip_postprocess", False)
                else ("minimal" if getattr(args, "skip_flows", False) else "full")
            )
            from ...tools.build import build_or_update_graph

            result = build_or_update_graph(
                full_rebuild=True,
                repo_root=str(repo_root),
                postprocess=pp,
                local_embedding=getattr(args, "local_embedding", "none"),
                local_embedding_port=getattr(args, "local_embedding_port", 18080),
                local_embedding_bin=getattr(
                    args,
                    "local_embedding_bin",
                    DEFAULT_LOCAL_EMBEDDING_BIN,
                ),
                keep_local_embedding_server=getattr(
                    args,
                    "keep_local_embedding_server",
                    False,
                ),
                local_embedding_timeout=getattr(args, "local_embedding_timeout", 300),
                local_embedding_request_timeout=getattr(
                    args,
                    "local_embedding_request_timeout",
                    60,
                ),
                local_embedding_batch_size=getattr(args, "local_embedding_batch_size", 1),
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

        elif args.command == "update":
            pp = (
                "none"
                if getattr(args, "skip_postprocess", False)
                else ("minimal" if getattr(args, "skip_flows", False) else "full")
            )
            from ...tools.build import build_or_update_graph

            result = build_or_update_graph(
                full_rebuild=False,
                repo_root=str(repo_root),
                base=args.base,
                postprocess=pp,
                local_embedding=getattr(args, "local_embedding", "none"),
                local_embedding_port=getattr(args, "local_embedding_port", 18080),
                local_embedding_bin=getattr(
                    args,
                    "local_embedding_bin",
                    DEFAULT_LOCAL_EMBEDDING_BIN,
                ),
                keep_local_embedding_server=getattr(
                    args,
                    "keep_local_embedding_server",
                    False,
                ),
                local_embedding_timeout=getattr(args, "local_embedding_timeout", 300),
                local_embedding_request_timeout=getattr(
                    args,
                    "local_embedding_request_timeout",
                    60,
                ),
                local_embedding_batch_size=getattr(args, "local_embedding_batch_size", 1),
            )
            updated = result.get("files_updated", 0)
            nodes = result.get("total_nodes", 0)
            edges = result.get("total_edges", 0)
            print(
                f"Incremental: {updated} files updated, "
                f"{nodes} nodes, {edges} edges"
                f" (postprocess={pp})"
            )
            _print_local_embedding_summary(result)
            if pp != "none" and result.get("files_updated", 0) > 0:
                _print_postprocess_summary(result)

        elif args.command == "status":
            stats = store.get_stats()
            print(f"Nodes: {stats.total_nodes}")
            print(f"Edges: {stats.total_edges}")
            print(f"Files: {stats.files_count}")
            print(f"Languages: {', '.join(stats.languages)}")
            print(f"Last updated: {stats.last_updated or 'never'}")
            # Show branch info and warn if stale
            stored_branch = store.get_metadata("git_branch")
            stored_sha = store.get_metadata("git_head_sha")
            if stored_branch:
                print(f"Built on branch: {stored_branch}")
            if stored_sha:
                print(f"Built at commit: {stored_sha[:12]}")
            from ...incremental import _git_branch_info, detect_vcs

            vcs = detect_vcs(repo_root)
            if vcs == "git":
                current_branch, current_sha = _git_branch_info(repo_root)
                if stored_branch and current_branch and stored_branch != current_branch:
                    print(
                        f"WARNING: Graph was built on '{stored_branch}' "
                        f"but you are now on '{current_branch}'. "
                        f"Run 'dagayn build' to rebuild."
                    )
            elif vcs == "svn":
                stored_rev = store.get_metadata("svn_revision")
                stored_svn_branch = store.get_metadata("svn_branch")
                if stored_svn_branch:
                    print(f"SVN branch: {stored_svn_branch}")
                if stored_rev:
                    print(f"SVN revision at build: {stored_rev}")

        elif args.command == "watch":
            from ...postprocessing import run_post_processing

            watch(repo_root, store, on_files_updated=run_post_processing)

        elif args.command == "visualize":
            from ...incremental import get_data_dir

            data_dir = get_data_dir(repo_root)
            fmt = getattr(args, "format", "html") or "html"

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
            else:
                from ...visualization import generate_html

                html_path = data_dir / "graph.html"
                vis_mode = getattr(args, "mode", "auto") or "auto"
                generate_html(store, html_path, mode=vis_mode)
                print(f"Visualization ({vis_mode}): {html_path}")
                if getattr(args, "serve", False):
                    import functools
                    import http.server

                    serve_dir = html_path.parent
                    port = 8765
                    handler = functools.partial(
                        http.server.SimpleHTTPRequestHandler,
                        directory=str(serve_dir),
                    )
                    print(f"Serving at http://localhost:{port}/graph.html")
                    print("Press Ctrl+C to stop.")
                    with http.server.HTTPServer(("localhost", port), handler) as httpd:
                        try:
                            httpd.serve_forever()
                        except KeyboardInterrupt:
                            print("\nServer stopped.")
                else:
                    print("Open in browser to explore.")

        elif args.command == "detect-adp":
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
                        f"ADP violations ({len(violations)} cycles, "
                        f"artifact_scope={args.artifact_scope}):"
                    )
                    for v in violations:
                        nodes = " -> ".join(v["nodes"]) + f" -> {v['nodes'][0]}"
                        print(f"  [{v['length']}-cycle, severity={v['severity']}] {nodes}")
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

        elif args.command == "sdp-metrics":
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
                    for m in top:
                        print(
                            f"  {m['name']:<50} I={m['instability']:.4f}  Ca={m['ca']} Ce={m['ce']}"
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

        elif args.command == "detect-sdp":
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
                    print(
                        f"SDP violations ({len(violations)}, artifact_scope={args.artifact_scope}):"
                    )
                    for v in violations:
                        print(
                            f"  {v['source']:<40} -> {v['target']:<40}"
                            f"  delta={v['delta']:.4f}"
                            f"  (I_src={v['source_instability']:.4f}"
                            f", I_tgt={v['target_instability']:.4f})"
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

        elif args.command == "sap-metrics":
            from ...sap import compute_sap_metrics

            unit_filter = (
                [p.strip() for p in args.unit_filter.split(",")] if args.unit_filter else None
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
                    for m in top:
                        print(
                            f"  {m['scope_key']:<50}"
                            f"  A={m['abstractness']:.4f}"
                            f"  I={m['instability']:.4f}"
                            f"  D={m['distance']:.4f}"
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

        elif args.command == "detect-sap":
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
                    print(
                        f"SAP violations ({len(violations)}, artifact_scope={args.artifact_scope}):"
                    )
                    for v in violations:
                        print(
                            f"  {v['scope_key']:<50}"
                            f"  D={v['distance']:.4f}"
                            f"  (A={v['abstractness']:.4f}"
                            f", I={v['instability']:.4f})"
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

    finally:
        store.close()
