"""Native `GraphStore` parity with the Python one.

Two halves:

- **Surface**: every public Python `GraphStore` method exists on the native
  store, so a tool can hold either without probing for a method's existence.
- **Behaviour**: for one graph built through each backend, the same call returns
  the same answer.

See: #153
"""

from __future__ import annotations

import inspect
from dataclasses import asdict
from pathlib import Path

import pytest

from dagayn.graph import GraphStore as PythonGraphStore
from dagayn.parser import EdgeInfo, NodeInfo

#: Python-store internals that are deliberately not mirrored natively. Each is
#: either an implementation detail of the SQLite/NetworkX layer or a bulk-write
#: helper whose native counterpart is `store_file_batch`.
PRIVATE_ONLY = frozenset(
    {
        "_batch_get_nodes",
        "_build_networkx_graph",
        "_bulk_insert_edges",
        "_bulk_insert_nodes",
        "_bulk_insert_nodes_with_meta",
        "_checkpoint_wal_on_close",
        "_edges_by_endpoint_column",
        "_get_impact_radius_networkx",
        "_init_schema",
        "_low_confidence_bridges_near_seeds",
        "_make_qualified",
        "_repair_stale_flow_paths",
        "_row_to_edge",
        "_row_to_node",
    }
)


def rust_graph_store():
    try:
        from dagayn._core import GraphStore as RustGraphStore
    except ImportError as exc:  # pragma: no cover - depends on build config
        pytest.skip(f"Rust extension is not available: {exc}")
    return RustGraphStore


def graph_fixture():
    """A small graph exercising every edge kind the parity checks read."""
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
        EdgeInfo("CALLS", "app.py::entry", "app.py::middle", "app.py", 3),
        EdgeInfo("CALLS", "app.py::middle", "app.py::leaf", "app.py", 20),
        EdgeInfo("CALLS", "app.py::entry", "leaf", "app.py", 5),
        EdgeInfo("INHERITS", "app.py::Derived", "app.py::Base", "app.py", 6),
        EdgeInfo("IMPORTS_FROM", "tests/test_app.py", "app.py", "tests/test_app.py", 1),
        EdgeInfo("TESTED_BY", "app.py::entry", "tests/test_app.py::test_entry", "app.py", 1),
        EdgeInfo(
            "CROSS_ARTIFACT",
            "README.md",
            "app.py::entry",
            "README.md",
            2,
            extra={"relationship_role": "describes_symbol", "confidence_tier": "HIGH"},
        ),
        EdgeInfo(
            "CROSS_ARTIFACT",
            "README.md",
            "<unresolved:missing_symbol>",
            "README.md",
            3,
            extra={"relationship_role": "maps_entrypoint", "symbol": "missing_symbol"},
        ),
    ]
    return nodes, edges


def build_store(store, repo_root: Path) -> None:
    from dagayn.postprocessing import PostprocessResult, _compute_signatures
    from dagayn.search import rebuild_fts_index

    nodes, edges = graph_fixture()
    store.set_metadata("repo_root", str(repo_root))
    by_file: dict[str, tuple[list[NodeInfo], list[EdgeInfo]]] = {}
    for node in nodes:
        by_file.setdefault(node.file_path, ([], []))[0].append(node)
    for edge in edges:
        by_file.setdefault(edge.file_path, ([], []))[1].append(edge)
    for file_path, (file_nodes, file_edges) in by_file.items():
        store.store_file_nodes_edges(file_path, file_nodes, file_edges)
    store.commit()
    # Signatures and the FTS index are what `fts_query` / `search_nodes` read,
    # so both must exist before the search parity checks run.
    _compute_signatures(store, PostprocessResult(), [])
    rebuild_fts_index(store)
    store.commit()


@pytest.fixture()
def stores(tmp_path):
    """One graph per backend over identical inputs, plus the repo root."""
    rust_cls = rust_graph_store()
    repo_root = tmp_path / "repo"
    repo_root.mkdir()

    python_store = PythonGraphStore(tmp_path / "python.db")
    rust_store = rust_cls(tmp_path / "rust.db")
    try:
        build_store(python_store, repo_root)
        build_store(rust_store, repo_root)
        yield python_store, rust_store, repo_root
    finally:
        python_store.close()
        rust_store.close()


def test_native_store_implements_every_public_python_method():
    """No public Python `GraphStore` method is missing natively. See: #153"""
    rust_cls = rust_graph_store()

    python_methods = {
        name
        for name, value in inspect.getmembers(PythonGraphStore)
        if callable(value) and not name.startswith("__")
    }
    native_members = {name for name in dir(rust_cls) if not name.startswith("__")}

    missing = sorted(python_methods - native_members - PRIVATE_ONLY)
    assert missing == [], f"native GraphStore is missing: {missing}"


def test_private_only_list_stays_private():
    """`PRIVATE_ONLY` must not accumulate public names."""
    public = sorted(name for name in PRIVATE_ONLY if not name.startswith("_"))
    assert public == []


def store_one_flow(store) -> None:
    """Persist a single two-node flow through `entry -> middle`."""
    from dagayn.flows import store_flows

    entry = store.get_node("app.py::entry")
    middle = store.get_node("app.py::middle")
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


def node_key(node) -> tuple:
    return (node.kind, node.name, node.qualified_name, node.file_path, node.signature)


def edge_key(edge) -> tuple:
    return (
        edge.kind,
        edge.source_qualified,
        edge.target_qualified,
        edge.file_path,
        edge.line,
        edge.confidence_tier,
    )


class TestQueryParity:
    def test_get_edges_by_kind(self, stores):
        python_store, rust_store, _ = stores
        for kind in ("CALLS", "INHERITS", "IMPORTS_FROM", "TESTED_BY", "CROSS_ARTIFACT"):
            assert sorted(edge_key(e) for e in python_store.get_edges_by_kind(kind)) == sorted(
                edge_key(e) for e in rust_store.get_edges_by_kind(kind)
            ), kind

    def test_get_edges_by_kind_unresolved_only(self, stores):
        python_store, rust_store, _ = stores
        expected = sorted(
            edge_key(e)
            for e in python_store.get_edges_by_kind("CROSS_ARTIFACT", unresolved_target_only=True)
        )
        assert expected == sorted(
            edge_key(e)
            for e in rust_store.get_edges_by_kind("CROSS_ARTIFACT", unresolved_target_only=True)
        )
        assert len(expected) == 1

    def test_get_edges_by_sources_and_targets(self, stores):
        python_store, rust_store, _ = stores
        qns = ["app.py::entry", "app.py::middle", "app.py::leaf", "leaf"]

        for kinds in (None, ["CALLS"], ["TESTED_BY"]):
            py_out = python_store.get_edges_by_sources(qns, kinds)
            rs_out = rust_store.get_edges_by_sources(qns, kinds)
            assert {k: sorted(edge_key(e) for e in v) for k, v in py_out.items()} == {
                k: sorted(edge_key(e) for e in v) for k, v in rs_out.items()
            }

            py_out = python_store.get_edges_by_targets(qns, kinds)
            rs_out = rust_store.get_edges_by_targets(qns, kinds)
            assert {k: sorted(edge_key(e) for e in v) for k, v in py_out.items()} == {
                k: sorted(edge_key(e) for e in v) for k, v in rs_out.items()
            }

    def test_get_edges_by_target_names(self, stores):
        python_store, rust_store, _ = stores
        names = ["leaf", "middle", "entry"]
        for qualified_only in (False, True):
            py_out = python_store.get_edges_by_target_names(
                names, kind="CALLS", qualified_only=qualified_only
            )
            rs_out = rust_store.get_edges_by_target_names(
                names, kind="CALLS", qualified_only=qualified_only
            )
            assert {k: sorted(edge_key(e) for e in v) for k, v in py_out.items()} == {
                k: sorted(edge_key(e) for e in v) for k, v in rs_out.items()
            }

    def test_count_edges_by_target_name_prefix(self, stores):
        python_store, rust_store, _ = stores
        for prefix in ("l", "middle", "zz"):
            assert python_store.count_edges_by_target_name_prefix(
                prefix
            ) == rust_store.count_edges_by_target_name_prefix(prefix)

    def test_has_edge_to_target(self, stores):
        python_store, rust_store, _ = stores
        for target in ("app.py::middle", "app.py::entry", "nope"):
            assert python_store.has_edge_to_target(target) == rust_store.has_edge_to_target(target)

    def test_search_edges_by_target_name(self, stores):
        python_store, rust_store, _ = stores
        assert sorted(
            edge_key(e) for e in python_store.search_edges_by_target_name("leaf")
        ) == sorted(edge_key(e) for e in rust_store.search_edges_by_target_name("leaf"))

    def test_search_import_edges_for_symbol(self, stores):
        python_store, rust_store, _ = stores
        py_out = python_store.search_import_edges_for_symbol("app.py", "entry")
        rs_out = rust_store.search_import_edges_for_symbol("app.py", "entry")
        assert sorted(edge_key(e) for e in py_out) == sorted(edge_key(e) for e in rs_out)
        assert py_out

    def test_get_edges_among(self, stores):
        python_store, rust_store, _ = stores
        qns = {"app.py::entry", "app.py::middle", "app.py::leaf"}
        assert sorted(edge_key(e) for e in python_store.get_edges_among(qns)) == sorted(
            edge_key(e) for e in rust_store.get_edges_among(qns)
        )

    def test_outgoing_and_incoming_endpoints(self, stores):
        python_store, rust_store, _ = stores
        qns = ["app.py::entry", "app.py::middle"]
        assert sorted(python_store.get_outgoing_targets(qns)) == sorted(
            rust_store.get_outgoing_targets(qns)
        )
        assert sorted(python_store.get_incoming_sources(qns)) == sorted(
            rust_store.get_incoming_sources(qns)
        )

    def test_get_node_by_id(self, stores):
        python_store, rust_store, _ = stores
        node = python_store.get_node("app.py::entry")
        assert node is not None
        assert node_key(python_store.get_node_by_id(node.id)) == node_key(
            rust_store.get_node_by_id(node.id)
        )
        assert python_store.get_node_by_id(10**9) is None
        assert rust_store.get_node_by_id(10**9) is None

    def test_get_nodes_by_size(self, stores):
        python_store, rust_store, _ = stores
        cases = [
            {},
            {"min_lines": 10},
            {"min_lines": 1, "max_lines": 6},
            {"min_lines": 1, "kind": "Function"},
            {"min_lines": 1, "file_path_pattern": "tests"},
            {"min_lines": 1, "limit": 2},
        ]
        for kwargs in cases:
            assert [node_key(n) for n in python_store.get_nodes_by_size(**kwargs)] == [
                node_key(n) for n in rust_store.get_nodes_by_size(**kwargs)
            ], kwargs

    def test_count_nodes_by_name(self, stores):
        python_store, rust_store, _ = stores
        for include_tests in (False, True):
            assert python_store.count_nodes_by_name(
                ["Function", "Class"], include_tests
            ) == rust_store.count_nodes_by_name(["Function", "Class"], include_tests)

    def test_get_nodes_by_parent_and_name(self, stores):
        python_store, rust_store, _ = stores
        py_out = python_store.get_nodes_by_parent_and_name("Base", "handle", ["Function", "Test"])
        rs_out = rust_store.get_nodes_by_parent_and_name("Base", "handle", ["Function", "Test"])
        assert sorted(node_key(n) for n in py_out) == sorted(node_key(n) for n in rs_out)
        assert py_out

    def test_get_node_ids_by_files(self, stores):
        python_store, rust_store, _ = stores
        # Ids differ between the two databases, so compare cardinality and the
        # nodes those ids resolve to rather than the raw ids.
        files = ["app.py", "tests/test_app.py"]
        py_ids = python_store.get_node_ids_by_files(files)
        rs_ids = rust_store.get_node_ids_by_files(files)
        assert len(py_ids) == len(rs_ids)
        assert sorted(
            node_key(n) for n in python_store.get_nodes_by_ids(list(py_ids)).values()
        ) == sorted(node_key(n) for n in rust_store.get_nodes_by_ids(list(rs_ids)).values())

    def test_resolve_file_path(self, stores):
        python_store, rust_store, repo_root = stores
        assert python_store.resolve_file_path("app.py") == rust_store.resolve_file_path("app.py")
        assert rust_store.resolve_file_path("app.py") == repo_root / "app.py"
        absolute = str(repo_root / "app.py")
        assert python_store.resolve_file_path(absolute) == rust_store.resolve_file_path(absolute)

    def test_normalize_key_helpers(self, stores):
        python_store, rust_store, repo_root = stores
        for candidate in ("app.py", str(repo_root / "app.py")):
            assert python_store._normalize_file_path_key(
                candidate
            ) == rust_store._normalize_file_path_key(candidate)
        for candidate in ("app.py::entry", f"{repo_root}/app.py::entry"):
            assert python_store._normalize_qualified_key(
                candidate
            ) == rust_store._normalize_qualified_key(candidate)


class TestSearchParity:
    def test_fts_query(self, stores):
        python_store, rust_store, _ = stores
        for query in ("entry", "app.py::entry", "app entry", "nonexistent_symbol_xyz"):
            py_result = python_store.fts_query(query)
            rs_result = rust_store.fts_query(query)
            assert py_result.match_mode == rs_result.match_mode, query
            assert len(py_result.hits) == len(rs_result.hits), query

    def test_keyword_query_matches_same_nodes(self, stores):
        python_store, rust_store, _ = stores
        for query in ("entry", "handle", "zzz"):
            py_hits = python_store.keyword_query(query)
            rs_hits = rust_store.keyword_query(query)
            assert sorted(score for _, score in py_hits) == sorted(score for _, score in rs_hits), (
                query
            )
            assert sorted(
                node_key(n) for n in python_store.get_nodes_by_ids([i for i, _ in py_hits]).values()
            ) == sorted(
                node_key(n) for n in rust_store.get_nodes_by_ids([i for i, _ in rs_hits]).values()
            ), query

    def test_search_nodes(self, stores):
        python_store, rust_store, _ = stores
        for query in ("entry", "middle", "zzz_missing"):
            assert sorted(node_key(n) for n in python_store.search_nodes(query)) == sorted(
                node_key(n) for n in rust_store.search_nodes(query)
            ), query


class TestSubgraphParity:
    def test_get_subgraph(self, stores):
        python_store, rust_store, _ = stores
        qns = ["app.py::entry", "app.py::middle", "app.py::leaf"]
        py_out = python_store.get_subgraph(qns)
        rs_out = rust_store.get_subgraph(qns)
        assert [node_key(n) for n in py_out["nodes"]] == [node_key(n) for n in rs_out["nodes"]]
        assert sorted(edge_key(e) for e in py_out["edges"]) == sorted(
            edge_key(e) for e in rs_out["edges"]
        )

    def test_get_local_subgraph(self, stores):
        python_store, rust_store, _ = stores
        for depth in (1, 2):
            py_nodes, py_adj = python_store.get_local_subgraph("app.py::entry", depth)
            rs_nodes, rs_adj = rust_store.get_local_subgraph("app.py::entry", depth)
            assert sorted(py_nodes) == sorted(rs_nodes), depth
            assert {k: sorted(v) for k, v in py_adj.items()} == {
                k: sorted(v) for k, v in rs_adj.items()
            }, depth


class TestImpactRadiusParity:
    def test_get_impact_radius(self, stores):
        python_store, rust_store, _ = stores
        py_out = python_store.get_impact_radius(["app.py"])
        rs_out = rust_store.get_impact_radius(["app.py"])

        assert sorted(node_key(n) for n in py_out["changed_nodes"]) == sorted(
            node_key(n) for n in rs_out["changed_nodes"]
        )
        assert [node_key(n) for n in py_out["impacted_nodes"]] == [
            node_key(n) for n in rs_out["impacted_nodes"]
        ]
        assert sorted(py_out["impacted_files"]) == sorted(rs_out["impacted_files"])
        assert sorted(edge_key(e) for e in py_out["edges"]) == sorted(
            edge_key(e) for e in rs_out["edges"]
        )
        assert py_out["truncated"] == rs_out["truncated"]
        assert py_out["total_impacted"] == rs_out["total_impacted"]

    def test_bridge_records_match(self, stores):
        python_store, rust_store, _ = stores
        py_out = python_store.get_impact_radius(["README.md"])
        rs_out = rust_store.get_impact_radius(["README.md"])

        def sort_key(record):
            return (record.get("source") or "", record.get("target") or "")

        assert sorted(py_out["bridge_transitions"], key=sort_key) == sorted(
            rs_out["bridge_transitions"], key=sort_key
        )
        assert sorted(
            py_out["low_confidence_bridges"], key=lambda r: sort_key(r["bridge"])
        ) == sorted(rs_out["low_confidence_bridges"], key=lambda r: sort_key(r["bridge"]))
        assert py_out["bridge_transitions"]
        assert py_out["low_confidence_bridges"]

    def test_empty_changed_files(self, stores):
        python_store, rust_store, _ = stores
        assert python_store.get_impact_radius([]) == rust_store.get_impact_radius([])
        assert python_store.get_impact_radius(["missing.py"]) == rust_store.get_impact_radius(
            ["missing.py"]
        )


class TestCommunityAndFlowParity:
    def test_get_communities_list_empty_before_detection(self, stores):
        python_store, rust_store, _ = stores
        assert list(python_store.get_communities_list()) == []
        assert list(rust_store.get_communities_list()) == []

    def test_get_communities_list_after_detection(self, stores):
        python_store, rust_store, _ = stores
        from dagayn.communities import store_communities

        for store in (python_store, rust_store):
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
                        "members": ["app.py::entry", "app.py::middle", "app.py::leaf"],
                    }
                ],
            )
            store.commit()

        py_rows = [(row["id"], row["name"]) for row in python_store.get_communities_list()]
        rs_rows = [(row["id"], row["name"]) for row in rust_store.get_communities_list()]
        assert py_rows == rs_rows
        assert py_rows

        community_id = py_rows[0][0]
        assert sorted(python_store.get_community_member_qns(community_id)) == sorted(
            rust_store.get_community_member_qns(community_id)
        )

    def test_flow_lookups_without_flows(self, stores):
        python_store, rust_store, _ = stores
        assert python_store.get_flow_ids_by_node_ids(set()) == rust_store.get_flow_ids_by_node_ids(
            set()
        )
        assert python_store.get_flow_qualified_names(1) == rust_store.get_flow_qualified_names(1)

    def test_flow_lookups_with_stored_flows(self, stores):
        python_store, rust_store, _ = stores

        for store in (python_store, rust_store):
            store_one_flow(store)

        for store in (python_store, rust_store):
            node_ids = store.get_node_ids_by_files(["app.py"])
            flow_ids = store.get_flow_ids_by_node_ids(node_ids)
            assert len(flow_ids) == 1
            assert store.get_flow_qualified_names(flow_ids[0]) == {
                "app.py::entry",
                "app.py::middle",
            }
            assert store.get_flow_qualified_names_for_flows(flow_ids) == {
                flow_ids[0]: {"app.py::entry", "app.py::middle"}
            }


class TestMaintenanceParity:
    def test_signature_roundtrip(self, stores):
        python_store, rust_store, _ = stores
        # `compute_missing_signatures` ran during setup, so nothing is missing.
        assert list(python_store.get_nodes_without_signature()) == []
        assert list(rust_store.get_nodes_without_signature()) == []

        for store in (python_store, rust_store):
            node = store.get_node("app.py::entry")
            store.update_node_signature(node.id, "def entry() -> int")
            store.commit()

        assert (
            python_store.get_node("app.py::entry").signature
            == rust_store.get_node("app.py::entry").signature
            == "def entry() -> int"
        )

    def test_upsert_node_and_edge(self, stores):
        python_store, rust_store, _ = stores
        node = NodeInfo("Function", "added", "app.py", 41, 45, "python")
        edge = EdgeInfo("CALLS", "app.py::entry", "app.py::added", "app.py", 4)

        for store in (python_store, rust_store):
            node_id = store.upsert_node(node)
            assert node_id > 0
            edge_id = store.upsert_edge(edge)
            assert edge_id > 0
            # Upserting again must update in place rather than duplicate.
            assert store.upsert_edge(edge) == edge_id
            store.commit()

        assert node_key(python_store.get_node("app.py::added")) == node_key(
            rust_store.get_node("app.py::added")
        )
        assert sorted(
            edge_key(e) for e in python_store.get_edges_by_target("app.py::added")
        ) == sorted(edge_key(e) for e in rust_store.get_edges_by_target("app.py::added"))

    def test_remove_node_keyed_rows_for_files(self, stores):
        python_store, rust_store, _ = stores
        for store in (python_store, rust_store):
            store.remove_node_keyed_rows_for_files(["app.py"])
            store.commit()
        # Nodes themselves are untouched by this call.
        assert len(python_store.get_nodes_by_file("app.py")) == len(
            rust_store.get_nodes_by_file("app.py")
        )

    def test_prune_orphaned_graph_structures_with_nothing_to_prune(self, stores):
        python_store, rust_store, _ = stores
        assert python_store.prune_orphaned_graph_structures() == (
            rust_store.prune_orphaned_graph_structures()
        )

    def test_prune_orphaned_graph_structures_after_nodes_disappear(self, stores):
        """A flow whose whole path was deleted must be pruned by both stores."""
        python_store, rust_store, _ = stores

        for store in (python_store, rust_store):
            store_one_flow(store)
            # Dropping the file deletes the nodes the flow path points at,
            # leaving the flow and its memberships orphaned.
            store.remove_files_data(["app.py"])
            store.commit()

        py_deleted = python_store.prune_orphaned_graph_structures()
        rs_deleted = rust_store.prune_orphaned_graph_structures()
        assert py_deleted == rs_deleted
        # `remove_files_data` already dropped the node-keyed memberships, so the
        # sweep only has the now-empty flow left to delete.
        assert py_deleted == {"flows": 1}

        for store in (python_store, rust_store):
            assert store.get_flow_ids_by_node_ids({1, 2, 3}) == []

    def test_get_stats_still_matches(self, stores):
        python_store, rust_store, _ = stores
        assert asdict(python_store.get_stats()) == asdict(rust_store.get_stats())


class TestAttributeParity:
    def test_db_path_is_a_path_on_both_stores(self, stores):
        """`db_path` is used as a path (`.stat()`), so a `str` breaks callers."""
        python_store, rust_store, _ = stores
        assert isinstance(python_store.db_path, Path)
        assert isinstance(rust_store.db_path, Path)
        assert rust_store.db_path.exists()

    def test_fts_index_health(self, stores):
        python_store, rust_store, _ = stores
        assert python_store.fts_index_health() == rust_store.fts_index_health()
        assert python_store.fts_index_health()["status"] == "synced"

    def test_count_non_file_nodes(self, stores):
        python_store, rust_store, _ = stores
        assert python_store.count_non_file_nodes() == rust_store.count_non_file_nodes()
        assert python_store.count_non_file_nodes() > 0

    def test_invalidate_cache_is_callable_on_both(self, stores):
        python_store, rust_store, _ = stores
        # Write paths call this unconditionally rather than probing for it.
        assert python_store._invalidate_cache() is None
        assert rust_store._invalidate_cache() is None


def test_semantic_search_works_under_native_backend(tmp_path, monkeypatch):
    """`semantic_search_nodes` used to die on `store._conn`. See: #153"""
    rust_graph_store()
    monkeypatch.setenv("DAGAYN_BACKEND", "rust")

    from dagayn.incremental import full_build
    from dagayn.tools import semantic_search_nodes
    from dagayn.tools._common import _get_store

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

    result = semantic_search_nodes(query="used", repo_root=str(repo))

    assert result["status"] == "ok", result
    assert result["embedding_health"]["status"] != "unknown"
