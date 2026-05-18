"""Tool 1: build_or_update_graph + run_postprocess."""

from __future__ import annotations

import logging
import os
import sqlite3
import sys
import threading
import time
from typing import Any

from ..incremental import full_build, incremental_update
from ._common import _evict_store_cache, _get_store

logger = logging.getLogger(__name__)

_LOCAL_EMBEDDING_DISABLED = {None, "", "none"}
_LOCAL_EMBEDDING_ENV_LOCK = threading.Lock()


def _can_run_minimal_postprocess(store: Any) -> bool:
    return all(
        hasattr(store, name)
        for name in (
            "compute_missing_signatures",
            "rebuild_fts_index",
            "resolve_markdown_artifact_refs",
        )
    )


def _can_trace_full_flows(store: Any) -> bool:
    return all(
        hasattr(store, name)
        for name in (
            "get_all_call_targets",
            "get_nodes_by_kind",
            "load_flow_adjacency",
            "store_flows_json",
        )
    )


def _can_trace_incremental_flows(store: Any) -> bool:
    return all(
        hasattr(store, name)
        for name in (
            "delete_affected_flows",
            "get_all_call_targets",
            "get_nodes_by_kind",
            "insert_flows_json",
            "load_flow_adjacency",
        )
    )


def _can_detect_full_communities(store: Any) -> bool:
    return all(
        hasattr(store, name)
        for name in (
            "get_all_nodes",
            "get_all_edges",
            "store_communities_json",
        )
    )


def _can_detect_incremental_communities(store: Any) -> bool:
    return _can_detect_full_communities(store) and hasattr(store, "count_affected_communities")


def _postprocess_store(store: Any, root: Any, postprocess: str):
    """Return the store used for post-processing."""
    if hasattr(store, "_conn"):
        return store, False
    if postprocess == "minimal" and _can_run_minimal_postprocess(store):
        return store, False
    raise RuntimeError(
        "Rust post-processing requires dagayn._core support for the requested "
        "postprocess level. Install a wheel with the native extension or rebuild "
        "from source."
    )


def _local_embedding_requested(local_embedding: str | None) -> bool:
    return (local_embedding or "").strip().lower() not in _LOCAL_EMBEDDING_DISABLED


def _run_local_embedding(
    root: Any,
    *,
    local_embedding: str,
    local_embedding_port: int,
    local_embedding_bin: str,
    keep_local_embedding_server: bool,
    local_embedding_timeout: int,
    local_embedding_request_timeout: int,
    local_embedding_batch_size: int,
) -> dict[str, Any]:
    """Run graph embedding through a managed local llama-server process."""
    from dagayn.local_embeddings import local_embedding_server
    from dagayn.tools.docs import embed_graph

    with local_embedding_server(
        local_embedding,
        port=local_embedding_port,
        binary=local_embedding_bin,
        keep_running=keep_local_embedding_server,
        startup_timeout=local_embedding_timeout,
    ) as server:
        env_keys = (
            "CRG_OPENAI_API_KEY",
            "CRG_OPENAI_BASE_URL",
            "CRG_OPENAI_BATCH_SIZE",
            "CRG_OPENAI_DIMENSION",
            "CRG_OPENAI_TIMEOUT",
            "DAGAYN_EMBEDDING_TEXT_MODE",
        )
        with _LOCAL_EMBEDDING_ENV_LOCK:
            old_env = {key: os.environ.get(key) for key in env_keys}
            try:
                os.environ["CRG_OPENAI_API_KEY"] = "dagayn-local"
                os.environ["CRG_OPENAI_BASE_URL"] = server.base_url
                os.environ["CRG_OPENAI_BATCH_SIZE"] = str(local_embedding_batch_size)
                os.environ["CRG_OPENAI_TIMEOUT"] = str(local_embedding_request_timeout)
                os.environ["DAGAYN_EMBEDDING_TEXT_MODE"] = server.preset.text_mode
                os.environ.pop("CRG_OPENAI_DIMENSION", None)
                result = embed_graph(
                    repo_root=str(root),
                    provider="openai",
                    model=server.preset.model,
                    show_progress=sys.stderr.isatty(),
                )
            finally:
                for key, value in old_env.items():
                    if value is None:
                        os.environ.pop(key, None)
                    else:
                        os.environ[key] = value

    if result.get("status") != "ok":
        raise RuntimeError(result.get("error") or "Local embedding generation failed.")

    return {
        "status": "ok",
        "preset": server.preset.level,
        "model": server.preset.model,
        "dimension": server.preset.dimension,
        "text_mode": server.preset.text_mode,
        "server_started": server.started,
        "server_url": server.base_url,
        "server_command": server.command,
        "newly_embedded": result.get("newly_embedded", 0),
        "orphans_removed": result.get("orphans_removed", 0),
        "total_embeddings": result.get("total_embeddings", 0),
        "summary": result.get("summary", ""),
    }


def _run_postprocess(
    store: Any,
    build_result: dict[str, Any],
    postprocess: str,
    full_rebuild: bool = False,
    changed_files: list[str] | None = None,
    skip_minimal_steps: bool = False,
    skip_flow_steps: bool = False,
    skip_community_steps: bool = False,
    skip_summary_steps: bool = False,
) -> list[str]:
    """Run post-build steps based on *postprocess* level.

    When *full_rebuild* is False and *changed_files* are available,
    uses incremental flow/community detection for faster updates.

    Returns a list of warning strings (empty on success).
    """
    warnings: list[str] = []
    build_result["postprocess_level"] = postprocess

    if postprocess == "none":
        return warnings

    if not skip_minimal_steps:
        # -- Signatures + FTS (fast, always run unless "none") --
        try:
            rust_compute = getattr(store, "compute_missing_signatures", None)
            if callable(rust_compute):
                rust_compute()
            else:
                rows = store.get_nodes_without_signature()
                for row in rows:
                    node_id, name, kind, params, ret = (
                        row[0],
                        row[1],
                        row[2],
                        row[3],
                        row[4],
                    )
                    if kind in ("Function", "Test"):
                        sig = f"def {name}({params or ''})"
                        if ret:
                            sig += f" -> {ret}"
                    elif kind == "Class":
                        sig = f"class {name}"
                    elif kind == "DocSection":
                        sig = f"# {name}"
                    else:
                        sig = name
                    store.update_node_signature(node_id, sig[:512])
                store.commit()
            build_result["signatures_updated"] = True
        except (sqlite3.OperationalError, RuntimeError, TypeError, KeyError) as e:
            logger.warning("Signature computation failed: %s", e)
            warnings.append(f"Signature computation failed: {type(e).__name__}: {e}")

        try:
            from dagayn.search import rebuild_fts_index

            fts_count = rebuild_fts_index(store)
            build_result["fts_indexed"] = fts_count
            build_result["fts_rebuilt"] = True
        except (sqlite3.OperationalError, ImportError) as e:
            logger.warning("FTS index rebuild failed: %s", e)
            warnings.append(f"FTS index rebuild failed: {type(e).__name__}: {e}")

        try:
            from dagayn.postprocessing import _resolve_markdown_artifact_refs

            _result: dict[str, Any] = {}
            _resolve_markdown_artifact_refs(store, _result, warnings)
            build_result["markdown_artifact_refs_resolved"] = _result.get(
                "markdown_artifact_refs_resolved", 0
            )
            build_result["markdown_artifact_refs_dropped"] = _result.get(
                "markdown_artifact_refs_dropped", 0
            )
        except (sqlite3.OperationalError, ImportError) as e:
            logger.warning("Markdown artifact ref resolution failed: %s", e)
            warnings.append(f"Markdown artifact ref resolution failed: {type(e).__name__}: {e}")

    if postprocess == "minimal":
        return warnings

    # -- Expensive: flows + communities (only for "full") --
    use_incremental = not full_rebuild and bool(changed_files)

    if not skip_flow_steps:
        try:
            if use_incremental:
                from dagayn.flows import incremental_trace_flows

                count = incremental_trace_flows(store, changed_files or [])
            else:
                from dagayn.flows import store_flows as _store_flows
                from dagayn.flows import trace_flows as _trace_flows

                flows = _trace_flows(store)
                count = _store_flows(store, flows)
            build_result["flows_detected"] = count
        except (sqlite3.OperationalError, RuntimeError, ImportError) as e:
            logger.warning("Flow detection failed: %s", e)
            warnings.append(f"Flow detection failed: {type(e).__name__}: {e}")

    if not skip_community_steps:
        try:
            if use_incremental:
                from dagayn.communities import (
                    incremental_detect_communities,
                )

                count = incremental_detect_communities(store, changed_files or [])
            else:
                from dagayn.communities import (
                    detect_communities as _detect_communities,
                )
                from dagayn.communities import (
                    store_communities as _store_communities,
                )

                comms = _detect_communities(store)
                count = _store_communities(store, comms)
            build_result["communities_detected"] = count
        except (sqlite3.OperationalError, RuntimeError, ImportError) as e:
            logger.warning("Community detection failed: %s", e)
            warnings.append(f"Community detection failed: {type(e).__name__}: {e}")

    if not skip_summary_steps:
        # -- Compute pre-computed summary tables --
        try:
            _compute_summaries(store)
            build_result["summaries_computed"] = True
        except (sqlite3.OperationalError, RuntimeError, Exception) as e:
            logger.warning("Summary computation failed: %s", e)
            warnings.append(f"Summary computation failed: {type(e).__name__}: {e}")

    store.set_metadata(
        "last_postprocessed_at",
        time.strftime("%Y-%m-%dT%H:%M:%S"),
    )
    store.set_metadata("postprocess_level", postprocess)

    return warnings


def _compute_summaries(store: Any) -> None:
    """Populate community_summaries, flow_snapshots, and risk_index tables.

    Uses batched aggregate queries and in-memory grouping instead of
    per-community/per-node loops. On graphs with ~100k edges this
    reduces the work from ``O(nodes + communities)`` SQLite round trips
    each doing their own B-tree scan to a handful of ``GROUP BY``
    queries, turning what used to be an effective hang into a few
    seconds.

    Each summary block (community_summaries, flow_snapshots, risk_index)
    is wrapped in an explicit transaction so the DELETE + INSERT sequence
    is atomic.  If a table doesn't exist yet the block is silently skipped.
    """
    rust_compute = getattr(store, "compute_summaries", None)
    if callable(rust_compute):
        rust_compute()
        return

    import json as _json
    from collections import defaultdict
    from os.path import commonprefix

    conn = store._conn

    # -- community_summaries --
    try:
        conn.execute("BEGIN IMMEDIATE")
        conn.execute("DELETE FROM community_summaries")

        # Pre-compute per-qualified_name edge counts once. Previously
        # this section ran a per-community triple-JOIN aggregate query
        # (nodes LEFT JOIN edges LEFT JOIN edges), which on graphs with
        # thousands of communities was the second-biggest hang.
        edge_counts: dict[str, int] = defaultdict(int)
        for row in conn.execute(
            "SELECT source_qualified, COUNT(*) FROM edges GROUP BY source_qualified"
        ):
            edge_counts[row[0]] += row[1]
        for row in conn.execute(
            "SELECT target_qualified, COUNT(*) FROM edges GROUP BY target_qualified"
        ):
            edge_counts[row[0]] += row[1]

        # Group non-File nodes per community for top-symbol selection.
        nodes_by_comm: dict[int, list[tuple[str, int]]] = defaultdict(list)
        for row in conn.execute(
            "SELECT community_id, name, qualified_name FROM nodes "
            "WHERE community_id IS NOT NULL AND kind != 'File'"
        ):
            cid, name, qn = row[0], row[1], row[2]
            nodes_by_comm[cid].append((name, edge_counts.get(qn, 0)))

        # Group distinct file paths per community (preserving first-seen
        # order for stable output, same as DISTINCT in the old query).
        files_by_comm: dict[int, list[str]] = defaultdict(list)
        seen_files: dict[int, set[str]] = defaultdict(set)
        for row in conn.execute(
            "SELECT community_id, file_path FROM nodes WHERE community_id IS NOT NULL"
        ):
            cid, fp = row[0], row[1]
            if fp not in seen_files[cid]:
                seen_files[cid].add(fp)
                files_by_comm[cid].append(fp)

        community_rows = conn.execute(
            "SELECT id, name, size, dominant_language FROM communities"
        ).fetchall()
        for r in community_rows:
            cid, cname, csize, clang = r[0], r[1], r[2], r[3]

            # Top 5 symbols by total edge count (in + out). Python's
            # sorted() is stable so ties break by original row order.
            members = sorted(
                nodes_by_comm.get(cid, []),
                key=lambda nc: nc[1],
                reverse=True,
            )
            key_syms = _json.dumps([m[0] for m in members[:5]])

            # Auto-generate purpose from common file path prefix.
            paths = files_by_comm.get(cid, [])[:20]
            purpose = ""
            if paths:
                prefix = commonprefix(paths)
                if "/" in prefix:
                    purpose = prefix.rsplit("/", 1)[0].split("/")[-1] if "/" in prefix else ""

            conn.execute(
                "INSERT OR REPLACE INTO community_summaries "
                "(community_id, name, purpose, key_symbols, size, dominant_language) "
                "VALUES (?, ?, ?, ?, ?, ?)",
                (cid, cname, purpose, key_syms, csize, clang or ""),
            )
        conn.commit()
    except sqlite3.OperationalError:
        conn.rollback()  # Table may not exist yet

    # -- flow_snapshots --
    try:
        conn.execute("BEGIN IMMEDIATE")
        conn.execute("DELETE FROM flow_snapshots")
        flow_rows = conn.execute(
            "SELECT id, name, entry_point_id, criticality, node_count, "
            "file_count, path_json FROM flows"
        ).fetchall()

        # Collect every node id referenced by any flow, then fetch
        # their qualified_names in one batched query instead of per-flow
        # per-node lookups.
        needed_ids: set[int] = set()
        parsed_paths: list[list[int]] = []
        for r in flow_rows:
            needed_ids.add(r[2])  # entry_point_id
            path_ids = _json.loads(r[6]) if r[6] else []
            parsed_paths.append(path_ids)
            # Match the old semantics: entry + up to 3 intermediates + last
            for nid in path_ids[1:4]:
                needed_ids.add(nid)
            if path_ids:
                needed_ids.add(path_ids[-1])

        id_to_name: dict[int, str] = {}
        if needed_ids:
            # Batch the IN clause in chunks of 450 to stay under SQLite's
            # default SQLITE_MAX_VARIABLE_NUMBER (999), same strategy as
            # GraphStore.get_edges_among.
            id_list = list(needed_ids)
            for i in range(0, len(id_list), 450):
                batch = id_list[i : i + 450]
                placeholders = ",".join("?" for _ in batch)
                node_rows = conn.execute(
                    f"SELECT id, qualified_name FROM nodes WHERE id IN ({placeholders})",  # nosec B608
                    batch,
                ).fetchall()
                for nr in node_rows:
                    id_to_name[nr[0]] = nr[1]

        for r, path_ids in zip(flow_rows, parsed_paths):
            fid, fname, ep_id = r[0], r[1], r[2]
            crit, ncount, fcount = r[3], r[4], r[5]
            ep_name = id_to_name.get(ep_id, str(ep_id))
            critical_path: list[str] = []
            if path_ids:
                critical_path.append(ep_name)
                if len(path_ids) > 2:
                    for nid in path_ids[1:4]:
                        nm = id_to_name.get(nid)
                        if nm:
                            critical_path.append(nm)
                if len(path_ids) > 1:
                    last = id_to_name.get(path_ids[-1])
                    if last and last not in critical_path:
                        critical_path.append(last)
            conn.execute(
                "INSERT OR REPLACE INTO flow_snapshots "
                "(flow_id, name, entry_point, critical_path, criticality, "
                "node_count, file_count) VALUES (?, ?, ?, ?, ?, ?, ?)",
                (fid, fname, ep_name, _json.dumps(critical_path), crit, ncount, fcount),
            )
        conn.commit()
    except sqlite3.OperationalError:
        conn.rollback()

    # -- risk_index --
    try:
        conn.execute("BEGIN IMMEDIATE")
        conn.execute("DELETE FROM risk_index")

        # Pre-compute caller and test-coverage counts in two aggregate
        # queries. Previously this section ran two COUNT(*) queries per
        # candidate node; on a ~100k-edge graph with tens of thousands
        # of Function/Class/Test nodes that was the primary hang
        # observed during Godot builds.
        caller_counts: dict[str, int] = {}
        for row in conn.execute(
            "SELECT target_qualified, COUNT(*) FROM edges "
            "WHERE kind = 'CALLS' GROUP BY target_qualified"
        ):
            caller_counts[row[0]] = row[1]

        tested_counts: dict[str, int] = {}
        for row in conn.execute(
            "SELECT source_qualified, COUNT(*) FROM edges "
            "WHERE kind = 'TESTED_BY' GROUP BY source_qualified"
        ):
            tested_counts[row[0]] = row[1]

        risk_nodes = conn.execute(
            "SELECT id, qualified_name, name FROM nodes WHERE kind IN ('Function', 'Class', 'Test')"
        ).fetchall()
        security_kw = {
            "auth",
            "login",
            "password",
            "token",
            "session",
            "crypt",
            "secret",
            "credential",
            "permission",
            "sql",
            "execute",
        }
        for n in risk_nodes:
            nid, qn, name = n[0], n[1], n[2]
            caller_count = caller_counts.get(qn, 0)
            tested = tested_counts.get(qn, 0)
            coverage = "tested" if tested > 0 else "untested"
            name_lower = name.lower()
            sec_relevant = 1 if any(kw in name_lower for kw in security_kw) else 0
            risk = 0.0
            if caller_count > 10:
                risk += 0.3
            elif caller_count > 3:
                risk += 0.15
            if coverage == "untested":
                risk += 0.3
            if sec_relevant:
                risk += 0.4
            risk = min(risk, 1.0)
            conn.execute(
                "INSERT OR REPLACE INTO risk_index "
                "(node_id, qualified_name, risk_score, caller_count, "
                "test_coverage, security_relevant, last_computed) "
                "VALUES (?, ?, ?, ?, ?, ?, datetime('now'))",
                (nid, qn, risk, caller_count, coverage, sec_relevant),
            )
        conn.commit()
    except sqlite3.OperationalError:
        conn.rollback()


def build_or_update_graph(
    full_rebuild: bool = False,
    repo_root: str | None = None,
    base: str = "HEAD~1",
    postprocess: str = "full",
    recurse_submodules: bool | None = None,
    local_embedding: str | None = None,
    local_embedding_port: int = 18080,
    local_embedding_bin: str = "llama-server",
    keep_local_embedding_server: bool = False,
    local_embedding_timeout: int = 300,
    local_embedding_request_timeout: int = 60,
    local_embedding_batch_size: int = 1,
) -> dict[str, Any]:
    """Build or incrementally update the code knowledge graph.

    Args:
        full_rebuild: If True, re-parse every file. If False (default),
                      only re-parse files changed since ``base``.
        repo_root: Path to the repository root. Auto-detected if omitted.
        base: Git ref for incremental diff (default: HEAD~1).
        postprocess: Post-processing level after build:
            ``"full"`` (default) — signatures, FTS, flows, communities.
            ``"minimal"`` — signatures + FTS only (fast, keeps search working).
            ``"none"`` — skip all post-processing (raw parse only).
        recurse_submodules: If True, include files from git submodules
            via ``git ls-files --recurse-submodules``. When None
            (default), falls back to the CRG_RECURSE_SUBMODULES
            environment variable. Default: disabled.
        local_embedding: Optional local Qwen embedding preset: ``"low"``.
            ``None`` / ``"none"`` skips embeddings.
        local_embedding_port: localhost port for the OpenAI-compatible
            llama-server endpoint.
        local_embedding_bin: ``llama-server`` executable name or path.
        keep_local_embedding_server: Leave a dagayn-started server running
            after embedding completes.
        local_embedding_timeout: Seconds to wait for llama-server readiness.
        local_embedding_request_timeout: Seconds to wait for each embedding
            HTTP request once the server is ready.
        local_embedding_batch_size: Texts to send in each local embedding
            HTTP request.

    Returns:
        Summary with files_parsed/updated, node/edge counts, and errors.
    """
    # Build/update is a write workload — opt out of the read-only store
    # cache so we don't hold a stale connection open across mutations.
    _evict_store_cache()
    store, root = _get_store(repo_root, cached=False, use_backend_default=True)
    try:
        if full_rebuild:
            result = full_build(root, store, recurse_submodules)
            build_result = {
                "status": "ok",
                "build_type": "full",
                "summary": (
                    f"Full build complete: parsed {result['files_parsed']} files, "
                    f"created {result['total_nodes']} nodes and "
                    f"{result['total_edges']} edges."
                ),
                **result,
            }
        else:
            result = incremental_update(root, store, base=base)
            if result["files_updated"] == 0:
                build_result = {
                    "status": "ok",
                    "build_type": "incremental",
                    "summary": "No changes detected. Graph is up to date.",
                    "postprocess_level": postprocess,
                    **result,
                }
                if _local_embedding_requested(local_embedding):
                    build_result["local_embedding"] = _run_local_embedding(
                        root,
                        local_embedding=local_embedding or "none",
                        local_embedding_port=local_embedding_port,
                        local_embedding_bin=local_embedding_bin,
                        keep_local_embedding_server=keep_local_embedding_server,
                        local_embedding_timeout=local_embedding_timeout,
                        local_embedding_request_timeout=local_embedding_request_timeout,
                        local_embedding_batch_size=local_embedding_batch_size,
                    )
                return build_result
            build_result = {
                "status": "ok",
                "build_type": "incremental",
                "summary": (
                    f"Incremental update: {result['files_updated']} files re-parsed, "
                    f"{result['total_nodes']} nodes and "
                    f"{result['total_edges']} edges updated. "
                    f"Changed: {result['changed_files']}. "
                    f"Dependents also updated: {result['dependent_files']}."
                ),
                **result,
            }

        # Pass changed_files for incremental flow/community detection.
        changed = result.get("changed_files") if not full_rebuild else None
        if postprocess == "none":
            warnings = _run_postprocess(
                store,
                build_result,
                postprocess,
                full_rebuild=full_rebuild,
                changed_files=changed,
            )
        elif (
            postprocess == "full"
            and not hasattr(store, "_conn")
            and _can_run_minimal_postprocess(store)
        ):
            can_compute_rust_summaries = hasattr(store, "compute_summaries")
            can_trace_rust_flows = (full_rebuild and _can_trace_full_flows(store)) or (
                not full_rebuild and _can_trace_incremental_flows(store)
            )
            can_detect_rust_communities = (
                full_rebuild and _can_detect_full_communities(store)
            ) or (not full_rebuild and _can_detect_incremental_communities(store))
            missing = []
            if not can_trace_rust_flows:
                missing.append("flow tracing")
            if not can_detect_rust_communities:
                missing.append("community detection")
            if not can_compute_rust_summaries:
                missing.append("summary computation")
            if missing:
                raise RuntimeError(
                    "Rust post-processing is missing support for "
                    + ", ".join(missing)
                    + ". Install a wheel with the native extension or rebuild from source."
                )
            warnings = _run_postprocess(
                store,
                build_result,
                "minimal",
                full_rebuild=full_rebuild,
                changed_files=changed,
            )
            if can_trace_rust_flows:
                try:
                    if full_rebuild:
                        from dagayn.flows import store_flows as _store_flows
                        from dagayn.flows import trace_flows as _trace_flows

                        traced = _trace_flows(store)
                        build_result["flows_detected"] = _store_flows(store, traced)
                    else:
                        from dagayn.flows import incremental_trace_flows

                        build_result["flows_detected"] = incremental_trace_flows(
                            store, changed or []
                        )
                except (sqlite3.OperationalError, RuntimeError, ImportError) as e:
                    logger.warning("Flow detection failed: %s", e)
                    warnings.append(f"Flow detection failed: {type(e).__name__}: {e}")
            if can_detect_rust_communities:
                try:
                    if full_rebuild:
                        from dagayn.communities import (
                            detect_communities as _detect_communities,
                        )
                        from dagayn.communities import (
                            store_communities as _store_communities,
                        )

                        comms = _detect_communities(store)
                        build_result["communities_detected"] = _store_communities(store, comms)
                    else:
                        from dagayn.communities import incremental_detect_communities

                        build_result["communities_detected"] = incremental_detect_communities(
                            store, changed or []
                        )
                except (sqlite3.OperationalError, RuntimeError, ImportError) as e:
                    logger.warning("Community detection failed: %s", e)
                    warnings.append(f"Community detection failed: {type(e).__name__}: {e}")

            try:
                _compute_summaries(store)
                build_result["summaries_computed"] = True
            except (sqlite3.OperationalError, RuntimeError, Exception) as e:
                logger.warning("Summary computation failed: %s", e)
                warnings.append(f"Summary computation failed: {type(e).__name__}: {e}")
            warnings.extend(
                _run_postprocess(
                    store,
                    build_result,
                    postprocess,
                    full_rebuild=full_rebuild,
                    changed_files=changed,
                    skip_minimal_steps=True,
                    skip_flow_steps=True,
                    skip_community_steps=True,
                    skip_summary_steps=True,
                )
            )
        else:
            pp_store, close_pp_store = _postprocess_store(store, root, postprocess)
            try:
                warnings = _run_postprocess(
                    pp_store,
                    build_result,
                    postprocess,
                    full_rebuild=full_rebuild,
                    changed_files=changed,
                )
            finally:
                if close_pp_store:
                    pp_store.close()
        if warnings:
            build_result["warnings"] = warnings
        if _local_embedding_requested(local_embedding):
            build_result["local_embedding"] = _run_local_embedding(
                root,
                local_embedding=local_embedding or "none",
                local_embedding_port=local_embedding_port,
                local_embedding_bin=local_embedding_bin,
                keep_local_embedding_server=keep_local_embedding_server,
                local_embedding_timeout=local_embedding_timeout,
                local_embedding_request_timeout=local_embedding_request_timeout,
                local_embedding_batch_size=local_embedding_batch_size,
            )
        return build_result
    finally:
        store.close()


def run_postprocess(
    flows: bool = True,
    communities: bool = True,
    fts: bool = True,
    repo_root: str | None = None,
) -> dict[str, Any]:
    """Run post-processing steps on an existing graph.

    Useful for running expensive steps (flows, communities) separately
    from the build, or for re-running after the graph has been updated
    with ``postprocess="none"``.

    Args:
        flows: Run flow detection. Default: True.
        communities: Run community detection. Default: True.
        fts: Rebuild FTS index. Default: True.
        repo_root: Repository root path. Auto-detected if omitted.

    Returns:
        Summary of what was computed.
    """
    # Postprocess writes to flows / communities / FTS — bypass the
    # read-only store cache for the duration of this call.
    _evict_store_cache()
    store, _root = _get_store(repo_root, cached=False)
    result: dict[str, Any] = {"status": "ok"}
    warnings: list[str] = []

    try:
        try:
            rust_compute = getattr(store, "compute_missing_signatures", None)
            if callable(rust_compute):
                rust_compute()
            else:
                rows = store.get_nodes_without_signature()
                for row in rows:
                    node_id, name, kind, params, ret = (
                        row[0],
                        row[1],
                        row[2],
                        row[3],
                        row[4],
                    )
                    if kind in ("Function", "Test"):
                        sig = f"def {name}({params or ''})"
                        if ret:
                            sig += f" -> {ret}"
                    elif kind == "Class":
                        sig = f"class {name}"
                    elif kind == "DocSection":
                        sig = f"# {name}"
                    else:
                        sig = name
                    store.update_node_signature(node_id, sig[:512])
                store.commit()
            result["signatures_updated"] = True
        except (sqlite3.OperationalError, RuntimeError, TypeError, KeyError) as e:
            logger.warning("Signature computation failed: %s", e)
            warnings.append(f"Signature computation failed: {type(e).__name__}: {e}")

        if fts:
            try:
                from dagayn.search import rebuild_fts_index

                fts_count = rebuild_fts_index(store)
                result["fts_indexed"] = fts_count
            except (sqlite3.OperationalError, ImportError) as e:
                store.rollback()
                logger.warning("FTS index rebuild failed: %s", e)
                warnings.append(f"FTS index rebuild failed: {type(e).__name__}: {e}")

        if flows:
            try:
                from dagayn.flows import store_flows as _store_flows
                from dagayn.flows import trace_flows as _trace_flows

                traced = _trace_flows(store)
                count = _store_flows(store, traced)
                result["flows_detected"] = count
            except (sqlite3.OperationalError, ImportError) as e:
                store.rollback()
                logger.warning("Flow detection failed: %s", e)
                warnings.append(f"Flow detection failed: {type(e).__name__}: {e}")

        if communities:
            try:
                from dagayn.communities import (
                    detect_communities as _detect_communities,
                )
                from dagayn.communities import (
                    store_communities as _store_communities,
                )

                comms = _detect_communities(store)
                count = _store_communities(store, comms)
                result["communities_detected"] = count
            except (sqlite3.OperationalError, ImportError) as e:
                store.rollback()
                logger.warning("Community detection failed: %s", e)
                warnings.append(f"Community detection failed: {type(e).__name__}: {e}")

        store.set_metadata(
            "last_postprocessed_at",
            time.strftime("%Y-%m-%dT%H:%M:%S"),
        )
        result["summary"] = "Post-processing complete."
        if warnings:
            result["warnings"] = warnings
        return result
    finally:
        store.close()
