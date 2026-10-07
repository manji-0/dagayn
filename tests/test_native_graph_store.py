"""Query behaviour of the native ``GraphStore`` on a small fixed graph.

Replaces the former Python/Rust parity suite: once the Python store became a
facade over the native one, comparing the two compared a store with itself.
These tests pin the answers instead. See: #153
"""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from dagayn.graph import GraphStore
from dagayn.parser import EdgeInfo, NodeInfo

ENTRY, MIDDLE, LEAF = "app.py::entry", "app.py::middle", "app.py::leaf"
TEST_ENTRY = "tests/test_app.py::test_entry"
UNRESOLVED = "<unresolved:missing_symbol>"


def graph_fixture() -> tuple[list[NodeInfo], list[EdgeInfo]]:
    """A small graph exercising every edge kind the checks read."""
    nodes = [
        NodeInfo("File", "app.py", "app.py", 1, 40, "python"),
        NodeInfo("File", "tests/test_app.py", "tests/test_app.py", 1, 10, "python"),
        NodeInfo("File", "README.md", "README.md", 1, 5, "markdown"),
        NodeInfo("Function", "entry", "app.py", 1, 12, "python"),
        NodeInfo("Function", "middle", "app.py", 13, 30, "python"),
        NodeInfo("Function", "leaf", "app.py", 31, 40, "python"),
        NodeInfo("Class", "Base", "app.py", 1, 5, "python"),
        NodeInfo("Class", "Derived", "app.py", 6, 10, "python"),
        NodeInfo("Function", "handle", "app.py", 7, 9, "python", parent_name="Derived"),
        NodeInfo("Function", "handle", "app.py", 2, 4, "python", parent_name="Base"),
        NodeInfo("Test", "test_entry", "tests/test_app.py", 1, 6, "python", is_test=True),
    ]
    edges = [
        EdgeInfo("CALLS", ENTRY, MIDDLE, "app.py", 3),
        EdgeInfo("CALLS", MIDDLE, LEAF, "app.py", 20),
        # A bare-name target, as the parser emits before resolution.
        EdgeInfo("CALLS", ENTRY, "leaf", "app.py", 5),
        EdgeInfo("INHERITS", "app.py::Derived", "app.py::Base", "app.py", 6),
        EdgeInfo("IMPORTS_FROM", "tests/test_app.py", "app.py", "tests/test_app.py", 1),
        EdgeInfo("TESTED_BY", ENTRY, TEST_ENTRY, "app.py", 1),
        EdgeInfo(
            "CROSS_ARTIFACT",
            "README.md",
            ENTRY,
            "README.md",
            2,
            extra={"relationship_role": "describes_symbol", "confidence_tier": "HIGH"},
        ),
        EdgeInfo(
            "CROSS_ARTIFACT",
            "README.md",
            UNRESOLVED,
            "README.md",
            3,
            extra={"relationship_role": "maps_entrypoint", "symbol": "missing_symbol"},
        ),
    ]
    return nodes, edges


@pytest.fixture()
def repo_root(tmp_path: Path) -> Path:
    root = tmp_path / "repo"
    root.mkdir()
    return root


@pytest.fixture()
def store(tmp_path: Path, repo_root: Path):
    from dagayn.search import rebuild_fts_index

    graph = GraphStore(tmp_path / "graph.db")
    nodes, edges = graph_fixture()
    graph.set_metadata("repo_root", str(repo_root))
    by_file: dict[str, tuple[list[NodeInfo], list[EdgeInfo]]] = {}
    for node in nodes:
        by_file.setdefault(node.file_path, ([], []))[0].append(node)
    for edge in edges:
        by_file.setdefault(edge.file_path, ([], []))[1].append(edge)
    for file_path, (file_nodes, file_edges) in by_file.items():
        graph.store_file_nodes_edges(file_path, file_nodes, file_edges)
    graph.compute_missing_signatures()
    rebuild_fts_index(graph)
    graph.commit()
    try:
        yield graph
    finally:
        graph.close()


def pairs(edges) -> list[tuple[str, str]]:
    return sorted((e.source_qualified, e.target_qualified) for e in edges)


def pairs_by_key(grouped) -> dict[str, list[tuple[str, str]]]:
    return {key: pairs(edges) for key, edges in grouped.items()}


def qns(nodes) -> list[str]:
    return [n.qualified_name for n in nodes]


def store_one_flow(store) -> None:
    """Persist a single two-node flow through `entry -> middle`."""
    from dagayn.flows import store_flows

    entry, middle = store.get_node(ENTRY), store.get_node(MIDDLE)
    store_flows(
        store,
        [
            {
                "name": "entry flow",
                "entry_point_id": entry.id,
                "depth": 1,
                "node_count": 2,
                "file_count": 1,
                "criticality": 0.5,
                "path": [entry.id, middle.id],
                "kind": "reachable_set",
                "truncated": False,
                "truncation_reason": None,
            }
        ],
    )
    store.commit()


class TestEdgeQueries:
    def test_get_edges_by_kind(self, store):
        assert pairs(store.get_edges_by_kind("CALLS")) == [
            (ENTRY, MIDDLE),
            (ENTRY, "leaf"),
            (MIDDLE, LEAF),
        ]
        assert pairs(store.get_edges_by_kind("TESTED_BY")) == [(ENTRY, TEST_ENTRY)]
        assert pairs(store.get_edges_by_kind("CROSS_ARTIFACT", unresolved_target_only=True)) == [
            ("README.md", UNRESOLVED)
        ]

    def test_get_edges_by_sources_and_targets(self, store):
        keys = [ENTRY, MIDDLE, LEAF, "leaf"]
        assert pairs_by_key(store.get_edges_by_sources(keys, ["CALLS"])) == {
            ENTRY: [(ENTRY, MIDDLE), (ENTRY, "leaf")],
            MIDDLE: [(MIDDLE, LEAF)],
        }
        assert pairs_by_key(store.get_edges_by_targets(keys, ["CALLS"])) == {
            MIDDLE: [(ENTRY, MIDDLE)],
            LEAF: [(MIDDLE, LEAF)],
            "leaf": [(ENTRY, "leaf")],
        }

    def test_get_edges_by_target_names(self, store):
        names = ["leaf", "middle", "entry"]
        assert pairs_by_key(store.get_edges_by_target_names(names, kind="CALLS")) == {
            "leaf": [(ENTRY, "leaf"), (MIDDLE, LEAF)],
            "middle": [(ENTRY, MIDDLE)],
        }
        # qualified_only drops the bare `leaf` target.
        assert pairs_by_key(
            store.get_edges_by_target_names(names, kind="CALLS", qualified_only=True)
        ) == {"leaf": [(MIDDLE, LEAF)], "middle": [(ENTRY, MIDDLE)]}

    def test_target_name_lookups(self, store):
        assert [store.count_edges_by_target_name_prefix(p) for p in ("l", "middle", "zz")] == [
            2,
            1,
            0,
        ]
        assert [store.has_edge_to_target(t) for t in (MIDDLE, ENTRY, "nope")] == [
            True,
            False,
            False,
        ]
        assert pairs(store.search_edges_by_target_name("leaf")) == [
            (ENTRY, "leaf"),
            (MIDDLE, LEAF),
        ]
        assert pairs(store.search_import_edges_for_symbol("app.py", "entry")) == [
            ("tests/test_app.py", "app.py")
        ]

    def test_edges_among_and_endpoints(self, store):
        assert pairs(store.get_edges_among({ENTRY, MIDDLE, LEAF})) == [
            (ENTRY, MIDDLE),
            (MIDDLE, LEAF),
        ]
        assert sorted(store.get_outgoing_targets([ENTRY, MIDDLE])) == [
            LEAF,
            MIDDLE,
            "leaf",
            TEST_ENTRY,
        ]
        assert sorted(store.get_incoming_sources([ENTRY, MIDDLE])) == ["README.md", ENTRY]


class TestNodeQueries:
    def test_get_nodes_by_size(self, store):
        assert qns(store.get_nodes_by_size(min_lines=1, kind="Function")) == [
            MIDDLE,
            ENTRY,
            LEAF,
            "app.py::Derived.handle",
            "app.py::Base.handle",
        ]
        assert qns(store.get_nodes_by_size(min_lines=1, file_path_pattern="tests")) == [
            "tests/test_app.py",
            TEST_ENTRY,
        ]
        assert qns(store.get_nodes_by_size(min_lines=1, limit=2)) == ["app.py", MIDDLE]
        assert all(
            n.line_end - n.line_start + 1 <= 6
            for n in store.get_nodes_by_size(min_lines=1, max_lines=6)
        )

    def test_count_nodes_by_name(self, store):
        assert store.count_nodes_by_name(["Function", "Class"], False) == {
            "Base": 1,
            "Derived": 1,
            "entry": 1,
            "handle": 2,
            "leaf": 1,
            "middle": 1,
        }

    def test_get_nodes_by_parent_and_name(self, store):
        found = store.get_nodes_by_parent_and_name("Base", "handle", ["Function", "Test"])
        assert qns(found) == ["app.py::Base.handle"]

    def test_resolve_file_path(self, store, repo_root):
        assert store.resolve_file_path("app.py") == repo_root / "app.py"
        assert store.resolve_file_path(str(repo_root / "app.py")) == repo_root / "app.py"

    def test_counts_and_health(self, store):
        stats = store.get_stats()
        assert (stats.total_nodes, stats.total_edges, stats.files_count) == (11, 8, 3)
        assert stats.edges_by_kind["CALLS"] == 3
        assert store.count_non_file_nodes() == 8
        assert store.fts_index_health()["status"] == "synced"

    def test_db_path_is_a_path(self, store):
        """`db_path` is used as a path (`.stat()`), so a `str` breaks callers."""
        assert isinstance(store.db_path, Path)
        assert store.db_path.exists()


class TestSearch:
    def test_fts_query(self, store):
        assert store.fts_query("entry").match_mode == "phrase"
        assert len(store.fts_query("app entry").hits) == 2
        missing = store.fts_query("nonexistent_symbol_xyz")
        assert (missing.match_mode, missing.hits) == ("none", [])

    def test_keyword_and_node_search(self, store):
        hits = store.keyword_query("handle")
        found = store.get_nodes_by_ids([node_id for node_id, _ in hits]).values()
        assert sorted(qns(found)) == ["app.py::Base.handle", "app.py::Derived.handle"]
        assert store.keyword_query("zzz") == []
        assert sorted(qns(store.search_nodes("entry"))) == [ENTRY, TEST_ENTRY]
        assert qns(store.search_nodes("zzz_missing")) == []


class TestSubgraphAndImpact:
    def test_get_subgraph(self, store):
        subgraph = store.get_subgraph([ENTRY, MIDDLE, LEAF])
        assert qns(subgraph["nodes"]) == [ENTRY, MIDDLE, LEAF]
        assert pairs(subgraph["edges"]) == [(ENTRY, MIDDLE), (MIDDLE, LEAF)]

    def test_get_local_subgraph_grows_with_depth(self, store):
        near, _ = store.get_local_subgraph(ENTRY, 1)
        far, adjacency = store.get_local_subgraph(ENTRY, 2)
        assert sorted(near) == ["README.md", ENTRY, MIDDLE, TEST_ENTRY]
        assert sorted(far) == sorted([*near, LEAF])
        assert sorted(adjacency[MIDDLE]) == [ENTRY, LEAF]

    def test_get_impact_radius(self, store):
        result = store.get_impact_radius(["app.py"])
        assert ENTRY in qns(result["changed_nodes"])
        impacted = sorted(qns(result["impacted_nodes"]))
        assert impacted == ["README.md", "tests/test_app.py", TEST_ENTRY]
        assert sorted(result["impacted_files"]) == ["README.md", "tests/test_app.py"]
        assert result["truncated"] is False

    def test_impact_radius_reports_bridges(self, store):
        result = store.get_impact_radius(["README.md"])
        assert [(b["source"], b["target"]) for b in result["bridge_transitions"]] == [
            ("README.md", ENTRY)
        ]
        [caveat] = result["low_confidence_bridges"]
        assert caveat["bridge"]["target"] == UNRESOLVED
        assert caveat["reason_code"] == "low_confidence_cross_artifact_bridge"

    def test_impact_radius_of_nothing_is_empty(self, store):
        for changed in ([], ["missing.py"]):
            result = store.get_impact_radius(changed)
            assert result["changed_nodes"] == result["impacted_nodes"] == []
            assert result["total_impacted"] == 0


class TestCommunitiesAndFlows:
    def test_communities_list(self, store):
        from dagayn.communities import store_communities

        assert list(store.get_communities_list()) == []
        store_communities(
            store,
            [
                {
                    "name": "app",
                    "level": 0,
                    "cohesion": 0.5,
                    "size": 3,
                    "dominant_language": "python",
                    "description": "app core",
                    "members": [ENTRY, MIDDLE, LEAF],
                }
            ],
        )
        store.commit()
        assert [row["name"] for row in store.get_communities_list()] == ["app"]

    def test_flow_lookups_with_stored_flows(self, store):
        store_one_flow(store)
        flow_ids = [flow["id"] for flow in json.loads(store.get_flows_json("criticality", 10))]
        assert len(flow_ids) == 1
        assert store.get_flow_qualified_names_for_flows(flow_ids) == {flow_ids[0]: {ENTRY, MIDDLE}}


class TestMaintenance:
    def test_signature_roundtrip(self, store):
        # `compute_missing_signatures` ran during setup, so nothing is missing.
        assert list(store.get_nodes_without_signature()) == []
        store.update_node_signature(store.get_node(ENTRY).id, "def entry() -> int")
        store.commit()
        assert store.get_node(ENTRY).signature == "def entry() -> int"

    def test_upsert_node_and_edge(self, store):
        node_id = store.upsert_node(NodeInfo("Function", "added", "app.py", 41, 45, "python"))
        edge = EdgeInfo("CALLS", ENTRY, "app.py::added", "app.py", 4)
        edge_id = store.upsert_edge(edge)
        # Upserting again must update in place rather than duplicate.
        assert store.upsert_edge(edge) == edge_id
        store.commit()
        assert store.get_node("app.py::added").id == node_id
        assert pairs(store.get_edges_by_target("app.py::added")) == [(ENTRY, "app.py::added")]

    def test_prune_orphaned_graph_structures(self, store):
        assert store.prune_orphaned_graph_structures() == {}
        store_one_flow(store)
        # Dropping the file deletes the nodes the flow path points at. It also
        # drops their memberships, so only the now-empty flow is left to prune.
        store.remove_files_data(["app.py"])
        store.commit()
        assert store.prune_orphaned_graph_structures() == {"flows": 1}


def test_semantic_search_works_under_native_backend(tmp_path, monkeypatch):
    """`semantic_search_nodes` used to die on `store_conn(store)`. See: #153"""
    monkeypatch.setenv("DAGAYN_BACKEND", "rust")

    from dagayn.incremental_build import full_build
    from dagayn.tools._common import _get_store
    from dagayn.tools.query import semantic_search_nodes

    repo = tmp_path / "proj"
    repo.mkdir()
    (repo / ".git").mkdir()
    (repo / "a.py").write_text(
        "def used():\n    return 1\n\n\ndef main():\n    return used()\n",
        encoding="utf-8",
    )

    store, root = _get_store(str(repo))
    try:
        full_build(root, store)
    finally:
        store.close()

    result = semantic_search_nodes(query="used", repo_root=str(repo), detail_level="verbose")

    assert result["status"] == "ok", result
    assert result["embedding_health"]["status"] != "unknown"


def test_native_store_implements_the_graph_protocols():
    """Tools type against these protocols; the native class must satisfy them."""
    import inspect

    from dagayn._core import GraphStore as NativeGraphStore
    from dagayn.graph._protocol import GraphQueryProtocol, GraphStoreProtocol

    required = {
        name
        for protocol in (GraphStoreProtocol, GraphQueryProtocol)
        for name, _ in inspect.getmembers(protocol, inspect.isfunction)
        if not name.startswith("__")
    }
    assert required
    assert sorted(required - set(dir(NativeGraphStore))) == []
