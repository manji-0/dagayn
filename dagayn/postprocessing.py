"""Post-process API backed by ``dagayn._core``.

Layer-2 manifest bridges are extracted natively, round-tripped through
Python, then handed to the native pipeline.
"""

from __future__ import annotations

import json
import logging
from collections.abc import Iterator
from contextlib import contextmanager
from dataclasses import asdict
from pathlib import Path
from typing import Any

from pydantic import ValidationError

from .contracts.state_types import (
    PostprocessResult,
)
from .graph import GraphStore

logger = logging.getLogger(__name__)

_MANIFEST_FILENAMES = frozenset({"pyproject.toml", "package.json", "openapitools.json"})


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


def run_post_processing(
    store: GraphStore,
    changed_files: list[str] | None = None,
) -> PostprocessResult:
    """Run all post-build steps on a populated graph."""
    from .parser.manifest_bridges import EXTRACTOR_ID

    manifest_nodes: list[dict[str, Any]] = []
    manifest_edges: list[dict[str, Any]] = []
    discovered = _discover_manifest_bridges(store)
    if discovered is not None and _should_scan_manifests(changed_files):
        manifest_nodes = [asdict(node) for node in discovered.nodes]
        manifest_edges = [asdict(edge) for edge in discovered.edges]
    try:
        raw = store.run_post_processing_json(
            EXTRACTOR_ID,
            json.dumps(manifest_nodes),
            json.dumps(manifest_edges),
            2,
            list(changed_files) if changed_files else None,
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
        if native_warnings:
            result.warnings = list(native_warnings)
        return result
    except (json.JSONDecodeError, ValidationError) as e:
        raise RuntimeError(f"Rust post-processing returned invalid payload: {e}") from e


_STEP_ERRORS = (OSError, RuntimeError, TypeError, ValueError)


@contextmanager
def _warn_on_failure(
    label: str, errors: tuple[type[Exception], ...], warnings: list[str]
) -> Iterator[None]:
    """Log and record *errors* raised by one post-process step instead of raising."""
    try:
        yield
    except errors as e:
        logger.warning(label + " failed: %s", e)
        warnings.append(f"{label} failed: {type(e).__name__}: {e}")


def _resolve_bare_name_edges(
    store: GraphStore, result: PostprocessResult, warnings: list[str]
) -> None:
    """Resolve bare-name CALLS and INHERITS/IMPLEMENTS edges."""
    with _warn_on_failure(
        "Bare-name edge resolution", (OSError, RuntimeError, TypeError, AttributeError), warnings
    ):
        result.bare_call_targets_resolved = int(store.resolve_bare_call_targets())
        result.bare_inheritance_targets_resolved = int(store.resolve_bare_inheritance_targets())


def _resolve_terraform_module_references(
    store: GraphStore, result: PostprocessResult, warnings: list[str]
) -> None:
    """Qualify bare Terraform REFERENCES declared in another file of the module."""
    with _warn_on_failure("Terraform module reference resolution", _STEP_ERRORS, warnings):
        resolved = store.resolve_terraform_module_references()
        result.terraform_module_references_resolved = int(resolved)


def _demote_unresolved_endpoint_edges(
    store: GraphStore, result: PostprocessResult, warnings: list[str]
) -> None:
    """Lower confidence on edges whose node-qualified endpoints are absent."""
    with _warn_on_failure("Unresolved endpoint demotion", _STEP_ERRORS, warnings):
        result.unresolved_endpoint_edges_demoted = int(store.demote_unresolved_endpoint_edges())


def _resolve_markdown_artifact_refs(
    store: GraphStore, result: PostprocessResult, warnings: list[str]
) -> None:
    """Resolve Markdown→code CROSS_ARTIFACT edges in the native store."""
    with _warn_on_failure("Markdown artifact ref resolution", _STEP_ERRORS, warnings):
        resolved, dropped, re_resolved, still_unresolved = store.resolve_markdown_artifact_refs()
        result.markdown_artifact_refs_resolved = int(resolved)
        result.markdown_artifact_refs_dropped = int(dropped)
        result.markdown_artifact_refs_re_resolved = int(re_resolved)
        result.markdown_artifact_refs_still_unresolved = int(still_unresolved)


def _resolve_terraform_artifact_refs(
    store: GraphStore, result: PostprocessResult, warnings: list[str]
) -> None:
    """Resolve Terraform entrypoint CROSS_ARTIFACT edges in the native store."""
    with _warn_on_failure("Terraform artifact ref resolution", _STEP_ERRORS, warnings):
        resolved, still_unresolved = store.resolve_terraform_artifact_refs()
        result.terraform_artifact_refs_resolved = int(resolved)
        result.terraform_artifact_refs_still_unresolved = int(still_unresolved)


def _resolve_native_bindings(
    store: GraphStore, result: PostprocessResult, warnings: list[str]
) -> None:
    """Bind Python imports / calls / ctypes loads to the Rust crates they reach.

    Runs after :func:`_apply_manifest_bridges`, whose ``Cargo.toml`` bridges
    name the crates.
    """
    with _warn_on_failure("Native binding resolution", _STEP_ERRORS, warnings):
        result.native_bindings_resolved = int(store.resolve_native_bindings())


def _apply_manifest_bridges(
    store: GraphStore,
    result: PostprocessResult,
    warnings: list[str],
    changed_files: list[str] | None = None,
) -> None:
    """Extract Layer-2 manifest bridges and swap them natively."""
    with _warn_on_failure("Manifest bridge extraction", _STEP_ERRORS, warnings):
        from .parser.manifest_bridges import EXTRACTOR_ID

        if not _should_scan_manifests(changed_files):
            return
        discovered = _discover_manifest_bridges(store)
        if discovered is None:
            result.manifest_bridges_edges = 0
            result.manifest_bridges_nodes = 0
            return
        nodes_upserted = int(
            store.replace_manifest_bridges_json(
                EXTRACTOR_ID,
                json.dumps([asdict(node) for node in discovered.nodes]),
                json.dumps([asdict(edge) for edge in discovered.edges]),
            )
        )
        result.manifest_bridges_edges = discovered.edge_count
        result.manifest_bridges_nodes = nodes_upserted


def _persist_centrality_scores(
    store: GraphStore,
    result: PostprocessResult,
    warnings: list[str],
    changed_files: list[str] | None = None,
) -> None:
    """Persist query-time hub / bridge scores after graph post-processing."""
    with _warn_on_failure(
        "Centrality score persistence", (OSError, ImportError, RuntimeError), warnings
    ):
        from .analysis import persist_centrality_scores

        counts = persist_centrality_scores(store, changed_files=changed_files)
        result.hub_scores_persisted = counts.get("hub_scores_persisted", 0)
        result.bridge_scores_persisted = counts.get("bridge_scores_persisted", 0)
        result.hub_scores_code_persisted = counts.get("hub_scores_code_persisted", 0)
        result.bridge_scores_code_persisted = counts.get("bridge_scores_code_persisted", 0)


__all__ = [
    "run_post_processing",
]
