"""Artifact-scoped package design principle analysis: ADP."""

from __future__ import annotations

import logging
from typing import Any, Literal, TypedDict

import networkx as nx

from ._scope import ArtifactScope, build_node_scope_maps, iter_scope_edges
from .contracts.dependency_profiles import (
    DependencyProfile,
    validate_dependency_profile,
)
from .graph import GraphStore

logger = logging.getLogger(__name__)


class AdpViolationRecord(TypedDict, total=False):
    nodes: list[str]
    length: int
    edge_weight: int
    severity: int
    dependency_profile: DependencyProfile
    truncated: bool
    cycles_examined: int
    cycle_limit: int


def _project_dependency_graph(
    store: GraphStore,
    granularity: Literal["file", "package"] = "package",
    artifact_scope: ArtifactScope = "code",
    dependency_profile: DependencyProfile = "strict_static",
    snapshot: Any | None = None,
) -> nx.DiGraph:
    """Build a directed dependency graph from dependency edges.

    The default ``strict_static`` profile includes IMPORTS_FROM, DEPENDS_ON,
    INHERITS, and IMPLEMENTS. Other profiles add implementation calls,
    Terraform/dataflow references, or high-confidence artifact trace edges.
    By default, Markdown documentation nodes are excluded so code architecture
    metrics are not skewed by documentation dependency directives. Pass
    artifact_scope="docs" for documentation-only dependencies, or "all" for
    the legacy mixed graph.
    Both endpoints are resolved to scope keys; nodes that cannot be resolved
    (e.g. stdlib types) are silently skipped. INHERITS/IMPLEMENTS targets are
    resolved first by qualified name, then by bare name when exactly one
    in-repo node carries that name.

    For granularity="package", nodes are aggregated by directory prefix.
    Self-loops are removed. Edge weight holds the aggregated edge count.

    ``snapshot`` may supply the full node/edge lists to skip re-reading the
    graph tables when several analyses run together.
    """
    dependency_profile = validate_dependency_profile(dependency_profile)
    g: nx.DiGraph = nx.DiGraph()

    qualified_to_scope, name_to_scope = build_node_scope_maps(
        store,
        granularity,
        artifact_scope=artifact_scope,
        nodes=getattr(snapshot, "all_nodes", None),
    )

    for src, tgt in iter_scope_edges(
        store, snapshot, dependency_profile, qualified_to_scope, name_to_scope
    ):
        if g.has_edge(src, tgt):
            g[src][tgt]["weight"] += 1
        else:
            g.add_edge(src, tgt, weight=1)

    return g


#: Elementary cycles are enumerated, and their count is combinatorial in a
#: densely mutually-importing package graph: 12 fully mutually-importing
#: packages yield 36,018,884 cycles (~106 s and many GB). This is a read path
#: (``architecture_analysis_tool``), so one such subsystem hung it. Enumeration
#: stops here and says it stopped.
_MAX_ADP_CYCLES = 5000
#: A walk budget for graphs whose cycles are few but whose paths are many.
_MAX_ADP_STEPS = 2_000_000


def find_adp_violations(
    store: GraphStore,
    granularity: Literal["file", "package"] = "package",
    min_cycle_size: int = 2,
    max_cycle_length: int = 10,
    artifact_scope: ArtifactScope = "code",
    dependency_profile: DependencyProfile = "strict_static",
    max_cycles: int = _MAX_ADP_CYCLES,
    snapshot: Any | None = None,
) -> list[AdpViolationRecord]:
    """Find cyclic dependencies (ADP violations).

    Enumerates the bounded simple cycles of the artifact-scoped dependency
    subgraph (IMPORTS_FROM, DEPENDS_ON, INHERITS, IMPLEMENTS) with
    ``_core.bounded_simple_cycles``. Each result includes the nodes in the
    cycle, its length, total edge weight, and a severity score
    (length × edge_weight).

    Enumeration stops after *max_cycles* cycles; when it does, the last entry
    carries ``truncated: True`` alongside ``cycles_examined`` so callers can say
    the list is partial rather than presenting it as exhaustive. The walk goes
    in node-name order, so a partial list is the same on every run (with
    ``networkx.simple_cycles`` it followed hash order and changed per process).

    ``snapshot`` may supply the shared node/edge lists to skip re-reading the
    graph tables.

    Returns list of dicts sorted by severity descending.
    """
    dependency_profile = validate_dependency_profile(dependency_profile)
    g = _project_dependency_graph(
        store,
        granularity=granularity,
        artifact_scope=artifact_scope,
        dependency_profile=dependency_profile,
        snapshot=snapshot,
    )

    if g.number_of_nodes() == 0:
        return []

    from ._core import bounded_simple_cycles

    nodes = list(g.nodes)
    index = {node: position for position, node in enumerate(nodes)}
    found, examined, truncated = bounded_simple_cycles(
        [str(node) for node in nodes],
        [(index[source], index[target]) for source, target in g.edges],
        max(min_cycle_size, 0),
        max(max_cycle_length, 0),
        max_cycles,
        _MAX_ADP_STEPS,
    )
    if truncated:
        logger.warning(
            "ADP cycle enumeration stopped at %d cycles; the graph has more",
            len(found),
        )
    violations: list[AdpViolationRecord] = []
    for positions in found:
        # Each cycle starts at its smallest member, so it reads the same on
        # every run.
        cycle = [nodes[position] for position in positions]
        edge_weight = sum(
            g[cycle[i]][cycle[(i + 1) % len(cycle)]].get("weight", 1) for i in range(len(cycle))
        )
        violations.append(
            {
                "nodes": [str(node) for node in cycle],
                "length": len(cycle),
                "edge_weight": edge_weight,
                "severity": len(cycle) * edge_weight,
                "dependency_profile": dependency_profile,
            }
        )

    # Deterministic tie-break: severity ties are common and callers truncate.
    violations.sort(key=lambda x: (-x["severity"], tuple(x["nodes"])))
    if truncated and violations:
        violations[-1]["truncated"] = True
        violations[-1]["cycles_examined"] = examined
        violations[-1]["cycle_limit"] = max_cycles
    return violations
