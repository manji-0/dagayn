"""Phased execution for ``incremental_update``."""

from __future__ import annotations

import concurrent.futures
import hashlib
import logging
import os
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, cast

from .graph import GraphStore
from .incremental_files import (
    _dedupe_preserve_order,
    _load_ignore_patterns,
    _make_repo_relative,
    _relativize_parsed_entities,
    _store_vcs_metadata,
)
from .parser import CodeParser
from .parser._base.types import EdgeInfo, NodeInfo
from .state_types import BuildResult

logger = logging.getLogger(__name__)


@dataclass
class IncrementalUpdateState:
    repo_root: Path
    store: GraphStore
    base: str
    ignore_patterns: list[str]
    change_file_sources: dict[str, list[str]]
    changed_files: list[str]
    diff_covers_graph: bool
    indexable: set[str]
    stale_scope: list[str]
    store_failures: list[str] = field(default_factory=list)
    removed_files: list[str] = field(default_factory=list)
    dependent_files: set[str] = field(default_factory=set)
    content_changed_files: set[str] = field(default_factory=set)
    all_files: set[str] = field(default_factory=set)
    candidates: list[str] = field(default_factory=list)
    rust_content_changed_files: set[str] = field(default_factory=set)
    to_parse_rust_forced: list[str] = field(default_factory=list)
    to_parse_rust_checked: list[str] = field(default_factory=list)
    to_parse: list[tuple[str, int]] = field(default_factory=list)
    mtime_only_updates: list[tuple[int, str]] = field(default_factory=list)
    total_nodes: int = 0
    total_edges: int = 0
    errors: list[dict[str, str]] = field(default_factory=list)


def _record_incremental_head_when_verified(
    *,
    repo_root: Path,
    store: GraphStore,
    diff_covers_graph: bool,
    store_failures: list[str],
) -> None:
    """Stamp ``git_head_sha`` = HEAD when the diff fully covered the graph commit."""
    if not diff_covers_graph:
        return
    if store_failures:
        logger.error(
            "Not recording git_head_sha: %d file(s) failed to store, so the graph "
            "does not describe HEAD",
            len(store_failures),
        )
        return
    _store_vcs_metadata(repo_root, store)
    store.commit()


def _noop_incremental_result(state: IncrementalUpdateState) -> BuildResult:
    return BuildResult(
        files_updated=0,
        total_nodes=0,
        total_edges=0,
        changed_files=[],
        change_file_sources=state.change_file_sources,
        dependent_files=[],
    )


def prepare_incremental_update(
    repo_root: Path,
    store: GraphStore,
    *,
    base: str = "HEAD~1",
    changed_files: list[str] | None = None,
    extra_files: list[str] | None = None,
) -> IncrementalUpdateState | BuildResult:
    """Resolve changed files and return state, or an early no-op result."""
    from .incremental_build import (
        _changed_file_sources,
        _diff_covers_graph_commit,
        _indexable_scope,
    )

    repo_root = repo_root.resolve()
    store.set_metadata("repo_root", str(repo_root))
    ignore_patterns = _load_ignore_patterns(repo_root)

    change_file_sources: dict[str, list[str]]
    diff_covers_graph = False
    if changed_files is None:
        change_file_sources = _changed_file_sources(repo_root, base)
        changed_files = change_file_sources["files"]
        diff_covers_graph = _diff_covers_graph_commit(repo_root, store, base)
    else:
        change_file_sources = {"files": changed_files, "explicit": changed_files}
    if extra_files:
        forced = [path for path in dict.fromkeys(extra_files) if path not in set(changed_files)]
        if forced:
            changed_files = [*changed_files, *forced]
            change_file_sources["files"] = changed_files
            change_file_sources["content_drift"] = forced

    indexable, stale_scope = _indexable_scope(repo_root, store)
    state = IncrementalUpdateState(
        repo_root=repo_root,
        store=store,
        base=base,
        ignore_patterns=ignore_patterns,
        change_file_sources=change_file_sources,
        changed_files=list(changed_files),
        diff_covers_graph=diff_covers_graph,
        indexable=indexable,
        stale_scope=stale_scope,
    )
    if not state.changed_files and not stale_scope:
        _record_incremental_head_when_verified(
            repo_root=repo_root,
            store=store,
            diff_covers_graph=diff_covers_graph,
            store_failures=state.store_failures,
        )
        return _noop_incremental_result(state)
    return state


def classify_incremental_changes(state: IncrementalUpdateState) -> None:
    """Classify changed roots, dependents, and content vs mtime-only updates."""
    from .incremental_build import (
        _callable_store_attr,
        _classify_python_changed_files,
        _expand_changed_submodules,
        _filter_incremental_candidates,
        _get_file_meta_for_candidates,
        _indexed_only,
        _split_rust_parser_files,
        find_dependents_for_files,
    )

    state.changed_files = _expand_changed_submodules(state.repo_root, state.changed_files)
    state.change_file_sources["files"] = state.changed_files

    changed_candidates, removed_files = _filter_incremental_candidates(
        state.repo_root,
        set(state.changed_files),
        state.ignore_patterns,
    )
    changed_candidates = [path for path in changed_candidates if path in state.indexable]
    removed_files.extend(path for path in state.changed_files if path not in state.indexable)
    removed_files = _indexed_only(state.store, removed_files)
    removed_files = _dedupe_preserve_order([*removed_files, *state.stale_scope])
    rust_changed_candidates, python_changed_candidates = _split_rust_parser_files(
        changed_candidates,
        state.repo_root,
        state.store,
    )
    if rust_changed_candidates:
        classify_changed_rust_owned_files = _callable_store_attr(
            state.store,
            "classify_changed_rust_owned_files",
        )
        if classify_changed_rust_owned_files is not None:
            rust_changed, raw_errors = classify_changed_rust_owned_files(
                state.repo_root,
                rust_changed_candidates,
            )
            state.rust_content_changed_files.update(rust_changed)
            state.content_changed_files.update(rust_changed)
            state.errors.extend(
                {"file": str(file_path), "error": str(error)} for file_path, error in raw_errors
            )
        else:
            state.content_changed_files.update(rust_changed_candidates)

    if python_changed_candidates:
        changed_file_meta = _get_file_meta_for_candidates(
            state.store,
            python_changed_candidates,
        )
        python_changed, python_mtime_updates = _classify_python_changed_files(
            state.repo_root,
            python_changed_candidates,
            changed_file_meta,
            trust_mtime=False,
        )
        state.content_changed_files.update(python_changed)
        state.mtime_only_updates.extend(python_mtime_updates)

    dependency_roots = set(removed_files) | state.content_changed_files
    state.dependent_files = {
        _make_repo_relative(dep, state.repo_root)
        for dep in find_dependents_for_files(state.store, dependency_roots)
    }
    state.all_files = state.content_changed_files | set(removed_files) | state.dependent_files

    if state.dependent_files:
        candidates, extra_removed = _filter_incremental_candidates(
            state.repo_root,
            state.all_files,
            state.ignore_patterns,
        )
        candidates = [path for path in candidates if path in state.indexable]
        extra_removed.extend(path for path in state.all_files if path not in state.indexable)
        extra_removed = _indexed_only(state.store, extra_removed)
        state.removed_files = _dedupe_preserve_order([*removed_files, *extra_removed])
        state.candidates = candidates
    else:
        state.removed_files = removed_files
        state.candidates = [path for path in state.content_changed_files if path in state.indexable]


def plan_incremental_reparses(state: IncrementalUpdateState) -> None:
    """Build rust/python reparse queues from classified candidates."""
    from .incremental_build import (
        _get_file_meta_for_candidates,
        _split_rust_parser_files,
    )

    rust_content_changed_files = state.rust_content_changed_files
    file_meta = _get_file_meta_for_candidates(state.store, state.candidates)
    rust_candidates, python_candidates = _split_rust_parser_files(
        state.candidates,
        state.repo_root,
        state.store,
    )

    for rel_path in rust_candidates:
        if rel_path in rust_content_changed_files:
            state.to_parse_rust_forced.append(rel_path)
            continue
        abs_path = state.repo_root / rel_path
        try:
            cur_mtime_ns = int(abs_path.stat().st_mtime_ns)
        except (OSError, PermissionError):
            state.to_parse_rust_checked.append(rel_path)
            continue
        meta = file_meta.get(rel_path)
        if meta and meta[1] == cur_mtime_ns:
            continue
        state.to_parse_rust_checked.append(rel_path)

    for rel_path in python_candidates:
        abs_path = state.repo_root / rel_path
        already_known_changed = rel_path in state.content_changed_files
        try:
            cur_mtime_ns = int(abs_path.stat().st_mtime_ns)
            meta = file_meta.get(rel_path)
            if not already_known_changed and meta and meta[1] == cur_mtime_ns:
                continue
            raw = abs_path.read_bytes()
            fhash = hashlib.sha256(raw).hexdigest()
            if meta and meta[0] == fhash:
                state.mtime_only_updates.append((cur_mtime_ns, rel_path))
                continue
        except (OSError, PermissionError):
            cur_mtime_ns = 0
        state.to_parse.append((rel_path, cur_mtime_ns))


def apply_incremental_graph_mutations(state: IncrementalUpdateState) -> BuildResult | None:
    """Apply deletions and mtime-only updates; return early if nothing left to parse."""
    state.store.remove_files_data(state.removed_files)

    if state.removed_files or state.mtime_only_updates:
        if state.mtime_only_updates:
            if hasattr(state.store, "update_file_mtimes"):
                state.store.update_file_mtimes(state.mtime_only_updates)
            elif hasattr(state.store, "update_file_mtime"):
                for mtime_ns, file_path in state.mtime_only_updates:
                    state.store.update_file_mtime(file_path, mtime_ns)
            elif hasattr(state.store, "_conn"):
                state.store._conn.executemany(
                    "UPDATE nodes SET mtime_ns=? WHERE file_path=?",
                    state.mtime_only_updates,
                )
        state.store.commit()

    if (
        not state.removed_files
        and not state.to_parse_rust_forced
        and not state.to_parse_rust_checked
        and not state.to_parse
    ):
        _record_incremental_head_when_verified(
            repo_root=state.repo_root,
            store=state.store,
            diff_covers_graph=state.diff_covers_graph,
            store_failures=state.store_failures,
        )
        return BuildResult(
            files_updated=len(state.all_files),
            total_nodes=state.total_nodes,
            total_edges=state.total_edges,
            changed_files=list(state.changed_files),
            change_file_sources=state.change_file_sources,
            dependent_files=list(state.dependent_files),
            errors=state.errors,
        )
    return None


def run_incremental_parsing(state: IncrementalUpdateState) -> None:
    """Parse rust and python file batches."""
    from .incremental_build import (
        _MAX_PARSE_WORKERS,
        _PARSE_FILE_ERRORS,
        _callable_store_attr,
        _flush_store_batch,
        _init_worker,
        _parse_single_python_file,
        _parse_single_python_file_compact,
        _queue_store_file,
        _store_rust_parse_batches,
        _uses_compact_entities,
    )

    use_serial = os.environ.get("CRG_SERIAL_PARSE", "") == "1"
    to_parse_mtime = dict(state.to_parse)
    store_changed_rust_owned_files = _callable_store_attr(
        state.store,
        "store_changed_rust_owned_files",
    )

    for rust_batch in (state.to_parse_rust_forced, state.to_parse_rust_checked):
        if not rust_batch:
            continue
        if store_changed_rust_owned_files is not None:
            rust_nodes, rust_edges, raw_errors = store_changed_rust_owned_files(
                state.repo_root,
                rust_batch,
            )
            rust_errors = [
                {"file": str(file_path), "error": str(error)} for file_path, error in raw_errors
            ]
        else:
            rust_nodes, rust_edges, rust_errors = _store_rust_parse_batches(
                state.repo_root,
                state.store,
                rust_batch,
            )
        state.total_nodes += rust_nodes
        state.total_edges += rust_edges
        state.errors.extend(rust_errors)

    if use_serial or len(state.to_parse) < 8:
        batch: list[tuple[str, list[Any], list[Any], str, int]] = []
        if state.to_parse:
            parser = CodeParser()
            for rel_path, _ in state.to_parse:
                mtime_ns = to_parse_mtime.get(rel_path, 0)
                abs_path = state.repo_root / rel_path
                try:
                    source = abs_path.read_bytes()
                    fhash = hashlib.sha256(source).hexdigest()
                    nodes, edges = parser.parse_bytes(abs_path, source)
                    nodes, edges = _relativize_parsed_entities(
                        cast(list[NodeInfo], nodes),
                        cast(list[EdgeInfo], edges),
                        state.repo_root,
                    )
                    _queue_store_file(
                        state.store,
                        batch,
                        rel_path,
                        nodes,
                        edges,
                        fhash,
                        mtime_ns,
                    )
                    state.total_nodes += len(nodes)
                    state.total_edges += len(edges)
                except _PARSE_FILE_ERRORS as exc:
                    logger.warning("Error parsing %s: %s", rel_path, exc)
                    state.errors.append({"file": rel_path, "error": str(exc)})
        _flush_store_batch(state.store, batch)
        return

    args_list = [(rel_path, str(state.repo_root)) for rel_path, _ in state.to_parse]
    batch = []
    parse_worker = (
        _parse_single_python_file_compact
        if _callable_store_attr(state.store, "store_file_batch_json") is not None
        else _parse_single_python_file
    )
    with concurrent.futures.ProcessPoolExecutor(
        max_workers=_MAX_PARSE_WORKERS,
        initializer=_init_worker,
    ) as executor:
        for rel_path, nodes, edges, error, fhash, mtime_ns in executor.map(
            parse_worker,
            args_list,
            chunksize=20,
        ):
            if error:
                logger.warning("Error parsing %s: %s", rel_path, error)
                state.errors.append({"file": rel_path, "error": error})
                continue
            if not _uses_compact_entities(nodes, edges):
                nodes, edges = _relativize_parsed_entities(
                    cast(list[NodeInfo], nodes),
                    cast(list[EdgeInfo], edges),
                    state.repo_root,
                )
            _queue_store_file(state.store, batch, rel_path, nodes, edges, fhash, mtime_ns)
            state.total_nodes += len(nodes)
            state.total_edges += len(edges)
    _flush_store_batch(state.store, batch)


def finalize_incremental_update(state: IncrementalUpdateState) -> BuildResult:
    """Persist metadata and build the incremental update result payload."""
    from .incremental_build import store_phase_failures

    state.store.set_metadata("last_updated", time.strftime("%Y-%m-%dT%H:%M:%S"))
    state.store.set_metadata("last_build_type", "incremental")
    state.store_failures.extend(store_phase_failures(state.errors))
    if state.diff_covers_graph and not state.store_failures:
        _store_vcs_metadata(state.repo_root, state.store)
    elif state.store_failures:
        logger.error(
            "Not recording git_head_sha: %d file(s) failed to store",
            len(state.store_failures),
        )
    state.store.commit()

    result = BuildResult(
        files_updated=len(state.all_files),
        total_nodes=state.total_nodes,
        total_edges=state.total_edges,
        changed_files=list(state.changed_files),
        change_file_sources=state.change_file_sources,
        dependent_files=list(state.dependent_files),
        errors=state.errors,
    )
    if state.store_failures:
        result.store_failed_files = state.store_failures
        result.status = "partial"
    return result


def execute_incremental_update(
    repo_root: Path,
    store: GraphStore,
    *,
    base: str = "HEAD~1",
    changed_files: list[str] | None = None,
    extra_files: list[str] | None = None,
) -> BuildResult:
    """Run the incremental update pipeline."""
    prepared = prepare_incremental_update(
        repo_root,
        store,
        base=base,
        changed_files=changed_files,
        extra_files=extra_files,
    )
    if isinstance(prepared, BuildResult):
        return prepared

    state = prepared
    classify_incremental_changes(state)
    plan_incremental_reparses(state)
    early = apply_incremental_graph_mutations(state)
    if early is not None:
        return early

    run_incremental_parsing(state)
    return finalize_incremental_update(state)
