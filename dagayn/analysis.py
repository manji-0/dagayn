"""Graph analysis: hub detection, bridge nodes, knowledge gaps,
surprise scoring, suggested questions."""

from __future__ import annotations

import dataclasses
import logging
import re
import sqlite3
from collections import Counter
from collections.abc import Mapping
from pathlib import Path, PurePosixPath
from typing import TypedDict

from ._scope import ArtifactScope, node_matches_artifact_scope
from .entry_point_heuristics import has_framework_decorator, matches_entry_name
from .graph import GraphEdge, GraphNode, GraphStore, _sanitize_name
from .graph.sqlite_errors import borrowed_sqlite_connection

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

    The architecture overview and health summary and the change analysis
    summary call several helpers in sequence, each of which would otherwise
    scan the full edge / node tables. Building a single :class:`GraphSnapshot` up front lets each
    helper skip its own SQL and reuse the same in-memory view.
    """

    edges: list[GraphEdge]
    nodes: list[GraphNode]
    community_map: dict[str, int | None]
    in_degree: Counter[str]
    out_degree: Counter[str]
    tested_sources: set[str]
    all_nodes: list[GraphNode] = dataclasses.field(default_factory=list)


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


def persist_centrality_scores(
    store: GraphStore,
    changed_files: list[str] | None = None,
) -> dict[str, int]:
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
    if changed_files:
        scores = store.persist_centrality_scores(list(changed_files))
    else:
        scores = store.persist_centrality_scores()
    return {key: int(value) for key, value in scores.items()}


#: DDL for one ranking variant; ``{s}`` is the table suffix ("" or "_code").
_CENTRALITY_SCORE_DDL = """
        CREATE TABLE IF NOT EXISTS hub_scores{s} (
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
        CREATE TABLE IF NOT EXISTS bridge_scores{s} (
            qualified_name TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            kind TEXT NOT NULL,
            file_path TEXT NOT NULL,
            betweenness REAL NOT NULL,
            community_id INTEGER,
            computed_at REAL NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_hub_scores{s}_total_degree
            ON hub_scores{s}(total_degree DESC);
        CREATE INDEX IF NOT EXISTS idx_bridge_scores{s}_betweenness
            ON bridge_scores{s}(betweenness DESC);"""
_CENTRALITY_SCORE_SCRIPT = (
    _CENTRALITY_SCORE_DDL.format(s="") + _CENTRALITY_SCORE_DDL.format(s="_code") + "\n        "
)


def _ensure_centrality_score_tables(store: GraphStore) -> None:
    with borrowed_sqlite_connection(store) as conn:
        conn.executescript(_CENTRALITY_SCORE_SCRIPT)


def _load_persisted_hub_scores(
    store: GraphStore, top_n: int, *, artifact_scope: ArtifactScope = "all"
) -> list[HubNodeRecord]:
    table = "hub_scores_code" if artifact_scope == "code" else "hub_scores"
    try:
        _ensure_centrality_score_tables(store)
        with borrowed_sqlite_connection(store) as conn:
            rows = conn.execute(
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
        with borrowed_sqlite_connection(store) as conn:
            rows = conn.execute(
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
