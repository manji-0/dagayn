"""Tests for bare-name edge resolution and query_graph target binding (issue #34)."""

from unittest.mock import patch

import pytest

from dagayn.bare_name_resolution import (
    SymbolVisibility,
    build_symbol_visibility,
    is_namespace_candidate,
    is_plausible_bare_edge,
    looks_like_file_target,
    normalize_namespace,
    resolve_bare_call_targets,
    resolve_bare_inheritance_targets,
)
from dagayn.graph import GraphStore
from dagayn.parser import EdgeInfo, NodeInfo
from dagayn.tools.query import query_graph
from tests.store_sql import store_conn


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

    def test_normalize_namespace_unifies_separators(self):
        assert normalize_namespace("App\\Util") == "App.Util"
        assert normalize_namespace("mycorp::infra") == "mycorp.infra"
        assert normalize_namespace(".Repro.Infra.") == "Repro.Infra"
        assert normalize_namespace("") == ""

    def test_is_namespace_candidate_keeps_php_backslash_paths(self):
        assert is_namespace_candidate("App\\Util\\Helper")
        assert is_namespace_candidate("System.Collections.Generic")
        assert not is_namespace_candidate("src/util/helper.cs")
        assert not is_namespace_candidate("Logger.h")

    def test_namespace_visibility_links_same_namespace_and_imports(self):
        visibility = SymbolVisibility(
            declared={
                "Factory.cs": {"Repro.Infra"},
                "Broker.cs": {"Repro.Infra"},
                "Other.cs": {"Repro.Other"},
                "Consumer.cs": {"Repro.App"},
            },
            imported={"Consumer.cs": {"Repro.Other"}},
            class_files={},
        )
        # Same namespace needs no import statement.
        assert visibility.can_see("Broker.cs", "Factory.cs")
        # A different namespace is only visible when imported.
        assert not visibility.can_see("Broker.cs", "Other.cs")
        assert visibility.can_see("Consumer.cs", "Other.cs")
        assert not visibility.can_see("Consumer.cs", "Factory.cs")
        # An unknown file declares nothing, so nothing reaches it.
        assert not visibility.can_see("Broker.cs", "Unknown.cs")


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
        row = store_conn(store).execute(
            "SELECT target_qualified FROM edges WHERE kind='CALLS'"
        ).fetchone()
        assert row["target_qualified"] == "helper"

    def test_resolves_same_namespace_without_any_import(self, tmp_path):
        """Issue #154: C# files in one namespace need no `using` between them,
        so only the namespace index can tell them apart from a same-named
        symbol elsewhere."""
        store = GraphStore(tmp_path / "namespace.db")
        store.upsert_node(_node("File", "Factory.cs", "Factory.cs", namespaces=["Repro.Infra"]))
        store.upsert_node(_node("Function", "CreateCriteria", "Factory.cs"))
        store.upsert_node(_node("File", "Broker.cs", "Broker.cs", namespaces=["Repro.Infra"]))
        store.upsert_node(_node("Function", "Resolve", "Broker.cs"))
        # Same method name in a different namespace must not win.
        store.upsert_node(_node("File", "Decoy.cs", "Decoy.cs", namespaces=["Repro.Other"]))
        store.upsert_node(_node("Function", "CreateCriteria", "Decoy.cs"))
        store.upsert_edge(_edge("CALLS", "Broker.cs::Resolve", "CreateCriteria", "Broker.cs"))
        store.commit()

        visibility = build_symbol_visibility(store_conn(store))
        assert visibility.declared["Broker.cs"] == {"Repro.Infra"}

        assert resolve_bare_call_targets(store) == 1
        row = store_conn(store).execute(
            "SELECT target_qualified FROM edges WHERE kind='CALLS'"
        ).fetchone()
        assert row["target_qualified"] == "Factory.cs::CreateCriteria"

    def test_resolves_imported_namespace(self, tmp_path):
        store = GraphStore(tmp_path / "namespace_import.db")
        store.upsert_node(_node("File", "Broker.php", "Broker.php", namespaces=["App\\Util"]))
        store.upsert_node(_node("Function", "phpBuild", "Broker.php"))
        store.upsert_node(_node("File", "Factory.php", "Factory.php", namespaces=["App\\Infra"]))
        store.upsert_node(_node("Function", "make", "Factory.php"))
        # `use App\Util\Broker` names a symbol inside the namespace.
        store.upsert_edge(_edge("IMPORTS_FROM", "Factory.php", "App\\Util\\Broker", "Factory.php"))
        store.upsert_edge(_edge("CALLS", "Factory.php::make", "phpBuild", "Factory.php"))
        store.commit()

        assert resolve_bare_call_targets(store) == 1
        row = store_conn(store).execute(
            "SELECT target_qualified FROM edges WHERE kind='CALLS'"
        ).fetchone()
        assert row["target_qualified"] == "Broker.php::phpBuild"

    def test_resolves_via_the_class_declaration_not_the_definition_file(self, tmp_path):
        """A C++ method is defined in a `.cpp` nobody includes; only its class
        declaration is reachable from the caller's header."""
        store = GraphStore(tmp_path / "declaring_class.db")
        store.upsert_node(_node("File", "factory.hpp", "factory.hpp"))
        store.upsert_node(_node("Class", "Factory", "factory.hpp", type_role="class"))
        store.upsert_node(_node("File", "factory.cpp", "factory.cpp"))
        store.upsert_node(_node("Function", "createAllowed", "factory.cpp"))
        store.upsert_node(_node("File", "broker.cpp", "broker.cpp"))
        store.upsert_node(_node("Function", "use", "broker.cpp"))
        store.upsert_edge(_edge("IMPORTS_FROM", "factory.cpp", "factory.hpp", "factory.cpp"))
        store.upsert_edge(_edge("IMPORTS_FROM", "broker.cpp", "factory.hpp", "broker.cpp"))
        store.upsert_edge(_edge("CALLS", "broker.cpp::use", "createAllowed", "broker.cpp"))
        store.commit()
        # The definition belongs to the class declared in the header.
        store_conn(store).execute(
            "UPDATE nodes SET parent_name = 'Factory', qualified_name = ? "
            "WHERE name = 'createAllowed'",
            ("factory.cpp::Factory.createAllowed",),
        )
        store_conn(store).commit()

        assert resolve_bare_call_targets(store) == 1
        row = store_conn(store).execute(
            "SELECT target_qualified FROM edges WHERE kind='CALLS'"
        ).fetchone()
        assert row["target_qualified"] == "factory.cpp::Factory.createAllowed"

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
        row = store_conn(store).execute(
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
        row = store_conn(store).execute(
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
        row = store_conn(store).execute(
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

        class _NoClose:
            def __init__(self, inner):
                object.__setattr__(self, "_inner", inner)

            def close(self) -> None:
                return None

            def __getattr__(self, name):
                return getattr(self._inner, name)

        monkeypatch.setattr(
            query_module,
            "_get_store",
            lambda repo_root: (_NoClose(self.store), self.root),
        )

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

    def test_callers_of_keeps_unique_bare_name_without_import_edge(self, monkeypatch):
        """Issue #154: C# ``using`` names namespaces, so no file-to-file import
        edge exists; a unique method name must still report its caller."""
        factory = str(self.root / "Factory.cs")
        broker = str(self.root / "Broker.cs")
        for path in (factory, broker):
            self.store.upsert_node(_node("File", path, path))
        self.store.upsert_node(_node("Function", "CreateCriteria", factory))
        self.store.upsert_node(_node("Function", "Resolve", broker))
        self.store.upsert_edge(_edge("IMPORTS_FROM", broker, "System", broker))
        self.store.upsert_edge(_edge("CALLS", f"{broker}::Resolve", "CreateCriteria", broker))
        self.store.commit()
        self._patch_store(monkeypatch)

        result = query_graph(
            pattern="callers_of",
            target=f"{factory}::CreateCriteria",
            repo_root=str(self.root),
        )

        assert [item["name"] for item in result["results"]] == ["Resolve"]

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
