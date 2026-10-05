"""Layer-2 manifest-backed CROSS_ARTIFACT bridge extraction.

Parses common build and codegen manifests and emits explainable
``CROSS_ARTIFACT`` edges with confidence/evidence in ``extra``. The
extraction runs natively (``dagayn-postproc``'s ``manifest_bridges``);
this module rebuilds its output as :class:`NodeInfo` / :class:`EdgeInfo`.
"""

from __future__ import annotations

import json
from dataclasses import dataclass, field
from pathlib import Path
from typing import Iterable

from ._base.types import EdgeInfo, NodeInfo

EXTRACTOR_ID = "manifest_bridges"


@dataclass
class ManifestBridgeResult:
    """Nodes and edges discovered from manifests under a repository root."""

    nodes: list[NodeInfo] = field(default_factory=list)
    edges: list[EdgeInfo] = field(default_factory=list)

    @property
    def edge_count(self) -> int:
        return len(self.edges)


def discover_manifest_bridges(
    repo_root: Path,
    scope: set[str] | None = None,
) -> ManifestBridgeResult:
    """Scan *repo_root* for supported manifests and build bridge edges.

    *scope* is the repo-relative indexable set (the VCS listing). When given,
    manifests outside it are not read, and nodes and edges that name a file
    outside it are dropped, including files a tracked manifest points at. A
    gitignored file stored here is pruned as out of scope by the next
    incremental update and re-added by the post-processing that update runs.

    File node ``line_end``s are already read from disk.
    """
    from dagayn._core import discover_manifest_bridges_json

    raw = discover_manifest_bridges_json(repo_root, sorted(scope) if scope is not None else None)
    payload = json.loads(raw)
    return ManifestBridgeResult(
        nodes=[NodeInfo(**node) for node in payload["nodes"]],
        edges=[EdgeInfo(**edge) for edge in payload["edges"]],
    )


def refine_node_line_ends(repo_root: Path, nodes: Iterable[NodeInfo]) -> None:
    """No-op: :func:`discover_manifest_bridges` returns refined line ends."""
