"""Thin post-process API. Step implementations live in ``dagayn.legacy_py.postprocessing``.

A native ``run_post_processing_json`` keeps the full pipeline in Rust after
Python extracts Layer-2 manifest bridges.
"""

from __future__ import annotations

import json
import logging
from dataclasses import asdict
from pathlib import Path
from typing import Any, Callable, cast

from pydantic import ValidationError

from .graph import GraphStore
from .state_types import PostprocessResult

logger = logging.getLogger(__name__)

_MANIFEST_FILENAMES = frozenset({"pyproject.toml", "package.json", "openapitools.json"})


def _legacy() -> Any:
    from dagayn.legacy_py import postprocessing as impl

    return impl


def _native_method(store: GraphStore, name: str) -> Any | None:
    """Return a native GraphStore method when the current connection exposes it."""
    method = getattr(store, name, None)
    return method if callable(method) else None


def _should_scan_manifests(changed_files: list[str] | None) -> bool:
    """Return False when an incremental update touched no manifest files."""
    if not changed_files:
        return True
    return any(Path(path).name in _MANIFEST_FILENAMES for path in changed_files)


def _store_repo_root(store: GraphStore) -> Path | None:
    """Resolve ``repo_root`` from Python or Rust GraphStore bindings."""
    getter = getattr(store, "get_repo_root", None)
    if callable(getter):
        root = cast(Callable[[], Path | None], getter)()
        if root is not None:
            return Path(root)
    get_meta = getattr(store, "get_metadata", None)
    if callable(get_meta):
        raw = cast(Callable[[str], str | None], get_meta)("repo_root")
        if raw:
            return Path(raw)
    return None


def _discover_manifest_bridges(store: GraphStore) -> Any | None:
    """Discover manifest-backed bridge nodes/edges without mutating the graph."""
    from .parser.manifest_bridges import discover_manifest_bridges, refine_node_line_ends

    repo_root = _store_repo_root(store)
    if repo_root is None or not repo_root.is_dir():
        return None
    discovered = discover_manifest_bridges(repo_root)
    refine_node_line_ends(repo_root, discovered.nodes)
    return discovered


def run_post_processing(
    store: GraphStore,
    changed_files: list[str] | None = None,
) -> PostprocessResult:
    """Run all post-build steps on a populated graph."""
    native = _native_method(store, "run_post_processing_json")
    if native is not None:
        from .parser.manifest_bridges import EXTRACTOR_ID

        manifest_nodes: list[dict[str, Any]] = []
        manifest_edges: list[dict[str, Any]] = []
        discovered = _discover_manifest_bridges(store)
        if discovered is not None and _should_scan_manifests(changed_files):
            manifest_nodes = [asdict(node) for node in discovered.nodes]
            manifest_edges = [asdict(edge) for edge in discovered.edges]
        try:
            raw = cast(Callable[..., str], native)(
                EXTRACTOR_ID,
                json.dumps(manifest_nodes),
                json.dumps(manifest_edges),
                2,
                list(changed_files) if changed_files else None,
            )
            payload = json.loads(raw)
            native_warnings = payload.pop("warnings", []) or []
            result = PostprocessResult(**payload)
            if native_warnings:
                result.warnings = list(native_warnings)
            return result
        except (
            OSError,
            RuntimeError,
            TypeError,
            ValueError,
            json.JSONDecodeError,
            ValidationError,
        ) as e:
            logger.warning("Rust post-processing failed, falling back to Python: %s", e)
    return _legacy().run_post_processing(store, changed_files)


def __getattr__(name: str) -> Any:
    value = getattr(_legacy(), name)
    globals()[name] = value
    return value


__all__ = [
    "run_post_processing",
]
