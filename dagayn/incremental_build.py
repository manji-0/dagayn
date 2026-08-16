"""Graph build/update orchestration and parsing pipelines."""

from __future__ import annotations

import concurrent.futures
import hashlib
import importlib.util
import json
import logging
import os
import sqlite3
import time
from pathlib import Path, PurePosixPath
from typing import Any, Callable, Optional, cast

from .graph import GraphStore
from .incremental_files import (
    _MAX_DEPENDENT_FILES,
    _MAX_DEPENDENT_HOPS,
    _dedupe_preserve_order,
    _is_binary,
    _load_ignore_patterns,
    _make_repo_relative,
    _relativize_parsed_entities,
    _rust_backend_enabled,
    _should_ignore,
    _store_vcs_metadata,
    collect_all_files,
    resolve_commit_sha,
)
from .parser import CodeParser
from .parser.dispatch import detect_language as _detect_parser_language
from .worktree import is_gitignored

_IGNORE_SCOPE_NAMES = frozenset({".gitignore", ".dagaynignore"})


def _changed_file_sources(repo_root: Path, base: str = "HEAD~1") -> dict[str, list[str]]:
    """Resolve changed-file sources through the public incremental shim."""
    from . import incremental as inc

    return inc.get_changed_file_sources(repo_root, base)


def _run_incremental_update(
    repo_root: Path,
    store: GraphStore,
    *,
    changed_files: list[str] | None = None,
    base: str = "HEAD~1",
) -> dict:
    """Run incremental_update through the public incremental shim."""
    from . import incremental as inc

    if changed_files is not None:
        return inc.incremental_update(repo_root, store, changed_files=changed_files)
    return inc.incremental_update(repo_root, store, base=base)


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
_DEFAULT_BACKEND = "rust"

StoreBatch = list[tuple[str, list[Any], list[Any], str, int]]

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

    rust_get = getattr(store, "get_direct_dependents", None)
    if callable(rust_get):
        direct = cast(Callable[[list[str]], list[str]], rust_get)(list(frontier))
        return set(direct) - frontier

    dependents: set[str] = set()
    # Include normalized path forms to match get_edges_by_target behavior.
    fp_keys: list[str] = []
    for fp in frontier:
        fp_keys.append(fp)
        norm = store._normalize_qualified_key(fp)
        if norm != fp:
            fp_keys.append(norm)

    batch_size = 450

    # 1. File-level IMPORTS_FROM: edges where target_qualified is a frontier file path.
    for i in range(0, len(fp_keys), batch_size):
        chunk = fp_keys[i : i + batch_size]
        placeholders = ",".join("?" for _ in chunk)
        rows = store._conn.execute(  # nosec B608
            f"SELECT file_path FROM edges"
            f" WHERE target_qualified IN ({placeholders}) AND kind = 'IMPORTS_FROM'",
            chunk,
        ).fetchall()
        for row in rows:
            dependents.add(row["file_path"])

    # 2. Node-level: collect QNs for all frontier files in one query.
    fp_list = list(frontier)
    all_node_qns: list[str] = []
    for i in range(0, len(fp_list), batch_size):
        chunk = fp_list[i : i + batch_size]
        placeholders = ",".join("?" for _ in chunk)
        rows = store._conn.execute(  # nosec B608
            f"SELECT qualified_name FROM nodes WHERE file_path IN ({placeholders})",
            chunk,
        ).fetchall()
        all_node_qns.extend(row["qualified_name"] for row in rows)

    # 3. Batch incoming edges for all node QNs in one call.
    if all_node_qns:
        _, incoming = store.get_edges_by_endpoints(all_node_qns)
        for node_edges in incoming.values():
            for e in node_edges:
                if e.kind in ("CALLS", "IMPORTS_FROM", "INHERITS", "IMPLEMENTS"):
                    dependents.add(e.file_path)

    dependents -= frontier
    return dependents


class DependentList(list):
    """A ``list[str]`` with a ``.truncated`` flag.

    When :func:`find_dependents` hits ``_MAX_DEPENDENT_FILES`` it truncates
    the result and sets ``truncated = True`` so callers can distinguish a
    complete expansion from a capped one.  See issue #261.

    This is a transparent ``list`` subclass — existing callers that iterate,
    ``len()``, or slice continue to work unchanged; only callers that
    specifically check ``.truncated`` benefit from the signal.
    """

    truncated: bool

    def __init__(self, items: list, *, truncated: bool = False) -> None:
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
    """
    roots = set(file_paths)
    if not roots:
        return DependentList([])
    all_dependents: set[str] = set()
    visited: set[str] = set(roots)
    frontier: set[str] = set(roots)
    for _hop in range(max_hops):
        new_deps = _batch_hop_dependents(store, frontier) - visited
        all_dependents.update(new_deps)
        visited.update(new_deps)
        frontier = new_deps
        if not frontier:
            break
        if len(all_dependents) > _MAX_DEPENDENT_FILES:
            logger.warning(
                "Dependent expansion capped at %d files for %d roots",
                len(all_dependents),
                len(roots),
            )
            return DependentList(
                list(all_dependents)[:_MAX_DEPENDENT_FILES],
                truncated=True,
            )
    return DependentList(list(all_dependents))


def _parse_single_file(
    args: tuple[str, str],
) -> tuple[str, list, list, str | None, str, int]:
    """Parse one file in a worker process.

    Returns ``(rel_path, nodes, edges, error_or_none, file_hash, mtime_ns)``.
    Must be a module-level function so ``ProcessPoolExecutor`` can
    serialise it across processes.
    """
    rel_path, repo_root_str = args
    abs_path = Path(repo_root_str) / rel_path
    try:
        mtime_ns = int(abs_path.stat().st_mtime_ns)
        raw = abs_path.read_bytes()
        fhash = hashlib.sha256(raw).hexdigest()
        rust_parsed = _parse_with_rust_if_enabled(rel_path, raw)
        if rust_parsed is not None:
            nodes, edges = rust_parsed
            return (rel_path, nodes, edges, None, fhash, mtime_ns)
        parser = _worker_parser if _worker_parser is not None else CodeParser()
        nodes, edges = parser.parse_bytes(abs_path, raw)
        return (rel_path, nodes, edges, None, fhash, mtime_ns)
    except _PARSE_FILE_ERRORS as e:
        return (rel_path, [], [], str(e), "", 0)


def _parse_single_python_file(
    args: tuple[str, str],
) -> tuple[str, list, list, str | None, str, int]:
    """Parse one file known not to be owned by the Rust parser."""
    rel_path, repo_root_str = args
    abs_path = Path(repo_root_str) / rel_path
    try:
        mtime_ns = int(abs_path.stat().st_mtime_ns)
        raw = abs_path.read_bytes()
        fhash = hashlib.sha256(raw).hexdigest()
        parser = _worker_parser if _worker_parser is not None else CodeParser()
        nodes, edges = parser.parse_bytes(abs_path, raw)
        return (rel_path, nodes, edges, None, fhash, mtime_ns)
    except _PARSE_FILE_ERRORS as e:
        return (rel_path, [], [], str(e), "", 0)


def _parse_single_python_file_compact(
    args: tuple[str, str],
) -> tuple[str, list, list, str | None, str, int]:
    """Parse one Python-owned file and return Rust compact store entities."""
    rel_path, repo_root_str = args
    abs_path = Path(repo_root_str) / rel_path
    try:
        mtime_ns = int(abs_path.stat().st_mtime_ns)
        raw = abs_path.read_bytes()
        fhash = hashlib.sha256(raw).hexdigest()
        parser = _worker_parser if _worker_parser is not None else CodeParser()
        nodes, edges = parser.parse_bytes(abs_path, raw)
        nodes, edges = _relativize_parsed_entities(nodes, edges, Path(repo_root_str))
        nodes = _serialize_nodes(nodes)
        edges = _serialize_edges(edges)
        return (rel_path, nodes, edges, None, fhash, mtime_ns)
    except _PARSE_FILE_ERRORS as e:
        return (rel_path, [], [], str(e), "", 0)


def _indexed_only(store: GraphStore, rel_paths: list[str]) -> list[str]:
    """Restrict *rel_paths* to files the graph actually holds nodes for."""
    if not rel_paths:
        return []
    getter = getattr(store, "get_file_meta_map", None)
    if not callable(getter):
        return rel_paths
    try:
        meta_map = cast(Callable[[], dict[str, Any]], getter)() or {}
        indexed = set(meta_map)
    except Exception:  # noqa: BLE001 — fall back to the unfiltered list
        return rel_paths
    return [rel_path for rel_path in rel_paths if rel_path in indexed]


def _is_ignore_scope_file(rel_path: str) -> bool:
    """Return True for gitignore / dagaynignore files that redefine graph scope."""
    return Path(rel_path).name in _IGNORE_SCOPE_NAMES


def _indexable_scope(
    repo_root: Path,
    store: GraphStore,
    recurse_submodules: bool | None = None,
) -> tuple[set[str], list[str]]:
    """Return ``(parseable_indexable, graph_files_outside_that_set)``."""
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
    rel_paths: set[str],
    ignore_patterns: list[str],
) -> tuple[list[str], list[str]]:
    """Return ``(parseable_files, removed_files)`` for incremental update."""
    if _rust_backend_enabled():
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

    existing_files: list[str] = []
    removed_files: list[str] = []
    for rel_path in rel_paths:
        if _should_ignore(rel_path, ignore_patterns):
            continue
        abs_path = repo_root / rel_path
        if not abs_path.is_file():
            removed_files.append(rel_path)
            continue
        # "Exists but is no longer indexable" is a removal, not a skip: a file
        # that became a symlink or binary otherwise kept its previous nodes
        # forever, while a full build's stale-file purge dropped them.
        if abs_path.is_symlink() or _is_binary(abs_path):
            removed_files.append(rel_path)
            continue
        # A case-only rename still answers is_file() under the old spelling on a
        # case-insensitive filesystem, so the old path would be re-parsed rather
        # than removed and the graph would hold two node sets for one file.
        on_disk = _on_disk_spelling(repo_root, rel_path)
        if on_disk is not None and on_disk != rel_path:
            removed_files.append(rel_path)
            continue
        existing_files.append(rel_path)

    parser = CodeParser()
    candidates = []
    for rel_path in existing_files:
        if parser.detect_language(repo_root / rel_path) is not None:
            candidates.append(rel_path)
        else:
            removed_files.append(rel_path)
    return candidates, removed_files


def _on_disk_spelling(repo_root: Path, rel_path: str) -> str | None:
    """Return how the filesystem spells *rel_path*, when that differs.

    Only the final component is checked; a case-only rename renames one entry
    and scanning every parent for every candidate would cost more than the case
    it guards. ``None`` means "same spelling, or undeterminable".
    """
    parent_rel, _, file_name = rel_path.rpartition("/")
    parent_dir = repo_root / parent_rel if parent_rel else repo_root
    try:
        entries = list(parent_dir.iterdir())
    except OSError:
        return None
    matched: str | None = None
    for entry in entries:
        name = entry.name
        if name == file_name:
            return None
        if name.lower() == file_name.lower():
            matched = name
    if matched is None:
        return None
    return f"{parent_rel}/{matched}" if parent_rel else matched


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
            cur_mtime_ns = int(abs_path.stat().st_mtime_ns)
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
    if hasattr(store, "get_file_meta_for_files"):
        return store.get_file_meta_for_files(file_paths)
    if hasattr(store, "get_file_meta_map"):
        return store.get_file_meta_map()
    return {path: (fhash, 0) for path, fhash in store.get_file_hashes(file_paths).items()}


def _callable_store_attr(store: GraphStore, name: str) -> Callable[..., Any] | None:
    attr = getattr(store, name, None)
    return attr if callable(attr) else None


class _StoreBulkLoad:
    def __init__(self, store: GraphStore) -> None:
        self._begin = _callable_store_attr(store, "begin_bulk_load")
        self._finish = _callable_store_attr(store, "finish_bulk_load")
        self._active = False

    def __enter__(self) -> None:
        if self._begin is not None and self._finish is not None:
            self._begin()
            self._active = True

    def __exit__(self, exc_type: object, exc: object, tb: object) -> None:
        if self._active and self._finish is not None:
            self._finish()


def _flush_store_batch(store: GraphStore, batch: StoreBatch) -> None:
    """Write parsed file results through one store call.

    The Rust backend is intentionally crossed at batch granularity so PyO3
    overhead is paid per DB write phase chunk, not once for each parsed file.
    """
    if not batch:
        return
    store_file_batch_json = _callable_store_attr(store, "store_file_batch_json")
    if store_file_batch_json is not None:
        store_file_batch_json(_serialize_store_batch(batch))
    else:
        store.store_file_batch(batch)
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


def _uses_compact_entities(nodes: list[Any], edges: list[Any]) -> bool:
    return _is_compact_entities(nodes) or _is_compact_entities(edges)


def _rust_backend_explicitly_requested() -> bool:
    return os.environ.get("DAGAYN_BACKEND", "").strip().lower() == "rust"


def _rust_backend_available() -> bool:
    return importlib.util.find_spec("dagayn._core") is not None


def _rust_parser_backend_enabled(store: GraphStore | None = None) -> bool:
    if not _rust_backend_enabled():
        return False
    if store is None:
        return True
    return (
        _callable_store_attr(store, "store_rust_owned_files") is not None
        or _callable_store_attr(store, "store_file_batch_json") is not None
    )


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
            ".ts",
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
            ".sol",
            ".dart",
            ".lua",
            ".luau",
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
    store: GraphStore | None = None,
) -> tuple[list[str], list[str]]:
    if not _rust_parser_backend_enabled(store):
        return [], rel_paths
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
        str(entry.get("file", ""))
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
    store_rust_owned_files = _callable_store_attr(store, "store_rust_owned_files")
    if store_rust_owned_files is not None:
        total_nodes = 0
        total_edges = 0
        errors: list[dict[str, str]] = []
        for idx in range(0, len(rel_paths), _RUST_PARSE_BATCH_SIZE):
            chunk = rel_paths[idx : idx + _RUST_PARSE_BATCH_SIZE]
            try:
                node_count, edge_count, raw_errors = store_rust_owned_files(
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
    store_file_batch_json = _callable_store_attr(store, "store_file_batch_json")
    if store_file_batch_json is None:
        raise RuntimeError("Rust parser batch requires a GraphStore with store_file_batch_json")
    try:
        from dagayn._core import parse_rust_owned_files_compact_json
    except ImportError as exc:
        raise RuntimeError(
            "DAGAYN_BACKEND=rust was requested, but dagayn._core is not installed."
        ) from exc

    total_nodes = 0
    total_edges = 0
    errors: list[dict[str, str]] = []
    for idx in range(0, len(rel_paths), _RUST_PARSE_BATCH_SIZE):
        chunk = rel_paths[idx : idx + _RUST_PARSE_BATCH_SIZE]
        try:
            payload = json.loads(parse_rust_owned_files_compact_json(repo_root, chunk))
        except (RuntimeError, TypeError, ValueError, json.JSONDecodeError) as exc:
            errors.extend({"file": rel_path, "error": str(exc)} for rel_path in chunk)
            continue
        batch = payload.get("batch", [])
        for raw_error in payload.get("errors", []):
            if isinstance(raw_error, list | tuple) and len(raw_error) >= 2:
                errors.append({"file": str(raw_error[0]), "error": str(raw_error[1])})
            else:
                errors.append({"file": "", "error": str(raw_error)})
        if not batch:
            continue
        batch_with_mtime = [
            (
                item[0],
                item[1],
                item[2],
                item[3],
                int((repo_root / item[0]).stat().st_mtime_ns),
            )
            if len(item) == 4
            else item
            for item in batch
        ]
        store_file_batch_json(json.dumps(batch_with_mtime, separators=(",", ":")))
        total_nodes += sum(len(item[1]) for item in batch)
        total_edges += sum(len(item[2]) for item in batch)
    return total_nodes, total_edges, errors


def _parse_with_rust_if_enabled(
    rel_path: str,
    source: bytes,
) -> tuple[list[Any], list[Any]] | None:
    if not _rust_backend_enabled():
        return None
    lowered = rel_path.lower()
    parser_name: str
    parser_fn_name: str
    if lowered.endswith((".md", ".markdown")):
        parser_name = "Markdown"
        parser_fn_name = "parse_markdown_compact_json"
    elif lowered.endswith((".tf", ".tfvars")):
        parser_name = "Terraform"
        parser_fn_name = "parse_terraform_compact_json"
    elif lowered.endswith(".rs"):
        parser_name = "Rust"
        parser_fn_name = "parse_rust_compact_json"
    else:
        return None
    try:
        import dagayn._core as rust_core

        parser_fn = getattr(rust_core, parser_fn_name)
        nodes, edges = json.loads(parser_fn(rel_path, source))
        return nodes, edges
    except (
        AttributeError,
        ImportError,
        RuntimeError,
        TypeError,
        ValueError,
        json.JSONDecodeError,
    ) as exc:
        raise RuntimeError(f"Rust {parser_name} parser unavailable for {rel_path}: {exc}") from exc


def _queue_store_file(
    store: GraphStore,
    batch: StoreBatch,
    rel_path: str,
    nodes: list[Any],
    edges: list[Any],
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
) -> dict:
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
        rust_files, python_files = _split_rust_parser_files(files, repo_root, store)
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
                        mtime_ns = int(full_path.stat().st_mtime_ns)
                        source = full_path.read_bytes()
                        fhash = hashlib.sha256(source).hexdigest()
                        nodes, edges = parser.parse_bytes(full_path, source)
                        nodes, edges = _relativize_parsed_entities(nodes, edges, repo_root)
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
                parse_worker = (
                    _parse_single_python_file_compact
                    if _callable_store_attr(store, "store_file_batch_json") is not None
                    else _parse_single_python_file
                )
                with concurrent.futures.ProcessPoolExecutor(
                    max_workers=_MAX_PARSE_WORKERS,
                    initializer=_init_worker,
                ) as executor:
                    for i, (rel_path, nodes, edges, error, fhash, mtime_ns) in enumerate(
                        executor.map(parse_worker, args_list, chunksize=20),
                        len(rust_files) + 1,
                    ):
                        if error:
                            logger.warning("Error parsing %s: %s", rel_path, error)
                            errors.append({"file": rel_path, "error": error})
                            continue
                        if not _uses_compact_entities(nodes, edges):
                            nodes, edges = _relativize_parsed_entities(nodes, edges, repo_root)
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
        store.commit()

    result: dict[str, Any] = {
        "files_parsed": len(files),
        "total_nodes": total_nodes,
        "total_edges": total_edges,
        "errors": errors,
    }
    if full_store_failures:
        result["store_failed_files"] = full_store_failures
        result["status"] = "partial"
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


def incremental_update(
    repo_root: Path,
    store: GraphStore,
    base: str = "HEAD~1",
    changed_files: list[str] | None = None,
    extra_files: list[str] | None = None,
) -> dict:
    """Incremental update: re-parse changed + dependent files only.

    *extra_files* are re-indexed on top of whatever the git diff reports. A
    file whose on-disk content matches ``base`` cannot appear in that diff, so
    content drift found by the diff tier of ``assess_graph_sync`` (a phantom
    node inherited from a seeded worktree, or an edit indexed and then
    discarded) is otherwise unreachable from here: the state that prescribes an
    update is one the update itself can never clear.
    """
    repo_root = repo_root.resolve()
    store.set_metadata("repo_root", str(repo_root))
    ignore_patterns = _load_ignore_patterns(repo_root)

    # Determine changed files
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

    store_failures: list[str] = []
    indexable, stale_scope = _indexable_scope(repo_root, store)

    def _record_head_when_verified() -> None:
        """Stamp ``git_head_sha`` = HEAD, but only if the diff really covered it.

        Two failure modes this guards:

        * The diff was empty because it failed, not because nothing changed
          (unreachable base after a rebase, a shallow clone, a sha from another
          checkout). Recording HEAD there would call an unexamined tree synced.
        * The base did not reach the commit the graph describes, so the commits
          in between were never parsed. ``dagayn update`` used to default to
          ``HEAD~1``, which meant an edit hook firing after a multi-commit
          ``git pull`` indexed the last commit only and then stamped HEAD —
          leaving files silently missing from a graph that claimed to be synced.

        Conversely, without any stamping the stored sha stays at the base commit
        and ``assess_graph_sync`` reports ``commit_drift`` forever, re-running a
        prepare on every session start that can never clear it.
        """
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

    if not changed_files and not stale_scope:
        _record_head_when_verified()
        return {
            "files_updated": 0,
            "total_nodes": 0,
            "total_edges": 0,
            "changed_files": [],
            "change_file_sources": change_file_sources,
            "dependent_files": [],
        }

    total_nodes = 0
    total_edges = 0
    errors = []
    mtime_only_updates: list[tuple[int, str]] = []  # (mtime_ns, file_path) pairs

    # git names a changed submodule by its *directory*, which is not a file, so
    # it used to land in removed_files as a no-op delete while the files inside
    # were never re-parsed or pruned — submodule content was frozen after the
    # first build.
    changed_files = _expand_changed_submodules(repo_root, changed_files)
    change_file_sources["files"] = changed_files

    # First classify the changed roots themselves. Touch-only changes only need
    # their stored mtime refreshed; they should not force dependent expansion.
    changed_candidates, removed_files = _filter_incremental_candidates(
        repo_root,
        set(changed_files),
        ignore_patterns,
    )
    # Gitignored / .dagaynignore-excluded files are out of scope even when git
    # status or watch still names them. Drop them from parse candidates and
    # treat previously indexed ones as removals.
    changed_candidates = [path for path in changed_candidates if path in indexable]
    removed_files.extend(path for path in changed_files if path not in indexable)
    # Keep only removals the graph can act on. The candidate filter reports
    # "not indexable" as a removal so a file that *became* binary/symlinked
    # loses its stale nodes -- but a path that was never indexable (a committed
    # .txt) has nothing to remove, and counting it would inflate files_updated
    # and turn a genuine no-op into a reported update.
    removed_files = _indexed_only(store, removed_files)
    removed_files = _dedupe_preserve_order([*removed_files, *stale_scope])
    rust_changed_candidates, python_changed_candidates = _split_rust_parser_files(
        changed_candidates,
        repo_root,
        store,
    )
    content_changed_files: set[str] = set()
    rust_content_changed_files: set[str] = set()

    if rust_changed_candidates:
        classify_changed_rust_owned_files = _callable_store_attr(
            store, "classify_changed_rust_owned_files"
        )
        if classify_changed_rust_owned_files is not None:
            rust_changed, raw_errors = classify_changed_rust_owned_files(
                repo_root,
                rust_changed_candidates,
            )
            rust_content_changed_files.update(rust_changed)
            content_changed_files.update(rust_changed)
            errors.extend(
                {"file": str(file_path), "error": str(error)} for file_path, error in raw_errors
            )
        else:
            content_changed_files.update(rust_changed_candidates)

    if python_changed_candidates:
        changed_file_meta = _get_file_meta_for_candidates(store, python_changed_candidates)
        python_changed, python_mtime_updates = _classify_python_changed_files(
            repo_root,
            python_changed_candidates,
            changed_file_meta,
            # These paths came from git, which already reported them changed.
            trust_mtime=False,
        )
        content_changed_files.update(python_changed)
        mtime_only_updates.extend(python_mtime_updates)

    dependency_roots = set(removed_files) | content_changed_files
    dependent_files = {
        _make_repo_relative(dep, repo_root)
        for dep in find_dependents_for_files(store, dependency_roots)
    }

    # Combine real content changes, deleted files, and their dependents.
    all_files = content_changed_files | set(removed_files) | dependent_files

    # Separate deleted/unparseable files from files that need re-parsing.
    # When there are no dependent files, the content-changed roots were already
    # filtered as parseable above, so avoid running candidate detection twice.
    if dependent_files:
        candidates, extra_removed = _filter_incremental_candidates(
            repo_root,
            all_files,
            ignore_patterns,
        )
        candidates = [path for path in candidates if path in indexable]
        extra_removed.extend(path for path in all_files if path not in indexable)
        extra_removed = _indexed_only(store, extra_removed)
        removed_files = _dedupe_preserve_order([*removed_files, *extra_removed])
    else:
        candidates = [path for path in content_changed_files if path in indexable]

    store.remove_files_data(removed_files)

    file_meta = _get_file_meta_for_candidates(store, candidates)

    rust_candidates, python_candidates = _split_rust_parser_files(candidates, repo_root, store)
    to_parse_rust_forced: list[str] = []
    to_parse_rust_checked: list[str] = []
    to_parse: list[tuple[str, int]] = []
    for rel_path in rust_candidates:
        if rel_path in rust_content_changed_files:
            to_parse_rust_forced.append(rel_path)
            continue
        abs_path = repo_root / rel_path
        try:
            cur_mtime_ns = int(abs_path.stat().st_mtime_ns)
        except (OSError, PermissionError):
            to_parse_rust_checked.append(rel_path)
            continue
        meta = file_meta.get(rel_path)
        if meta and meta[1] == cur_mtime_ns:
            continue
        to_parse_rust_checked.append(rel_path)

    for rel_path in python_candidates:
        abs_path = repo_root / rel_path
        already_known_changed = rel_path in content_changed_files
        try:
            cur_mtime_ns = int(abs_path.stat().st_mtime_ns)
            meta = file_meta.get(rel_path)
            if not already_known_changed and meta and meta[1] == cur_mtime_ns:
                # mtime unchanged and nothing else says otherwise — skip the read.
                # Files already classified as content-changed above are exempt:
                # their mtime can match while the bytes differ.
                continue
            raw = abs_path.read_bytes()
            fhash = hashlib.sha256(raw).hexdigest()
            if meta and meta[0] == fhash:
                # Content identical despite mtime change (e.g. 'touch') — only
                # update the stored mtime so the fast path fires next time.
                mtime_only_updates.append((cur_mtime_ns, rel_path))
                continue
        except (OSError, PermissionError):
            cur_mtime_ns = 0
        to_parse.append((rel_path, cur_mtime_ns))

    # Persist deletions and mtime-only updates before store_file_nodes_edges()
    # opens its own explicit transaction — avoids nested transaction errors.
    if removed_files or mtime_only_updates:
        if mtime_only_updates:
            if hasattr(store, "update_file_mtimes"):
                store.update_file_mtimes(mtime_only_updates)
            elif hasattr(store, "update_file_mtime"):
                for mtime_ns, file_path in mtime_only_updates:
                    store.update_file_mtime(file_path, mtime_ns)
            elif hasattr(store, "_conn"):
                store._conn.executemany(
                    "UPDATE nodes SET mtime_ns=? WHERE file_path=?", mtime_only_updates
                )
        store.commit()

    if (
        not removed_files
        and not to_parse_rust_forced
        and not to_parse_rust_checked
        and not to_parse
    ):
        _record_head_when_verified()
        return {
            "files_updated": len(all_files),
            "total_nodes": total_nodes,
            "total_edges": total_edges,
            "changed_files": list(changed_files),
            "change_file_sources": change_file_sources,
            "dependent_files": list(dependent_files),
            "errors": errors,
        }

    use_serial = os.environ.get("CRG_SERIAL_PARSE", "") == "1"
    to_parse_mtime = dict(to_parse)
    store_changed_rust_owned_files = _callable_store_attr(store, "store_changed_rust_owned_files")
    if to_parse_rust_forced:
        if store_changed_rust_owned_files is not None:
            rust_nodes, rust_edges, raw_errors = store_changed_rust_owned_files(
                repo_root,
                to_parse_rust_forced,
            )
            rust_errors = [
                {"file": str(file_path), "error": str(error)} for file_path, error in raw_errors
            ]
        else:
            rust_nodes, rust_edges, rust_errors = _store_rust_parse_batches(
                repo_root,
                store,
                to_parse_rust_forced,
            )
        total_nodes += rust_nodes
        total_edges += rust_edges
        errors.extend(rust_errors)

    if to_parse_rust_checked:
        if store_changed_rust_owned_files is not None:
            rust_nodes, rust_edges, raw_errors = store_changed_rust_owned_files(
                repo_root,
                to_parse_rust_checked,
            )
            rust_errors = [
                {"file": str(file_path), "error": str(error)} for file_path, error in raw_errors
            ]
        else:
            rust_nodes, rust_edges, rust_errors = _store_rust_parse_batches(
                repo_root,
                store,
                to_parse_rust_checked,
            )
        total_nodes += rust_nodes
        total_edges += rust_edges
        errors.extend(rust_errors)

    if use_serial or len(to_parse) < 8:
        batch: StoreBatch = []
        if to_parse:
            parser = CodeParser()
            for rel_path, _ in to_parse:
                mtime_ns = to_parse_mtime.get(rel_path, 0)
                abs_path = repo_root / rel_path
                try:
                    source = abs_path.read_bytes()
                    fhash = hashlib.sha256(source).hexdigest()
                    nodes, edges = parser.parse_bytes(abs_path, source)
                    nodes, edges = _relativize_parsed_entities(nodes, edges, repo_root)
                    _queue_store_file(store, batch, rel_path, nodes, edges, fhash, mtime_ns)
                    total_nodes += len(nodes)
                    total_edges += len(edges)
                except _PARSE_FILE_ERRORS as e:
                    logger.warning("Error parsing %s: %s", rel_path, e)
                    errors.append({"file": rel_path, "error": str(e)})
        _flush_store_batch(store, batch)
    else:
        args_list = [(rel_path, str(repo_root)) for rel_path, _ in to_parse]
        batch: StoreBatch = []
        parse_worker = (
            _parse_single_python_file_compact
            if _callable_store_attr(store, "store_file_batch_json") is not None
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
                    errors.append({"file": rel_path, "error": error})
                    continue
                if not _uses_compact_entities(nodes, edges):
                    nodes, edges = _relativize_parsed_entities(nodes, edges, repo_root)
                _queue_store_file(store, batch, rel_path, nodes, edges, fhash, mtime_ns)
                total_nodes += len(nodes)
                total_edges += len(edges)
        _flush_store_batch(store, batch)

    store.set_metadata("last_updated", time.strftime("%Y-%m-%dT%H:%M:%S"))
    store.set_metadata("last_build_type", "incremental")
    store_failures.extend(store_phase_failures(errors))
    if diff_covers_graph and not store_failures:
        # Same contract as the no-op paths: only a diff that reached the graph's
        # own commit proves the graph now describes HEAD -- and only if every
        # file it named actually landed in the graph.
        _store_vcs_metadata(repo_root, store)
    elif store_failures:
        logger.error("Not recording git_head_sha: %d file(s) failed to store", len(store_failures))
    store.commit()

    result = {
        "files_updated": len(all_files),
        "total_nodes": total_nodes,
        "total_edges": total_edges,
        "changed_files": list(changed_files),
        "change_file_sources": change_file_sources,
        "dependent_files": list(dependent_files),
        "errors": errors,
    }
    if store_failures:
        result["store_failed_files"] = store_failures
        result["status"] = "partial"
    return result


# ---------------------------------------------------------------------------
# Watch mode
# ---------------------------------------------------------------------------


_DEBOUNCE_SECONDS = 0.3
#: Upper bound on how long the debounce may be pushed out by further events.
#: Without it, sustained churn reset the timer forever and the graph was never
#: updated while writes kept arriving.
_MAX_DEBOUNCE_SECONDS = float(os.environ.get("DAGAYN_WATCH_MAX_DEBOUNCE_SECONDS", "5"))


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

        def on_created(self, event):
            if event.is_directory:
                return
            if self._should_handle(event.src_path):
                self._schedule(event.src_path)

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
                prune = getattr(store, "prune_orphaned_graph_structures", None)
                if callable(prune):
                    prune()
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
                    result = _run_incremental_update(
                        repo_root,
                        store,
                        changed_files=rels,
                    )
                    updated = int(result.get("files_updated", 0))
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
        import time as _time

        while True:
            _time.sleep(1)
    except KeyboardInterrupt:
        observer.stop()
    observer.join()
    logger.info("Watch stopped.")
