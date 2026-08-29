"""Tests for the graph storage and query engine."""

import tempfile
from pathlib import Path

from dagayn.graph import GraphStore
from dagayn.parser import EdgeInfo, NodeInfo
from tests.store_sql import store_conn


class TestGraphStore:
    def setup_method(self):
        self.tmp = tempfile.NamedTemporaryFile(suffix=".db", delete=False)
        self.store = GraphStore(self.tmp.name)

    def teardown_method(self):
        self.store.close()
        Path(self.tmp.name).unlink(missing_ok=True)

    def _make_file_node(self, path="/test/file.py"):
        return NodeInfo(
            kind="File",
            name=path,
            file_path=path,
            line_start=1,
            line_end=100,
            language="python",
        )

    def _make_func_node(self, name="my_func", path="/test/file.py", parent=None, is_test=False):
        return NodeInfo(
            kind="Test" if is_test else "Function",
            name=name,
            file_path=path,
            line_start=10,
            line_end=20,
            language="python",
            parent_name=parent,
            is_test=is_test,
        )

    def _make_class_node(self, name="MyClass", path="/test/file.py"):
        return NodeInfo(
            kind="Class",
            name=name,
            file_path=path,
            line_start=5,
            line_end=50,
            language="python",
        )

    def test_upsert_and_get_node(self):
        node = self._make_file_node()
        self.store.upsert_node(node)
        self.store.commit()

        result = self.store.get_node("/test/file.py")
        assert result is not None
        assert result.kind == "File"
        assert result.name == "/test/file.py"

    def test_upsert_function_node(self):
        func = self._make_func_node()
        self.store.upsert_node(func)
        self.store.commit()

        result = self.store.get_node("/test/file.py::my_func")
        assert result is not None
        assert result.kind == "Function"
        assert result.name == "my_func"

    def test_get_node_includes_signature(self):
        func = self._make_func_node()
        node_id = self.store.upsert_node(func)
        self.store.update_node_signature(node_id, "def my_func(value: int) -> bool")
        self.store.commit()

        result = self.store.get_node("/test/file.py::my_func")
        assert result is not None
        assert result.signature == "def my_func(value: int) -> bool"

    def test_upsert_method_node(self):
        method = self._make_func_node(name="do_thing", parent="MyClass")
        self.store.upsert_node(method)
        self.store.commit()

        result = self.store.get_node("/test/file.py::MyClass.do_thing")
        assert result is not None
        assert result.parent_name == "MyClass"

    def test_upsert_edge(self):
        edge = EdgeInfo(
            kind="CALLS",
            source="/test/file.py::func_a",
            target="/test/file.py::func_b",
            file_path="/test/file.py",
            line=15,
        )
        self.store.upsert_edge(edge)
        self.store.commit()

        edges = self.store.get_edges_by_source("/test/file.py::func_a")
        assert len(edges) == 1
        assert edges[0].kind == "CALLS"
        assert edges[0].target_qualified == "/test/file.py::func_b"

    def test_upsert_edge_normalizes_unknown_confidence_tier(self):
        edge = EdgeInfo(
            kind="CROSS_ARTIFACT",
            source="/test/file.py::func_a",
            target="/test/file.py::func_b",
            file_path="/test/file.py",
            line=15,
            extra={"confidence_tier": "surprising", "confidence": 0.4},
        )
        self.store.upsert_edge(edge)
        self.store.commit()

        edges = self.store.get_edges_by_source("/test/file.py::func_a")
        assert len(edges) == 1
        assert edges[0].confidence_tier == "EXTRACTED"
        assert edges[0].confidence == 0.4

    def test_upsert_edge_updates_existing_edge_metadata(self):
        edge = EdgeInfo(
            kind="CROSS_ARTIFACT",
            source="/test/file.py::func_a",
            target="/test/file.py::func_b",
            file_path="/test/file.py",
            line=15,
            extra={"confidence": 0.4, "confidence_tier": "low"},
        )
        edge_id = self.store.upsert_edge(edge)
        updated_id = self.store.upsert_edge(
            EdgeInfo(
                kind=edge.kind,
                source=edge.source,
                target=edge.target,
                file_path=edge.file_path,
                line=edge.line,
                extra={"confidence": 0.9, "confidence_tier": "exact", "role": "contract"},
            )
        )
        self.store.commit()

        edges = self.store.get_edges_by_source("/test/file.py::func_a")
        assert updated_id == edge_id
        assert len(edges) == 1
        assert edges[0].confidence == 0.9
        assert edges[0].confidence_tier == "EXACT"
        assert edges[0].extra["role"] == "contract"

    def test_store_file_batch_normalizes_edge_confidence_metadata(self):
        edge = EdgeInfo(
            kind="CROSS_ARTIFACT",
            source="/test/file.py",
            target="/test/file.py::my_func",
            file_path="/test/file.py",
            line=9,
            extra={"confidence": 0.25, "confidence_tier": "medium"},
        )

        self.store.store_file_batch(
            [
                (
                    "/test/file.py",
                    [self._make_file_node(), self._make_func_node()],
                    [edge],
                    "hash-a",
                    123,
                )
            ]
        )

        edges = self.store.get_edges_by_source("/test/file.py")
        assert len(edges) == 1
        assert edges[0].confidence == 0.25
        assert edges[0].confidence_tier == "MEDIUM"
        assert edges[0].extra["confidence_tier"] == "medium"

    def test_remove_file_data(self):
        node = self._make_file_node()
        func = self._make_func_node()
        self.store.upsert_node(node)
        self.store.upsert_node(func)
        self.store.commit()

        self.store.remove_file_data("/test/file.py")
        self.store.commit()

        assert self.store.get_node("/test/file.py") is None
        assert self.store.get_node("/test/file.py::my_func") is None

    def test_store_file_nodes_edges(self):
        nodes = [self._make_file_node(), self._make_func_node()]
        edges = [
            EdgeInfo(
                kind="CONTAINS",
                source="/test/file.py",
                target="/test/file.py::my_func",
                file_path="/test/file.py",
            )
        ]
        self.store.store_file_nodes_edges("/test/file.py", nodes, edges)

        result = self.store.get_nodes_by_file("/test/file.py")
        assert len(result) == 2

    def test_store_file_batch(self):
        batch = [
            ("/test/a.py", [self._make_file_node("/test/a.py")], [], "hash-a"),
            (
                "/test/b.py",
                [self._make_file_node("/test/b.py"), self._make_func_node(path="/test/b.py")],
                [],
                "hash-b",
            ),
        ]

        self.store.store_file_batch(batch)

        assert len(self.store.get_nodes_by_file("/test/a.py")) == 1
        assert len(self.store.get_nodes_by_file("/test/b.py")) == 2

    def test_remove_files_data_deletes_multiple_files_in_batch(self):
        self.store.store_file_batch(
            [
                ("/test/a.py", [self._make_file_node("/test/a.py")], [], "hash-a"),
                ("/test/b.py", [self._make_file_node("/test/b.py")], [], "hash-b"),
                ("/test/c.py", [self._make_file_node("/test/c.py")], [], "hash-c"),
            ]
        )

        self.store.remove_files_data(["/test/a.py", "/test/b.py"])

        assert self.store.get_node("/test/a.py") is None
        assert self.store.get_node("/test/b.py") is None
        assert self.store.get_node("/test/c.py") is not None

    def test_upsert_edge_avoids_select_id_round_trip(self):
        """upsert_edge should not issue SELECT … id lookups (RETURNING path)."""
        selects: list[str] = []

        def trace(sql: str) -> None:
            normalized = sql.strip().lower()
            if normalized.startswith("select"):
                selects.append(sql)

        edge = EdgeInfo(
            kind="CALLS",
            source="/test/file.py::func_a",
            target="/test/file.py::func_b",
            file_path="/test/file.py",
            line=15,
            extra={"confidence": 0.4, "confidence_tier": "low"},
        )
        conn = store_conn(self.store)
        conn.set_trace_callback(trace)
        try:
            edge_id = self.store.upsert_edge(edge)
            updated_id = self.store.upsert_edge(
                EdgeInfo(
                    kind=edge.kind,
                    source=edge.source,
                    target=edge.target,
                    file_path=edge.file_path,
                    line=edge.line,
                    extra={"confidence": 0.9, "confidence_tier": "exact"},
                )
            )
        finally:
            conn.set_trace_callback(None)

        assert edge_id > 0
        assert updated_id == edge_id
        assert selects == [], f"unexpected SELECT during upsert_edge: {selects}"

    def test_store_after_remove_no_transaction_error(self):
        """Regression test for #135: store_file_nodes_edges after
        remove_file_data must not raise 'cannot start a transaction
        within a transaction'.
        """
        # Seed initial data for two files
        nodes_a = [self._make_file_node("/test/a.py")]
        nodes_b = [self._make_file_node("/test/b.py")]
        self.store.store_file_nodes_edges("/test/a.py", nodes_a, [])
        self.store.store_file_nodes_edges("/test/b.py", nodes_b, [])

        # Without the isolation_level=None fix, this would leave an
        # implicit transaction open and the next call would crash.
        self.store.remove_file_data("/test/a.py")
        # Must not raise sqlite3.OperationalError
        nodes_c = [self._make_file_node("/test/c.py")]
        self.store.store_file_nodes_edges("/test/c.py", nodes_c, [])

        assert self.store.get_node("/test/a.py") is None
        assert self.store.get_node("/test/c.py") is not None

    def test_store_after_multiple_removes_no_transaction_error(self):
        """Regression test for #181: full_build stale-file purge leaves
        implicit transaction open after multiple remove_file_data calls.
        """
        # Seed data for several files
        for i in range(5):
            path = f"/test/file_{i}.py"
            self.store.store_file_nodes_edges(
                path,
                [self._make_file_node(path)],
                [],
            )

        # Simulates full_build's stale-file purge: multiple deletes in a
        # row without explicit commit between them.
        for i in range(3):
            self.store.remove_file_data(f"/test/file_{i}.py")

        # Next store call must succeed regardless of prior connection state.
        new_path = "/test/new_file.py"
        nodes = [self._make_file_node(new_path)]
        self.store.store_file_nodes_edges(new_path, nodes, [])

        assert self.store.get_node(new_path) is not None
        assert self.store.get_node("/test/file_0.py") is None

    def test_search_nodes(self):
        self.store.upsert_node(self._make_func_node("authenticate"))
        self.store.upsert_node(self._make_func_node("authorize"))
        self.store.upsert_node(self._make_func_node("process"))
        self.store.commit()

        results = self.store.search_nodes("auth")
        names = {r.name for r in results}
        assert "authenticate" in names
        assert "authorize" in names
        assert "process" not in names

    def test_get_stats(self):
        self.store.upsert_node(self._make_file_node())
        self.store.upsert_node(self._make_func_node())
        self.store.upsert_node(self._make_class_node())
        self.store.upsert_edge(
            EdgeInfo(
                kind="CONTAINS",
                source="/test/file.py",
                target="/test/file.py::my_func",
                file_path="/test/file.py",
            )
        )
        self.store.commit()

        stats = self.store.get_stats()
        assert stats.total_nodes == 3
        assert stats.total_edges == 1
        assert stats.nodes_by_kind["File"] == 1
        assert stats.nodes_by_kind["Function"] == 1
        assert stats.nodes_by_kind["Class"] == 1
        assert "python" in stats.languages

    def test_impact_radius(self):
        # Create a chain: file_a -> func_a -> (calls) -> func_b in file_b
        self.store.upsert_node(self._make_file_node("/a.py"))
        self.store.upsert_node(self._make_func_node("func_a", "/a.py"))
        self.store.upsert_node(self._make_file_node("/b.py"))
        self.store.upsert_node(self._make_func_node("func_b", "/b.py"))
        self.store.upsert_edge(
            EdgeInfo(
                kind="CALLS",
                source="/a.py::func_a",
                target="/b.py::func_b",
                file_path="/a.py",
                line=10,
            )
        )
        self.store.commit()

        result = self.store.get_impact_radius(["/a.py"], max_depth=2)
        assert len(result["changed_nodes"]) > 0
        # func_b in /b.py should be impacted
        impacted_qns = {n.qualified_name for n in result["impacted_nodes"]}
        assert "/b.py::func_b" in impacted_qns or "/b.py" in impacted_qns

    def test_upsert_edge_preserves_multiple_call_sites(self):
        """Multiple CALLS edges to the same target from the same source on different lines."""
        edge1 = EdgeInfo(
            kind="CALLS",
            source="/test/file.py::caller",
            target="/test/file.py::helper",
            file_path="/test/file.py",
            line=10,
        )
        edge2 = EdgeInfo(
            kind="CALLS",
            source="/test/file.py::caller",
            target="/test/file.py::helper",
            file_path="/test/file.py",
            line=20,
        )
        self.store.upsert_edge(edge1)
        self.store.upsert_edge(edge2)
        self.store.commit()

        edges = self.store.get_edges_by_source("/test/file.py::caller")
        assert len(edges) == 2
        lines = {e.line for e in edges}
        assert lines == {10, 20}

    def test_metadata(self):
        self.store.set_metadata("test_key", "test_value")
        assert self.store.get_metadata("test_key") == "test_value"
        assert self.store.get_metadata("nonexistent") is None
