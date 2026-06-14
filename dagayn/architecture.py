"""Artifact-scoped package design principle analysis: ADP and SDP."""

from __future__ import annotations

import logging
from typing import Literal

import networkx as nx

from ._scope import ArtifactScope, build_node_scope_maps
from .dependency_profiles import (
    DependencyProfile,
    edge_matches_dependency_profile,
    validate_dependency_profile,
)
from .graph import GraphStore

logger = logging.getLogger(__name__)

def _project_dependency_graph(
    store: GraphStore,
    granularity: Literal["file", "package"] = "package",
    artifact_scope: ArtifactScope = "code",
    dependency_profile: DependencyProfile = "strict_static",
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
    """
    dependency_profile = validate_dependency_profile(dependency_profile)
    g: nx.DiGraph = nx.DiGraph()

    qualified_to_scope, name_to_scope = build_node_scope_maps(
        store,
        granularity,
        artifact_scope=artifact_scope,
    )

    for e in store.get_all_edges():
        if not edge_matches_dependency_profile(e, dependency_profile):
            continue
        src = qualified_to_scope.get(e.source_qualified)
        if src is None:
            continue
        tgt = qualified_to_scope.get(e.target_qualified)
        if tgt is None:
            tgt = name_to_scope.get(e.target_qualified)
        if tgt is None or src == tgt:
            continue

        if g.has_edge(src, tgt):
            g[src][tgt]["weight"] += 1
        else:
            g.add_edge(src, tgt, weight=1)

    return g


def find_adp_violations(
    store: GraphStore,
    granularity: Literal["file", "package"] = "package",
    min_cycle_size: int = 2,
    max_cycle_length: int = 10,
    artifact_scope: ArtifactScope = "code",
    dependency_profile: DependencyProfile = "strict_static",
) -> list[dict]:
    """Find cyclic dependencies (ADP violations).

    Uses nx.simple_cycles on the artifact-scoped dependency subgraph
    (IMPORTS_FROM, DEPENDS_ON, INHERITS, IMPLEMENTS). Each result includes the
    nodes in the cycle, its length, total edge weight, and a severity score
    (length × edge_weight).

    Returns list of dicts sorted by severity descending.
    """
    dependency_profile = validate_dependency_profile(dependency_profile)
    g = _project_dependency_graph(
        store,
        granularity=granularity,
        artifact_scope=artifact_scope,
        dependency_profile=dependency_profile,
    )

    if g.number_of_nodes() == 0:
        return []

    violations = []
    try:
        for cycle in nx.simple_cycles(g, length_bound=max_cycle_length):
            if len(cycle) < min_cycle_size:
                continue
            edge_weight = sum(
                g[cycle[i]][cycle[(i + 1) % len(cycle)]].get("weight", 1)
                for i in range(len(cycle))
                if g.has_edge(cycle[i], cycle[(i + 1) % len(cycle)])
            )
            violations.append(
                {
                    "nodes": cycle,
                    "length": len(cycle),
                    "edge_weight": edge_weight,
                    "severity": len(cycle) * edge_weight,
                    "dependency_profile": dependency_profile,
                }
            )
    except Exception as exc:
        logger.warning("Cycle detection failed: %s", exc)

    violations.sort(key=lambda x: x["severity"], reverse=True)
    return violations


def compute_sdp_metrics(
    store: GraphStore,
    granularity: Literal["file", "package"] = "package",
    artifact_scope: ArtifactScope = "code",
    dependency_profile: DependencyProfile = "strict_static",
) -> list[dict]:
    """Compute SDP instability metrics for each module/package.

    Instability I = Ce / (Ca + Ce), where:
    - Ca (afferent couplings) = in-degree: number of modules that import this one
    - Ce (efferent couplings) = out-degree: number of modules this one imports
    - I = 0: maximally stable (others depend on it, it depends on nothing)
    - I = 1: maximally unstable (nothing depends on it, it depends on many things)
    Isolated nodes (Ca + Ce = 0) are assigned I = 0.

    ``artifact_scope`` keeps code and Markdown documentation dependencies from
    contributing to each other's Ca/Ce counts. Returns list of dicts sorted by
    instability descending.
    """
    dependency_profile = validate_dependency_profile(dependency_profile)
    g = _project_dependency_graph(
        store,
        granularity=granularity,
        artifact_scope=artifact_scope,
        dependency_profile=dependency_profile,
    )

    if g.number_of_nodes() == 0:
        return []

    results = []
    for node in g.nodes():
        ca = g.in_degree(node)
        ce = g.out_degree(node)
        total = ca + ce
        instability = ce / total if total > 0 else 0.0
        results.append(
            {
                "name": node,
                "ca": ca,
                "ce": ce,
                "instability": round(instability, 4),
                "dependency_profile": dependency_profile,
            }
        )

    results.sort(key=lambda x: x["instability"], reverse=True)
    return results


def find_sdp_violations(
    store: GraphStore,
    granularity: Literal["file", "package"] = "package",
    min_delta: float = 0.1,
    artifact_scope: ArtifactScope = "code",
    dependency_profile: DependencyProfile = "strict_static",
) -> list[dict]:
    """Find SDP violations: dependencies pointing toward instability.

    An edge A -> B violates SDP when I(A) < I(B) - min_delta, i.e., a more
    stable module depends on a less stable one.

    Returns list of dicts sorted by delta descending.
    """
    dependency_profile = validate_dependency_profile(dependency_profile)
    g = _project_dependency_graph(
        store,
        granularity=granularity,
        artifact_scope=artifact_scope,
        dependency_profile=dependency_profile,
    )

    if g.number_of_nodes() == 0:
        return []

    instability: dict[str, float] = {}
    for node in g.nodes():
        ca = g.in_degree(node)
        ce = g.out_degree(node)
        total = ca + ce
        instability[node] = ce / total if total > 0 else 0.0

    violations = []
    for src, tgt in g.edges():
        i_src = instability[src]
        i_tgt = instability[tgt]
        delta = i_tgt - i_src
        if delta > min_delta:
            violations.append(
                {
                    "source": src,
                    "target": tgt,
                    "source_instability": round(i_src, 4),
                    "target_instability": round(i_tgt, 4),
                    "delta": round(delta, 4),
                    "dependency_profile": dependency_profile,
                }
            )

    violations.sort(key=lambda x: x["delta"], reverse=True)
    return violations
