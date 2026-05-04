"""Tests for dagayn/sap.py: SAP (Stable Abstractions Principle) analysis."""

from __future__ import annotations

import pytest

from dagayn.graph import GraphStore
from dagayn.parser import EdgeInfo, NodeInfo
from dagayn.sap import compute_sap_metrics, find_sap_violations


def _node(kind: str, name: str, file_path: str, extra: dict | None = None) -> NodeInfo:
    return NodeInfo(
        kind=kind,
        name=name,
        file_path=file_path,
        line_start=1,
        line_end=10,
        language="java",
        parent_name=None,
        params=None,
        return_type=None,
        modifiers=None,
        is_test=False,
        extra=extra or {},
    )


def _edge(kind: str, source: str, target: str) -> EdgeInfo:
    return EdgeInfo(kind=kind, source=source, target=target, file_path="src/a.java", line=1)


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------


@pytest.fixture
def empty_store(tmp_path):
    s = GraphStore(tmp_path / "empty.db")
    s.commit()
    return s


@pytest.fixture
def single_concrete_stable_store(tmp_path):
    """One package with a concrete class and no outgoing deps: A=0, I=0, D=1 (Zone of Pain)."""
    s = GraphStore(tmp_path / "cstable.db")
    s.upsert_node(_node("File", "src/a.java", "src/a.java"))
    s.upsert_node(_node("Class", "Foo", "src/a.java", extra={"type_role": "class"}))
    s.upsert_edge(_edge("CONTAINS", "src/a.java", "Foo"))
    # Another package depends on src
    s.upsert_node(_node("File", "client/b.java", "client/b.java"))
    s.upsert_edge(_edge("IMPORTS_FROM", "client/b.java", "src/a.java"))
    s.commit()
    return s


@pytest.fixture
def abstract_only_store(tmp_path):
    """One package with only an interface and no outgoing deps: A=1, I=0, D=0 (main sequence)."""
    s = GraphStore(tmp_path / "abstract.db")
    s.upsert_node(_node("File", "api/i.java", "api/i.java"))
    s.upsert_node(
        _node("Class", "IFoo", "api/i.java", extra={"type_role": "interface", "is_contract": True})
    )
    s.upsert_edge(_edge("CONTAINS", "api/i.java", "IFoo"))
    # Another package depends on api
    s.upsert_node(_node("File", "impl/c.java", "impl/c.java"))
    s.upsert_edge(_edge("IMPORTS_FROM", "impl/c.java", "api/i.java"))
    s.commit()
    return s


@pytest.fixture
def balanced_store(tmp_path):
    """api: A=1, I=0.5 → D=0.5; impl: A=0, I=1 → D=0.0."""
    s = GraphStore(tmp_path / "balanced.db")
    # api package: abstract, receives deps from impl and app
    s.upsert_node(_node("File", "api/i.java", "api/i.java"))
    s.upsert_node(
        _node("Class", "IFoo", "api/i.java", extra={"type_role": "interface", "is_contract": True})
    )
    s.upsert_edge(_edge("CONTAINS", "api/i.java", "IFoo"))
    # api depends on util
    s.upsert_node(_node("File", "util/u.java", "util/u.java"))
    s.upsert_edge(_edge("IMPORTS_FROM", "api/i.java", "util/u.java"))
    # impl depends on api
    s.upsert_node(_node("File", "impl/c.java", "impl/c.java"))
    s.upsert_edge(_edge("IMPORTS_FROM", "impl/c.java", "api/i.java"))
    s.commit()
    return s


# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------


def test_empty_store(empty_store):
    result = compute_sap_metrics(empty_store)
    assert result == []


def test_empty_store_violations(empty_store):
    assert find_sap_violations(empty_store) == []


def test_single_concrete_stable_zone_of_pain(single_concrete_stable_store):
    metrics = compute_sap_metrics(single_concrete_stable_store, scope_kind="package")
    src_entry = next((m for m in metrics if m["scope_key"] == "src"), None)
    assert src_entry is not None
    assert src_entry["abstractness"] == 0.0
    # Ca >= 1 (client depends on it), Ce = 0 → I = 0
    assert src_entry["instability"] == 0.0
    # D = |0 + 0 - 1| = 1.0
    assert src_entry["distance"] == 1.0


def test_abstract_only_main_sequence(abstract_only_store):
    metrics = compute_sap_metrics(abstract_only_store, scope_kind="package")
    api_entry = next((m for m in metrics if m["scope_key"] == "api"), None)
    assert api_entry is not None
    assert api_entry["abstractness"] == 1.0
    assert api_entry["instability"] == 0.0
    assert api_entry["distance"] == 0.0


def test_no_eligible_types_note(tmp_path):
    """A scope with only Functions gets no-eligible-types note."""
    s = GraphStore(tmp_path / "noclass.db")
    s.upsert_node(_node("File", "util/f.java", "util/f.java"))
    s.upsert_node(
        NodeInfo(
            kind="Function",
            name="helper",
            file_path="util/f.java",
            line_start=1,
            line_end=5,
            language="java",
        )
    )
    s.commit()
    metrics = compute_sap_metrics(s, scope_kind="package")
    util = next((m for m in metrics if m["scope_key"] == "util"), None)
    assert util is not None
    assert "no-eligible-types" in util.get("notes", [])
    assert util["abstractness"] == 0.0


def test_isolated_note(tmp_path):
    """A scope with no incoming or outgoing deps gets isolated note."""
    s = GraphStore(tmp_path / "iso.db")
    s.upsert_node(_node("File", "alone/a.java", "alone/a.java"))
    s.upsert_node(_node("Class", "Solo", "alone/a.java", extra={"type_role": "class"}))
    s.commit()
    metrics = compute_sap_metrics(s, scope_kind="package")
    entry = next((m for m in metrics if m["scope_key"] == "alone"), None)
    assert entry is not None
    assert "isolated" in entry.get("notes", [])


def test_inherits_target_name_fallback(tmp_path):
    """INHERITS target resolves via bare name when exactly one in-repo node has that name."""
    s = GraphStore(tmp_path / "namefb.db")
    # api package: abstract base class
    s.upsert_node(_node("File", "api/base.java", "api/base.java"))
    s.upsert_node(_node("Class", "Base", "api/base.java", extra={"type_role": "abstract_class"}))
    s.upsert_edge(_edge("CONTAINS", "api/base.java", "Base"))
    # impl package: concrete class inheriting from bare name "Base"
    s.upsert_node(_node("File", "impl/foo.java", "impl/foo.java"))
    s.upsert_node(_node("Class", "Foo", "impl/foo.java", extra={"type_role": "class"}))
    # INHERITS edge: source is qualified, target is bare name
    s.upsert_edge(_edge("INHERITS", "impl/foo.java::Foo", "Base"))
    s.commit()
    metrics = compute_sap_metrics(s, scope_kind="package")
    impl = next((m for m in metrics if m["scope_key"] == "impl"), None)
    api = next((m for m in metrics if m["scope_key"] == "api"), None)
    assert impl is not None
    assert api is not None
    # impl has Ce = 1 (depends on api), Ca = 0
    assert impl["ce"] == 1
    # api has Ca = 1 (impl depends on it)
    assert api["ca"] == 1


def test_inherits_ambiguous_name_skipped(tmp_path):
    """INHERITS target with the same name in multiple scopes is skipped (ambiguous)."""
    s = GraphStore(tmp_path / "ambig.db")
    # Two packages both define a class named "Base"
    s.upsert_node(_node("File", "pkg_a/base.java", "pkg_a/base.java"))
    s.upsert_node(_node("Class", "Base", "pkg_a/base.java", extra={"type_role": "class"}))
    s.upsert_node(_node("File", "pkg_b/base.java", "pkg_b/base.java"))
    s.upsert_node(_node("Class", "Base", "pkg_b/base.java", extra={"type_role": "class"}))
    # impl inherits from bare "Base" — ambiguous
    s.upsert_node(_node("File", "impl/foo.java", "impl/foo.java"))
    s.upsert_node(_node("Class", "Foo", "impl/foo.java", extra={"type_role": "class"}))
    s.upsert_edge(_edge("INHERITS", "impl/foo.java::Foo", "Base"))
    s.commit()
    metrics = compute_sap_metrics(s, scope_kind="package")
    impl = next((m for m in metrics if m["scope_key"] == "impl"), None)
    assert impl is not None
    # ambiguous target → edge is skipped → Ce = 0
    assert impl["ce"] == 0


def test_self_loop_ignored(tmp_path):
    """An edge from a node to another node in the same scope does not inflate Ce/Ca."""
    s = GraphStore(tmp_path / "loop.db")
    s.upsert_node(_node("File", "pkg/a.java", "pkg/a.java"))
    s.upsert_node(_node("File", "pkg/b.java", "pkg/b.java"))
    s.upsert_node(_node("Class", "A", "pkg/a.java", extra={"type_role": "class"}))
    s.upsert_node(_node("Class", "B", "pkg/b.java", extra={"type_role": "class"}))
    s.upsert_edge(_edge("IMPORTS_FROM", "pkg/a.java", "pkg/b.java"))
    s.commit()
    metrics = compute_sap_metrics(s, scope_kind="package")
    pkg_entry = next((m for m in metrics if m["scope_key"] == "pkg"), None)
    assert pkg_entry is not None
    # intra-package edge → Ce = 0, Ca = 0
    assert pkg_entry["ce"] == 0
    assert pkg_entry["ca"] == 0


def test_output_fields_complete(balanced_store):
    metrics = compute_sap_metrics(balanced_store, scope_kind="package")
    required = {
        "scope_kind",
        "scope_key",
        "display_name",
        "na",
        "nt",
        "ca",
        "ce",
        "abstractness",
        "instability",
        "distance",
        "member_count",
        "top_incoming_dependencies",
        "top_outgoing_dependencies",
    }
    for m in metrics:
        missing = required - m.keys()
        assert not missing, f"Missing fields in {m['scope_key']}: {missing}"


def test_compute_sap_metrics_reports_dependency_counts(balanced_store):
    """Direct regression coverage for compute_sap_metrics dependency summaries."""
    metrics = compute_sap_metrics(balanced_store, scope_kind="package")
    by_scope = {m["scope_key"]: m for m in metrics}

    assert by_scope["api"]["top_outgoing_dependencies"] == [{"scope": "util", "count": 1}]
    assert by_scope["api"]["top_incoming_dependencies"] == [{"scope": "impl", "count": 1}]
    assert by_scope["impl"]["top_outgoing_dependencies"] == [{"scope": "api", "count": 1}]


def test_sorted_by_distance_descending(balanced_store):
    metrics = compute_sap_metrics(balanced_store, scope_kind="package")
    distances = [m["distance"] for m in metrics]
    assert distances == sorted(distances, reverse=True)


def test_unit_filter(balanced_store):
    metrics = compute_sap_metrics(balanced_store, scope_kind="package", unit_filter=["api"])
    assert all(m["scope_key"].startswith("api") for m in metrics)


def test_find_sap_violations_threshold(single_concrete_stable_store):
    violations = find_sap_violations(single_concrete_stable_store, min_distance=0.5)
    # src has D=1.0 which is > 0.5
    assert any(v["scope_key"] == "src" for v in violations)


def test_find_sap_violations_empty_on_high_threshold(single_concrete_stable_store):
    violations = find_sap_violations(single_concrete_stable_store, min_distance=1.0)
    assert violations == []


def test_file_scope_kind(tmp_path):
    """scope_kind='file' uses file path as scope key."""
    s = GraphStore(tmp_path / "file.db")
    s.upsert_node(_node("File", "src/a.java", "src/a.java"))
    s.upsert_node(_node("Class", "A", "src/a.java", extra={"type_role": "class"}))
    s.commit()
    metrics = compute_sap_metrics(s, scope_kind="file")
    assert any(m["scope_key"] == "src/a.java" for m in metrics)


def test_isolated_scopes_not_flagged_as_violations(tmp_path):
    """Isolated scopes (Ca=0, Ce=0) should not appear in violations even
    though their distance is 1.0. They have no dependencies so they cannot
    violate SAP."""
    s = GraphStore(tmp_path / "iso_viol.db")
    # Isolated package: no incoming or outgoing deps
    s.upsert_node(_node("File", "alone/a.java", "alone/a.java"))
    s.upsert_node(_node("Class", "Solo", "alone/a.java", extra={"type_role": "class"}))
    # Another isolated package with an abstract class
    s.upsert_node(_node("File", "ghost/g.java", "ghost/g.java"))
    s.upsert_node(_node("Class", "IGhost", "ghost/g.java", extra={"type_role": "interface"}))
    s.commit()
    # Both have D=1.0 but should NOT be violations
    violations = find_sap_violations(s, min_distance=0.5)
    assert violations == []
    # compute_sap_metrics should still return them (full data)
    metrics = compute_sap_metrics(s, scope_kind="package")
    assert any(m["scope_key"] == "alone" for m in metrics)
    assert any(m["scope_key"] == "ghost" for m in metrics)
    assert all(m["ca"] + m["ce"] == 0 for m in metrics)
