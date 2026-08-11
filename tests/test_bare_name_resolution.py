"""Tests for bare-name edge resolution and query_graph target binding (issue #34)."""

from unittest.mock import patch

import pytest

from dagayn.bare_name_resolution import (
    is_plausible_bare_edge,
    looks_like_file_target,
    resolve_bare_call_targets,
    resolve_bare_inheritance_targets,
)
from dagayn.graph import GraphStore
from dagayn.parser import EdgeInfo, NodeInfo
from dagayn.tools.query import query_graph


def _node(kind: str, name: str, file_path: str, **extra) -> NodeInfo:
    return NodeInfo(
        kind=kind,
        name=name,
        file_path=file_path,
        line_start=1,
        line_end=10,
        language="python",
        extra=extra,
    )


def _edge(kind: str, source: str, target: str, file_path: str) -> EdgeInfo:
    return EdgeInfo(kind=kind, source=source, target=target, file_path=file_path, line=1)


class TestBareNameResolutionHelpers:
    def test_looks_like_file_target(self):
        assert looks_like_file_target("docs/api.md")
        assert looks_like_file_target("src/foo.py")
        assert not looks_like_file_target("MyClass")
        assert not looks_like_file_target("pkg::Symbol")

    def test_is_plausible_bare_edge_same_file(self):
        imports = {"a.py": {"b.py"}}
        assert is_plausible_bare_edge("a.py", "a.py", imports)
        assert is_plausible_bare_edge("a.py", "b.py", imports)
        assert not is_plausible_bare_edge("a.py", "c.py", imports)


class TestResolveBareCallTargets:
    def test_requires_import_context_even_for_unique_name(self, tmp_path):
        store = GraphStore(tmp_path / "calls.db")
        store.upsert_node(_node("File", "a.py", "a.py"))
        store.upsert_node(_node("Function", "helper", "a.py"))
        store.upsert_node(_node("File", "b.py", "b.py"))
        store.upsert_node(_node("Function", "run", "b.py"))
        store.upsert_edge(_edge("CALLS", "b.py::run", "helper", "b.py"))
        store.commit()

        assert resolve_bare_call_targets(store) == 0
        row = store._conn.execute(
            "SELECT target_qualified FROM edges WHERE kind='CALLS'"
        ).fetchone()
        assert row["target_qualified"] == "helper"

    def test_resolves_when_import_context_is_unique(self, tmp_path):
        store = GraphStore(tmp_path / "calls_import.db")
        store.upsert_node(_node("File", "a.py", "a.py"))
        store.upsert_node(_node("Function", "helper", "a.py"))
        store.upsert_node(_node("File", "b.py", "b.py"))
        store.upsert_node(_node("Function", "run", "b.py"))
        store.upsert_edge(_edge("IMPORTS_FROM", "b.py", "a.py", "b.py"))
        store.upsert_edge(_edge("CALLS", "b.py::run", "helper", "b.py"))
        store.commit()

        assert resolve_bare_call_targets(store) == 1
        row = store._conn.execute(
            "SELECT target_qualified, confidence_tier FROM edges WHERE kind='CALLS'"
        ).fetchone()
        assert row["target_qualified"] == "a.py::helper"
        assert row["confidence_tier"] == "MEDIUM"


class TestResolveBareInheritanceTargets:
    def test_resolves_inherits_via_import(self, tmp_path):
        store = GraphStore(tmp_path / "inherit.db")
        store.upsert_node(_node("File", "base.py", "base.py"))
        store.upsert_node(_node("Class", "Base", "base.py", type_role="class"))
        store.upsert_node(_node("File", "child.py", "child.py"))
        store.upsert_node(_node("Class", "Child", "child.py", type_role="class"))
        store.upsert_edge(_edge("IMPORTS_FROM", "child.py", "base.py", "child.py"))
        store.upsert_edge(_edge("INHERITS", "child.py::Child", "Base", "child.py"))
        store.commit()

        assert resolve_bare_inheritance_targets(store) == 1
        row = store._conn.execute(
            "SELECT target_qualified, confidence_tier FROM edges WHERE kind='INHERITS'"
        ).fetchone()
        assert row["target_qualified"] == "base.py::Base"
        assert row["confidence_tier"] == "MEDIUM"

    def test_demotes_unresolved_ambiguous_inherits(self, tmp_path):
        store = GraphStore(tmp_path / "inherit_ambig.db")
        for pkg in ("a", "b"):
            store.upsert_node(_node("File", f"{pkg}/base.py", f"{pkg}/base.py"))
            store.upsert_node(_node("Class", "Base", f"{pkg}/base.py", type_role="class"))
        store.upsert_node(_node("File", "child.py", "child.py"))
        store.upsert_node(_node("Class", "Child", "child.py", type_role="class"))
        store.upsert_edge(_edge("INHERITS", "child.py::Child", "Base", "child.py"))
        store.commit()

        assert resolve_bare_inheritance_targets(store) == 0
        row = store._conn.execute(
            "SELECT target_qualified, confidence_tier, extra FROM edges WHERE kind='INHERITS'"
        ).fetchone()
        assert row["target_qualified"] == "Base"
        assert row["confidence_tier"] == "LOW"


class TestQueryGraphBareNameBinding:
    @pytest.fixture(autouse=True)
    def _setup_store(self, tmp_path):
        self.root = tmp_path / "repo"
        self.root.mkdir()
        self.store = GraphStore(tmp_path / "query.db")
        yield
        self.store.close()

    def _patch_store(self, monkeypatch):
        from dagayn.tools import query as query_module

        monkeypatch.setattr(
            query_module,
            "_get_store",
            lambda repo_root: (self.store, self.root),
        )
        self.store.close = lambda: None

    def test_fuzzy_resolution_reports_non_exact_match(self, monkeypatch):
        self.store.upsert_node(_node("File", "other.py", str(self.root / "other.py")))
        self.store.upsert_node(_node("Function", "unique_helper", str(self.root / "other.py")))
        self.store.commit()
        self._patch_store(monkeypatch)

        with patch.object(
            self.store,
            "search_nodes",
            return_value=[self.store.get_node(f"{self.root}/other.py::unique_helper")],
        ):
            result = query_graph(pattern="callers_of", target="uniq", repo_root=str(self.root))

        assert result["status"] == "ok"
        assert result["resolution"] == "fuzzy"
        assert result["exact_match_count"] == 0
        assert result["resolved_target"].endswith("::unique_helper")

    def test_callers_of_filters_cross_file_bare_name_fallback(self, monkeypatch):
        base_a = str(self.root / "a" / "base.py")
        base_b = str(self.root / "b" / "base.py")
        child_a = str(self.root / "a" / "child.py")
        child_b = str(self.root / "b" / "child.py")
        for path in (base_a, base_b, child_a, child_b):
            self.store.upsert_node(_node("File", path, path))
        self.store.upsert_node(_node("Class", "Base", base_a, type_role="class"))
        self.store.upsert_node(_node("Class", "Base", base_b, type_role="class"))
        self.store.upsert_node(_node("Class", "ChildA", child_a, type_role="class"))
        self.store.upsert_node(_node("Class", "ChildB", child_b, type_role="class"))
        self.store.upsert_edge(_edge("IMPORTS_FROM", child_a, base_a, child_a))
        self.store.upsert_edge(_edge("IMPORTS_FROM", child_b, base_b, child_b))
        self.store.upsert_edge(_edge("INHERITS", f"{child_a}::ChildA", "Base", child_a))
        self.store.upsert_edge(_edge("INHERITS", f"{child_b}::ChildB", "Base", child_b))
        self.store.commit()
        self._patch_store(monkeypatch)

        result_a = query_graph(
            pattern="inheritors_of",
            target=f"{base_a}::Base",
            repo_root=str(self.root),
        )
        result_b = query_graph(
            pattern="inheritors_of",
            target=f"{base_b}::Base",
            repo_root=str(self.root),
        )

        names_a = {item["name"] for item in result_a["results"]}
        names_b = {item["name"] for item in result_b["results"]}
        assert names_a == {"ChildA"}
        assert names_b == {"ChildB"}

    def test_file_summary_unknown_path_returns_not_found(self, monkeypatch):
        self._patch_store(monkeypatch)

        result = query_graph(
            pattern="file_summary",
            target="missing.py",
            repo_root=str(self.root),
        )

        assert result["status"] == "not_found"
        assert result["result_count"] == 0
        assert result["results"] == []

    def test_ambiguous_target_includes_result_count(self, monkeypatch):
        self.store.upsert_node(_node("File", "one.py", str(self.root / "one.py")))
        self.store.upsert_node(_node("File", "two.py", str(self.root / "two.py")))
        self.store.upsert_node(_node("Function", "dup", str(self.root / "one.py")))
        self.store.upsert_node(_node("Function", "dup", str(self.root / "two.py")))
        self.store.commit()
        self._patch_store(monkeypatch)

        with patch.object(self.store, "search_nodes") as search_nodes:
            search_nodes.return_value = [
                self.store.get_node(f"{self.root}/one.py::dup"),
                self.store.get_node(f"{self.root}/two.py::dup"),
            ]
            result = query_graph(pattern="callers_of", target="dup", repo_root=str(self.root))

        assert result["status"] == "ambiguous"
        assert result["result_count"] == 0
        assert result["results"] == []
