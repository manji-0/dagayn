"""Tests for dagayn/architecture.py: ADP analysis."""

from __future__ import annotations

from typing import cast

import pytest

from dagayn.contracts.dependency_profiles import DependencyProfile
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


def _cross_edge(source: str, target: str, *, tier: str = "HIGH") -> EdgeInfo:
    return EdgeInfo(
        kind="CROSS_ARTIFACT",
        source=source,
        target=target,
        file_path=source.split("::", 1)[0],
        line=1,
        extra={
            "relationship_role": "implemented_by",
            "bridge_kind": "documentation",
            "confidence_tier": tier,
        },
    )


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
            assert v["dependency_profile"] == "strict_static"
            assert isinstance(v["nodes"], list)
            assert v["length"] == len(v["nodes"])

    def test_unknown_dependency_profile_is_rejected(self, two_cycle_store):
        from dagayn.architecture import find_adp_violations

        with pytest.raises(ValueError, match="Unknown dependency_profile"):
            find_adp_violations(
                two_cycle_store,
                granularity="file",
                dependency_profile=cast(DependencyProfile, "typo"),
            )

    def test_a_limit_keeps_the_same_prefix_in_name_order(self, tmp_path):
        """Past ``max_cycles`` the kept cycles are the first ones walking from
        the smallest name, not whatever ``networkx`` met first in hash order
        (which changed per process, #179)."""
        from dagayn.architecture import find_adp_violations

        store = GraphStore(tmp_path / "complete.db")
        files = ("src/c.py", "src/a.py", "src/b.py")
        for f in files:
            store.upsert_node(_node("File", f, f))
        for source in files:
            for target in files:
                if source != target:
                    store.upsert_edge(_edge("IMPORTS_FROM", source, target))
        store.commit()
        assert len(find_adp_violations(store, granularity="file")) == 5
        violations = find_adp_violations(store, granularity="file", max_cycles=2)
        assert [v["nodes"] for v in violations] == [
            ["src/a.py", "src/b.py", "src/c.py"],
            ["src/a.py", "src/b.py"],
        ]
        assert violations[-1]["truncated"] is True
        assert violations[-1]["cycles_examined"] == 3
        assert violations[-1]["cycle_limit"] == 2
        store.close()

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

    def test_dependency_profile_artifact_trace_adds_high_confidence_cross_artifact(self, tmp_path):
        from dagayn.architecture import find_adp_violations

        s = GraphStore(tmp_path / "artifact_trace_adp.db")
        s.upsert_node(_node("File", "docs/a.md", "docs/a.md", language="markdown"))
        s.upsert_node(_node("File", "src/a.py", "src/a.py"))
        s.upsert_edge(_cross_edge("docs/a.md", "src/a.py"))
        s.upsert_edge(_edge("DEPENDS_ON", "src/a.py", "docs/a.md"))
        s.commit()

        assert find_adp_violations(s, granularity="file", artifact_scope="all") == []
        violations = find_adp_violations(
            s,
            granularity="file",
            artifact_scope="all",
            dependency_profile="artifact_trace",
        )
        assert len(violations) == 1
        assert violations[0]["dependency_profile"] == "artifact_trace"
