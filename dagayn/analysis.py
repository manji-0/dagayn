"""Graph analysis: hub detection, bridge nodes, knowledge gaps,
surprise scoring, suggested questions."""

from __future__ import annotations

import dataclasses
import json
import logging
import math
import re
import sqlite3
import time
from collections import Counter, defaultdict
from collections.abc import Mapping
from pathlib import Path, PurePosixPath
from typing import Callable, TypedDict, cast

from ._scope import ArtifactScope, node_matches_artifact_scope
from .communities import CommunityMetricsPayload
from .cross_artifact import is_reportable_bridge
from .entry_point_heuristics import has_framework_decorator, matches_entry_name
from .graph import GraphEdge, GraphNode, GraphStore, _sanitize_name

logger = logging.getLogger(__name__)


def _sort_key_int(item: Mapping[str, object], field: str) -> int:
    value = item.get(field)
    if isinstance(value, bool):
        return int(value)
    if isinstance(value, int):
        return value
    if isinstance(value, float):
        return int(value)
    return 0


def _sort_key_float(item: Mapping[str, object], field: str) -> float:
    value = item.get(field)
    if isinstance(value, (int, float)):
        return float(value)
    return 0.0


@dataclasses.dataclass(frozen=True)
class GraphSnapshot:
    """Pre-computed slice of the graph shared by analysis helpers.

    :func:`generate_suggested_questions` calls four helpers in sequence,
    each of which independently scans the full edge / node tables. Building
    a single :class:`GraphSnapshot` up front lets each helper skip its own
    SQL and reuse the same in-memory view.
    """

    edges: list[GraphEdge]
    nodes: list[GraphNode]
    community_map: dict[str, int | None]
    in_degree: Counter[str]
    out_degree: Counter[str]
    tested_sources: set[str]
    all_nodes: list[GraphNode] = dataclasses.field(default_factory=list)


@dataclasses.dataclass(frozen=True)
class CommunityEdgeMetrics:
    """Edge-shape metrics for deciding whether a community finding is useful."""

    internal_edges: int
    external_edges: int
    external_degree: int
    cohesion: float
    external_edge_ratio: float


class KnowledgeGapRecord(TypedDict, total=False):
    """Node or community finding returned by :func:`find_knowledge_gaps`."""

    name: str
    qualified_name: str
    kind: str
    file: str
    degree: int
    hotspot_min_degree: int
    community_id: int
    size: int
    internal_edges: int
    external_edges: int
    external_degree: int
    cohesion: float
    external_edge_ratio: float
    classification: str
    evidence: str


class KnowledgeGapMeta(TypedDict):
    thresholds: dict[str, int | float]
    degree_distribution: dict[str, int]
    artifact_scope: ArtifactScope
    include_tests: bool
    scoped_counts: dict[str, int]
    top_n: int
    raw_counts: dict[str, int]
    returned_counts: dict[str, int]
    truncated: bool
    exclusions: dict[str, list[str]]
    classified_noise_counts: dict[str, int]
    classified_noise_examples: dict[str, list[KnowledgeGapRecord]]


class KnowledgeGapsResult(TypedDict):
    untested_hotspots: list[KnowledgeGapRecord]
    single_file_communities: list[KnowledgeGapRecord]
    isolated_nodes: list[KnowledgeGapRecord]
    thin_communities: list[KnowledgeGapRecord]
    _meta: KnowledgeGapMeta


class HubNodeRecord(TypedDict, total=False):
    name: str
    qualified_name: str
    kind: str
    file: str
    in_degree: int
    out_degree: int
    total_degree: int
    community_id: int | None
    score_source: str


class BridgeNodeRecord(TypedDict, total=False):
    name: str
    qualified_name: str
    kind: str
    file: str
    betweenness: float
    community_id: int | None
    score_source: str


class SurpriseConnectionRecord(TypedDict):
    source: str
    source_qualified: str
    target: str
    target_qualified: str
    edge_kind: str
    surprise_score: float
    reasons: list[str]
    source_community: int | None
    target_community: int | None


class SuggestedQuestionRecord(TypedDict):
    category: str
    question: str
    target: str
    priority: str


def build_graph_snapshot(store: GraphStore) -> GraphSnapshot:
    """Build a :class:`GraphSnapshot` with one read of edges/nodes/communities."""
    edges = store.get_all_edges()
    nodes = store.get_all_nodes(exclude_files=True)
    all_nodes = store.get_all_nodes(exclude_files=False)
    community_map = store.get_all_community_ids()
    in_degree: Counter[str] = Counter()
    out_degree: Counter[str] = Counter()
    tested_sources: set[str] = set()
    for e in edges:
        out_degree[e.source_qualified] += 1
        in_degree[e.target_qualified] += 1
        if e.kind == "TESTED_BY":
            tested_sources.add(e.source_qualified)
    return GraphSnapshot(
        edges=edges,
        nodes=nodes,
        community_map=community_map,
        in_degree=in_degree,
        out_degree=out_degree,
        tested_sources=tested_sources,
        all_nodes=all_nodes,
    )


def find_hub_nodes(
    store: GraphStore,
    top_n: int = 10,
    *,
    snapshot: GraphSnapshot | None = None,
    use_persisted: bool = True,
    artifact_scope: ArtifactScope = "all",
    include_tests: bool = True,
) -> list[HubNodeRecord]:
    """Find the most connected nodes (highest in+out degree), excluding File nodes.

    Returns list of dicts with: name, qualified_name, kind, file,
    in_degree, out_degree, total_degree, community_id
    """
    if use_persisted and _persisted_scope_matches(artifact_scope, include_tests):
        persisted = _load_persisted_hub_scores(store, top_n=top_n, artifact_scope=artifact_scope)
        if persisted:
            return persisted

    if snapshot is None:
        snapshot = build_graph_snapshot(store)
    nodes, scoped_edges = _scoped_nodes_and_edges(
        snapshot, artifact_scope=artifact_scope, include_tests=include_tests
    )
    in_degree, out_degree = _degree_counters(scoped_edges)
    community_map = snapshot.community_map

    scored: list[HubNodeRecord] = []
    for n in nodes:
        qn = n.qualified_name
        ind = in_degree.get(qn, 0)
        outd = out_degree.get(qn, 0)
        total = ind + outd
        if total == 0:
            continue
        scored.append(
            {
                "name": _sanitize_name(n.name),
                "qualified_name": n.qualified_name,
                "kind": n.kind,
                "file": n.file_path,
                "in_degree": ind,
                "out_degree": outd,
                "total_degree": total,
                "community_id": community_map.get(qn),
            }
        )

    scored.sort(
        key=lambda x: _sort_key_int(x, "total_degree"),
        reverse=True,
    )
    return scored[:top_n]


def find_bridge_nodes(
    store: GraphStore,
    top_n: int = 10,
    *,
    snapshot: GraphSnapshot | None = None,
    use_persisted: bool = True,
    artifact_scope: ArtifactScope = "all",
    include_tests: bool = True,
) -> list[BridgeNodeRecord]:
    """Find nodes with highest betweenness centrality.

    These are architectural chokepoints that sit on shortest paths
    between many node pairs. If they break, multiple communities
    lose connectivity.

    Returns list of dicts with: name, qualified_name, kind, file,
    betweenness, community_id
    """
    if use_persisted and _persisted_scope_matches(artifact_scope, include_tests):
        persisted = _load_persisted_bridge_scores(store, top_n=top_n, artifact_scope=artifact_scope)
        if persisted:
            return persisted

    if snapshot is None:
        snapshot = build_graph_snapshot(store)
    nodes, scoped_edges = _scoped_nodes_and_edges(
        snapshot, artifact_scope=artifact_scope, include_tests=include_tests
    )
    node_map = {n.qualified_name: n for n in nodes}

    # Build a scoped graph so documentation and test fixtures do not dominate
    # production architecture bridge rankings.
    import networkx as nx

    nxg = nx.DiGraph()
    nxg.add_nodes_from(node_map)
    nxg.add_edges_from((e.source_qualified, e.target_qualified) for e in scoped_edges)
    # Compute betweenness centrality (approximate for large graphs)
    n_nodes = nxg.number_of_nodes()
    if n_nodes > 5000:
        # Sample-based approximation for large graphs
        k = min(500, n_nodes)
        bc = nx.betweenness_centrality(nxg, k=k, normalized=True, seed=0)
    elif n_nodes > 0:
        bc = nx.betweenness_centrality(nxg, normalized=True)
    else:
        return []

    community_map = snapshot.community_map

    results: list[BridgeNodeRecord] = []
    for qn, score in bc.items():
        if score <= 0 or qn not in node_map:
            continue
        n = node_map[qn]
        if n.kind == "File":
            continue
        results.append(
            {
                "name": _sanitize_name(n.name),
                "qualified_name": n.qualified_name,
                "kind": n.kind,
                "file": n.file_path,
                "betweenness": round(score, 6),
                "community_id": community_map.get(qn),
            }
        )

    results.sort(
        key=lambda x: _sort_key_float(x, "betweenness"),
        reverse=True,
    )
    return results[:top_n]


def _persisted_scope_matches(artifact_scope: ArtifactScope, include_tests: bool) -> bool:
    """Whether persisted hub/bridge scores cover this analysis scope.

    The persistence pass stores an all-scope variant (tests included) and a
    code-scope variant (tests excluded); other scope combinations (docs, or
    all/code with the opposite test setting) must be computed on demand.
    """
    return (artifact_scope == "all" and include_tests) or (
        artifact_scope == "code" and not include_tests
    )


def persist_centrality_scores(store: GraphStore) -> dict[str, int]:
    """Compute and persist hub / bridge scores for query-time analysis.

    Bridge centrality is the expensive part of architecture analysis. Persisting
    the values during post-processing keeps MCP calls on the read path unless a
    graph write invalidates the score tables.

    Two variants are persisted: the all-scope ranking (tests included, used by
    ``artifact_scope="all"`` queries) and the code-scope ranking (tests and
    Markdown excluded, used by the default ``artifact_scope="code"`` tool
    calls). Each lands in its own table so loaders can pick the matching
    ranking without re-computing betweenness.
    """
    rust_persist = getattr(store, "persist_centrality_scores", None)
    if callable(rust_persist):
        try:
            scores = cast(Callable[[], dict[str, int]], rust_persist)()
            return {key: int(value) for key, value in scores.items()}
        except Exception:  # noqa: BLE001 — native acceleration must be optional
            if not hasattr(store, "_conn"):
                raise
            logger.debug("Native centrality persist failed; falling back", exc_info=True)

    _ensure_centrality_score_tables(store)
    snapshot = build_graph_snapshot(store)
    hubs = find_hub_nodes(
        store, top_n=10**9, snapshot=snapshot, use_persisted=False, artifact_scope="all"
    )
    bridges = find_bridge_nodes(
        store, top_n=10**9, snapshot=snapshot, use_persisted=False, artifact_scope="all"
    )
    hubs_code = find_hub_nodes(
        store,
        top_n=10**9,
        snapshot=snapshot,
        use_persisted=False,
        artifact_scope="code",
        include_tests=False,
    )
    bridges_code = find_bridge_nodes(
        store,
        top_n=10**9,
        snapshot=snapshot,
        use_persisted=False,
        artifact_scope="code",
        include_tests=False,
    )
    now = time.time()
    with store._conn:
        store._conn.execute("DELETE FROM hub_scores")
        store._conn.execute("DELETE FROM bridge_scores")
        store._conn.execute("DELETE FROM hub_scores_code")
        store._conn.execute("DELETE FROM bridge_scores_code")
        store._conn.executemany(
            "INSERT INTO hub_scores "
            "(qualified_name, name, kind, file_path, in_degree, out_degree, total_degree, "
            "community_id, computed_at) "
            "VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            [
                (
                    h["qualified_name"],
                    h["name"],
                    h["kind"],
                    h["file"],
                    int(h["in_degree"]),
                    int(h["out_degree"]),
                    int(h["total_degree"]),
                    h.get("community_id"),
                    now,
                )
                for h in hubs
            ],
        )
        store._conn.executemany(
            "INSERT INTO bridge_scores "
            "(qualified_name, name, kind, file_path, betweenness, community_id, computed_at) "
            "VALUES (?, ?, ?, ?, ?, ?, ?)",
            [
                (
                    b["qualified_name"],
                    b["name"],
                    b["kind"],
                    b["file"],
                    float(b["betweenness"]),
                    b.get("community_id"),
                    now,
                )
                for b in bridges
            ],
        )
        store._conn.executemany(
            "INSERT INTO hub_scores_code "
            "(qualified_name, name, kind, file_path, in_degree, out_degree, total_degree, "
            "community_id, computed_at) "
            "VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            [
                (
                    h["qualified_name"],
                    h["name"],
                    h["kind"],
                    h["file"],
                    int(h["in_degree"]),
                    int(h["out_degree"]),
                    int(h["total_degree"]),
                    h.get("community_id"),
                    now,
                )
                for h in hubs_code
            ],
        )
        store._conn.executemany(
            "INSERT INTO bridge_scores_code "
            "(qualified_name, name, kind, file_path, betweenness, community_id, computed_at) "
            "VALUES (?, ?, ?, ?, ?, ?, ?)",
            [
                (
                    b["qualified_name"],
                    b["name"],
                    b["kind"],
                    b["file"],
                    float(b["betweenness"]),
                    b.get("community_id"),
                    now,
                )
                for b in bridges_code
            ],
        )
    return {
        "hub_scores_persisted": len(hubs),
        "bridge_scores_persisted": len(bridges),
        "hub_scores_code_persisted": len(hubs_code),
        "bridge_scores_code_persisted": len(bridges_code),
    }


def _ensure_centrality_score_tables(store: GraphStore) -> None:
    store._conn.executescript(
        """
        CREATE TABLE IF NOT EXISTS hub_scores (
            qualified_name TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            kind TEXT NOT NULL,
            file_path TEXT NOT NULL,
            in_degree INTEGER NOT NULL,
            out_degree INTEGER NOT NULL,
            total_degree INTEGER NOT NULL,
            community_id INTEGER,
            computed_at REAL NOT NULL
        );
        CREATE TABLE IF NOT EXISTS bridge_scores (
            qualified_name TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            kind TEXT NOT NULL,
            file_path TEXT NOT NULL,
            betweenness REAL NOT NULL,
            community_id INTEGER,
            computed_at REAL NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_hub_scores_total_degree
            ON hub_scores(total_degree DESC);
        CREATE INDEX IF NOT EXISTS idx_bridge_scores_betweenness
            ON bridge_scores(betweenness DESC);
        CREATE TABLE IF NOT EXISTS hub_scores_code (
            qualified_name TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            kind TEXT NOT NULL,
            file_path TEXT NOT NULL,
            in_degree INTEGER NOT NULL,
            out_degree INTEGER NOT NULL,
            total_degree INTEGER NOT NULL,
            community_id INTEGER,
            computed_at REAL NOT NULL
        );
        CREATE TABLE IF NOT EXISTS bridge_scores_code (
            qualified_name TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            kind TEXT NOT NULL,
            file_path TEXT NOT NULL,
            betweenness REAL NOT NULL,
            community_id INTEGER,
            computed_at REAL NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_hub_scores_code_total_degree
            ON hub_scores_code(total_degree DESC);
        CREATE INDEX IF NOT EXISTS idx_bridge_scores_code_betweenness
            ON bridge_scores_code(betweenness DESC);
        """
    )


def _load_persisted_hub_scores(
    store: GraphStore, top_n: int, *, artifact_scope: ArtifactScope = "all"
) -> list[HubNodeRecord]:
    table = "hub_scores_code" if artifact_scope == "code" else "hub_scores"
    try:
        _ensure_centrality_score_tables(store)
        rows = store._conn.execute(
            f"SELECT name, qualified_name, kind, file_path, in_degree, out_degree, "
            f"total_degree, community_id "
            f"FROM {table} ORDER BY total_degree DESC, qualified_name LIMIT ?",  # noqa: S608
            (top_n,),
        ).fetchall()
    except sqlite3.OperationalError:
        return []
    return [
        {
            "name": row["name"],
            "qualified_name": row["qualified_name"],
            "kind": row["kind"],
            "file": row["file_path"],
            "in_degree": row["in_degree"],
            "out_degree": row["out_degree"],
            "total_degree": row["total_degree"],
            "community_id": row["community_id"],
            "score_source": "persisted",
        }
        for row in rows
    ]


def _load_persisted_bridge_scores(
    store: GraphStore, top_n: int, *, artifact_scope: ArtifactScope = "all"
) -> list[BridgeNodeRecord]:
    table = "bridge_scores_code" if artifact_scope == "code" else "bridge_scores"
    try:
        _ensure_centrality_score_tables(store)
        rows = store._conn.execute(
            f"SELECT name, qualified_name, kind, file_path, betweenness, community_id "
            f"FROM {table} ORDER BY betweenness DESC, qualified_name LIMIT ?",  # noqa: S608
            (top_n,),
        ).fetchall()
    except sqlite3.OperationalError:
        return []
    return [
        {
            "name": row["name"],
            "qualified_name": row["qualified_name"],
            "kind": row["kind"],
            "file": row["file_path"],
            "betweenness": row["betweenness"],
            "community_id": row["community_id"],
            "score_source": "persisted",
        }
        for row in rows
    ]


def find_knowledge_gaps(
    store: GraphStore,
    top_n: int = 20,
    *,
    snapshot: GraphSnapshot | None = None,
    artifact_scope: ArtifactScope = "all",
    include_tests: bool = True,
) -> KnowledgeGapsResult:
    """Identify structural weaknesses in the codebase graph.

    Returns dict with categories:
    - isolated_nodes: degree <= isolated_max_degree, disconnected from graph
    - thin_communities: fewer than thin_community_min_size members
    - untested_hotspots: degree >= p95 degree threshold and no TESTED_BY edges
    - single_file_communities: entire community in one file

    The hotspot threshold is derived from the observed non-file node degree
    distribution instead of a fixed language-specific constant.
    """
    if snapshot is None:
        snapshot = build_graph_snapshot(store)
    nodes, scoped_edges = _scoped_nodes_and_edges(
        snapshot, artifact_scope=artifact_scope, include_tests=include_tests
    )
    community_map = snapshot.community_map

    # Build degree map from snapshot's pre-computed counters.
    in_degree, out_degree = _degree_counters(scoped_edges)
    degree: Counter[str] = Counter()
    for qn, c in in_degree.items():
        degree[qn] += c
    for qn, c in out_degree.items():
        degree[qn] += c
    full_degree: Counter[str] = Counter()
    for qn, c in snapshot.in_degree.items():
        full_degree[qn] += c
    for qn, c in snapshot.out_degree.items():
        full_degree[qn] += c
    scoped_qns = {n.qualified_name for n in nodes}
    tested_nodes = {qn for qn in snapshot.tested_sources if qn in scoped_qns}
    positive_degrees = sorted(
        full_degree.get(n.qualified_name, 0)
        for n in nodes
        if not _is_analysis_excluded_from_test_gap(n) and full_degree.get(n.qualified_name, 0) > 0
    )
    source_cache: dict[str, list[str]] = {}
    degree_p95 = _nearest_rank_percentile(positive_degrees, 0.95)
    hotspot_min_degree = max(5, degree_p95)
    top_n = max(1, top_n)
    noise_example_limit = min(top_n, 10)
    category_keys = (
        "untested_hotspots",
        "single_file_communities",
        "isolated_nodes",
        "thin_communities",
    )

    # 1. Isolated nodes (degree <= 1, not File)
    isolated: list[KnowledgeGapRecord] = []
    low_signal_isolated: list[KnowledgeGapRecord] = []
    for n in nodes:
        d = degree.get(n.qualified_name, 0)
        if d <= 1:
            item: KnowledgeGapRecord = {
                "name": _sanitize_name(n.name),
                "qualified_name": n.qualified_name,
                "kind": n.kind,
                "file": n.file_path,
                "degree": d,
            }
            low_signal_reason = _low_signal_isolated_reason(store, n, source_cache)
            if low_signal_reason:
                low_signal_isolated.append({**item, "classification": low_signal_reason})
            else:
                isolated.append(item)

    # 2. Build community sizes and file maps from node data
    comm_sizes: Counter[int] = Counter()
    comm_files: dict[int, set[str]] = defaultdict(set)
    qn_to_community: dict[str, int] = {}
    for n in nodes:
        cid = community_map.get(n.qualified_name)
        if cid is not None:
            comm_sizes[cid] += 1
            comm_files[cid].add(n.file_path)
            qn_to_community[n.qualified_name] = cid
    community_edge_metrics = _community_edge_metrics(comm_sizes, qn_to_community, scoped_edges)

    # Thin communities (< 3 members)
    communities = store.get_communities_list()
    thin: list[KnowledgeGapRecord] = []
    small_single_file_thin: list[KnowledgeGapRecord] = []
    for c in communities:
        cid = int(c["id"])
        if cid not in comm_sizes:
            continue
        size = comm_sizes.get(cid, 0)
        if size < 3:
            item: KnowledgeGapRecord = {
                "community_id": cid,
                "name": str(c["name"]),
                "size": size,
            }
            if len(comm_files.get(cid, set())) == 1:
                file_path = next(iter(comm_files[cid]))
                metrics = _community_metrics_payload(community_edge_metrics.get(cid))
                small_single_file_thin.append(
                    {
                        **item,
                        "file": file_path,
                        **metrics,
                        "classification": "small_single_file_cluster",
                    }
                )
            else:
                thin.append(item)

    # 3. Untested hotspots (p95 production-candidate degree, no TESTED_BY)
    untested_hotspots: list[KnowledgeGapRecord] = []
    for n in nodes:
        d = full_degree.get(n.qualified_name, 0)
        if (
            d >= hotspot_min_degree
            and n.qualified_name not in tested_nodes
            and not _is_analysis_excluded_from_test_gap(n)
        ):
            untested_hotspots.append(
                {
                    "name": _sanitize_name(n.name),
                    "qualified_name": n.qualified_name,
                    "kind": n.kind,
                    "file": n.file_path,
                    "degree": d,
                    "hotspot_min_degree": hotspot_min_degree,
                    "evidence": (
                        "degree is at or above the repository p95 non-file "
                        "degree threshold and no TESTED_BY edge starts from it"
                    ),
                }
            )
    untested_hotspots.sort(key=lambda x: _sort_key_int(x, "degree"), reverse=True)

    # 4. Single-file communities
    single_file: list[KnowledgeGapRecord] = []
    natural_single_file: list[KnowledgeGapRecord] = []
    small_single_file: list[KnowledgeGapRecord] = []
    integrated_single_file: list[KnowledgeGapRecord] = []
    for c in communities:
        cid = int(c["id"])
        if cid not in comm_sizes:
            continue
        files = comm_files.get(cid, set())
        size = comm_sizes.get(cid, 0)
        if len(files) == 1 and size >= 3:
            file_path = next(iter(files))
            metrics = _community_metrics_payload(community_edge_metrics.get(cid))
            item: KnowledgeGapRecord = {
                "community_id": cid,
                "name": str(c["name"]),
                "size": size,
                "file": file_path,
                **metrics,
            }
            natural_reason = _natural_single_file_community_reason(file_path)
            if natural_reason:
                natural_single_file.append({**item, "classification": natural_reason})
            elif size < 10:
                small_single_file.append({**item, "classification": "small_single_file_cluster"})
            elif _is_integrated_single_file_community(size, community_edge_metrics.get(cid)):
                integrated_single_file.append(
                    {**item, "classification": "integrated_single_file_component"}
                )
            else:
                single_file.append(
                    {
                        **item,
                        "evidence": (
                            "community members are concentrated in one file and have limited "
                            "external graph connectivity"
                        ),
                    }
                )

    raw_counts: dict[str, int] = {
        "untested_hotspots": len(untested_hotspots),
        "single_file_communities": len(single_file),
        "isolated_nodes": len(isolated),
        "thin_communities": len(thin),
    }
    returned_counts: dict[str, int] = {
        "untested_hotspots": len(untested_hotspots[:top_n]),
        "single_file_communities": len(single_file[:top_n]),
        "isolated_nodes": len(isolated[:top_n]),
        "thin_communities": len(thin[:top_n]),
    }
    result: KnowledgeGapsResult = {
        "untested_hotspots": untested_hotspots[:top_n],
        "single_file_communities": single_file[:top_n],
        "isolated_nodes": isolated[:top_n],
        "thin_communities": thin[:top_n],
        "_meta": {
            "thresholds": {
                "isolated_max_degree": 1,
                "thin_community_min_size": 3,
                "single_file_min_size": 3,
                "untested_hotspot_min_degree": hotspot_min_degree,
                "untested_hotspot_percentile": 0.95,
            },
            "degree_distribution": {
                "candidate_positive_degree_count": len(positive_degrees),
                "p95_degree": degree_p95,
            },
            "artifact_scope": artifact_scope,
            "include_tests": include_tests,
            "scoped_counts": {
                "nodes": len(nodes),
                "edges": len(scoped_edges),
            },
            "top_n": top_n,
            "raw_counts": raw_counts,
            "returned_counts": returned_counts,
            "truncated": any(raw_counts[key] > returned_counts[key] for key in category_keys),
            "exclusions": {
                "isolated_nodes": [
                    "public API candidates, conventional entry points, test-only nodes, "
                    "and implementation-block containers",
                ],
                "thin_communities": [
                    "single-file clusters with fewer than 3 members",
                ],
                "untested_hotspots": [
                    "test nodes and test-like file paths",
                    "markdown documentation sections",
                ],
                "single_file_communities": [
                    "natural standalone repo documents such as README, LICENSE, "
                    "SECURITY, CODE_OF_CONDUCT",
                    "single-file clusters with fewer than 10 members",
                    "single-file communities with enough external graph connectivity "
                    "to look like integrated components",
                ],
            },
            "classified_noise_counts": {
                "low_signal_isolated_nodes": len(low_signal_isolated),
                "small_single_file_thin_communities": len(small_single_file_thin),
                "natural_single_file_communities": len(natural_single_file),
                "small_single_file_communities": len(small_single_file),
                "integrated_single_file_communities": len(integrated_single_file),
            },
            "classified_noise_examples": {
                "low_signal_isolated_nodes": low_signal_isolated[:noise_example_limit],
                "small_single_file_thin_communities": small_single_file_thin[:noise_example_limit],
                "natural_single_file_communities": natural_single_file[:noise_example_limit],
                "small_single_file_communities": small_single_file[:noise_example_limit],
                "integrated_single_file_communities": integrated_single_file[:noise_example_limit],
            },
        },
    }
    return result


def _community_edge_metrics(
    comm_sizes: Counter[int],
    qn_to_community: dict[str, int],
    edges: list[GraphEdge],
) -> dict[int, CommunityEdgeMetrics]:
    """Compute internal/external community edge shape for scoped graph edges."""
    internal: Counter[int] = Counter()
    external: Counter[int] = Counter()
    external_neighbors: dict[int, set[str]] = defaultdict(set)
    for edge in edges:
        source_cid = qn_to_community.get(edge.source_qualified)
        target_cid = qn_to_community.get(edge.target_qualified)
        if source_cid is None and target_cid is None:
            continue
        if source_cid is not None and source_cid == target_cid:
            internal[source_cid] += 1
            continue
        if source_cid is not None:
            external[source_cid] += 1
            external_neighbors[source_cid].add(edge.target_qualified)
        if target_cid is not None:
            external[target_cid] += 1
            external_neighbors[target_cid].add(edge.source_qualified)

    metrics: dict[int, CommunityEdgeMetrics] = {}
    for cid, size in comm_sizes.items():
        internal_edges = internal.get(cid, 0)
        external_edges = external.get(cid, 0)
        max_internal_edges = max(1, size * (size - 1))
        edge_total = internal_edges + external_edges
        metrics[cid] = CommunityEdgeMetrics(
            internal_edges=internal_edges,
            external_edges=external_edges,
            external_degree=len(external_neighbors.get(cid, set())),
            cohesion=round(min(1.0, internal_edges / max_internal_edges), 4),
            external_edge_ratio=round(external_edges / edge_total, 4) if edge_total else 0.0,
        )
    return metrics


def _community_metrics_payload(
    metrics: CommunityEdgeMetrics | None,
) -> CommunityMetricsPayload:
    if metrics is None:
        return {
            "internal_edges": 0,
            "external_edges": 0,
            "external_degree": 0,
            "cohesion": 0.0,
            "external_edge_ratio": 0.0,
        }
    return dataclasses.asdict(metrics)


def _is_integrated_single_file_community(size: int, metrics: CommunityEdgeMetrics | None) -> bool:
    """Classify large one-file communities that are visibly connected elsewhere."""
    if metrics is None:
        return False
    min_external_degree = max(3, math.ceil(size * 0.2))
    return metrics.external_degree >= min_external_degree and metrics.external_edge_ratio >= 0.25


def _nearest_rank_percentile(values: list[int], percentile: float) -> int:
    """Return a deterministic nearest-rank percentile for integer metrics."""
    if not values:
        return 0
    rank = math.ceil(percentile * len(values))
    index = min(max(rank - 1, 0), len(values) - 1)
    return values[index]


def _is_analysis_excluded_from_test_gap(node: GraphNode) -> bool:
    """Filter nodes where missing TESTED_BY is not production test-risk evidence."""
    if node.is_test or node.kind == "Test" or node.language == "markdown":
        return True
    path = PurePosixPath(node.file_path.replace("\\", "/"))
    name = path.name.lower()
    parts = {part.lower() for part in path.parts}
    if "tests" in parts or "test" in parts or "__tests__" in parts:
        return True
    return (
        name.startswith("test_")
        or name in {"test.rs", "tests.rs"}
        or name.endswith("_test.py")
        or name.endswith("_tests.py")
        or name.endswith("_test.rs")
        or name.endswith("_tests.rs")
        or ".test." in name
        or ".spec." in name
    )


def _load_source_lines_for_node(
    store: GraphStore, file_path: str, source_cache: dict[str, list[str]]
) -> list[str]:
    """Read source lines once per file for source-level signal classification."""
    if file_path in source_cache:
        return source_cache[file_path]
    try:
        path = store.resolve_file_path(file_path)
    except (AttributeError, TypeError):
        path = Path(file_path)
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except (OSError, UnicodeDecodeError):
        lines = []
    source_cache[file_path] = lines
    return lines


def _source_line(lines: list[str], line_number: int | None) -> str:
    if line_number is None or line_number <= 0 or line_number > len(lines):
        return ""
    return lines[line_number - 1].strip()


_RUST_PUBLIC_ITEM_RE = re.compile(r"^pub(\([^)]*\))?\s+(fn|struct|enum|trait|type|const|static)\b")
_JS_PUBLIC_ITEM_MARKERS = (
    "export ",
    "export default ",
    "export async ",
    "export function ",
    "export class ",
    "export interface ",
    "export const ",
    "export let ",
    "export var ",
)


def _low_signal_isolated_reason(
    store: GraphStore, node: GraphNode, source_cache: dict[str, list[str]]
) -> str | None:
    """Classify isolated nodes that are expected to have few internal graph edges."""
    if matches_entry_name(node) or has_framework_decorator(node):
        return "entry_point"
    lines = _load_source_lines_for_node(store, node.file_path, source_cache)
    line = _source_line(lines, node.line_start)
    if not line:
        return None
    if _is_rust_cfg_test_candidate(node, lines):
        return "test_candidate"
    if node.language == "rust" and node.kind == "Class" and line.startswith("impl "):
        return "implementation_block"
    if node.language == "rust" and _RUST_PUBLIC_ITEM_RE.match(line):
        return "public_api_candidate"
    if node.language in {"typescript", "tsx", "javascript", "vue", "svelte"}:
        if line.startswith(_JS_PUBLIC_ITEM_MARKERS) or " export " in f" {line} ":
            return "public_api_candidate"
    if line.startswith(("public ", "export ")):
        return "public_api_candidate"
    return None


def _is_rust_cfg_test_candidate(node: GraphNode, lines: list[str]) -> bool:
    """Return whether a Rust node sits under a local #[cfg(test)] tests module."""
    if node.language != "rust":
        return False
    line_number = node.line_start
    if not isinstance(line_number, int) or line_number <= 0:
        return False
    target_idx = min(line_number - 1, len(lines) - 1)
    for idx in range(target_idx, -1, -1):
        line = lines[idx]
        if "mod tests" not in line:
            continue
        window = "\n".join(lines[max(0, idx - 3) : idx + 1])
        if "#[cfg(test)]" not in window:
            continue
        depth = 0
        for scoped_line in lines[idx : target_idx + 1]:
            depth += scoped_line.count("{")
            depth -= scoped_line.count("}")
        if depth > 0:
            return True
    return False


def _natural_single_file_community_reason(file_path: str) -> str | None:
    """Classify standalone repository documents that are expected to stay isolated."""
    path = PurePosixPath(file_path.replace("\\", "/"))
    name = path.name.lower()
    stem = path.stem.lower()

    if name.startswith("readme") and path.suffix.lower() in {".md", ".rst", ".txt"}:
        return "standalone_readme"
    if stem in {
        "license",
        "licence",
        "copying",
        "security",
        "code_of_conduct",
        "contributing",
        "authors",
        "contributors",
        "changelog",
        "changes",
        "release_notes",
    }:
        return f"standalone_{stem}"
    if name in {
        "license",
        "licence",
        "copying",
        "notice",
        "authors",
        "contributors",
        "changelog",
    }:
        return f"standalone_{name}"
    return None


def find_surprising_connections(
    store: GraphStore,
    top_n: int = 15,
    *,
    snapshot: GraphSnapshot | None = None,
    artifact_scope: ArtifactScope = "all",
    include_tests: bool = True,
) -> list[SurpriseConnectionRecord]:
    """Find edges with high surprise scores.

    Detects unexpected architectural coupling based on:
    - Cross-community: source and target in different communities
    - Cross-language: different file languages
    - Peripheral-to-hub: low-degree node to high-degree node
    - Cross-file-type: test calling production or vice versa
    - Non-standard edge kind for the node types
    """
    if snapshot is None:
        snapshot = build_graph_snapshot(store)
    nodes, edges = _scoped_nodes_and_edges(
        snapshot, artifact_scope=artifact_scope, include_tests=include_tests
    )
    community_map = snapshot.community_map
    node_map = {n.qualified_name: n for n in nodes}

    # Build degree map from snapshot's pre-computed counters.
    in_degree, out_degree = _degree_counters(edges)
    degree: Counter[str] = Counter()
    for qn, c in in_degree.items():
        degree[qn] += c
    for qn, c in out_degree.items():
        degree[qn] += c

    # Median degree for peripheral detection
    degrees = [d for d in degree.values() if d > 0]
    if not degrees:
        return []
    median_deg = sorted(degrees)[len(degrees) // 2]
    high_deg_threshold = max(median_deg * 3, 10)
    max_degree = max(degrees)

    pair_counts: Counter[tuple[int, int, str]] = Counter()
    for e in edges:
        src_cid = community_map.get(e.source_qualified)
        tgt_cid = community_map.get(e.target_qualified)
        if src_cid is None or tgt_cid is None or src_cid == tgt_cid:
            continue
        pair_counts[(min(src_cid, tgt_cid), max(src_cid, tgt_cid), e.kind)] += 1

    scored_edges: list[SurpriseConnectionRecord] = []
    for e in edges:
        if e.kind == "CONTAINS":
            continue
        src = node_map.get(e.source_qualified)
        tgt = node_map.get(e.target_qualified)
        if not src or not tgt:
            continue
        if src.kind == "File" or tgt.kind == "File":
            continue

        score = 0.0
        reasons = []
        boundary_signal = False

        # Cross-community (+0.3)
        src_cid = community_map.get(e.source_qualified)
        tgt_cid = community_map.get(e.target_qualified)
        if src_cid is not None and tgt_cid is not None and src_cid != tgt_cid:
            score += 0.3
            reasons.append("cross-community")
            boundary_signal = True
            pair_key = (min(src_cid, tgt_cid), max(src_cid, tgt_cid), e.kind)
            rarity_bonus = min(0.05, round(0.05 / pair_counts[pair_key], 3))
            score += rarity_bonus
            if rarity_bonus:
                reasons.append("rare-community-pair")

        # Cross-language (+0.2)
        src_lang = src.file_path.rsplit(".", 1)[-1] if "." in src.file_path else ""
        tgt_lang = tgt.file_path.rsplit(".", 1)[-1] if "." in tgt.file_path else ""
        if src_lang and tgt_lang and src_lang != tgt_lang:
            score += 0.2
            reasons.append("cross-language")
            boundary_signal = True

        # Peripheral-to-hub (+0.2)
        src_deg = degree.get(e.source_qualified, 0)
        tgt_deg = degree.get(e.target_qualified, 0)
        if (src_deg <= 2 and tgt_deg >= high_deg_threshold) or (
            tgt_deg <= 2 and src_deg >= high_deg_threshold
        ):
            score += 0.2
            reasons.append("peripheral-to-hub")

        degree_imbalance_bonus = round(
            min(0.09, (abs(src_deg - tgt_deg) / max(max_degree, 1)) * 0.09),
            3,
        )
        if degree_imbalance_bonus:
            score += degree_imbalance_bonus
            reasons.append("degree-imbalance")

        # Cross-file-type: test <-> non-test (+0.15)
        if src.is_test != tgt.is_test and e.kind == "CALLS":
            score += 0.15
            reasons.append("cross-test-boundary")
            boundary_signal = True

        # Explicit reportable cross-artifact bridge (+0.25)
        if is_reportable_bridge(e):
            score += 0.25
            reasons.append("cross-artifact-bridge")
            boundary_signal = True

        # Non-standard edge kind (+0.15)
        if e.kind == "CALLS" and src.kind == "Type":
            score += 0.15
            reasons.append("unusual-edge-kind")
            boundary_signal = True

        if score > 0 and boundary_signal:
            scored_edges.append(
                {
                    "source": _sanitize_name(src.name),
                    "source_qualified": e.source_qualified,
                    "target": _sanitize_name(tgt.name),
                    "target_qualified": e.target_qualified,
                    "edge_kind": e.kind,
                    "surprise_score": round(score, 3),
                    "reasons": reasons,
                    "source_community": src_cid,
                    "target_community": tgt_cid,
                }
            )

    scored_edges.sort(
        key=lambda x: _sort_key_float(x, "surprise_score"),
        reverse=True,
    )
    return scored_edges[:top_n]


def _scoped_nodes_and_edges(
    snapshot: GraphSnapshot,
    *,
    artifact_scope: ArtifactScope,
    include_tests: bool,
) -> tuple[list[GraphNode], list[GraphEdge]]:
    """Return nodes and internal edges that belong to the requested analysis scope."""
    nodes = [
        n
        for n in snapshot.nodes
        if node_matches_artifact_scope(n, artifact_scope)
        and (include_tests or not _is_analysis_excluded_from_test_gap(n))
    ]
    scoped_qns = {n.qualified_name for n in nodes}
    edges = [
        e
        for e in snapshot.edges
        if e.source_qualified in scoped_qns and e.target_qualified in scoped_qns
    ]
    return nodes, edges


def _degree_counters(edges: list[GraphEdge]) -> tuple[Counter[str], Counter[str]]:
    """Build in/out degree counters for a scoped edge set."""
    in_degree: Counter[str] = Counter()
    out_degree: Counter[str] = Counter()
    for e in edges:
        out_degree[e.source_qualified] += 1
        in_degree[e.target_qualified] += 1
    return in_degree, out_degree


def generate_suggested_questions(
    store: GraphStore,
) -> list[SuggestedQuestionRecord]:
    """Auto-generate review questions from graph analysis.

    Categories:
    - bridge_node: Why does X connect communities A and B?
    - isolated_node: Is X dead code or dynamically invoked?
    - low_cohesion: Should community X be split?
    - hub_risk: Does hub node X have adequate test coverage?
    - surprising: Why does A call B across community boundary?
    """
    native_questions = _generate_suggested_questions_native(store)
    if native_questions is not None:
        return native_questions

    questions: list[SuggestedQuestionRecord] = []
    snapshot = build_graph_snapshot(store)

    # Bridge node questions
    bridges = find_bridge_nodes(
        store,
        top_n=3,
        snapshot=snapshot,
        artifact_scope="code",
        include_tests=False,
    )
    for b in bridges:
        questions.append(
            {
                "category": "bridge_node",
                "question": (
                    f"'{b['name']}' is a critical connector "
                    f"between multiple code regions. Is it "
                    f"adequately tested and documented?"
                ),
                "target": b["qualified_name"],
                "priority": "high",
            }
        )

    # Hub risk questions
    hubs = find_hub_nodes(
        store,
        top_n=3,
        snapshot=snapshot,
        artifact_scope="code",
        include_tests=False,
    )
    tested = snapshot.tested_sources
    for h in hubs:
        if h["qualified_name"] not in tested:
            questions.append(
                {
                    "category": "hub_risk",
                    "question": (
                        f"Hub node '{h['name']}' has "
                        f"{h['total_degree']} connections but no "
                        f"direct test coverage. Should it be "
                        f"tested?"
                    ),
                    "target": h["qualified_name"],
                    "priority": "high",
                }
            )

    # Surprising connection questions
    surprises = find_surprising_connections(store, top_n=3, snapshot=snapshot)
    for s in surprises:
        if "cross-community" in s["reasons"]:
            questions.append(
                {
                    "category": "surprising_connection",
                    "question": (
                        f"'{s['source']}' (community "
                        f"{s['source_community']}) calls "
                        f"'{s['target']}' (community "
                        f"{s['target_community']}). Is this "
                        f"coupling intentional?"
                    ),
                    "target": s["source_qualified"],
                    "priority": "medium",
                }
            )

    # Knowledge gap questions
    gaps = find_knowledge_gaps(store, snapshot=snapshot)

    for c in gaps["thin_communities"][:2]:
        questions.append(
            {
                "category": "thin_community",
                "question": (
                    f"Community '{c['name']}' has only "
                    f"{c['size']} member(s). Should it be "
                    f"merged with a neighbor?"
                ),
                "target": f"community:{c['community_id']}",
                "priority": "low",
            }
        )

    for h in gaps["untested_hotspots"][:2]:
        questions.append(
            {
                "category": "untested_hotspot",
                "question": (
                    f"'{h['name']}' has {h['degree']} "
                    f"connections but no test coverage. "
                    f"Is this a risk?"
                ),
                "target": h["qualified_name"],
                "priority": "medium",
            }
        )

    return questions


def _generate_suggested_questions_native(store: GraphStore) -> list[SuggestedQuestionRecord] | None:
    native_generate = getattr(store, "generate_suggested_questions_json", None)
    if not callable(native_generate):
        return None
    try:
        raw = cast(Callable[[], str], native_generate)()
        decoded = json.loads(raw)
    except Exception:  # noqa: BLE001  # native acceleration must be optional
        logger.debug(
            "Native suggested-question generation failed; falling back to Python",
            exc_info=True,
        )
        return None
    if not isinstance(decoded, list) or not all(isinstance(item, dict) for item in decoded):
        return None
    return cast(list[SuggestedQuestionRecord], decoded)
