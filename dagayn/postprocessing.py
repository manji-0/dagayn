"""Post-process API backed by ``dagayn._core``.

Layer-2 manifest bridges are extracted natively, round-tripped through
Python, then handed to the native pipeline.
"""

from __future__ import annotations

import json
import logging
from dataclasses import asdict
from pathlib import Path
from typing import Any

from pydantic import ValidationError

from .contracts.state_types import (
    PostprocessResult,
)
from .graph import GraphStore

logger = logging.getLogger(__name__)

_MANIFEST_FILENAMES = frozenset(
    {
        "pyproject.toml",
        "package.json",
        "openapitools.json",
        "wrangler.toml",
        "wrangler.json",
        "wrangler.jsonc",
    }
)


def _should_scan_manifests(changed_files: list[str] | None) -> bool:
    """Return False when an incremental update touched no manifest files."""
    if not changed_files:
        return True
    return any(Path(path).name in _MANIFEST_FILENAMES for path in changed_files)


def _store_repo_root(store: GraphStore) -> Path | None:
    """Resolve ``repo_root`` from the GraphStore bindings."""
    root = store.get_repo_root()
    return Path(root) if root is not None else None


def _discover_manifest_bridges(store: GraphStore) -> Any | None:
    """Discover manifest-backed bridge nodes/edges without mutating the graph."""
    from .incremental_build import _vcs_scope
    from .parser.manifest_bridges import discover_manifest_bridges

    repo_root = _store_repo_root(store)
    if repo_root is None or not repo_root.is_dir():
        return None
    return discover_manifest_bridges(repo_root, _vcs_scope(repo_root, None))


_DISCOVERY_ERRORS = (OSError, RuntimeError, TypeError, ValueError)


def _manifest_arguments(
    store: GraphStore,
    changed_files: list[str] | None,
    for_build: bool,
    warnings: list[str],
) -> tuple[str, list[dict[str, Any]], list[dict[str, Any]]]:
    """``(manifest_mode, nodes, edges)`` for ``run_post_processing_json``."""
    if not for_build:
        discovered = _discover_manifest_bridges(store)
        if discovered is None or not _should_scan_manifests(changed_files):
            return "auto", [], []
        nodes = [asdict(node) for node in discovered.nodes]
        return "auto", nodes, [asdict(edge) for edge in discovered.edges]
    if not _should_scan_manifests(changed_files):
        return "keep", [], []
    try:
        discovered = _discover_manifest_bridges(store)
    except _DISCOVERY_ERRORS as e:
        logger.warning("Manifest bridge extraction failed: %s", e)
        warnings.append(f"Manifest bridge extraction failed: {type(e).__name__}: {e}")
        return "keep", [], []
    if discovered is None:
        return "auto", [], []
    nodes = [asdict(node) for node in discovered.nodes]
    return "replace", nodes, [asdict(edge) for edge in discovered.edges]


def run_post_processing(
    store: GraphStore,
    changed_files: list[str] | None = None,
    *,
    communities: bool = True,
    for_build: bool = False,
) -> PostprocessResult:
    """Run all post-build steps on a populated graph.

    The passes and their order live in Rust (``dagayn_postproc::run_passes``).
    ``communities=False`` leaves community detection to the caller.
    ``for_build`` runs the manifest step as a build does: not at all when
    *changed_files* names no manifest, replacing the stored bridges even when
    none were discovered, and with a discovery error as a warning. Without
    it (``dagayn watch``), two empty arrays leave the stored bridges.
    """
    from .parser.manifest_bridges import EXTRACTOR_ID

    warnings: list[str] = []
    manifest_mode, manifest_nodes, manifest_edges = _manifest_arguments(
        store, changed_files, for_build, warnings
    )
    try:
        raw = store.run_post_processing_json(
            EXTRACTOR_ID,
            json.dumps(manifest_nodes),
            json.dumps(manifest_edges),
            2 if communities else None,
            list(changed_files) if changed_files else None,
            manifest_mode,
        )
    except (
        OSError,
        RuntimeError,
        TypeError,
        ValueError,
    ) as e:
        raise RuntimeError(f"Rust post-processing failed: {type(e).__name__}: {e}") from e
    try:
        payload = json.loads(raw)
        native_warnings = payload.pop("warnings", []) or []
        result = PostprocessResult(**payload)
        if warnings or native_warnings:
            result.warnings = [*warnings, *native_warnings]
        return result
    except (json.JSONDecodeError, ValidationError) as e:
        raise RuntimeError(f"Rust post-processing returned invalid payload: {e}") from e


__all__ = [
    "run_post_processing",
]
