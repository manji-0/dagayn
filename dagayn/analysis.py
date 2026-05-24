"""Graph analysis: hub detection, bridge nodes, knowledge gaps,
surprise scoring, suggested questions."""

from __future__ import annotations

import dataclasses
import json
import logging
import math
import sqlite3
import time
from collections import Counter, defaultdict
from pathlib import PurePosixPath
from typing import Any

from .graph import GraphEdge, GraphNode, GraphStore, _sanitize_name

logger = logging.getLogger(__name__)


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


def build_graph_snapshot(store: GraphStore) -> GraphSnapshot:
    """Build a :class:`GraphSnapshot` with one read of edges/nodes/communities."""
    edges = store.get_all_edges()
    nodes = store.get_all_nodes(exclude_files=True)
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
    )


def find_hub_nodes(
    store: GraphStore,
    top_n: int = 10,
    *,
    snapshot: GraphSnapshot | None = None,
    use_persisted: bool = True,
) -> list[dict]:
    """Find the most connected nodes (highest in+out degree), excluding File nodes.

    Returns list of dicts with: name, qualified_name, kind, file,
    in_degree, out_degree, total_degree, community_id
    """
    if use_persisted:
        persisted = _load_persisted_hub_scores(store, top_n=top_n)
        if persisted:
            return persisted

    if snapshot is None:
        snapshot = build_graph_snapshot(store)
    in_degree = snapshot.in_degree
    out_degree = snapshot.out_degree
    nodes = snapshot.nodes
    community_map = snapshot.community_map

    scored = []
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
        key=lambda x: x.get("total_degree", 0),  # type: ignore[arg-type,return-value]
        reverse=True,
    )
    return scored[:top_n]


def find_bridge_nodes(
    store: GraphStore,
    top_n: int = 10,
    *,
    snapshot: GraphSnapshot | None = None,
    use_persisted: bool = True,
) -> list[dict]:
    """Find nodes with highest betweenness centrality.

    These are architectural chokepoints that sit on shortest paths
    between many node pairs. If they break, multiple communities
    lose connectivity.

    Returns list of dicts with: name, qualified_name, kind, file,
    betweenness, community_id
    """
    if use_persisted:
        persisted = _load_persisted_bridge_scores(store, top_n=top_n)
        if persisted:
            return persisted

    import networkx as nx

    # Build the graph — use cached version if available
    nxg = store._build_networkx_graph()

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

    if snapshot is None:
        snapshot = build_graph_snapshot(store)
    community_map = snapshot.community_map
    node_map = {n.qualified_name: n for n in snapshot.nodes}

    results = []
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
        key=lambda x: float(x.get("betweenness", 0)),  # type: ignore[arg-type,return-value]
        reverse=True,
    )
    return results[:top_n]


def persist_centrality_scores(store: GraphStore) -> dict[str, int]:
    """Compute and persist hub / bridge scores for query-time analysis.

    Bridge centrality is the expensive part of architecture analysis. Persisting
    the values during post-processing keeps MCP calls on the read path unless a
    graph write invalidates the score tables.
    """
    rust_persist = getattr(store, "persist_centrality_scores", None)
    if callable(rust_persist) and not hasattr(store, "_conn"):
        return {key: int(value) for key, value in dict(rust_persist()).items()}

    _ensure_centrality_score_tables(store)
    snapshot = build_graph_snapshot(store)
    hubs = find_hub_nodes(store, top_n=10**9, snapshot=snapshot, use_persisted=False)
    bridges = find_bridge_nodes(store, top_n=10**9, snapshot=snapshot, use_persisted=False)
    now = time.time()
    with store._conn:
        store._conn.execute("DELETE FROM hub_scores")
        store._conn.execute("DELETE FROM bridge_scores")
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
    return {"hub_scores_persisted": len(hubs), "bridge_scores_persisted": len(bridges)}


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
        """
    )


def _load_persisted_hub_scores(store: GraphStore, top_n: int) -> list[dict]:
    try:
        _ensure_centrality_score_tables(store)
        rows = store._conn.execute(
            "SELECT name, qualified_name, kind, file_path, in_degree, out_degree, "
            "total_degree, community_id "
            "FROM hub_scores ORDER BY total_degree DESC, qualified_name LIMIT ?",
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


def _load_persisted_bridge_scores(store: GraphStore, top_n: int) -> list[dict]:
    try:
        _ensure_centrality_score_tables(store)
        rows = store._conn.execute(
            "SELECT name, qualified_name, kind, file_path, betweenness, community_id "
            "FROM bridge_scores ORDER BY betweenness DESC, qualified_name LIMIT ?",
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
) -> dict[str, Any]:
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
    nodes = snapshot.nodes
    community_map = snapshot.community_map

    # Build degree map from snapshot's pre-computed counters.
    degree: Counter[str] = Counter()
    for qn, c in snapshot.in_degree.items():
        degree[qn] += c
    for qn, c in snapshot.out_degree.items():
        degree[qn] += c
    tested_nodes = snapshot.tested_sources
    positive_degrees = sorted(
        degree.get(n.qualified_name, 0)
        for n in nodes
        if not _is_analysis_excluded_from_test_gap(n) and degree.get(n.qualified_name, 0) > 0
    )
    degree_p95 = _nearest_rank_percentile(positive_degrees, 0.95)
    hotspot_min_degree = max(5, degree_p95)
    top_n = max(1, top_n)
    category_keys = (
        "isolated_nodes",
        "thin_communities",
        "untested_hotspots",
        "single_file_communities",
    )

    # 1. Isolated nodes (degree <= 1, not File)
    isolated = []
    for n in nodes:
        d = degree.get(n.qualified_name, 0)
        if d <= 1:
            isolated.append(
                {
                    "name": _sanitize_name(n.name),
                    "qualified_name": n.qualified_name,
                    "kind": n.kind,
                    "file": n.file_path,
                    "degree": d,
                }
            )

    # 2. Build community sizes and file maps from node data
    comm_sizes: Counter[int] = Counter()
    comm_files: dict[int, set[str]] = defaultdict(set)
    for n in nodes:
        cid = community_map.get(n.qualified_name)
        if cid is not None:
            comm_sizes[cid] += 1
            comm_files[cid].add(n.file_path)

    # Thin communities (< 3 members)
    communities = store.get_communities_list()
    thin = []
    for c in communities:
        cid = int(c["id"])
        size = comm_sizes.get(cid, 0)
        if size < 3:
            thin.append(
                {
                    "community_id": cid,
                    "name": str(c["name"]),
                    "size": size,
                }
            )

    # 3. Untested hotspots (p95 production-candidate degree, no TESTED_BY)
    untested_hotspots = []
    for n in nodes:
        d = degree.get(n.qualified_name, 0)
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
    untested_hotspots.sort(
        key=lambda x: x.get("degree", 0),  # type: ignore[arg-type,return-value]
        reverse=True,
    )

    # 4. Single-file communities
    single_file = []
    natural_single_file = []
    for c in communities:
        cid = int(c["id"])
        files = comm_files.get(cid, set())
        size = comm_sizes.get(cid, 0)
        if len(files) == 1 and size >= 3:
            file_path = next(iter(files))
            item = {
                "community_id": cid,
                "name": str(c["name"]),
                "size": size,
                "file": file_path,
            }
            natural_reason = _natural_single_file_community_reason(file_path)
            if natural_reason:
                natural_single_file.append({**item, "classification": natural_reason})
            else:
                single_file.append(item)

    raw_counts = {
        "isolated_nodes": len(isolated),
        "thin_communities": len(thin),
        "untested_hotspots": len(untested_hotspots),
        "single_file_communities": len(single_file),
    }
    returned = {
        "isolated_nodes": isolated[:top_n],
        "thin_communities": thin[:top_n],
        "untested_hotspots": untested_hotspots[:top_n],
        "single_file_communities": single_file[:top_n],
    }
    returned_counts = {key: len(returned[key]) for key in category_keys}
    return {
        **returned,
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
            "top_n": top_n,
            "raw_counts": raw_counts,
            "returned_counts": returned_counts,
            "truncated": any(raw_counts[key] > returned_counts[key] for key in category_keys),
            "exclusions": {
                "untested_hotspots": [
                    "test nodes and test-like file paths",
                    "markdown documentation sections",
                ],
                "single_file_communities": [
                    "natural standalone repo documents such as README, LICENSE, "
                    "SECURITY, CODE_OF_CONDUCT",
                ],
            },
            "classified_noise_counts": {
                "natural_single_file_communities": len(natural_single_file),
            },
            "classified_noise_examples": {
                "natural_single_file_communities": natural_single_file[:top_n],
            },
        },
    }


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
) -> list[dict]:
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
    edges = snapshot.edges
    community_map = snapshot.community_map
    node_map = {n.qualified_name: n for n in snapshot.nodes}

    # Build degree map from snapshot's pre-computed counters.
    degree: Counter[str] = Counter()
    for qn, c in snapshot.in_degree.items():
        degree[qn] += c
    for qn, c in snapshot.out_degree.items():
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

    scored_edges = []
    for e in edges:
        src = node_map.get(e.source_qualified)
        tgt = node_map.get(e.target_qualified)
        if not src or not tgt:
            continue
        if src.kind == "File" or tgt.kind == "File":
            continue

        score = 0.0
        reasons = []

        # Cross-community (+0.3)
        src_cid = community_map.get(e.source_qualified)
        tgt_cid = community_map.get(e.target_qualified)
        if src_cid is not None and tgt_cid is not None and src_cid != tgt_cid:
            score += 0.3
            reasons.append("cross-community")
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

        # Non-standard edge kind (+0.15)
        if e.kind == "CALLS" and src.kind == "Type":
            score += 0.15
            reasons.append("unusual-edge-kind")

        if score > 0:
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
        key=lambda x: float(x.get("surprise_score", 0)),  # type: ignore[arg-type,return-value]
        reverse=True,
    )
    return scored_edges[:top_n]


def generate_suggested_questions(
    store: GraphStore,
) -> list[dict]:
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

    questions = []
    snapshot = build_graph_snapshot(store)

    # Bridge node questions
    bridges = find_bridge_nodes(store, top_n=3, snapshot=snapshot)
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
    hubs = find_hub_nodes(store, top_n=3, snapshot=snapshot)
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


def _generate_suggested_questions_native(store: GraphStore) -> list[dict] | None:
    native_generate = getattr(store, "generate_suggested_questions_json", None)
    if not callable(native_generate):
        return None
    try:
        raw = native_generate()
        decoded = json.loads(raw)
    except Exception:  # noqa: BLE001  # native acceleration must be optional
        logger.debug(
            "Native suggested-question generation failed; falling back to Python",
            exc_info=True,
        )
        return None
    if not isinstance(decoded, list):
        return None
    return decoded
