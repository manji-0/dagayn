"""Phased execution for ``incremental_update``."""

from __future__ import annotations

import concurrent.futures
import hashlib
import logging
import os
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Callable, Optional, cast

from .contracts.state_types import BuildResult
from .extractor_versions import (
    files_for_extractors,
    outdated_extractors,
    record_extractor_versions,
)
from .graph import GraphStore
from .incremental_build import (
    _GRAPH_STORE_ERRORS,
    _MAX_PARSE_WORKERS,
    _PARSE_FILE_ERRORS,
    BULK_LOAD_FILE_THRESHOLD,
    _classify_python_changed_files,
    _diff_covers_graph_commit,
    _expand_changed_submodules,
    _filter_incremental_candidates,
    _flush_store_batch,
    _get_file_meta_for_candidates,
    _indexable_scope,
    _indexed_only,
    _init_worker,
    _is_ignore_scope_file,
    _parse_single_python_file_compact,
    _queue_store_file,
    _split_rust_parser_files,
    _store_rust_parse_batches,
    _StoreBulkLoad,
    find_dependents_for_files,
    store_phase_failures,
)
from .incremental_files import (
    _dedupe_preserve_order,
    _is_binary,
    _make_repo_relative,
    _relativize_parsed_entities,
    _store_vcs_metadata,
    get_changed_file_sources,
)
from .parser import CodeParser
from .parser._base.types import EdgeInfo, NodeInfo
from .parser.ignore import _load_ignore_patterns, _should_ignore
from .worktree import is_gitignored

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
    #: In ``find_dependents_for_files`` order (hop distance, then path).
    dependent_files: list[str] = field(default_factory=list)
    content_changed_files: set[str] = field(default_factory=set)
    all_files: set[str] = field(default_factory=set)
    candidates: list[str] = field(default_factory=list)
    rust_content_changed_files: set[str] = field(default_factory=set)
    to_parse_rust_forced: list[str] = field(default_factory=list)
    #: Extractors whose stored output version is behind the running parser.
    outdated_extractors: list[str] = field(default_factory=list)
    #: Indexed files those extractors own: re-parsed whether or not they changed.
    extractor_reparse_files: list[str] = field(default_factory=list)
    #: Rust-owned subset of ``extractor_reparse_files`` (parsed unconditionally).
    to_parse_rust_full: list[str] = field(default_factory=list)
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


def _extractor_reparse_scope(
    repo_root: Path,
    store: GraphStore,
    indexable: set[str],
) -> tuple[list[str], list[str]]:
    """Return ``(outdated_extractors, indexable files they own)``.

    A graph parsed by an older extractor keeps that extractor's output for
    every file that has not changed since, so those files are re-parsed once.
    Files the graph does not hold yet are included: a new version can take
    on an extension the old one did not parse (Objective-C++ `.mm`).
    """
    outdated = outdated_extractors(store)
    if not outdated:
        return [], []
    files = files_for_extractors(repo_root, indexable, outdated)
    if files:
        logger.info(
            "Re-parsing %d file(s) produced by an older %s extractor",
            len(files),
            "/".join(outdated),
        )
    return outdated, files


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
    change_file_sources: dict[str, list[str]] | None = None,
) -> IncrementalUpdateState | BuildResult:
    """Resolve changed files and return state, or an early no-op result."""
    repo_root = repo_root.resolve()
    store.set_metadata("repo_root", str(repo_root))
    ignore_patterns = _load_ignore_patterns(repo_root)

    diff_covers_graph = False
    if changed_files is None:
        change_file_sources = (
            dict(change_file_sources)
            if change_file_sources is not None
            else get_changed_file_sources(repo_root, base)
        )
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
    outdated, extractor_files = _extractor_reparse_scope(repo_root, store, indexable)
    if outdated and not extractor_files:
        # Nothing the outdated extractors own is indexed: the stamp is all
        # that is stale.
        record_extractor_versions(store)
        store.commit()
        outdated = []
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
        outdated_extractors=outdated,
        extractor_reparse_files=extractor_files,
    )
    if not state.changed_files and not stale_scope and not extractor_files:
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
    state.changed_files = _expand_changed_submodules(state.repo_root, state.changed_files)
    state.change_file_sources["files"] = state.changed_files

    changed_candidates, removed_files = _filter_incremental_candidates(
        state.repo_root,
        sorted(set(state.changed_files)),
        state.ignore_patterns,
    )
    changed_candidates = [path for path in changed_candidates if path in state.indexable]
    removed_files.extend(path for path in state.changed_files if path not in state.indexable)
    removed_files = _indexed_only(state.store, removed_files)
    removed_files = _dedupe_preserve_order([*removed_files, *state.stale_scope])
    rust_changed_candidates, python_changed_candidates = _split_rust_parser_files(
        changed_candidates,
        state.repo_root,
    )
    if rust_changed_candidates:
        rust_changed, raw_errors = state.store.classify_changed_rust_owned_files(
            state.repo_root,
            rust_changed_candidates,
        )
        state.rust_content_changed_files.update(rust_changed)
        state.content_changed_files.update(rust_changed)
        state.errors.extend(
            {"file": str(file_path), "error": str(error)} for file_path, error in raw_errors
        )

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
    # Ordered lists, not sets: the parse order assigns the new rows' ids, so
    # walking these in hash order stored the same change differently in each
    # process (the Rust update walks them sorted).
    state.dependent_files = _dedupe_preserve_order(
        [
            _make_repo_relative(dep, state.repo_root)
            for dep in find_dependents_for_files(state.store, dependency_roots)
        ]
    )
    state.all_files = state.content_changed_files | set(removed_files) | set(state.dependent_files)

    if state.dependent_files:
        candidates, extra_removed = _filter_incremental_candidates(
            state.repo_root,
            sorted(state.all_files),
            state.ignore_patterns,
        )
        candidates = [path for path in candidates if path in state.indexable]
        extra_removed.extend(path for path in state.all_files if path not in state.indexable)
        extra_removed = _indexed_only(state.store, extra_removed)
        state.removed_files = _dedupe_preserve_order([*removed_files, *extra_removed])
        state.candidates = candidates
    else:
        state.removed_files = removed_files
        state.candidates = [
            path for path in sorted(state.content_changed_files) if path in state.indexable
        ]


def plan_incremental_reparses(state: IncrementalUpdateState) -> None:
    """Build rust/python reparse queues from classified candidates."""
    rust_content_changed_files = state.rust_content_changed_files
    if state.extractor_reparse_files:
        state.all_files |= set(state.extractor_reparse_files)
        # Files whose content changed are re-parsed by the normal path below;
        # the rest have unchanged content and would be skipped there.
        full = [
            path
            for path in state.extractor_reparse_files
            if path not in state.content_changed_files
        ]
        full_set = set(full)
        state.candidates = [path for path in state.candidates if path not in full_set]
        rust_forced, python_forced = _split_rust_parser_files(full, state.repo_root)
        state.to_parse_rust_full.extend(rust_forced)
        for rel_path in python_forced:
            try:
                mtime_ns = int((state.repo_root / rel_path).stat().st_mtime_ns)
            except (OSError, PermissionError):
                mtime_ns = 0
            state.to_parse.append((rel_path, mtime_ns))
    file_meta = _get_file_meta_for_candidates(state.store, state.candidates)
    rust_candidates, python_candidates = _split_rust_parser_files(
        state.candidates,
        state.repo_root,
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


def _state_result(state: IncrementalUpdateState) -> BuildResult:
    return BuildResult(
        files_updated=len(state.all_files),
        total_nodes=state.total_nodes,
        total_edges=state.total_edges,
        changed_files=list(state.changed_files),
        change_file_sources=state.change_file_sources,
        dependent_files=list(state.dependent_files),
        errors=state.errors,
    )


def apply_incremental_graph_mutations(state: IncrementalUpdateState) -> BuildResult | None:
    """Apply deletions and mtime-only updates; return early if nothing left to parse."""
    state.store.remove_files_data(state.removed_files)

    if state.removed_files or state.mtime_only_updates:
        if state.mtime_only_updates:
            state.store.update_file_mtimes(state.mtime_only_updates)
        state.store.commit()

    if (
        not state.removed_files
        and not state.to_parse_rust_forced
        and not state.to_parse_rust_checked
        and not state.to_parse_rust_full
        and not state.to_parse
    ):
        _record_incremental_head_when_verified(
            repo_root=state.repo_root,
            store=state.store,
            diff_covers_graph=state.diff_covers_graph,
            store_failures=state.store_failures,
        )
        return _state_result(state)
    return None


def run_incremental_parsing(state: IncrementalUpdateState) -> None:
    """Parse rust and python file batches."""
    parse_files = (
        len(state.to_parse)
        + len(state.to_parse_rust_forced)
        + len(state.to_parse_rust_checked)
        + len(state.to_parse_rust_full)
    )
    if parse_files >= BULK_LOAD_FILE_THRESHOLD:
        with _StoreBulkLoad(state.store):
            _run_incremental_parsing_body(state)
        return
    _run_incremental_parsing_body(state)


def _run_incremental_parsing_body(state: IncrementalUpdateState) -> None:
    """Parse rust and python file batches without toggling bulk-load."""
    use_serial = os.environ.get("CRG_SERIAL_PARSE", "") == "1"
    to_parse_mtime = dict(state.to_parse)

    if state.to_parse_rust_full:
        # Unchanged content, older extractor: parse without the hash check the
        # changed-file paths below apply.
        rust_nodes, rust_edges, rust_errors = _store_rust_parse_batches(
            state.repo_root,
            state.store,
            state.to_parse_rust_full,
        )
        state.total_nodes += rust_nodes
        state.total_edges += rust_edges
        state.errors.extend(rust_errors)

    for rust_batch in (state.to_parse_rust_forced, state.to_parse_rust_checked):
        if not rust_batch:
            continue
        rust_nodes, rust_edges, raw_errors = state.store.store_changed_rust_owned_files(
            state.repo_root,
            rust_batch,
        )
        rust_errors = [
            {"file": str(file_path), "error": str(error)} for file_path, error in raw_errors
        ]
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
    with concurrent.futures.ProcessPoolExecutor(
        max_workers=_MAX_PARSE_WORKERS,
        initializer=_init_worker,
    ) as executor:
        for rel_path, nodes, edges, error, fhash, mtime_ns in executor.map(
            _parse_single_python_file_compact,
            args_list,
            chunksize=20,
        ):
            if error:
                logger.warning("Error parsing %s: %s", rel_path, error)
                state.errors.append({"file": rel_path, "error": error})
                continue
            _queue_store_file(state.store, batch, rel_path, nodes, edges, fhash, mtime_ns)
            state.total_nodes += len(nodes)
            state.total_edges += len(edges)
    _flush_store_batch(state.store, batch)


def finalize_incremental_update(state: IncrementalUpdateState) -> BuildResult:
    """Persist metadata and build the incremental update result payload."""
    state.store.set_metadata("last_updated", time.strftime("%Y-%m-%dT%H:%M:%S"))
    state.store.set_metadata("last_build_type", "incremental")
    state.store_failures.extend(store_phase_failures(state.errors))
    if not state.store_failures:
        # Every file an outdated extractor owns was re-parsed above (and any
        # extractor that was not outdated already matches the stamp).
        record_extractor_versions(state.store)
    if state.diff_covers_graph and not state.store_failures:
        _store_vcs_metadata(state.repo_root, state.store)
    elif state.store_failures:
        logger.error(
            "Not recording git_head_sha: %d file(s) failed to store",
            len(state.store_failures),
        )
    state.store.commit()

    result = _state_result(state)
    if state.store_failures:
        result.store_failed_files = state.store_failures
        result.status = "partial"
    return result


def incremental_update(
    repo_root: Path,
    store: GraphStore,
    base: str = "HEAD~1",
    changed_files: list[str] | None = None,
    extra_files: list[str] | None = None,
    change_file_sources: dict[str, list[str]] | None = None,
) -> BuildResult:
    """Incremental update: re-parse changed + dependent files only.

    *change_file_sources* is a ``get_changed_file_sources(repo_root, base)``
    result the caller already has, so the git diff and status are not run a
    second time. It is ignored when *changed_files* is given.

    *extra_files* are re-indexed on top of whatever the git diff reports. A
    file whose on-disk content matches ``base`` cannot appear in that diff, so
    content drift found by the diff tier of ``assess_graph_sync`` (a phantom
    node inherited from a seeded worktree, or an edit indexed and then
    discarded) is otherwise unreachable from here: the state that prescribes an
    update is one the update itself can never clear.
    """
    prepared = prepare_incremental_update(
        repo_root,
        store,
        base=base,
        changed_files=changed_files,
        extra_files=extra_files,
        change_file_sources=change_file_sources,
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


# ---------------------------------------------------------------------------
# Watch mode
# ---------------------------------------------------------------------------


_DEBOUNCE_SECONDS = 0.3
#: Upper bound on how long the debounce may be pushed out by further events.
#: Without it, sustained churn reset the timer forever and the graph was never
#: updated while writes kept arriving.
_MAX_DEBOUNCE_SECONDS = float(os.environ.get("DAGAYN_WATCH_MAX_DEBOUNCE_SECONDS", "5"))


def _idle() -> None:
    """One tick of the watch loop's wait for Ctrl+C (patched in tests)."""
    time.sleep(1)


def watch(
    repo_root: Path,
    store: GraphStore,
    on_files_updated: Optional[Callable] = None,
) -> None:
    """Watch for file changes and auto-update the graph.

    Uses a 300ms debounce to batch rapid-fire saves into a single update.

    Args:
        repo_root: Repository root to watch.
        store: Graph database to update.
        on_files_updated: Optional callback invoked after each debounced
            batch of file updates completes.  Receives the store as its
            only argument.  Used by the CLI to run post-processing
            (FTS, flows, communities) after watch updates.
    """
    import threading

    from watchdog.events import FileSystemEventHandler
    from watchdog.observers import Observer

    parser = CodeParser()
    repo_root = repo_root.resolve()
    store.set_metadata("repo_root", str(repo_root))
    scope = {"ignore_patterns": _load_ignore_patterns(repo_root)}

    class GraphUpdateHandler(FileSystemEventHandler):
        def __init__(self):
            self._pending: set[str] = set()
            self._lock = threading.Lock()
            self._timer: threading.Timer | None = None
            self._first_pending_at: float | None = None

        def _should_handle(self, path: str) -> bool:
            if Path(path).is_symlink():
                return False
            try:
                rel = str(Path(path).relative_to(repo_root))
            except ValueError:
                return False
            if _is_ignore_scope_file(rel):
                return True
            if is_gitignored(repo_root, rel):
                return False
            if _should_ignore(rel, scope["ignore_patterns"]):
                return False
            if parser.detect_language(Path(path)) is None:
                return False
            return True

        def on_modified(self, event):
            if event.is_directory:
                return
            if self._should_handle(event.src_path):
                self._schedule(event.src_path)

        on_created = on_modified

        def on_deleted(self, event):
            if event.is_directory:
                return
            try:
                rel = str(Path(event.src_path).relative_to(repo_root))
            except ValueError:
                return
            if _is_ignore_scope_file(rel):
                self._schedule(event.src_path)
                return
            if is_gitignored(repo_root, rel):
                return
            if _should_ignore(rel, scope["ignore_patterns"]):
                return
            try:
                store.remove_file_data(rel)
                # Derived rows are keyed on node ids, so dropping the nodes
                # leaves flow memberships and community assignments dangling.
                # Pruning only ran from ``dagayn build``, so under watch/serve a
                # deleted package's communities survived with size N and zero
                # assigned members until the next full build.
                store.prune_orphaned_graph_structures()
                store.commit()
                logger.info("Removed: %s", rel)
            except _GRAPH_STORE_ERRORS as e:
                logger.error("Error removing %s: %s", rel, e)

        def _schedule(self, abs_path: str):
            """Add file to pending set and reset the debounce timer.

            The reset is capped by ``_MAX_DEBOUNCE_SECONDS`` from the *first*
            pending event: with an uncapped reset, sustained churn (a large
            ``git checkout``, a bundler write loop, a formatter pass) kept
            pushing the deadline out and the graph was never updated.
            """
            with self._lock:
                now = time.monotonic()
                if not self._pending:
                    self._first_pending_at = now
                self._pending.add(abs_path)
                deadline = (self._first_pending_at or now) + _MAX_DEBOUNCE_SECONDS
                delay = max(0.0, min(_DEBOUNCE_SECONDS, deadline - now))
                if self._timer is not None:
                    self._timer.cancel()
                self._timer = threading.Timer(delay, self._flush)
                self._timer.start()

        def _flush(self):
            """Process all pending files after the debounce window."""
            with self._lock:
                paths = list(self._pending)
                self._pending.clear()
                self._first_pending_at = None
                self._timer = None

            rels: list[str] = []
            for abs_path in paths:
                path = Path(abs_path)
                try:
                    rel = str(path.relative_to(repo_root))
                except ValueError:
                    continue
                if _is_ignore_scope_file(rel):
                    rels.append(rel)
                    continue
                if not path.is_file() or path.is_symlink() or _is_binary(path):
                    continue
                rels.append(rel)
            rels = sorted(set(rels))
            if any(_is_ignore_scope_file(rel) for rel in rels):
                scope["ignore_patterns"] = _load_ignore_patterns(repo_root)
            updated = 0
            if rels:
                try:
                    result = incremental_update(repo_root, store, changed_files=rels)
                    updated = result.files_updated or 0
                except _GRAPH_STORE_ERRORS as e:
                    logger.error("Error updating watched files %s: %s", rels, e)

            if updated > 0 and on_files_updated is not None:
                try:
                    on_files_updated(store)
                except (OSError, RuntimeError, ValueError, TypeError) as e:
                    logger.error("Post-update callback failed: %s", e)

    handler = GraphUpdateHandler()
    observer = Observer()
    observer.schedule(handler, str(repo_root), recursive=True)
    observer.start()

    logger.info("Watching %s for changes... (Ctrl+C to stop)", repo_root)
    try:
        while True:
            _idle()
    except KeyboardInterrupt:
        observer.stop()
    observer.join()
    logger.info("Watch stopped.")
