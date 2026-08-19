"""Tool 1: build_or_update_graph + run_postprocess."""

from __future__ import annotations

import logging
import os
import sqlite3
import sys
import threading
import time
from pathlib import Path
from typing import Any, Callable, cast

from ..incremental import full_build, incremental_update
from ..paths import get_db_path
from ..state_types import BuildResult, build_result_payload
from ..write_lock import WriteLockUnavailableError, graph_write_lock
from ._common import _evict_store_cache, _get_store, _validate_repo_root

logger = logging.getLogger(__name__)

type BuildValue = Any
type BuildPayload = dict[str, BuildValue]

_LOCAL_EMBEDDING_DISABLED = {None, "", "none"}
_LOCAL_EMBEDDING_BGE = "bge-m3"
_LOCAL_EMBEDDING_LLAMA_QWEN3 = "llama-qwen3"
_LOCAL_EMBEDDING_ENV_LOCK = threading.Lock()
_HOOK_UPDATE_ENV = "DAGAYN_HOOK_UPDATE"

#: How long one embedding slice may hold the exclusive graph lock. Kept well
#: under ``DEFAULT_READ_LOCK_TIMEOUT`` (10s) so an MCP reader that arrives
#: mid-run waits for one slice instead of timing out on the whole pass.
_DEFAULT_EMBED_SLICE_SECONDS = 4.0


#: How long the embedding pass stays out of the lock between slices. Waiters
#: poll for the file lock, so releasing and immediately re-taking it hands over
#: to nobody: this has to exceed ``write_lock._MAX_POLL_INTERVAL`` for a queued
#: reader to actually get its turn.
_EMBED_SLICE_HANDOFF_SECONDS = 0.25


def _embed_slice_seconds() -> float | None:
    """Seconds of embedding per lock acquisition; ``None`` disables slicing."""
    raw = os.environ.get("DAGAYN_EMBED_SLICE_SECONDS")
    if raw is None:
        return _DEFAULT_EMBED_SLICE_SECONDS
    try:
        value = float(raw)
    except ValueError:
        return _DEFAULT_EMBED_SLICE_SECONDS
    return None if value <= 0 else value


def _can_run_minimal_postprocess(store: Any) -> bool:
    return all(
        hasattr(store, name)
        for name in (
            "compute_missing_signatures",
            "rebuild_fts_index",
            "resolve_markdown_artifact_refs",
            "demote_unresolved_endpoint_edges",
            "resolve_terraform_artifact_refs",
            "resolve_bare_call_targets",
            "resolve_bare_inheritance_targets",
            "replace_manifest_bridges_json",
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


def _resolve_local_embedding_mode(
    local_embedding: str | None,
    local_embedding_mode: str | None = None,
) -> str:
    if local_embedding_mode:
        return local_embedding_mode.strip().lower()
    normalized = (local_embedding or "").strip().lower()
    if normalized in {"low", "llama", "qwen", "qwen3", _LOCAL_EMBEDDING_LLAMA_QWEN3}:
        return _LOCAL_EMBEDDING_LLAMA_QWEN3
    return _LOCAL_EMBEDDING_BGE


def _hook_update_requested() -> bool:
    return os.environ.get(_HOOK_UPDATE_ENV, "").strip().lower() in {"1", "true", "yes"}


def _resolve_write_root(repo_root: str | None) -> Path:
    """Resolve the repository root before any store is opened.

    The write lock has to be keyed on the database path, and the database path
    needs the root -- but taking the lock only makes sense *before* opening the
    store, so this cannot come from the store itself.
    """
    from ..incremental import find_project_root

    if repo_root:
        return _validate_repo_root(Path(repo_root))
    return Path(find_project_root())


def _run_local_embedding(
    root: Any,
    *,
    local_embedding: str,
    local_embedding_mode: str | None = None,
    local_embedding_port: int | None,
    local_embedding_bin: str,
    keep_local_embedding_server: bool,
    local_embedding_timeout: int,
    local_embedding_request_timeout: int,
    local_embedding_batch_size: int,
    pass_seconds: float | None = None,
) -> BuildPayload:
    """Run graph embedding through the selected local embedding mode.

    The pass runs as a series of time-bounded slices, each taking the graph
    lock on its own, so readers and queued updates get a turn between slices
    instead of waiting out the whole corpus. ``pass_seconds`` additionally caps
    the total time spent here and reports what is left in
    ``embedding_remaining``; the caller re-queues to finish the rest.
    """
    mode = _resolve_local_embedding_mode(local_embedding, local_embedding_mode)
    from dagayn.local_embeddings import local_embedding_server, resolve_local_embedding_port

    preset_level = _LOCAL_EMBEDDING_BGE if mode == _LOCAL_EMBEDDING_BGE else "low"
    port = resolve_local_embedding_port(local_embedding_port, preset_level)

    with local_embedding_server(
        preset_level,
        port=port,
        binary=local_embedding_bin,
        keep_running=keep_local_embedding_server,
        startup_timeout=local_embedding_timeout,
    ) as server:
        env_keys = (
            "CRG_OPENAI_API_KEY",
            "CRG_OPENAI_BASE_URL",
            "CRG_OPENAI_BATCH_SIZE",
            "CRG_OPENAI_DIMENSION",
            "CRG_OPENAI_MAX_LENGTH",
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
                if server.preset.request_max_length is None:
                    os.environ.pop("CRG_OPENAI_MAX_LENGTH", None)
                else:
                    os.environ["CRG_OPENAI_MAX_LENGTH"] = str(server.preset.request_max_length)
                # The graph lock is taken inside, per slice. Starting the
                # sidecar and loading its model take up to
                # ``local_embedding_timeout`` seconds without reading or
                # writing the database, and holding the exclusive lock across
                # that made every MCP tool call in the meantime wait (and then
                # fail) for no reason.
                result = _embed_in_slices(
                    root,
                    model=server.preset.model,
                    pass_seconds=pass_seconds,
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
        "mode": mode,
        "model": server.preset.model,
        "dimension": server.preset.dimension,
        "text_mode": server.preset.text_mode,
        "server_started": server.started,
        "server_url": server.base_url,
        "server_command": server.command,
        "newly_embedded": result.get("newly_embedded", 0),
        "orphans_removed": result.get("orphans_removed", 0),
        "total_embeddings": result.get("total_embeddings", 0),
        "embedding_remaining": result.get("remaining", 0),
        "embedding_slices": result.get("slices", 1),
        "summary": result.get("summary", ""),
    }


def _embed_in_slices(
    root: Any,
    *,
    model: str | None,
    pass_seconds: float | None,
) -> BuildPayload:
    """Embed the graph in time-bounded slices, releasing the lock between them.

    Aggregates the per-slice results into a single ``embed_graph``-shaped
    payload. Stops early when a slice makes no progress, so a node the provider
    keeps rejecting cannot spin here forever.
    """
    from dagayn.tools.docs import embed_graph

    db_path = get_db_path(Path(root))
    slice_seconds = _embed_slice_seconds()
    deadline = None if pass_seconds is None else time.monotonic() + pass_seconds
    newly_embedded = 0
    orphans_removed = 0
    slices = 0
    result: BuildPayload = {}

    while True:
        with graph_write_lock(db_path):
            result = embed_graph(
                repo_root=str(root),
                provider="openai",
                model=model,
                show_progress=sys.stderr.isatty(),
                slice_seconds=slice_seconds,
                # The orphan and retired-partition sweeps look at the whole
                # corpus, so they belong to the run and not to every slice.
                prune_orphans=slices == 0,
            )
        slices += 1
        if result.get("status") != "ok":
            return result
        embedded_now = int(result.get("newly_embedded", 0) or 0)
        newly_embedded += embedded_now
        orphans_removed += int(result.get("orphans_removed", 0) or 0)
        remaining = int(result.get("remaining", 0) or 0)
        if remaining <= 0:
            break
        if embedded_now == 0:
            logger.warning(
                "Embedding slice %d made no progress with %d node(s) left; stopping.",
                slices,
                remaining,
            )
            break
        if deadline is not None and time.monotonic() >= deadline:
            logger.info(
                "Embedding pass budget reached after %d slice(s); %d node(s) left.",
                slices,
                remaining,
            )
            break
        time.sleep(_EMBED_SLICE_HANDOFF_SECONDS)

    result = dict(result)
    result["newly_embedded"] = newly_embedded
    result["orphans_removed"] = orphans_removed
    result["slices"] = slices
    if slices > 1:
        # The last slice's summary only describes that slice.
        left = int(result.get("remaining", 0) or 0)
        result["summary"] = (
            f"Embedded {newly_embedded} new node(s) across {slices} slice(s). "
            f"Removed {orphans_removed} orphan embedding(s). "
            f"Total embeddings: {result.get('total_embeddings', 0)}. "
            + (
                f"{left} node(s) still queued for a later pass."
                if left
                else "Semantic search is now active."
            )
        )
    return result


def _prune_orphaned_structures(store: Any, build_result: BuildResult) -> list[str]:
    """Prune derived rows orphaned by a re-parse; return warning strings."""
    warnings: list[str] = []
    try:
        prune = getattr(store, "prune_orphaned_graph_structures", None)
        if callable(prune):
            pruned = cast(Callable[[], dict[str, int]], prune)()
            store.commit()
            if pruned:
                build_result.orphans_pruned = pruned
    except (sqlite3.OperationalError, RuntimeError, TypeError) as e:
        logger.warning("Orphaned structure pruning failed: %s", e)
        warnings.append(f"Orphaned structure pruning failed: {type(e).__name__}: {e}")
    return warnings


def _prune_orphaned_embeddings(repo_root: Path, build_result: BuildResult) -> list[str]:
    """Delete vectors for nodes the graph no longer has.

    ``remove_orphans`` used to be reachable only from ``embed_all_nodes``, so
    updating after a deletion *without* embeddings enabled left the deleted
    nodes' vectors in place. They then won top-k slots in semantic search and
    were dropped when their nodes could not be resolved, silently returning
    fewer results than the caller asked for. Pruning needs no provider and makes
    no API calls.

    Must run with the graph store closed: embeddings live in the same SQLite
    file, and opening a second writer alongside the native store's own
    connection corrupted the database.
    """
    warnings: list[str] = []
    try:
        from ..embeddings_store import EmbeddingStore
        from ..graph import GraphStore
        from ..paths import get_db_path

        db_path = get_db_path(repo_root)
        graph = GraphStore(db_path)
        try:
            live = {node.qualified_name for node in graph.get_all_nodes(exclude_files=True)}
        finally:
            graph.close()
        emb_store = EmbeddingStore(db_path)
        try:
            removed = emb_store.remove_orphans(live, all_providers=True)
        finally:
            emb_store.close()
        if removed:
            build_result.embedding_orphans_pruned = removed
    except (sqlite3.Error, OSError, RuntimeError, TypeError, AttributeError) as e:
        logger.warning("Orphaned embedding pruning failed: %s", e)
        warnings.append(f"Orphaned embedding pruning failed: {type(e).__name__}: {e}")
    return warnings


def _run_postprocess(
    store: Any,
    build_result: BuildResult,
    postprocess: str,
    full_rebuild: bool = False,
    changed_files: list[str] | None = None,
    pre_affected_communities: int = 0,
    skip_minimal_steps: bool = False,
    skip_flow_steps: bool = False,
    skip_community_steps: bool = False,
    skip_summary_steps: bool = False,
    skip_centrality_steps: bool = False,
    skip_orphan_prune: bool = False,
) -> list[str]:
    """Run post-build steps based on *postprocess* level.

    When *full_rebuild* is False and *changed_files* are available,
    uses incremental flow/community detection for faster updates.

    Returns a list of warning strings (empty on success).
    """
    warnings: list[str] = []
    build_result.postprocess_level = postprocess

    if postprocess == "none":
        return warnings

    post_result = build_result.postprocess

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
            build_result.signatures_updated = True
        except (sqlite3.OperationalError, RuntimeError, TypeError, KeyError) as e:
            logger.warning("Signature computation failed: %s", e)
            warnings.append(f"Signature computation failed: {type(e).__name__}: {e}")

        try:
            from dagayn.search import rebuild_fts_index

            fts_count = rebuild_fts_index(store)
            build_result.fts_indexed = fts_count
            build_result.fts_rebuilt = True
        except (sqlite3.OperationalError, ImportError) as e:
            logger.warning("FTS index rebuild failed: %s", e)
            warnings.append(f"FTS index rebuild failed: {type(e).__name__}: {e}")

        try:
            from dagayn.postprocessing import _resolve_bare_name_edges

            _resolve_bare_name_edges(store, post_result, warnings)
        except (sqlite3.OperationalError, ImportError) as e:
            logger.warning("Bare-name edge resolution failed: %s", e)
            warnings.append(f"Bare-name edge resolution failed: {type(e).__name__}: {e}")

        try:
            from dagayn.postprocessing import _demote_unresolved_endpoint_edges

            _demote_unresolved_endpoint_edges(store, post_result, warnings)
        except (sqlite3.OperationalError, ImportError) as e:
            logger.warning("Unresolved endpoint demotion failed: %s", e)
            warnings.append(f"Unresolved endpoint demotion failed: {type(e).__name__}: {e}")

        try:
            from dagayn.postprocessing import _resolve_markdown_artifact_refs

            _resolve_markdown_artifact_refs(store, post_result, warnings)
        except (sqlite3.OperationalError, ImportError) as e:
            logger.warning("Markdown artifact ref resolution failed: %s", e)
            warnings.append(f"Markdown artifact ref resolution failed: {type(e).__name__}: {e}")

        try:
            from dagayn.postprocessing import _resolve_terraform_artifact_refs

            _resolve_terraform_artifact_refs(store, post_result, warnings)
        except (sqlite3.OperationalError, ImportError) as e:
            logger.warning("Terraform artifact ref resolution failed: %s", e)
            warnings.append(f"Terraform artifact ref resolution failed: {type(e).__name__}: {e}")

        try:
            from dagayn.postprocessing import _apply_manifest_bridges

            _apply_manifest_bridges(store, post_result, warnings)
        except (sqlite3.OperationalError, ImportError) as e:
            logger.warning("Manifest bridge extraction failed: %s", e)
            warnings.append(f"Manifest bridge extraction failed: {type(e).__name__}: {e}")

    if postprocess != "none" and not skip_centrality_steps:
        # File re-parses invalidate hub_scores / bridge_scores wholesale (see
        # remove_files_data_tx), so every non-none postprocess level must
        # recompute them or the tables stay empty after skip-flows updates.
        from dagayn.postprocessing import _persist_centrality_scores

        _persist_centrality_scores(store, post_result, warnings)

    if postprocess == "minimal":
        if not skip_orphan_prune:
            warnings.extend(_prune_orphaned_structures(store, build_result))

        # The minimal path returns before the bottom of this function, so record
        # the level here too. Leaving the previous run's ``postprocess_level``
        # in place made a graph whose flows had just been pruned still advertise
        # itself as fully post-processed.
        _record_postprocess_level(store, postprocess)
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
            post_result.flows_detected = count
        except (sqlite3.OperationalError, RuntimeError, ImportError) as e:
            logger.warning("Flow detection failed: %s", e)
            warnings.append(f"Flow detection failed: {type(e).__name__}: {e}")

        if postprocess != "minimal":
            try:
                prune = getattr(store, "prune_orphaned_graph_structures", None)
                if callable(prune):
                    pruned = cast(Callable[[], dict[str, int]], prune)()
                    store.commit()
                    if pruned:
                        existing = build_result.orphans_pruned
                        if existing is not None:
                            for key, value in pruned.items():
                                existing[key] = existing.get(key, 0) + value
                        else:
                            build_result.orphans_pruned = pruned
            except (sqlite3.OperationalError, RuntimeError, TypeError) as e:
                logger.warning("Post-flow orphan pruning failed: %s", e)
                warnings.append(f"Post-flow orphan pruning failed: {type(e).__name__}: {e}")

    if not skip_community_steps:
        try:
            if use_incremental:
                from dagayn.communities import (
                    incremental_detect_communities,
                )

                count = incremental_detect_communities(
                    store,
                    changed_files or [],
                    pre_affected_count=pre_affected_communities or None,
                )
            else:
                from dagayn.communities import (
                    detect_communities as _detect_communities,
                )
                from dagayn.communities import (
                    store_communities as _store_communities,
                )

                comms = _detect_communities(store)
                count = _store_communities(store, comms)
            post_result.communities_detected = count
        except (sqlite3.OperationalError, RuntimeError, ImportError) as e:
            logger.warning("Community detection failed: %s", e)
            warnings.append(f"Community detection failed: {type(e).__name__}: {e}")

    if not skip_orphan_prune:
        warnings.extend(_prune_orphaned_structures(store, build_result))

    if not skip_summary_steps:
        # -- Compute pre-computed summary tables --
        try:
            _compute_summaries(store)
            build_result.summaries_computed = True
        except (sqlite3.OperationalError, RuntimeError, Exception) as e:
            logger.warning("Summary computation failed: %s", e)
            warnings.append(f"Summary computation failed: {type(e).__name__}: {e}")

    _record_postprocess_level(store, postprocess)

    return warnings


def _record_postprocess_level(store: Any, postprocess: str) -> None:
    """Persist which post-processing level the graph last received."""
    store.set_metadata(
        "last_postprocessed_at",
        time.strftime("%Y-%m-%dT%H:%M:%S"),
    )
    store.set_metadata("postprocess_level", postprocess)


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
            cid, cname, _csize, clang = r[0], r[1], r[2], r[3]
            live_members = nodes_by_comm.get(cid, [])
            live_size = len(live_members)

            # Top 5 symbols by total edge count (in + out). Python's
            # sorted() is stable so ties break by original row order.
            members = sorted(
                live_members,
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
                (cid, cname, purpose, key_syms, live_size, clang or ""),
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
    local_embedding_mode: str | None = None,
    local_embedding_port: int | None = None,
    local_embedding_bin: str = "auto",
    keep_local_embedding_server: bool = False,
    local_embedding_timeout: int = 300,
    local_embedding_request_timeout: int = 60,
    local_embedding_batch_size: int = 1,
    extra_files: list[str] | None = None,
    embed_pass_seconds: float | None = None,
) -> BuildPayload:
    """Build or incrementally update the code knowledge graph.

    Args:
        full_rebuild: If True, re-parse every file. If False (default),
                      only re-parse files changed since ``base``.
        repo_root: Path to the repository root. Auto-detected if omitted.
        base: Git ref for incremental diff (default: HEAD~1).
        extra_files: Files to re-index in addition to the git diff, for
            content drift the diff cannot see (see ``incremental_update``).
        postprocess: Post-processing level after build:
            ``"full"`` (default) — signatures, FTS, flows, communities.
            ``"minimal"`` — signatures + FTS only (fast, keeps search working).
            ``"none"`` — skip all post-processing (raw parse only).
        recurse_submodules: If True, include files from git submodules
            via ``git ls-files --recurse-submodules``. When None
            (default), falls back to the CRG_RECURSE_SUBMODULES
            environment variable. Default: disabled.
        local_embedding: Optional local embedding request. ``"bge-m3"`` runs
            the managed BGE-M3 sidecar; ``"low"`` / ``"llama-qwen3"`` runs the
            managed Qwen sidecar.
        local_embedding_mode: Optional explicit local embedding execution mode:
            ``"bge-m3"`` or ``"llama-qwen3"``.
            ``None`` / ``"none"`` skips embeddings.
        local_embedding_port: localhost port for the OpenAI-compatible local
            embedding endpoint. ``None`` selects the preset default (18080 for
            bge-m3, 18081 for low).
        local_embedding_bin: executable name/path, or ``"auto"`` for the
            preset default.
        keep_local_embedding_server: Leave a dagayn-started server running
            after embedding completes.
        local_embedding_timeout: Seconds to wait for local embedding server readiness.
        local_embedding_request_timeout: Seconds to wait for each embedding
            HTTP request once the server is ready.
        local_embedding_batch_size: Texts to send in each local embedding
            HTTP request.
        embed_pass_seconds: Cap on total time spent embedding. The pass stops
            at a slice boundary once exceeded and reports
            ``local_embedding.embedding_remaining`` so a scheduler can finish
            the rest in a later run. ``None`` (default) embeds everything.

    Returns:
        Summary with files_parsed/updated, node/edge counts, and errors.
    """
    # Build/update is a write workload — opt out of the read-only store
    # cache so we don't hold a stale connection open across mutations.
    _evict_store_cache()
    root_path = _resolve_write_root(repo_root)
    db_path = get_db_path(root_path)
    hook_update = _hook_update_requested() and not full_rebuild
    # The lock is taken *before* the store is opened, so the migrations and
    # column backfills that opening performs are inside it too. Hook-triggered
    # runs stay non-blocking: overlapping hook updates should skip rather than
    # queue. Everything else waits, because failing on a busy database is the
    # behaviour this replaces.
    try:
        write_lock = graph_write_lock(db_path, blocking=not hook_update)
        write_lock.__enter__()
    except WriteLockUnavailableError as exc:
        if hook_update:
            return build_result_payload(
                BuildResult(
                    status="ok",
                    build_type="incremental",
                    files_updated=0,
                    total_nodes=0,
                    total_edges=0,
                    postprocess_level=postprocess,
                    skipped=True,
                    skip_reason="hook_update_already_running",
                    summary="Skipped: another hook-triggered dagayn update is already running.",
                )
            )
        return build_result_payload(
            BuildResult(
                status="error",
                build_type="full" if full_rebuild else "incremental",
                files_updated=0,
                total_nodes=0,
                total_edges=0,
                postprocess_level=postprocess,
                skipped=True,
                skip_reason="write_lock_unavailable",
                summary=f"Skipped: {exc}",
                errors=[{"file": "", "error": str(exc)}],
            )
        )
    store, root = _get_store(repo_root, cached=False, use_backend_default=True)
    build_result = BuildResult()
    run_embedding = False
    no_changes = False
    try:
        pre_affected_communities = 0
        if full_rebuild:
            build_result = full_build(root, store, recurse_submodules)
            # ``partial`` when files failed to *store*: the graph is missing
            # content it was asked to hold, and callers must not treat it as
            # a complete description of HEAD.
            build_result.status = build_result.status or "ok"
            build_result.build_type = "full"
            build_result.summary = (
                f"Full build complete: parsed {build_result.files_parsed} files, "
                f"created {build_result.total_nodes} nodes and "
                f"{build_result.total_edges} edges."
            )
        else:
            from dagayn.communities import count_affected_communities
            from dagayn.incremental import get_changed_files

            pre_affected_communities = 0
            preview_changed = get_changed_files(root, base)
            if extra_files:
                preview_changed = list(dict.fromkeys([*preview_changed, *extra_files]))
            if preview_changed:
                pre_affected_communities = count_affected_communities(store, preview_changed)
            build_result = incremental_update(root, store, base=base, extra_files=extra_files)
            if build_result.files_updated == 0:
                build_result.status = "ok"
                build_result.build_type = "incremental"
                build_result.summary = "No changes detected. Graph is up to date."
                build_result.postprocess_level = postprocess
                if _local_embedding_requested(local_embedding):
                    if hook_update:
                        build_result.local_embedding_skipped = {
                            "reason": "hook_update_no_changes",
                        }
                    else:
                        run_embedding = True
                no_changes = True
            else:
                build_result.status = build_result.status or "ok"
                build_result.build_type = "incremental"
                build_result.summary = (
                    f"Incremental update: {build_result.files_updated} files re-parsed, "
                    f"{build_result.total_nodes} nodes and "
                    f"{build_result.total_edges} edges updated. "
                    f"Changed: {build_result.changed_files}. "
                    f"Dependents also updated: {build_result.dependent_files}."
                )

        # Pass changed_files for incremental flow/community detection.
        changed = build_result.changed_files if not full_rebuild else None
        if not no_changes:
            if postprocess == "none":
                warnings = _run_postprocess(
                    store,
                    build_result,
                    postprocess,
                    full_rebuild=full_rebuild,
                    changed_files=changed,
                    pre_affected_communities=pre_affected_communities,
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
                    pre_affected_communities=pre_affected_communities,
                    skip_centrality_steps=True,
                    skip_orphan_prune=True,
                )
                if can_trace_rust_flows:
                    try:
                        if full_rebuild:
                            from dagayn.flows import store_flows as _store_flows
                            from dagayn.flows import trace_flows as _trace_flows

                            traced = _trace_flows(store)
                            build_result.postprocess.flows_detected = _store_flows(store, traced)
                        else:
                            from dagayn.flows import incremental_trace_flows

                            build_result.postprocess.flows_detected = incremental_trace_flows(
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
                            build_result.postprocess.communities_detected = _store_communities(
                                store, comms
                            )
                        else:
                            from dagayn.communities import incremental_detect_communities

                            build_result.postprocess.communities_detected = (
                                incremental_detect_communities(
                                    store,
                                    changed or [],
                                    pre_affected_count=pre_affected_communities or None,
                                )
                            )
                    except (sqlite3.OperationalError, RuntimeError, ImportError) as e:
                        logger.warning("Community detection failed: %s", e)
                        warnings.append(f"Community detection failed: {type(e).__name__}: {e}")

                try:
                    _compute_summaries(store)
                    build_result.summaries_computed = True
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
                        pre_affected_communities=pre_affected_communities,
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
                        pre_affected_communities=pre_affected_communities,
                    )
                finally:
                    if close_pp_store:
                        pp_store.close()
            if warnings:
                build_result.warnings = warnings
            if _local_embedding_requested(local_embedding):
                run_embedding = True
    finally:
        store.close()
        # Nothing of ours is open on graph.db from here on, so the structural
        # build's lock can go back. The embedding pass and the orphan prune each
        # take it again around their own database work: both spend most of their
        # wall clock outside sqlite (sidecar startup, model load, HTTP batches),
        # and holding the exclusive lock across that starves every reader.
        write_lock.__exit__(None, None, None)
        try:
            if run_embedding:
                build_result.local_embedding = _run_local_embedding(
                    root,
                    local_embedding=local_embedding or "none",
                    local_embedding_mode=local_embedding_mode,
                    local_embedding_port=local_embedding_port,
                    local_embedding_bin=local_embedding_bin,
                    keep_local_embedding_server=keep_local_embedding_server,
                    local_embedding_timeout=local_embedding_timeout,
                    local_embedding_request_timeout=local_embedding_request_timeout,
                    local_embedding_batch_size=local_embedding_batch_size,
                    pass_seconds=embed_pass_seconds,
                )
        finally:
            if postprocess != "none":
                # Embeddings share the SQLite file, so this must not run
                # alongside another writer or the native store's connection.
                with graph_write_lock(db_path):
                    emb_warnings = _prune_orphaned_embeddings(Path(root), build_result)
                if emb_warnings:
                    build_result.warnings = [
                        *(build_result.warnings or []),
                        *emb_warnings,
                    ]
    return build_result_payload(build_result)


def run_postprocess(
    flows: bool = True,
    communities: bool = True,
    fts: bool = True,
    repo_root: str | None = None,
) -> BuildPayload:
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
    root_path = _resolve_write_root(repo_root)
    db_path = get_db_path(root_path)
    try:
        write_lock = graph_write_lock(db_path, blocking=True)
        write_lock.__enter__()
    except WriteLockUnavailableError as exc:
        return build_result_payload(
            BuildResult(
                status="error",
                skipped=True,
                skip_reason="write_lock_unavailable",
                summary=f"Skipped: {exc}",
                errors=[{"file": "", "error": str(exc)}],
            )
        )
    store, _root = _get_store(repo_root, cached=False)
    result = BuildResult(status="ok")
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
            result.signatures_updated = True
        except (sqlite3.OperationalError, RuntimeError, TypeError, KeyError) as e:
            logger.warning("Signature computation failed: %s", e)
            warnings.append(f"Signature computation failed: {type(e).__name__}: {e}")

        if fts:
            try:
                from dagayn.search import rebuild_fts_index

                fts_count = rebuild_fts_index(store)
                result.fts_indexed = fts_count
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
                result.postprocess.flows_detected = count
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
                result.postprocess.communities_detected = count
            except (sqlite3.OperationalError, ImportError) as e:
                store.rollback()
                logger.warning("Community detection failed: %s", e)
                warnings.append(f"Community detection failed: {type(e).__name__}: {e}")

        store.set_metadata(
            "last_postprocessed_at",
            time.strftime("%Y-%m-%dT%H:%M:%S"),
        )
        result.summary = "Post-processing complete."
        if warnings:
            result.warnings = warnings
        return build_result_payload(result)
    finally:
        store.close()
        write_lock.__exit__(None, None, None)
