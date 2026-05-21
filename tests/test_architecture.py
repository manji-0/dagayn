"""Tests for dagayn/architecture.py: ADP and SDP analysis."""

from __future__ import annotations

import pytest

from dagayn.graph import GraphStore
from dagayn.parser import EdgeInfo, NodeInfo


def _node(kind: str, name: str, file_path: str, language: str = "python") -> NodeInfo:
    return NodeInfo(
        kind=kind,
        name=name,
        file_path=file_path,
        line_start=1,
        line_end=10,
        language=language,
        parent_name=None,
        params=None,
        return_type=None,
        modifiers=None,
        is_test=False,
        extra={},
    )


def _edge(kind: str, source: str, target: str) -> EdgeInfo:
    return EdgeInfo(kind=kind, source=source, target=target, file_path="src/a.py", line=1, extra={})


@pytest.fixture
def empty_store(tmp_path):
    s = GraphStore(tmp_path / "empty.db")
    s.commit()
    return s


@pytest.fixture
def linear_store(tmp_path):
    """A -> B -> C (no cycles, file-level)."""
    s = GraphStore(tmp_path / "linear.db")
    for f in ("src/a.py", "src/b.py", "src/c.py"):
        s.upsert_node(_node("File", f, f))
    s.upsert_edge(_edge("IMPORTS_FROM", "src/a.py", "src/b.py"))
    s.upsert_edge(_edge("IMPORTS_FROM", "src/b.py", "src/c.py"))
    s.commit()
    return s


@pytest.fixture
def two_cycle_store(tmp_path):
    """A <-> B (2-cycle)."""
    s = GraphStore(tmp_path / "two_cycle.db")
    for f in ("src/a.py", "src/b.py"):
        s.upsert_node(_node("File", f, f))
    s.upsert_edge(_edge("IMPORTS_FROM", "src/a.py", "src/b.py"))
    s.upsert_edge(_edge("IMPORTS_FROM", "src/b.py", "src/a.py"))
    s.commit()
    return s


@pytest.fixture
def three_cycle_store(tmp_path):
    """A -> B -> C -> A (3-cycle)."""
    s = GraphStore(tmp_path / "three_cycle.db")
    for f in ("src/a.py", "src/b.py", "src/c.py"):
        s.upsert_node(_node("File", f, f))
    s.upsert_edge(_edge("IMPORTS_FROM", "src/a.py", "src/b.py"))
    s.upsert_edge(_edge("IMPORTS_FROM", "src/b.py", "src/c.py"))
    s.upsert_edge(_edge("IMPORTS_FROM", "src/c.py", "src/a.py"))
    s.commit()
    return s


@pytest.fixture
def package_store(tmp_path):
    """Three packages: core, utils, web.
    core imports utils (stable depending on stable: ok)
    web imports core and utils
    utils imports web (violation: utils should be stable)

    File layout:
      core/a.py, core/b.py
      utils/u.py
      web/w.py
    """
    s = GraphStore(tmp_path / "pkg.db")
    for f in ("core/a.py", "core/b.py", "utils/u.py", "web/w.py"):
        s.upsert_node(_node("File", f, f))
    # core -> utils
    s.upsert_edge(_edge("IMPORTS_FROM", "core/a.py", "utils/u.py"))
    s.upsert_edge(_edge("IMPORTS_FROM", "core/b.py", "utils/u.py"))
    # web -> core
    s.upsert_edge(_edge("IMPORTS_FROM", "web/w.py", "core/a.py"))
    # web -> utils
    s.upsert_edge(_edge("IMPORTS_FROM", "web/w.py", "utils/u.py"))
    # utils -> web (cycle between packages + SDP violation)
    s.upsert_edge(_edge("IMPORTS_FROM", "utils/u.py", "web/w.py"))
    s.commit()
    return s


@pytest.fixture
def sdp_store(tmp_path):
    """Known instability values for exact assertions.

    Graph (file-level):
      a.py -> b.py   (a imports b)
      a.py -> c.py   (a imports c)
      d.py -> b.py   (d imports b)

    Degrees:
      a.py: Ca=0, Ce=2 => I=1.0
      b.py: Ca=2, Ce=0 => I=0.0
      c.py: Ca=1, Ce=0 => I=0.0
      d.py: Ca=0, Ce=1 => I=1.0
    """
    s = GraphStore(tmp_path / "sdp.db")
    for f in ("a.py", "b.py", "c.py", "d.py"):
        s.upsert_node(_node("File", f, f))
    s.upsert_edge(_edge("IMPORTS_FROM", "a.py", "b.py"))
    s.upsert_edge(_edge("IMPORTS_FROM", "a.py", "c.py"))
    s.upsert_edge(_edge("IMPORTS_FROM", "d.py", "b.py"))
    s.commit()
    return s


# ---------------------------------------------------------------------------
# _project_dependency_graph
# ---------------------------------------------------------------------------


class TestProjectDependencyGraph:
    def test_file_granularity_returns_digraph(self, linear_store):
        from dagayn.architecture import _project_dependency_graph

        g = _project_dependency_graph(linear_store, granularity="file")
        assert g.number_of_nodes() == 3
        assert g.number_of_edges() == 2

    def test_package_granularity_aggregates_by_directory(self, package_store):
        from dagayn.architecture import _project_dependency_graph

        g = _project_dependency_graph(package_store, granularity="package")
        nodes = set(g.nodes())
        assert "core" in nodes
        assert "utils" in nodes
        assert "web" in nodes
        assert g.number_of_nodes() == 3

    def test_package_granularity_aggregates_edge_weights(self, package_store):
        from dagayn.architecture import _project_dependency_graph

        g = _project_dependency_graph(package_store, granularity="package")
        # core -> utils has 2 file-level edges (core/a.py and core/b.py)
        assert g.has_edge("core", "utils")
        assert g["core"]["utils"]["weight"] == 2

    def test_self_loops_removed(self, tmp_path):
        from dagayn.architecture import _project_dependency_graph

        s = GraphStore(tmp_path / "self.db")
        s.upsert_node(_node("File", "pkg/a.py", "pkg/a.py"))
        s.upsert_node(_node("File", "pkg/b.py", "pkg/b.py"))
        # Two files in same package: self-loop at package level
        s.upsert_edge(_edge("IMPORTS_FROM", "pkg/a.py", "pkg/b.py"))
        s.commit()
        g = _project_dependency_graph(s, granularity="package")
        # Self-loop is removed; node not added since it has no cross-package edges
        assert not g.has_edge("pkg", "pkg")

    def test_non_dependency_edges_excluded(self, tmp_path):
        from dagayn.architecture import _project_dependency_graph

        s = GraphStore(tmp_path / "other.db")
        for f in ("a.py", "b.py"):
            s.upsert_node(_node("File", f, f))
        s.upsert_edge(_edge("CALLS", "a.py", "b.py"))  # not a dependency edge
        s.commit()
        g = _project_dependency_graph(s, granularity="file")
        assert g.number_of_edges() == 0

    def test_empty_store_returns_empty_graph(self, empty_store):
        from dagayn.architecture import _project_dependency_graph

        g = _project_dependency_graph(empty_store)
        assert g.number_of_nodes() == 0
        assert g.number_of_edges() == 0

    def test_depends_on_edges_included(self, tmp_path):
        from dagayn.architecture import _project_dependency_graph

        s = GraphStore(tmp_path / "dep.db")
        for f in ("a.py", "b.py"):
            s.upsert_node(_node("File", f, f))
        s.upsert_edge(_edge("DEPENDS_ON", "a.py", "b.py"))
        s.commit()
        g = _project_dependency_graph(s, granularity="file")
        assert g.has_edge("a.py", "b.py")

    def test_default_code_scope_excludes_markdown_dependencies(self, tmp_path):
        from dagayn.architecture import _project_dependency_graph

        s = GraphStore(tmp_path / "docs.db")
        s.upsert_node(_node("File", "docs/a.md", "docs/a.md", language="markdown"))
        s.upsert_node(_node("File", "docs/b.md", "docs/b.md", language="markdown"))
        s.upsert_edge(_edge("DEPENDS_ON", "docs/a.md", "docs/b.md"))
        s.upsert_edge(_edge("DEPENDS_ON", "docs/b.md", "docs/a.md"))
        s.commit()

        g = _project_dependency_graph(s, granularity="file")
        assert g.number_of_edges() == 0

        docs_g = _project_dependency_graph(s, granularity="file", artifact_scope="docs")
        assert docs_g.has_edge("docs/a.md", "docs/b.md")
        assert docs_g.has_edge("docs/b.md", "docs/a.md")

    def test_all_scope_preserves_legacy_mixed_dependencies(self, tmp_path):
        from dagayn.architecture import _project_dependency_graph

        s = GraphStore(tmp_path / "mixed.db")
        s.upsert_node(_node("File", "src/a.py", "src/a.py"))
        s.upsert_node(_node("File", "docs/spec.md", "docs/spec.md", language="markdown"))
        s.upsert_edge(_edge("DEPENDS_ON", "src/a.py", "docs/spec.md"))
        s.commit()

        g = _project_dependency_graph(s, granularity="package", artifact_scope="all")
        assert g.has_edge("src", "docs")


# ---------------------------------------------------------------------------
# find_adp_violations
# ---------------------------------------------------------------------------


class TestFindAdpViolations:
    def test_no_cycle_returns_empty(self, linear_store):
        from dagayn.architecture import find_adp_violations

        assert find_adp_violations(linear_store, granularity="file") == []

    def test_two_cycle_detected(self, two_cycle_store):
        from dagayn.architecture import find_adp_violations

        violations = find_adp_violations(two_cycle_store, granularity="file")
        assert len(violations) == 1
        v = violations[0]
        assert v["length"] == 2
        assert set(v["nodes"]) == {"src/a.py", "src/b.py"}

    def test_three_cycle_detected(self, three_cycle_store):
        from dagayn.architecture import find_adp_violations

        violations = find_adp_violations(three_cycle_store, granularity="file")
        assert len(violations) == 1
        v = violations[0]
        assert v["length"] == 3
        assert set(v["nodes"]) == {"src/a.py", "src/b.py", "src/c.py"}

    def test_package_cycle_detected(self, package_store):
        from dagayn.architecture import find_adp_violations

        violations = find_adp_violations(package_store, granularity="package")
        assert len(violations) >= 1
        nodes_in_cycles = {n for v in violations for n in v["nodes"]}
        assert "utils" in nodes_in_cycles
        assert "web" in nodes_in_cycles

    def test_min_cycle_size_filter(self, two_cycle_store):
        from dagayn.architecture import find_adp_violations

        # min_cycle_size=3 should filter out 2-cycles
        assert find_adp_violations(two_cycle_store, granularity="file", min_cycle_size=3) == []

    def test_max_cycle_length_respected(self, three_cycle_store):
        from dagayn.architecture import find_adp_violations

        # max_cycle_length=2 should not find the 3-cycle
        violations = find_adp_violations(three_cycle_store, granularity="file", max_cycle_length=2)
        assert violations == []

    def test_result_fields(self, two_cycle_store):
        from dagayn.architecture import find_adp_violations

        violations = find_adp_violations(two_cycle_store, granularity="file")
        for v in violations:
            assert "nodes" in v
            assert "length" in v
            assert "edge_weight" in v
            assert "severity" in v
            assert isinstance(v["nodes"], list)
            assert v["length"] == len(v["nodes"])

    def test_severity_equals_length_times_edge_weight(self, two_cycle_store):
        from dagayn.architecture import find_adp_violations

        violations = find_adp_violations(two_cycle_store, granularity="file")
        for v in violations:
            assert v["severity"] == v["length"] * v["edge_weight"]

    def test_sorted_by_severity_descending(self, tmp_path):
        from dagayn.architecture import find_adp_violations

        # Build a graph with 2-cycle and 3-cycle
        s = GraphStore(tmp_path / "multi.db")
        for f in ("a.py", "b.py", "c.py", "d.py"):
            s.upsert_node(_node("File", f, f))
        # 2-cycle: a <-> b
        s.upsert_edge(_edge("IMPORTS_FROM", "a.py", "b.py"))
        s.upsert_edge(_edge("IMPORTS_FROM", "b.py", "a.py"))
        # 3-cycle: c -> d -> c (wait, we need 3: c -> d, d -> ??)
        # 3-cycle: c -> d, d -> a is wrong. Let's do a -> b -> c -> a as well.
        # Actually "a -> b" already exists. Let's add c -> a and b -> c.
        s.upsert_edge(_edge("IMPORTS_FROM", "b.py", "c.py"))
        s.upsert_edge(_edge("IMPORTS_FROM", "c.py", "a.py"))
        s.commit()
        violations = find_adp_violations(s, granularity="file")
        severities = [v["severity"] for v in violations]
        assert severities == sorted(severities, reverse=True)

    def test_empty_store_returns_empty(self, empty_store):
        from dagayn.architecture import find_adp_violations

        assert find_adp_violations(empty_store) == []

    def test_docs_scope_finds_markdown_cycles(self, tmp_path):
        from dagayn.architecture import find_adp_violations

        s = GraphStore(tmp_path / "docs_adp.db")
        s.upsert_node(_node("File", "docs/a.md", "docs/a.md", language="markdown"))
        s.upsert_node(_node("File", "docs/b.md", "docs/b.md", language="markdown"))
        s.upsert_edge(_edge("DEPENDS_ON", "docs/a.md", "docs/b.md"))
        s.upsert_edge(_edge("DEPENDS_ON", "docs/b.md", "docs/a.md"))
        s.commit()

        assert find_adp_violations(s, granularity="file") == []
        violations = find_adp_violations(s, granularity="file", artifact_scope="docs")
        assert len(violations) == 1
        assert set(violations[0]["nodes"]) == {"docs/a.md", "docs/b.md"}


# ---------------------------------------------------------------------------
# compute_sdp_metrics
# ---------------------------------------------------------------------------


class TestComputeSdpMetrics:
    def test_returns_list(self, sdp_store):
        from dagayn.architecture import compute_sdp_metrics

        result = compute_sdp_metrics(sdp_store, granularity="file")
        assert isinstance(result, list)

    def test_instability_values_correct(self, sdp_store):
        from dagayn.architecture import compute_sdp_metrics

        result = compute_sdp_metrics(sdp_store, granularity="file")
        by_name = {r["name"]: r for r in result}

        # a.py: Ca=0, Ce=2 => I=1.0
        assert by_name["a.py"]["ca"] == 0
        assert by_name["a.py"]["ce"] == 2
        assert by_name["a.py"]["instability"] == 1.0

        # b.py: Ca=2, Ce=0 => I=0.0
        assert by_name["b.py"]["ca"] == 2
        assert by_name["b.py"]["ce"] == 0
        assert by_name["b.py"]["instability"] == 0.0

        # c.py: Ca=1, Ce=0 => I=0.0
        assert by_name["c.py"]["ca"] == 1
        assert by_name["c.py"]["ce"] == 0
        assert by_name["c.py"]["instability"] == 0.0

        # d.py: Ca=0, Ce=1 => I=1.0
        assert by_name["d.py"]["ca"] == 0
        assert by_name["d.py"]["ce"] == 1
        assert by_name["d.py"]["instability"] == 1.0

    def test_sorted_by_instability_descending(self, sdp_store):
        from dagayn.architecture import compute_sdp_metrics

        result = compute_sdp_metrics(sdp_store, granularity="file")
        vals = [r["instability"] for r in result]
        assert vals == sorted(vals, reverse=True)

    def test_result_fields(self, sdp_store):
        from dagayn.architecture import compute_sdp_metrics

        result = compute_sdp_metrics(sdp_store, granularity="file")
        for item in result:
            assert "name" in item
            assert "ca" in item
            assert "ce" in item
            assert "instability" in item
            assert 0.0 <= item["instability"] <= 1.0

    def test_empty_store_returns_empty(self, empty_store):
        from dagayn.architecture import compute_sdp_metrics

        assert compute_sdp_metrics(empty_store) == []

    def test_package_granularity(self, package_store):
        from dagayn.architecture import compute_sdp_metrics

        result = compute_sdp_metrics(package_store, granularity="package")
        names = {r["name"] for r in result}
        assert "core" in names
        assert "utils" in names
        assert "web" in names

    def test_artifact_scope_separates_code_and_docs_metrics(self, tmp_path):
        from dagayn.architecture import compute_sdp_metrics

        s = GraphStore(tmp_path / "scoped_sdp.db")
        s.upsert_node(_node("File", "src/a.py", "src/a.py"))
        s.upsert_node(_node("File", "lib/b.py", "lib/b.py"))
        s.upsert_node(_node("File", "docs/a.md", "docs/a.md", language="markdown"))
        s.upsert_node(_node("File", "guides/b.md", "guides/b.md", language="markdown"))
        s.upsert_edge(_edge("IMPORTS_FROM", "src/a.py", "lib/b.py"))
        s.upsert_edge(_edge("DEPENDS_ON", "docs/a.md", "guides/b.md"))
        s.commit()

        code_names = {r["name"] for r in compute_sdp_metrics(s, granularity="package")}
        docs_names = {
            r["name"]
            for r in compute_sdp_metrics(s, granularity="package", artifact_scope="docs")
        }
        assert code_names == {"src", "lib"}
        assert docs_names == {"docs", "guides"}


# ---------------------------------------------------------------------------
# find_sdp_violations
# ---------------------------------------------------------------------------


class TestFindSdpViolations:
    def test_no_violations_when_stable_depends_on_stable(self, linear_store):
        from dagayn.architecture import find_sdp_violations

        # linear: a -> b -> c; all leaf nodes have I=1, root has I=0
        # c: Ca=1, Ce=0 => I=0 (maximally stable)
        # b: Ca=1, Ce=1 => I=0.5
        # a: Ca=0, Ce=1 => I=1.0
        # a -> b: I(a)=1.0 >= I(b)=0.5, no violation
        # b -> c: I(b)=0.5 >= I(c)=0, no violation
        violations = find_sdp_violations(linear_store, granularity="file", min_delta=0.0)
        # No violation expected since dependencies go from unstable to stable
        assert violations == []

    def test_violation_detected(self, tmp_path):
        from dagayn.architecture import find_sdp_violations

        # Explicit violation: s (stable) depends on u (unstable).
        # s: Ca=3 (a, b, c depend on it), Ce=1 (the violating dep to u)
        #    => I(s) = 1/(3+1) = 0.25
        # u: Ca=1 (from s), Ce=2 (depends on d and e)
        #    => I(u) = 2/(1+2) = 0.667
        # delta = I(u) - I(s) = 0.417 > 0.1 => violation
        s = GraphStore(tmp_path / "violation.db")
        for f in ("s.py", "u.py", "a.py", "b.py", "c.py", "d.py", "e.py"):
            s.upsert_node(_node("File", f, f))
        for caller in ("a.py", "b.py", "c.py"):
            s.upsert_edge(_edge("IMPORTS_FROM", caller, "s.py"))
        s.upsert_edge(_edge("IMPORTS_FROM", "s.py", "u.py"))  # violating edge
        s.upsert_edge(_edge("IMPORTS_FROM", "u.py", "d.py"))
        s.upsert_edge(_edge("IMPORTS_FROM", "u.py", "e.py"))
        s.commit()

        violations = find_sdp_violations(s, granularity="file", min_delta=0.1)
        assert any(v["source"] == "s.py" and v["target"] == "u.py" for v in violations)

    def test_min_delta_filters_small_differences(self, tmp_path):
        from dagayn.architecture import find_sdp_violations

        # Same violation setup: delta ≈ 0.417
        s = GraphStore(tmp_path / "delta.db")
        for f in ("s.py", "u.py", "a.py", "b.py", "c.py", "d.py", "e.py"):
            s.upsert_node(_node("File", f, f))
        for caller in ("a.py", "b.py", "c.py"):
            s.upsert_edge(_edge("IMPORTS_FROM", caller, "s.py"))
        s.upsert_edge(_edge("IMPORTS_FROM", "s.py", "u.py"))
        s.upsert_edge(_edge("IMPORTS_FROM", "u.py", "d.py"))
        s.upsert_edge(_edge("IMPORTS_FROM", "u.py", "e.py"))
        s.commit()

        # delta ≈ 0.417, so min_delta=0.3 should find it
        assert len(find_sdp_violations(s, granularity="file", min_delta=0.3)) >= 1
        # min_delta=0.5 exceeds the actual delta, so nothing is flagged
        assert find_sdp_violations(s, granularity="file", min_delta=0.5) == []

    def test_sorted_by_delta_descending(self, tmp_path):
        from dagayn.architecture import find_sdp_violations

        # Build a graph with multiple violations of varying severity
        s = GraphStore(tmp_path / "multi_sdp.db")
        for f in ("a.py", "b.py", "c.py", "d.py", "e.py"):
            s.upsert_node(_node("File", f, f))
        # Make a highly stable (many dependents)
        s.upsert_edge(_edge("IMPORTS_FROM", "b.py", "a.py"))
        s.upsert_edge(_edge("IMPORTS_FROM", "c.py", "a.py"))
        s.upsert_edge(_edge("IMPORTS_FROM", "d.py", "a.py"))
        # e is very unstable (no dependents, depends on b)
        s.upsert_edge(_edge("IMPORTS_FROM", "e.py", "b.py"))
        # b depends on e: violation (b is somewhat stable, e is unstable)
        s.upsert_edge(_edge("IMPORTS_FROM", "b.py", "e.py"))
        s.commit()

        violations = find_sdp_violations(s, granularity="file", min_delta=0.0)
        deltas = [v["delta"] for v in violations]
        assert deltas == sorted(deltas, reverse=True)

    def test_result_fields(self, tmp_path):
        from dagayn.architecture import find_sdp_violations

        s = GraphStore(tmp_path / "fields.db")
        for f in ("a.py", "b.py", "c.py"):
            s.upsert_node(_node("File", f, f))
        s.upsert_edge(_edge("IMPORTS_FROM", "c.py", "b.py"))
        s.upsert_edge(_edge("IMPORTS_FROM", "b.py", "a.py"))
        s.commit()

        violations = find_sdp_violations(s, granularity="file", min_delta=0.0)
        for v in violations:
            assert "source" in v
            assert "target" in v
            assert "source_instability" in v
            assert "target_instability" in v
            assert "delta" in v
            assert v["delta"] > 0

    def test_empty_store_returns_empty(self, empty_store):
        from dagayn.architecture import find_sdp_violations

        assert find_sdp_violations(empty_store) == []


# ---------------------------------------------------------------------------
# INHERITS/IMPLEMENTS in SDP
# ---------------------------------------------------------------------------


def _class_node(name: str, file_path: str) -> NodeInfo:
    return NodeInfo(
        kind="Class",
        name=name,
        file_path=file_path,
        line_start=1,
        line_end=10,
        language="python",
        parent_name=None,
        params=None,
        return_type=None,
        modifiers=None,
        is_test=False,
        extra={"type_role": "class"},
    )


class TestInheritsInSdp:
    def test_inherits_edge_contributes_to_ce(self, tmp_path):
        """INHERITS edge where target resolves by qualified name contributes to Ce."""
        from dagayn.architecture import compute_sdp_metrics

        s = GraphStore(tmp_path / "inh.db")
        for f in ("base/b.py", "impl/i.py"):
            s.upsert_node(_node("File", f, f))
        s.upsert_node(_class_node("Base", "base/b.py"))
        s.upsert_node(_class_node("Impl", "impl/i.py"))
        # INHERITS: Impl inherits Base (source=qualified_name, target=bare name)
        s.upsert_edge(_edge("INHERITS", "impl/i.py::Impl", "Base"))
        s.commit()

        result = compute_sdp_metrics(s, granularity="package")
        by_name = {r["name"]: r for r in result}
        assert "impl" in by_name
        assert "base" in by_name
        # impl has outgoing dep on base → Ce=1, I=1.0
        assert by_name["impl"]["ce"] == 1
        # base has incoming dep from impl → Ca=1
        assert by_name["base"]["ca"] == 1

    def test_sdp_sap_instability_consistency(self, tmp_path):
        """SDP and SAP produce the same instability for each scope (same edge set)."""
        from dagayn.architecture import compute_sdp_metrics
        from dagayn.sap import compute_sap_metrics

        s = GraphStore(tmp_path / "consistency.db")
        for f in ("api/i.py", "impl/c.py", "util/u.py"):
            s.upsert_node(_node("File", f, f))
        s.upsert_node(
            NodeInfo(
                kind="Class",
                name="IFoo",
                file_path="api/i.py",
                line_start=1,
                line_end=5,
                language="python",
                parent_name=None,
                params=None,
                return_type=None,
                modifiers=None,
                is_test=False,
                extra={"type_role": "interface", "is_contract": True},
            )
        )
        s.upsert_edge(_edge("IMPORTS_FROM", "impl/c.py", "api/i.py"))
        s.upsert_edge(_edge("IMPORTS_FROM", "api/i.py", "util/u.py"))
        s.commit()

        sdp = {r["name"]: r["instability"] for r in compute_sdp_metrics(s, granularity="package")}
        sap = {
            r["scope_key"]: r["instability"] for r in compute_sap_metrics(s, scope_kind="package")
        }

        common = set(sdp) & set(sap)
        assert common, "No common scopes between SDP and SAP"
        for sk in common:
            assert abs(sdp[sk] - sap[sk]) < 1e-6, (
                f"Instability mismatch for {sk}: SDP={sdp[sk]}, SAP={sap[sk]}"
            )
