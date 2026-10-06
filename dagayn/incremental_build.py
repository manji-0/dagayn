"""Graph build/update orchestration and parsing pipelines."""

from __future__ import annotations

import concurrent.futures
import hashlib
import json
import logging
import os
import sqlite3
import time
from collections.abc import Collection
from pathlib import Path, PurePosixPath
from typing import Any, cast

from .contracts.state_types import BuildResult
from .extractor_versions import record_extractor_versions
from .graph import GraphStore
from .incremental_files import (
    _MAX_DEPENDENT_FILES,
    _MAX_DEPENDENT_HOPS,
    _dedupe_preserve_order,
    _relativize_parsed_entities,
    _require_rust_backend,
    _store_vcs_metadata,
    collect_all_files,
    get_vcs_indexable_files,
    resolve_commit_sha,
)
from .parser import CodeParser
from .parser._base.types import EdgeInfo, NodeInfo
from .parser.dispatch import detect_language as _detect_parser_language
from .parser.ignore import _load_ignore_patterns

_IGNORE_SCOPE_NAMES = frozenset({".gitignore", ".dagaynignore"})


logger = logging.getLogger(__name__)
_PARSE_FILE_ERRORS = (
    OSError,
    PermissionError,
    UnicodeDecodeError,
    ValueError,
    TypeError,
    RuntimeError,
    SyntaxError,
)
_GRAPH_STORE_ERRORS = (sqlite3.Error, OSError, RuntimeError, ValueError, TypeError)
_MAX_PARSE_WORKERS = int(os.environ.get("CRG_PARSE_WORKERS", str(min(os.cpu_count() or 4, 8))))
_STORE_BATCH_SIZE = int(os.environ.get("DAGAYN_STORE_BATCH_SIZE", "128"))
_RUST_PARSE_BATCH_SIZE = int(os.environ.get("DAGAYN_RUST_PARSE_BATCH_SIZE", "500"))

type ParsedNodes = list[NodeInfo] | list[list[Any]]
type ParsedEdges = list[EdgeInfo] | list[list[Any]]
type WorkerParseResult = tuple[str, ParsedNodes, ParsedEdges, str | None, str, int]
StoreBatch = list[tuple[str, ParsedNodes, ParsedEdges, str, int]]

logger = logging.getLogger(__name__)

_worker_parser: CodeParser | None = None


def _init_worker() -> None:
    global _worker_parser
    _worker_parser = CodeParser()


def _single_hop_dependents(store: GraphStore, file_path: str) -> set[str]:
    """Find files that directly depend on *file_path* (single hop)."""
    return _batch_hop_dependents(store, {file_path})


def _batch_hop_dependents(store: GraphStore, frontier: set[str]) -> set[str]:
    """Find all files that directly depend on any file in *frontier* (batched).

    Replaces N calls to ``_single_hop_dependents`` with 2-3 SQL queries
    regardless of frontier size.
    """
    if not frontier:
        return set()

    return set(store.get_direct_dependents(list(frontier))) - frontier


class DependentList(list[str]):
    """A ``list[str]`` with a ``.truncated`` flag.

    When :func:`find_dependents` hits ``_MAX_DEPENDENT_FILES`` it truncates
    the result and sets ``truncated = True`` so callers can distinguish a
    complete expansion from a capped one.  See issue #261.

    This is a transparent ``list`` subclass — existing callers that iterate,
    ``len()``, or slice continue to work unchanged; only callers that
    specifically check ``.truncated`` benefit from the signal.
    """

    truncated: bool

    def __init__(self, items: list[str], *, truncated: bool = False) -> None:
        super().__init__(items)
        self.truncated = truncated


def find_dependents(
    store: GraphStore,
    file_path: str,
    max_hops: int = _MAX_DEPENDENT_HOPS,
) -> DependentList:
    """Find files that import from or depend on the given file.

    Performs up to *max_hops* iterations of expansion (default 2).
    Stops early if the total exceeds 500 files.

    Returns a :class:`DependentList` — a regular ``list[str]`` that also
    carries a ``.truncated`` flag.  When ``truncated is True`` the
    returned list is capped at ``_MAX_DEPENDENT_FILES`` and the full
    set of dependents was not explored.  See issue #261.
    """
    return find_dependents_for_files(store, [file_path], max_hops=max_hops)


def find_dependents_for_files(
    store: GraphStore,
    file_paths: list[str] | set[str],
    max_hops: int = _MAX_DEPENDENT_HOPS,
) -> DependentList:
    """Find files that depend on any file in *file_paths*.

    Performs multi-source expansion so incremental updates with many changed
    files pay one batched traversal per hop instead of one traversal per file.
    The result is ordered by hop distance and then path; when the
    ``_MAX_DEPENDENT_FILES`` cap truncates it, the files closest to the roots
    are kept.
    """
    roots = set(file_paths)
    if not roots:
        return DependentList([])
    # Ordered by hop distance, then path, so the cap below keeps the closest
    # dependents and the same graph always yields the same list.
    ordered: list[str] = []
    visited: set[str] = set(roots)
    frontier: set[str] = set(roots)
    for _hop in range(max_hops):
        new_deps = _batch_hop_dependents(store, frontier) - visited
        ordered.extend(sorted(new_deps))
        visited.update(new_deps)
        frontier = new_deps
        if not frontier:
            break
        if len(ordered) > _MAX_DEPENDENT_FILES:
            logger.warning(
                "Dependent expansion capped at %d of %d files for %d roots",
                _MAX_DEPENDENT_FILES,
                len(ordered),
                len(roots),
            )
            return DependentList(ordered[:_MAX_DEPENDENT_FILES], truncated=True)
    return DependentList(ordered)


def _parse_single_python_file_compact(
    args: tuple[str, str],
) -> WorkerParseResult:
    """Parse one Python-owned file and return Rust compact store entities."""
    rel_path, repo_root_str = args
    abs_path = Path(repo_root_str) / rel_path
    try:
        mtime_ns = abs_path.stat().st_mtime_ns
        raw = abs_path.read_bytes()
        fhash = hashlib.sha256(raw).hexdigest()
        parser = _worker_parser if _worker_parser is not None else CodeParser()
        nodes, edges = parser.parse_bytes(abs_path, raw)
        nodes, edges = _relativize_parsed_entities(
            cast(list[NodeInfo], nodes),
            cast(list[EdgeInfo], edges),
            Path(repo_root_str),
        )
        nodes = _serialize_nodes(nodes)
        edges = _serialize_edges(edges)
        return (rel_path, nodes, edges, None, fhash, mtime_ns)
    except _PARSE_FILE_ERRORS as e:
        return (rel_path, [], [], str(e), "", 0)


def _indexed_only(store: GraphStore, rel_paths: list[str]) -> list[str]:
    """Restrict *rel_paths* to files the graph actually holds nodes for."""
    if not rel_paths:
        return []
    indexed = set(store.get_file_meta_for_files(rel_paths))
    return [rel_path for rel_path in rel_paths if rel_path in indexed]


def _is_ignore_scope_file(rel_path: str) -> bool:
    """Return True for gitignore / dagaynignore files that redefine graph scope."""
    return Path(rel_path).name in _IGNORE_SCOPE_NAMES


def _vcs_scope(repo_root: Path, recurse_submodules: bool | None) -> set[str] | None:
    """VCS-indexable paths minus ignore patterns, or ``None`` without a VCS listing.

    Unlike :func:`collect_all_files` this opens no file: parseability is left
    to the per-changed-file filter, so an update costs one listing instead of
    a stat and an 8 KiB read for every file in the repository.
    """
    candidates = get_vcs_indexable_files(repo_root, recurse_submodules)
    if not candidates:
        return None
    patterns = _load_ignore_patterns(repo_root)
    _require_rust_backend()
    from dagayn._core import filter_ignored_paths

    return set(filter_ignored_paths(candidates, patterns))


def _indexable_scope(
    repo_root: Path,
    store: GraphStore,
    recurse_submodules: bool | None = None,
) -> tuple[set[str], list[str]]:
    """Return ``(indexable_scope, graph_files_outside_that_set)``.

    The scope is the VCS listing minus ignore patterns; callers still run
    changed paths through :func:`_filter_incremental_candidates`. Without a
    VCS listing it falls back to the full parseable walk.
    """
    indexable = _vcs_scope(repo_root, recurse_submodules)
    if indexable is None:
        indexable = set(collect_all_files(repo_root, recurse_submodules))
    try:
        graph_files = set(store.get_all_files() or [])
    except Exception:  # noqa: BLE001 — never block an update on a listing failure
        logger.debug("Could not list graph files for scope prune", exc_info=True)
        return indexable, []
    stale = [path for path in graph_files if path not in indexable]
    return indexable, stale


def _expand_changed_submodules(repo_root: Path, rel_paths: list[str]) -> list[str]:
    """Replace changed-submodule directories with the files they track.

    ``git status``/``git diff`` report a modified submodule as the bare
    directory (`` M sub``). Expanding it costs one ``git ls-files`` per changed
    submodule and makes the content-hash comparison downstream skip whatever is
    genuinely unchanged, so the cost is bounded by the submodule's file count and
    only paid when git says the submodule moved.
    """
    expanded: list[str] = []
    for rel_path in rel_paths:
        candidate = repo_root / rel_path
        if not candidate.is_dir() or not (candidate / ".git").exists():
            expanded.append(rel_path)
            continue
        inner = _submodule_tracked_files(candidate)
        if not inner:
            # Cannot enumerate it; keep the original entry rather than dropping
            # the signal entirely.
            expanded.append(rel_path)
            continue
        prefix = PurePosixPath(rel_path)
        expanded.extend(str(prefix / name) for name in inner)
        logger.info("Expanded changed submodule %s into %d tracked file(s)", rel_path, len(inner))
    return _dedupe_preserve_order(expanded)


def _submodule_tracked_files(submodule_root: Path) -> list[str]:
    """Return the submodule's tracked files, relative to the submodule root."""
    import subprocess

    try:
        result = subprocess.run(
            ["git", "ls-files", "-z"],
            capture_output=True,
            text=True,
            cwd=str(submodule_root),
            timeout=30,
        )
    except (FileNotFoundError, subprocess.TimeoutExpired, OSError):
        return []
    if result.returncode != 0:
        return []
    return [field for field in result.stdout.split("\0") if field]


def _filter_incremental_candidates(
    repo_root: Path,
    rel_paths: Collection[str],
    ignore_patterns: list[str],
) -> tuple[list[str], list[str]]:
    """Return ``(parseable_files, removed_files)`` for incremental update."""
    _require_rust_backend()
    try:
        from dagayn._core import filter_incremental_candidates

        return filter_incremental_candidates(
            repo_root,
            list(rel_paths),
            ignore_patterns,
        )
    except (ImportError, RuntimeError, TypeError, ValueError) as exc:
        raise RuntimeError(
            "Rust incremental candidate filtering requires dagayn._core. "
            "Install a wheel with the native extension or rebuild from source."
        ) from exc


def _classify_python_changed_files(
    repo_root: Path,
    file_paths: list[str],
    file_meta: dict[str, tuple[str, int]],
    *,
    trust_mtime: bool = True,
) -> tuple[list[str], list[tuple[int, str]]]:
    """Return content-changed Python-owned files and mtime-only updates.

    With *trust_mtime* false, a stored mtime equal to the current one is not
    taken as proof the content is unchanged and the bytes are hashed anyway.
    Callers that got their list from ``git diff``/``git status`` pass false:
    git has already said the file changed, and an mtime can be equal for a
    changed file (``cp -p``/``rsync -a``/``tar x`` restore it, and coarse
    filesystem granularity can hide two writes in one tick). Trusting it there
    skipped the file forever, since the stored hash also stayed stale.
    """
    changed_files: list[str] = []
    mtime_only_updates: list[tuple[int, str]] = []
    for rel_path in file_paths:
        abs_path = repo_root / rel_path
        try:
            cur_mtime_ns = abs_path.stat().st_mtime_ns
            meta = file_meta.get(rel_path)
            if trust_mtime and meta and meta[1] == cur_mtime_ns:
                continue
            raw = abs_path.read_bytes()
            fhash = hashlib.sha256(raw).hexdigest()
            if meta and meta[0] == fhash:
                mtime_only_updates.append((cur_mtime_ns, rel_path))
                continue
        except (OSError, PermissionError):
            pass
        changed_files.append(rel_path)
    return changed_files, mtime_only_updates


def _get_file_meta_for_candidates(
    store: GraphStore,
    file_paths: list[str],
) -> dict[str, tuple[str, int]]:
    """Return stored file metadata for only the requested paths."""
    if not file_paths:
        return {}
    return store.get_file_meta_for_files(file_paths)


class _StoreBulkLoad:
    def __init__(self, store: GraphStore) -> None:
        self._store = store

    def __enter__(self) -> None:
        self._store.begin_bulk_load()

    def __exit__(self, exc_type: object, exc: object, tb: object) -> None:
        self._store.finish_bulk_load()


#: Match the Rust writer: drop/rebuild indexes only for large file batches.
BULK_LOAD_FILE_THRESHOLD = 64


def _flush_store_batch(store: GraphStore, batch: StoreBatch) -> None:
    """Write parsed file results through one store call.

    The Rust backend is intentionally crossed at batch granularity so PyO3
    overhead is paid per DB write phase chunk, not once for each parsed file.
    """
    if not batch:
        return
    store.store_file_batch_json(_serialize_store_batch(batch))
    batch.clear()


def _serialize_store_batch(batch: StoreBatch) -> str:
    """Serialize parsed graph data in a compact Rust-owned wire format."""
    return json.dumps(
        [
            (
                file_path,
                _serialize_nodes(nodes),
                _serialize_edges(edges),
                fhash,
                mtime_ns,
            )
            for file_path, nodes, edges, fhash, mtime_ns in batch
        ],
        separators=(",", ":"),
    )


def _serialize_nodes(nodes: list[Any]) -> list[Any]:
    if _is_compact_entities(nodes):
        return nodes
    return [
        (
            n.kind,
            n.name,
            n.file_path,
            n.line_start,
            n.line_end,
            n.language,
            n.parent_name,
            n.params,
            n.return_type,
            n.modifiers,
            n.is_test,
            n.extra or {},
        )
        for n in nodes
    ]


def _serialize_edges(edges: list[Any]) -> list[Any]:
    if _is_compact_entities(edges):
        return edges
    return [
        (
            e.kind,
            e.source,
            e.target,
            e.file_path,
            e.line,
            e.extra or {},
        )
        for e in edges
    ]


def _is_compact_entities(entities: list[Any]) -> bool:
    return bool(entities) and isinstance(entities[0], (list, tuple))


def _rust_parser_owns_path(rel_path: str, repo_root: Path | None = None) -> bool:
    lower = rel_path.lower()
    if lower.endswith(
        (
            ".md",
            ".markdown",
            ".tf",
            ".tfvars",
            ".rs",
            ".py",
            ".ipynb",
            ".js",
            ".jsx",
            ".mjs",
            ".cjs",
            ".ts",
            ".mts",
            ".cts",
            ".tsx",
            ".astro",
            ".sh",
            ".bash",
            ".zsh",
            ".ksh",
            ".go",
            ".java",
            ".rb",
            ".cs",
            ".php",
            ".kt",
            ".kts",
            ".scala",
            ".dart",
            ".lua",
            ".c",
            ".h",
            ".xs",
            ".cpp",
            ".cc",
            ".cxx",
            ".hpp",
            ".m",
            ".ex",
            ".exs",
            ".gd",
            ".r",
            ".jl",
            ".pl",
            ".pm",
            ".t",
            ".vue",
            ".svelte",
            ".zig",
            ".ps1",
            ".psm1",
            ".psd1",
            ".swift",
        )
    ):
        return True
    if PurePosixPath(rel_path).suffix or repo_root is None:
        return False
    return _detect_parser_language(repo_root / rel_path) in {
        "bash",
        "python",
        "javascript",
        "ruby",
        "perl",
        "lua",
        "r",
        "php",
    }


def _split_rust_parser_files(
    rel_paths: list[str],
    repo_root: Path | None = None,
) -> tuple[list[str], list[str]]:
    _require_rust_backend()
    rust_files: list[str] = []
    python_files: list[str] = []
    for rel_path in rel_paths:
        if _rust_parser_owns_path(rel_path, repo_root):
            rust_files.append(rel_path)
        else:
            python_files.append(rel_path)
    return rust_files, python_files


def store_phase_failures(errors: list[dict[str, str]] | None) -> list[str]:
    """Return files that failed to be *stored* (not merely to parse).

    A parse failure is a fact about one file and does not invalidate the rest of
    the run. A store failure means the graph is missing content it was asked to
    hold, so it must not be described as covering HEAD.
    """
    if not errors:
        return []
    return [
        entry.get("file", "")
        for entry in errors
        if isinstance(entry, dict) and entry.get("phase") == "store"
    ]


def _store_rust_parse_batches(
    repo_root: Path,
    store: GraphStore,
    rel_paths: list[str],
) -> tuple[int, int, list[dict[str, str]]]:
    if not rel_paths:
        return 0, 0, []
    total_nodes = 0
    total_edges = 0
    errors: list[dict[str, str]] = []
    for idx in range(0, len(rel_paths), _RUST_PARSE_BATCH_SIZE):
        chunk = rel_paths[idx : idx + _RUST_PARSE_BATCH_SIZE]
        try:
            node_count, edge_count, raw_errors = store.store_rust_owned_files(
                repo_root,
                chunk,
            )
        except (RuntimeError, TypeError, ValueError) as exc:
            # A whole chunk (up to _RUST_PARSE_BATCH_SIZE files) failed to
            # *store* — e.g. `database is locked` surfaced as RuntimeError
            # by PyO3. Tagged so the caller can refuse to stamp HEAD: with
            # these recorded as ordinary parse errors, the update returned
            # ok, claimed to describe HEAD, and later diffs started from
            # HEAD, so the dropped files were never revisited.
            logger.error("Failed to store %d file(s): %s", len(chunk), exc)
            errors.extend(
                {"file": rel_path, "error": str(exc), "phase": "store"} for rel_path in chunk
            )
            continue
        total_nodes += int(node_count)
        total_edges += int(edge_count)
        errors.extend(
            {"file": str(file_path), "error": str(error)} for file_path, error in raw_errors
        )
    return total_nodes, total_edges, errors


def _queue_store_file(
    store: GraphStore,
    batch: StoreBatch,
    rel_path: str,
    nodes: ParsedNodes,
    edges: ParsedEdges,
    fhash: str,
    mtime_ns: int,
) -> None:
    batch.append((rel_path, nodes, edges, fhash, mtime_ns))
    if len(batch) >= _STORE_BATCH_SIZE:
        _flush_store_batch(store, batch)


def full_build(
    repo_root: Path,
    store: GraphStore,
    recurse_submodules: bool | None = None,
) -> BuildResult:
    """Full rebuild of the entire graph.

    Args:
        repo_root: Repository root directory.
        store: Graph database store.
        recurse_submodules: If True, include files from git submodules.
            When *None*, falls back to ``CRG_RECURSE_SUBMODULES`` env var.
    """
    repo_root = repo_root.resolve()
    store.set_metadata("repo_root", str(repo_root))
    files = collect_all_files(repo_root, recurse_submodules)

    # Purge stale data from files no longer on disk
    existing_files = set(store.get_all_files())
    current_rel = set(files)
    stale_files = existing_files - current_rel
    store.remove_files_data(list(stale_files))
    # Ensure deletions are persisted before store_file_nodes_edges()
    # starts its own explicit transaction via BEGIN IMMEDIATE.
    if stale_files:
        store.commit()

    total_nodes = 0
    total_edges = 0
    errors = []
    file_count = len(files)

    with _StoreBulkLoad(store):
        use_serial = os.environ.get("CRG_SERIAL_PARSE", "") == "1"
        rust_files, python_files = _split_rust_parser_files(files, repo_root)
        if rust_files:
            rust_nodes, rust_edges, rust_errors = _store_rust_parse_batches(
                repo_root,
                store,
                rust_files,
            )
            total_nodes += rust_nodes
            total_edges += rust_edges
            errors.extend(rust_errors)
            logger.info("Progress: %d/%d files parsed", len(rust_files), file_count)

        if python_files:
            if use_serial or len(python_files) < 8:
                # Serial fallback (for debugging or tiny repos)
                batch: StoreBatch = []
                parser = CodeParser()
                for offset, rel_path in enumerate(python_files, 1):
                    i = len(rust_files) + offset
                    full_path = repo_root / rel_path
                    try:
                        mtime_ns = full_path.stat().st_mtime_ns
                        source = full_path.read_bytes()
                        fhash = hashlib.sha256(source).hexdigest()
                        nodes, edges = parser.parse_bytes(full_path, source)
                        nodes, edges = _relativize_parsed_entities(
                            cast(list[NodeInfo], nodes),
                            cast(list[EdgeInfo], edges),
                            repo_root,
                        )
                        _queue_store_file(store, batch, rel_path, nodes, edges, fhash, mtime_ns)
                        total_nodes += len(nodes)
                        total_edges += len(edges)
                    except _PARSE_FILE_ERRORS as e:
                        logger.warning("Error parsing %s: %s", rel_path, e)
                        errors.append({"file": rel_path, "error": str(e)})
                    if i % 50 == 0 or i == file_count:
                        logger.info("Progress: %d/%d files parsed", i, file_count)
                _flush_store_batch(store, batch)
            else:
                # Parallel parsing — store calls remain serial (SQLite single-writer)
                args_list = [(rel_path, str(repo_root)) for rel_path in python_files]
                batch: StoreBatch = []
                with concurrent.futures.ProcessPoolExecutor(
                    max_workers=_MAX_PARSE_WORKERS,
                    initializer=_init_worker,
                ) as executor:
                    for i, (rel_path, nodes, edges, error, fhash, mtime_ns) in enumerate(
                        executor.map(_parse_single_python_file_compact, args_list, chunksize=20),
                        len(rust_files) + 1,
                    ):
                        if error:
                            logger.warning("Error parsing %s: %s", rel_path, error)
                            errors.append({"file": rel_path, "error": error})
                            continue
                        _queue_store_file(store, batch, rel_path, nodes, edges, fhash, mtime_ns)
                        total_nodes += len(nodes)
                        total_edges += len(edges)
                        if i % 200 == 0 or i == file_count:
                            logger.info("Progress: %d/%d files parsed", i, file_count)
                _flush_store_batch(store, batch)

        store.set_metadata("last_updated", time.strftime("%Y-%m-%dT%H:%M:%S"))
        store.set_metadata("last_build_type", "full")
        full_store_failures = store_phase_failures(errors)
        if full_store_failures:
            # A graph missing files it was asked to hold must not claim to
            # describe HEAD: later incremental runs diff from there.
            logger.error(
                "Not recording git_head_sha: %d file(s) failed to store",
                len(full_store_failures),
            )
        else:
            _store_vcs_metadata(repo_root, store)
            record_extractor_versions(store)
        store.commit()

    result = BuildResult(
        files_parsed=len(files),
        total_nodes=total_nodes,
        total_edges=total_edges,
        errors=errors,
    )
    if full_store_failures:
        result.store_failed_files = full_store_failures
        result.status = "partial"
    return result


def _diff_covers_graph_commit(repo_root: Path, store: GraphStore, base: str) -> bool:
    """Return True when ``diff base..HEAD`` covers everything the graph misses.

    ``git diff`` compares trees, not history, so a base that resolves to the
    exact commit the graph was built at yields the complete file-level delta —
    including files reverted along the way. Any other base may leave commits
    unexamined, so its result must not be recorded as "the graph describes
    HEAD".

    A graph with no stored commit (pre-metadata graphs, non-git working copies)
    keeps the historical behaviour: there is nothing to fall short of, and
    refusing to stamp would strand it in permanent drift.
    """
    resolved_base = resolve_commit_sha(repo_root, base)
    if resolved_base is None:
        return False
    stored = store.get_metadata("git_head_sha") or None
    if not stored:
        return True
    return resolved_base == stored
