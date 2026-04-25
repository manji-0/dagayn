"""Package design principle analysis: ADP (Acyclic Dependencies) and SDP (Stable Dependencies)."""

from __future__ import annotations

import logging
from pathlib import Path
from typing import Literal

import networkx as nx

from .graph import GraphStore

logger = logging.getLogger(__name__)

_DEPENDENCY_EDGE_KINDS = frozenset({"IMPORTS_FROM", "DEPENDS_ON"})


def _file_to_package(file_path: str) -> str:
    parent = Path(file_path).parent.as_posix()
    return "<root>" if parent == "." else parent


def _project_dependency_graph(
    store: GraphStore,
    granularity: Literal["file", "package"] = "package",
) -> nx.DiGraph:
    """Build a directed dependency graph from IMPORTS_FROM/DEPENDS_ON edges.

    Only edges where both endpoints are known File nodes in the graph are
    included — this filters out stdlib/external imports recorded as bare
    names (e.g. "logging", "json") that are not File nodes.

    For granularity="package", File nodes are aggregated by directory prefix.
    Self-loops are removed. Edge weight holds the aggregated edge count.
    """
    g: nx.DiGraph = nx.DiGraph()

    # Build the set of known file paths to filter out external/stdlib targets
    file_paths: set[str] = {
        n.qualified_name for n in store.get_all_nodes(exclude_files=False) if n.kind == "File"
    }

    for e in store.get_all_edges():
        if e.kind not in _DEPENDENCY_EDGE_KINDS:
            continue
        if e.source_qualified not in file_paths or e.target_qualified not in file_paths:
            continue
        if granularity == "package":
            src = _file_to_package(e.source_qualified)
            tgt = _file_to_package(e.target_qualified)
        else:
            src = e.source_qualified
            tgt = e.target_qualified

        if src == tgt:
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
) -> list[dict]:
    """Find cyclic dependencies (ADP violations).

    Uses nx.simple_cycles on the dependency subgraph (IMPORTS_FROM,
    DEPENDS_ON). Each result includes the nodes in the cycle, its length,
    total edge weight, and a severity score (length × edge_weight).

    Returns list of dicts sorted by severity descending.
    """
    g = _project_dependency_graph(store, granularity=granularity)

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
                }
            )
    except Exception as exc:
        logger.warning("Cycle detection failed: %s", exc)

    violations.sort(key=lambda x: x["severity"], reverse=True)
    return violations


def compute_sdp_metrics(
    store: GraphStore,
    granularity: Literal["file", "package"] = "package",
) -> list[dict]:
    """Compute SDP instability metrics for each module/package.

    Instability I = Ce / (Ca + Ce), where:
    - Ca (afferent couplings) = in-degree: number of modules that import this one
    - Ce (efferent couplings) = out-degree: number of modules this one imports
    - I = 0: maximally stable (others depend on it, it depends on nothing)
    - I = 1: maximally unstable (nothing depends on it, it depends on many things)
    Isolated nodes (Ca + Ce = 0) are assigned I = 0.

    Returns list of dicts sorted by instability descending.
    """
    g = _project_dependency_graph(store, granularity=granularity)

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
            }
        )

    results.sort(key=lambda x: x["instability"], reverse=True)
    return results


def find_sdp_violations(
    store: GraphStore,
    granularity: Literal["file", "package"] = "package",
    min_delta: float = 0.1,
) -> list[dict]:
    """Find SDP violations: dependencies pointing toward instability.

    An edge A -> B violates SDP when I(A) < I(B) - min_delta, i.e., a more
    stable module depends on a less stable one.

    Returns list of dicts sorted by delta descending.
    """
    g = _project_dependency_graph(store, granularity=granularity)

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
                }
            )

    violations.sort(key=lambda x: x["delta"], reverse=True)
    return violations
