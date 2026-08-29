"""Post-process API backed by ``dagayn._core``.

Layer-2 manifest bridges are still extracted in Python, then handed to the
native pipeline.
"""

from __future__ import annotations

import json
import logging
from dataclasses import asdict
from pathlib import Path
from typing import Any, Callable, cast

from pydantic import ValidationError

from .graph import GraphStore
from .state_types import (
    MarkdownArtifactResolution,
    PostprocessResult,
    build_markdown_artifact_resolution,
)

logger = logging.getLogger(__name__)

_MANIFEST_FILENAMES = frozenset({"pyproject.toml", "package.json", "openapitools.json"})


def _should_scan_manifests(changed_files: list[str] | None) -> bool:
    """Return False when an incremental update touched no manifest files."""
    if not changed_files:
        return True
    return any(Path(path).name in _MANIFEST_FILENAMES for path in changed_files)


def _store_repo_root(store: GraphStore) -> Path | None:
    """Resolve ``repo_root`` from the GraphStore bindings."""
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
    native = getattr(store, "run_post_processing_json", None)
    if not callable(native):
        raise RuntimeError("GraphStore.run_post_processing_json is required (Rust GraphStore).")
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


def _native_method(store: GraphStore, name: str) -> Any | None:
    method = getattr(store, name, None)
    return method if callable(method) else None


def _markdown_artifact_resolution(
    *,
    edge_id: int,
    current_target: str,
    symbol: str,
    extra: dict[str, Any],
    matches: list[tuple[str, str]],
) -> MarkdownArtifactResolution:
    """Return the typed target state for one Markdown artifact edge."""
    unresolved_target = f"<unresolved:{symbol}>"
    is_implicit_code_span = (
        extra.get("evidence_kind") == "markdown_code_span"
        and extra.get("evidence_source") == "code_span"
    )

    if len(matches) == 1:
        qname, lang = matches[0]
        new_extra = dict(extra)
        new_extra["target_language"] = lang
        if is_implicit_code_span:
            confidence = 0.4
            confidence_tier = "MEDIUM"
        else:
            confidence = 0.8
            confidence_tier = "HIGH"
        new_extra["confidence"] = confidence
        new_extra["confidence_tier"] = confidence_tier
        return build_markdown_artifact_resolution(
            state="resolved" if current_target.startswith("<unresolved:") else "re_resolved",
            edge_id=edge_id,
            target_qualified=qname,
            target_language=lang,
            confidence=confidence,
            confidence_tier=confidence_tier,
            extra=new_extra,
        )

    if is_implicit_code_span:
        return build_markdown_artifact_resolution(state="dropped", edge_id=edge_id)

    if current_target == unresolved_target:
        return build_markdown_artifact_resolution(
            state="still_unresolved",
            edge_id=edge_id,
            target_qualified=unresolved_target,
            confidence=0.2,
            confidence_tier="LOW",
        )

    new_extra = dict(extra)
    new_extra.pop("target_language", None)
    new_extra["confidence"] = 0.2
    new_extra["confidence_tier"] = "LOW"
    return build_markdown_artifact_resolution(
        state="dropped",
        edge_id=edge_id,
        target_qualified=unresolved_target,
        confidence=0.2,
        confidence_tier="LOW",
        extra=new_extra,
    )


def _resolve_bare_name_edges(
    store: GraphStore,
    result: PostprocessResult,
    warnings: list[str],
) -> None:
    """Resolve bare-name CALLS and INHERITS/IMPLEMENTS edges."""
    native_calls = _native_method(store, "resolve_bare_call_targets")
    native_inherits = _native_method(store, "resolve_bare_inheritance_targets")
    if native_calls is None or native_inherits is None:
        raise RuntimeError("bare-name resolution requires the Rust GraphStore")
    try:
        result.bare_call_targets_resolved = int(native_calls())
        result.bare_inheritance_targets_resolved = int(native_inherits())
    except (OSError, RuntimeError, TypeError, AttributeError) as e:
        logger.warning("Bare-name edge resolution failed: %s", e)
        warnings.append(f"Bare-name edge resolution failed: {type(e).__name__}: {e}")


def _demote_unresolved_endpoint_edges(
    store: GraphStore,
    result: PostprocessResult,
    warnings: list[str],
) -> None:
    """Lower confidence on edges whose node-qualified endpoints are absent."""
    native = _native_method(store, "demote_unresolved_endpoint_edges")
    if native is None:
        raise RuntimeError("endpoint demotion requires the Rust GraphStore")
    try:
        result.unresolved_endpoint_edges_demoted = int(native())
    except (OSError, RuntimeError, TypeError, ValueError) as e:
        logger.warning("Unresolved endpoint demotion failed: %s", e)
        warnings.append(f"Unresolved endpoint demotion failed: {type(e).__name__}: {e}")


def _resolve_markdown_artifact_refs(
    store: GraphStore,
    result: PostprocessResult,
    warnings: list[str],
) -> None:
    """Resolve Markdown→code CROSS_ARTIFACT edges in the native store."""
    native = _native_method(store, "resolve_markdown_artifact_refs")
    if native is None:
        raise RuntimeError("markdown artifact resolution requires the Rust GraphStore")
    try:
        resolved, dropped, re_resolved, still_unresolved = cast(
            Callable[[], tuple[int, int, int, int]], native
        )()
        result.markdown_artifact_refs_resolved = int(resolved)
        result.markdown_artifact_refs_dropped = int(dropped)
        result.markdown_artifact_refs_re_resolved = int(re_resolved)
        result.markdown_artifact_refs_still_unresolved = int(still_unresolved)
    except (OSError, RuntimeError, TypeError, ValueError) as e:
        logger.warning("Markdown artifact ref resolution failed: %s", e)
        warnings.append(f"Markdown artifact ref resolution failed: {type(e).__name__}: {e}")


def _resolve_terraform_artifact_refs(
    store: GraphStore,
    result: PostprocessResult,
    warnings: list[str],
) -> None:
    """Resolve Terraform entrypoint CROSS_ARTIFACT edges in the native store."""
    native = _native_method(store, "resolve_terraform_artifact_refs")
    if native is None:
        raise RuntimeError("terraform artifact resolution requires the Rust GraphStore")
    try:
        resolved, still_unresolved = cast(Callable[[], tuple[int, int]], native)()
        result.terraform_artifact_refs_resolved = int(resolved)
        result.terraform_artifact_refs_still_unresolved = int(still_unresolved)
    except (OSError, RuntimeError, TypeError, ValueError) as e:
        logger.warning("Terraform artifact ref resolution failed: %s", e)
        warnings.append(f"Terraform artifact ref resolution failed: {type(e).__name__}: {e}")


def _apply_manifest_bridges(
    store: GraphStore,
    result: PostprocessResult,
    warnings: list[str],
    changed_files: list[str] | None = None,
) -> None:
    """Extract Layer-2 manifest bridges in Python and swap them natively."""
    try:
        from .parser.manifest_bridges import EXTRACTOR_ID

        if not _should_scan_manifests(changed_files):
            return
        discovered = _discover_manifest_bridges(store)
        if discovered is None:
            result.manifest_bridges_edges = 0
            result.manifest_bridges_nodes = 0
            return
        native = _native_method(store, "replace_manifest_bridges_json")
        if native is None:
            raise RuntimeError("manifest bridges require the Rust GraphStore")
        nodes_upserted = int(
            native(
                EXTRACTOR_ID,
                json.dumps([asdict(node) for node in discovered.nodes]),
                json.dumps([asdict(edge) for edge in discovered.edges]),
            )
        )
        result.manifest_bridges_edges = discovered.edge_count
        result.manifest_bridges_nodes = nodes_upserted
    except (OSError, RuntimeError, TypeError, ValueError) as e:
        logger.warning("Manifest bridge extraction failed: %s", e)
        warnings.append(f"Manifest bridge extraction failed: {type(e).__name__}: {e}")


def _persist_centrality_scores(
    store: GraphStore,
    result: PostprocessResult,
    warnings: list[str],
    changed_files: list[str] | None = None,
) -> None:
    """Persist query-time hub / bridge scores after graph post-processing."""
    try:
        from .analysis import persist_centrality_scores

        counts = persist_centrality_scores(store, changed_files=changed_files)
        result.hub_scores_persisted = counts.get("hub_scores_persisted", 0)
        result.bridge_scores_persisted = counts.get("bridge_scores_persisted", 0)
        result.hub_scores_code_persisted = counts.get("hub_scores_code_persisted", 0)
        result.bridge_scores_code_persisted = counts.get("bridge_scores_code_persisted", 0)
    except (OSError, ImportError, RuntimeError) as e:
        logger.warning("Centrality score persistence failed: %s", e)
        warnings.append(f"Centrality score persistence failed: {type(e).__name__}: {e}")


__all__ = [
    "run_post_processing",
]
