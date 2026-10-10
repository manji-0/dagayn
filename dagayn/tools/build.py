"""Tool 1: build_or_update_graph + run_postprocess."""

from __future__ import annotations

import logging
import os
import sqlite3
import sys
import threading
import time
from pathlib import Path
from typing import Any

from ..contracts.state_types import BuildResult, build_result_payload
from ..incremental_build import full_build
from ..incremental_update_pipeline import incremental_update
from ..paths import get_db_path
from ..write_lock import WriteLockUnavailableError, graph_write_lock
from ._common import _evict_store_cache, _get_store, _validate_repo_root
from .sync_status import _local_embedding_requested

logger = logging.getLogger(__name__)

type BuildValue = Any
type BuildPayload = dict[str, BuildValue]

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
    from ..incremental_files import find_project_root
    from ..paths import ALLOW_WIDE_ROOT_ENV, unsafe_root_reason

    if repo_root:
        return _validate_repo_root(Path(repo_root))
    root = Path(find_project_root())
    # Building an auto-detected ``$HOME`` indexes every checkout below it into
    # one graph, after which every search answers from the wrong repository.
    reason = unsafe_root_reason(root)
    if reason is not None:
        raise ValueError(
            f"refusing to build a graph for {reason} ({root}): pass repo_root explicitly,"
            f" set CRG_REPO_ROOT, or give the MCP server entry a cwd/--repo for the"
            f" project; set {ALLOW_WIDE_ROOT_ENV}=1 to index it anyway."
        )
    return root


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
    file_paths: list[str] | None = None,
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
                    file_paths=file_paths,
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
    file_paths: list[str] | None = None,
) -> BuildPayload:
    """Embed the graph in time-bounded slices, releasing the lock between them.

    The corpus is scanned once, under the lock, and the resulting work list is
    then written in bounded windows that need no graph store. Scanning per window
    instead made the scan two thirds of the run: on a 42k-node graph it is ~8 s
    against 4 s of embedding per window.

    Stops early when a window makes no progress, so an item the provider keeps
    rejecting cannot spin here forever.
    """
    from dagayn.tools.docs import scan_embed_work, write_embed_work

    db_path = get_db_path(Path(root))
    slice_seconds = _embed_slice_seconds()
    deadline = None if pass_seconds is None else time.monotonic() + pass_seconds
    show_progress = sys.stderr.isatty()

    scan_started = time.monotonic()
    with graph_write_lock(db_path):
        scan, work = scan_embed_work(
            repo_root=str(root),
            provider="openai",
            model=model,
            prune_orphans=not file_paths,
            file_paths=file_paths,
        )
    if scan.get("status") != "ok":
        return scan
    orphans_removed = int(scan.get("orphans_removed", 0) or 0)
    pending = len(work)
    logger.info(
        "Embedding scan took %.1fs; %d node(s) to embed",
        time.monotonic() - scan_started,
        pending,
    )

    newly_embedded = 0
    slices = 0
    offset = 0
    total_embeddings = 0
    result: BuildPayload = {}

    # Runs at least once even with nothing to embed, so the run is still closed
    # out (provider pointer, retired-partition sweep) and the total reported.
    while True:
        slice_started = time.monotonic()
        with graph_write_lock(db_path):
            result = write_embed_work(
                work[offset:],
                repo_root=str(root),
                provider="openai",
                model=model,
                show_progress=show_progress,
                slice_seconds=slice_seconds,
                finalize=True,
                # A file-scoped pass must never prune: its keep-set is a subset.
                prune_orphans=not file_paths,
            )
        slices += 1
        if result.get("status") != "ok":
            return result
        embedded_now = int(result.get("newly_embedded", 0) or 0)
        remaining_in_slice = int(result.get("remaining", 0) or 0)
        total_embeddings = int(result.get("total_embeddings", 0) or 0)
        # The window reports what is left of the list it was handed, so what it
        # consumed is the rest -- including items it failed on, which must not be
        # retried forever inside one pass.
        consumed = (pending - offset) - remaining_in_slice
        offset += max(consumed, 0)
        newly_embedded += embedded_now
        logger.info(
            "Embedding slice %d: %.1fs wall for %d node(s) (%d left)",
            slices,
            time.monotonic() - slice_started,
            embedded_now,
            pending - offset,
        )
        if offset >= pending:
            break
        if embedded_now == 0:
            logger.warning(
                "Embedding slice %d made no progress with %d node(s) left; stopping.",
                slices,
                pending - offset,
            )
            break
        if deadline is not None and time.monotonic() >= deadline:
            logger.info(
                "Embedding pass budget reached after %d slice(s); %d node(s) left.",
                slices,
                pending - offset,
            )
            break
        time.sleep(_EMBED_SLICE_HANDOFF_SECONDS)

    left = max(0, pending - offset)
    return {
        "status": "ok",
        "summary": (
            f"Embedded {newly_embedded} new node(s) across {slices} slice(s). "
            f"Removed {orphans_removed} orphan embedding(s). "
            f"Total embeddings: {total_embeddings}. "
            + (
                f"{left} node(s) still queued for a later pass."
                if left
                else "Semantic search is now active."
            )
        ),
        "newly_embedded": newly_embedded,
        "orphans_removed": orphans_removed,
        "total_embeddings": total_embeddings,
        "remaining": left,
        "slices": slices,
        "text_mode": result.get("text_mode") or scan.get("text_mode"),
    }


def _warn(warnings: list[str], label: str, e: BaseException) -> None:
    """Log and collect a non-fatal ``<label> failed`` post-processing warning."""
    logger.warning(f"{label} failed: %s", e)
    warnings.append(f"{label} failed: {type(e).__name__}: {e}")


def _detect_communities(
    store: Any,
    post_result: Any,
    warnings: list[str],
    incremental: bool,
    changed_files: list[str] | None,
    pre_affected_communities: int,
) -> None:
    """Detect communities incrementally for *changed_files*, or from scratch."""
    try:
        if incremental:
            from dagayn.communities import incremental_detect_communities

            count = incremental_detect_communities(
                store,
                changed_files or [],
                pre_affected_count=pre_affected_communities or None,
            )
        else:
            from dagayn.communities import detect_communities, store_communities

            count = store_communities(store, detect_communities(store))
        post_result.communities_detected = count
    except (sqlite3.OperationalError, RuntimeError, ImportError) as e:
        _warn(warnings, "Community detection", e)


def _prune_orphaned_structures(store: Any, build_result: BuildResult) -> list[str]:
    """Prune derived rows orphaned by a re-parse; return warning strings."""
    warnings: list[str] = []
    try:
        pruned = store.prune_orphaned_graph_structures()
        store.commit()
        if pruned:
            build_result.orphans_pruned = pruned
    except (sqlite3.OperationalError, RuntimeError, TypeError) as e:
        _warn(warnings, "Orphaned structure pruning", e)
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
        _warn(warnings, "Orphaned embedding pruning", e)
    return warnings


def _run_postprocess(
    store: Any,
    build_result: BuildResult,
    postprocess: str,
    full_rebuild: bool = False,
    changed_files: list[str] | None = None,
    pre_affected_communities: int = 0,
) -> list[str]:
    """Run post-build steps based on *postprocess* level.

    ``minimal`` runs signatures, FTS, the edge resolvers, centrality, and the
    orphan prune. ``full`` adds communities and the summary tables, detected
    incrementally for *changed_files* unless *full_rebuild*.

    Returns a list of warning strings (empty on success).
    """
    warnings: list[str] = []
    build_result.postprocess_level = postprocess

    if postprocess == "none":
        return warnings

    post_result = build_result.postprocess
    # -- Signatures + FTS (fast, always run unless "none") --
    try:
        store.compute_missing_signatures()
        build_result.signatures_updated = True
    except (sqlite3.OperationalError, RuntimeError, TypeError, KeyError) as e:
        _warn(warnings, "Signature computation", e)

    try:
        if changed_files and not full_rebuild:
            fts_count = int(store.sync_fts_for_file_paths(changed_files))
        else:
            from dagayn.search import rebuild_fts_index

            fts_count = rebuild_fts_index(store)
        build_result.fts_indexed = fts_count
        build_result.fts_rebuilt = True
    except (sqlite3.OperationalError, ImportError, RuntimeError, TypeError) as e:
        _warn(warnings, "FTS index rebuild", e)

    try:
        from dagayn.postprocessing import _resolve_bare_name_edges

        _resolve_bare_name_edges(store, post_result, warnings)
    except (sqlite3.OperationalError, ImportError) as e:
        _warn(warnings, "Bare-name edge resolution", e)

    try:
        from dagayn.postprocessing import _resolve_terraform_module_references

        _resolve_terraform_module_references(store, post_result, warnings)
    except (sqlite3.OperationalError, ImportError) as e:
        _warn(warnings, "Terraform module reference resolution", e)

    try:
        from dagayn.postprocessing import _demote_unresolved_endpoint_edges

        _demote_unresolved_endpoint_edges(store, post_result, warnings)
    except (sqlite3.OperationalError, ImportError) as e:
        _warn(warnings, "Unresolved endpoint demotion", e)

    try:
        from dagayn.postprocessing import _resolve_markdown_artifact_refs

        _resolve_markdown_artifact_refs(store, post_result, warnings)
    except (sqlite3.OperationalError, ImportError) as e:
        _warn(warnings, "Markdown artifact ref resolution", e)

    try:
        from dagayn.postprocessing import _resolve_terraform_artifact_refs

        _resolve_terraform_artifact_refs(store, post_result, warnings)
    except (sqlite3.OperationalError, ImportError) as e:
        _warn(warnings, "Terraform artifact ref resolution", e)

    try:
        from dagayn.postprocessing import _apply_manifest_bridges

        _apply_manifest_bridges(store, post_result, warnings, changed_files)
    except (sqlite3.OperationalError, ImportError) as e:
        _warn(warnings, "Manifest bridge extraction", e)

    try:
        from dagayn.postprocessing import _resolve_native_bindings

        _resolve_native_bindings(store, post_result, warnings)
    except (sqlite3.OperationalError, ImportError, RuntimeError) as e:
        _warn(warnings, "Native binding resolution", e)

    if postprocess != "minimal":
        # -- Expensive: communities + summaries (only for "full") --
        incremental = not full_rebuild
        _detect_communities(
            store,
            post_result,
            warnings,
            incremental,
            changed_files,
            pre_affected_communities,
        )
        try:
            _compute_summaries(store)
            build_result.summaries_computed = True
        except (sqlite3.OperationalError, RuntimeError, Exception) as e:
            _warn(warnings, "Summary computation", e)

    # File re-parses invalidate hub_scores / bridge_scores wholesale (see
    # remove_files_data_tx), so every non-none postprocess level must
    # recompute them or the tables stay empty after skip-flows updates.
    from dagayn.postprocessing import _persist_centrality_scores

    _persist_centrality_scores(
        store,
        post_result,
        warnings,
        changed_files if not full_rebuild else None,
    )

    warnings.extend(_prune_orphaned_structures(store, build_result))

    # Recorded on every non-none level: leaving the previous run's
    # ``postprocess_level`` in place made a graph whose communities had just
    # been pruned still advertise itself as fully post-processed.
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
    """Populate the community_summaries and risk_index tables."""
    store.compute_summaries()


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
    embed_files: list[str] | None = None,
    scip: bool = False,
) -> BuildPayload:
    """Build or incrementally update the code knowledge graph.

    Args:
        full_rebuild: If True, re-parse every file. If False (default),
                      only re-parse files changed since ``base``.
        repo_root: Path to the repository root. Auto-detected if omitted.
        base: Git ref for incremental diff (default: HEAD~1).
        scip: On a full build, settle ``CALLS`` edges by the SCIP indexers
            available for the repository's languages before post-processing
            (see ``dagayn.scip_overlay``).
        extra_files: Files to re-index in addition to the git diff, for
            content drift the diff cannot see (see ``incremental_update``).
        postprocess: Post-processing level after build:
            ``"full"`` (default) — signatures, FTS, communities.
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
        embed_files: If set, only these files' nodes are hash-checked and
            (re)embedded. When omitted, an incremental update with file
            changes scopes itself to ``changed_files`` plus
            ``dependent_files``; a full rebuild or a no-change incremental
            still scans the whole corpus.

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
    store, root = _get_store(repo_root, cached=False)
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
            from dagayn.incremental_files import get_changed_file_sources

            pre_affected_communities = 0
            change_file_sources = get_changed_file_sources(root, base)
            preview_changed = list(change_file_sources.get("files", []))
            if extra_files:
                preview_changed = list(dict.fromkeys([*preview_changed, *extra_files]))
            if preview_changed:
                pre_affected_communities = count_affected_communities(store, preview_changed)
            build_result = incremental_update(
                root,
                store,
                base=base,
                extra_files=extra_files,
                change_file_sources=change_file_sources,
            )
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

        # Pass changed_files for incremental community detection.
        changed = build_result.changed_files if not full_rebuild else None
        scip_warnings: list[str] = []
        if scip and full_rebuild and not no_changes:
            from dagayn.scip_overlay import run_scip_overlay

            scip_report = run_scip_overlay(store, Path(root), Path(db_path).parent / "scip")
            build_result.scip_overlay = scip_report.runs
            build_result.scip_hints = scip_report.hints or None
            scip_warnings = scip_report.warnings
        if not no_changes:
            warnings = _run_postprocess(
                store,
                build_result,
                postprocess,
                full_rebuild=full_rebuild,
                changed_files=changed,
                pre_affected_communities=pre_affected_communities,
            )
            if warnings:
                build_result.warnings = warnings
            if scip_warnings:
                build_result.warnings = [*(build_result.warnings or []), *scip_warnings]
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
                scope = embed_files
                if (
                    scope is None
                    and build_result.build_type == "incremental"
                    and int(build_result.files_updated or 0) > 0
                ):
                    scope = (
                        list(
                            dict.fromkeys(
                                [
                                    *(build_result.changed_files or []),
                                    *(build_result.dependent_files or []),
                                ]
                            )
                        )
                        or None
                    )
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
                    file_paths=scope,
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


def run_embedding_pass(
    repo_root: str | None = None,
    *,
    local_embedding: str = "bge-m3",
    local_embedding_mode: str | None = None,
    local_embedding_port: int | None = None,
    local_embedding_bin: str = "auto",
    keep_local_embedding_server: bool = False,
    local_embedding_timeout: int = 300,
    local_embedding_request_timeout: int = 60,
    local_embedding_batch_size: int = 1,
    embed_pass_seconds: float | None = None,
    embed_files: list[str] | None = None,
) -> BuildPayload:
    """Embed against the graph as it stands, without a structural update first.

    For callers that have just run that update themselves (the queue's
    edit-triggered ``update`` task): :func:`build_or_update_graph` would repeat
    the git diff and the scope resolution only to find nothing changed. The
    embedding slices take the graph lock on their own.
    """
    root = _resolve_write_root(repo_root)
    local_result = _run_local_embedding(
        root,
        local_embedding=local_embedding,
        local_embedding_mode=local_embedding_mode,
        local_embedding_port=local_embedding_port,
        local_embedding_bin=local_embedding_bin,
        keep_local_embedding_server=keep_local_embedding_server,
        local_embedding_timeout=local_embedding_timeout,
        local_embedding_request_timeout=local_embedding_request_timeout,
        local_embedding_batch_size=local_embedding_batch_size,
        pass_seconds=embed_pass_seconds,
        file_paths=embed_files,
    )
    return build_result_payload(
        BuildResult(
            status="ok",
            summary=str(local_result.get("summary") or ""),
            local_embedding=local_result,
        )
    )


def run_postprocess(
    communities: bool = True,
    fts: bool = True,
    repo_root: str | None = None,
) -> BuildPayload:
    """Run post-processing steps on an existing graph.

    Useful for running expensive steps (communities) separately
    from the build, or for re-running after the graph has been updated
    with ``postprocess="none"``.

    Args:
        communities: Run community detection. Default: True.
        fts: Rebuild FTS index. Default: True.
        repo_root: Repository root path. Auto-detected if omitted.

    Returns:
        Summary of what was computed.
    """
    # Postprocess writes to communities / FTS — bypass the
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
            store.compute_missing_signatures()
            result.signatures_updated = True
        except (sqlite3.OperationalError, RuntimeError, TypeError, KeyError) as e:
            _warn(warnings, "Signature computation", e)

        if fts:
            try:
                from dagayn.search import rebuild_fts_index

                fts_count = rebuild_fts_index(store)
                result.fts_indexed = fts_count
            except (sqlite3.OperationalError, ImportError) as e:
                store.rollback()
                _warn(warnings, "FTS index rebuild", e)

        if communities:
            try:
                from dagayn.communities import detect_communities, store_communities

                count = store_communities(store, detect_communities(store))
                result.postprocess.communities_detected = count
            except (sqlite3.OperationalError, ImportError) as e:
                store.rollback()
                _warn(warnings, "Community detection", e)

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
