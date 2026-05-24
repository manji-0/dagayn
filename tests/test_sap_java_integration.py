"""Integration tests for SAP/SDP metrics using a real multi-package Java fixture.

Fixture layout (tests/fixtures/java_multipackage/):

  api/
    IRepository.java   -- interface IRepository      (abstract, contract)
    IUserService.java  -- interface IUserService      (abstract, contract)
  domain/
    User.java          -- class User                  (concrete)
  impl/
    InMemoryRepository.java  -- abstract class InMemoryRepository implements IRepository
    UserServiceImpl.java     -- class UserServiceImpl implements IUserService

Expected cross-package coupling:
  impl -> api  (2 IMPLEMENTS edges, collapses to Ce=1 for impl / Ca=1 for api)

Expected SAP values:
  api:    Na=2, Nt=2, A=1.0  Ca=1, Ce=0, I=0.0  D=0.0  (main sequence)
  impl:   Na=1, Nt=2, A=0.5  Ca=0, Ce=1, I=1.0  D=0.5
  domain: Na=0, Nt=1, A=0.0  Ca=0, Ce=0, I=0.0  D=1.0  (isolated — not a violation)
"""

from __future__ import annotations

from pathlib import Path

import pytest

from dagayn.architecture import compute_sdp_metrics
from dagayn.graph import GraphStore
from dagayn.parser import CodeParser
from dagayn.sap import compute_sap_metrics, find_sap_violations

JAVA_FIXTURES = Path(__file__).parent / "fixtures" / "java_multipackage"


# ---------------------------------------------------------------------------
# Store fixture
# ---------------------------------------------------------------------------


@pytest.fixture(scope="module")
def java_store(tmp_path_factory):
    """Parse the java_multipackage fixture into a shared GraphStore."""
    db_path = tmp_path_factory.mktemp("java_sap") / "graph.db"
    store = GraphStore(db_path)
    parser = CodeParser()
    for java_file in sorted(JAVA_FIXTURES.rglob("*.java")):
        nodes, edges = parser.parse_file(java_file)
        for node in nodes:
            store.upsert_node(node)
        for edge in edges:
            store.upsert_edge(edge)
    store.commit()
    yield store
    store.close()


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _scope(pkg: str) -> str:
    """Expected scope key for a given package directory name."""
    return str(JAVA_FIXTURES / pkg)


def _find(metrics: list[dict], pkg: str) -> dict | None:
    return next((m for m in metrics if m["scope_key"] == _scope(pkg)), None)


# ---------------------------------------------------------------------------
# Parser layer: IMPLEMENTS edges emitted correctly
# ---------------------------------------------------------------------------


class TestJavaParserEdges:
    def setup_method(self):
        parser = CodeParser()
        self.all_edges = []
        for java_file in sorted(JAVA_FIXTURES.rglob("*.java")):
            _, edges = parser.parse_file(java_file)
            self.all_edges.extend(edges)
        self.impl_edges = [e for e in self.all_edges if e.kind in ("INHERITS", "IMPLEMENTS")]

    def test_implements_edges_emitted_for_interface_conformance(self):
        kinds = {e.kind for e in self.impl_edges}
        assert "IMPLEMENTS" in kinds, "Expected IMPLEMENTS edges from impl classes"

    def test_no_inherits_edges_for_pure_interface_conformance(self):
        """Java interface conformance uses 'implements' keyword → IMPLEMENTS, not INHERITS."""
        for e in self.impl_edges:
            if e.target in ("IRepository", "IUserService"):
                assert e.kind == "IMPLEMENTS", f"Expected IMPLEMENTS for {e.target}, got {e.kind}"

    def test_inmemoryrrepository_implements_irepository(self):
        edge = next(
            (
                e
                for e in self.impl_edges
                if "InMemoryRepository" in e.source and e.target == "IRepository"
            ),
            None,
        )
        assert edge is not None, "Missing IMPLEMENTS edge: InMemoryRepository -> IRepository"
        assert edge.kind == "IMPLEMENTS"

    def test_userserviceimpl_implements_iuserservice(self):
        edge = next(
            (
                e
                for e in self.impl_edges
                if "UserServiceImpl" in e.source and e.target == "IUserService"
            ),
            None,
        )
        assert edge is not None, "Missing IMPLEMENTS edge: UserServiceImpl -> IUserService"
        assert edge.kind == "IMPLEMENTS"

    def test_interfaces_have_no_base_edges(self):
        """IRepository and IUserService extend nothing → no INHERITS/IMPLEMENTS from api/."""
        api_edges = [e for e in self.impl_edges if str(JAVA_FIXTURES / "api") in e.file_path]
        assert api_edges == [], f"api/ classes should not emit inheritance edges: {api_edges}"


# ---------------------------------------------------------------------------
# Node type roles
# ---------------------------------------------------------------------------


class TestJavaTypeRoles:
    def setup_method(self):
        parser = CodeParser()
        self.classes = []
        for java_file in sorted(JAVA_FIXTURES.rglob("*.java")):
            nodes, _ = parser.parse_file(java_file)
            self.classes.extend(n for n in nodes if n.kind == "Class")
        self.by_name = {c.name: c for c in self.classes}

    def test_irepository_is_interface(self):
        assert self.by_name["IRepository"].extra["type_role"] == "interface"
        assert self.by_name["IRepository"].extra["is_abstract"] is True
        assert self.by_name["IRepository"].extra["is_contract"] is True

    def test_iuserservice_is_interface(self):
        assert self.by_name["IUserService"].extra["type_role"] == "interface"
        assert self.by_name["IUserService"].extra["is_contract"] is True

    def test_inmemoryrrepository_is_abstract_class(self):
        assert self.by_name["InMemoryRepository"].extra["type_role"] == "abstract_class"
        assert self.by_name["InMemoryRepository"].extra["is_abstract"] is True

    def test_userserviceimpl_is_concrete(self):
        assert self.by_name["UserServiceImpl"].extra.get("type_role") == "class"
        assert not self.by_name["UserServiceImpl"].extra.get("is_abstract", False)

    def test_user_is_concrete(self):
        assert self.by_name["User"].extra.get("type_role") == "class"


# ---------------------------------------------------------------------------
# SAP metrics
# ---------------------------------------------------------------------------


class TestSapMetrics:
    def test_api_is_fully_abstract(self, java_store):
        """api/ has 2 interfaces → Na=2, Nt=2, A=1.0."""
        metrics = compute_sap_metrics(java_store)
        api = _find(metrics, "api")
        available = [m["scope_key"] for m in metrics]
        assert api is not None, f"api scope not found. Available: {available}"
        assert api["nt"] == 2
        assert api["na"] == 2
        assert api["abstractness"] == 1.0

    def test_impl_has_mixed_abstractness(self, java_store):
        """impl/ has 1 abstract class + 1 concrete class → Na=1, Nt=2, A=0.5."""
        metrics = compute_sap_metrics(java_store)
        impl = _find(metrics, "impl")
        assert impl is not None
        assert impl["nt"] == 2
        assert impl["na"] == 1
        assert impl["abstractness"] == 0.5

    def test_domain_is_concrete(self, java_store):
        """domain/ has 1 concrete class → Na=0, A=0.0."""
        metrics = compute_sap_metrics(java_store)
        domain = _find(metrics, "domain")
        assert domain is not None
        assert domain["na"] == 0
        assert domain["abstractness"] == 0.0

    def test_implements_cross_package_coupling(self, java_store):
        """IMPLEMENTS edges from impl/ to api/ → Ce(impl)=1, Ca(api)=1."""
        metrics = compute_sap_metrics(java_store)
        api = _find(metrics, "api")
        impl = _find(metrics, "impl")
        assert impl["ce"] == 1, f"Expected impl.Ce=1, got {impl['ce']}"
        assert api["ca"] == 1, f"Expected api.Ca=1, got {api['ca']}"

    def test_api_has_no_outgoing_deps(self, java_store):
        """api/ does not depend on anything in this fixture → Ce=0, I=0.0."""
        metrics = compute_sap_metrics(java_store)
        api = _find(metrics, "api")
        assert api["ce"] == 0
        assert api["instability"] == 0.0

    def test_impl_is_maximally_unstable(self, java_store):
        """impl/ has only outgoing deps (Ce=1, Ca=0) → I=1.0."""
        metrics = compute_sap_metrics(java_store)
        impl = _find(metrics, "impl")
        assert impl["ca"] == 0
        assert impl["instability"] == 1.0

    def test_api_on_main_sequence(self, java_store):
        """api/ is abstract+stable → D=0.0."""
        metrics = compute_sap_metrics(java_store)
        api = _find(metrics, "api")
        assert api["distance"] == 0.0

    def test_domain_zone_of_pain(self, java_store):
        """domain/ is concrete (A=0) and isolated (I=0) → D=1.0 (Zone of Pain)."""
        metrics = compute_sap_metrics(java_store)
        domain = _find(metrics, "domain")
        assert domain["abstractness"] == 0.0
        assert domain["instability"] == 0.0
        assert domain["distance"] == 1.0

    def test_sorted_by_distance_descending(self, java_store):
        metrics = compute_sap_metrics(java_store)
        distances = [m["distance"] for m in metrics]
        assert distances == sorted(distances, reverse=True)

    def test_top_incoming_dependency_for_api(self, java_store):
        """api.top_incoming_dependencies should list impl as a dependent."""
        metrics = compute_sap_metrics(java_store)
        api = _find(metrics, "api")
        incoming = {d["scope"] for d in api["top_incoming_dependencies"]}
        assert _scope("impl") in incoming, (
            f"Expected impl in api.top_incoming_dependencies, got: {incoming}"
        )

    def test_top_outgoing_dependency_for_impl(self, java_store):
        """impl.top_outgoing_dependencies should list api."""
        metrics = compute_sap_metrics(java_store)
        impl = _find(metrics, "impl")
        outgoing = {d["scope"] for d in impl["top_outgoing_dependencies"]}
        assert _scope("api") in outgoing, (
            f"Expected api in impl.top_outgoing_dependencies, got: {outgoing}"
        )

    def test_file_scope_kind_gives_per_file_keys(self, java_store):
        """scope_kind='file' produces per-file scope keys."""
        metrics = compute_sap_metrics(java_store, scope_kind="file")
        keys = {m["scope_key"] for m in metrics}
        assert any(k.endswith("IRepository.java") for k in keys)
        assert any(k.endswith("UserServiceImpl.java") for k in keys)

    def test_unit_filter_restricts_output(self, java_store):
        """unit_filter limits results to scopes with matching prefix."""
        metrics = compute_sap_metrics(java_store, unit_filter=[_scope("api")])
        assert all(m["scope_key"].startswith(_scope("api")) for m in metrics)


# ---------------------------------------------------------------------------
# SAP violation detection
# ---------------------------------------------------------------------------


class TestSapViolations:
    def test_domain_not_flagged_as_zone_of_pain(self, java_store):
        """domain/ has D=1.0 but Ca=0, Ce=0 (isolated) — should NOT appear in
        violations. The Java fixture itself is also under tests/fixtures, so
        find_sap_violations suppresses its raw far-from-sequence readings."""
        violations = find_sap_violations(java_store, min_distance=0.4)
        flagged = {v["scope_key"] for v in violations}
        assert _scope("domain") not in flagged
        assert _scope("impl") not in flagged

        metrics = {m["scope_key"]: m for m in compute_sap_metrics(java_store)}
        assert metrics[_scope("impl")]["distance"] == 0.5
        assert "fixture-scope" in metrics[_scope("impl")].get("notes", [])

    def test_api_not_flagged_as_violation(self, java_store):
        """api/ has D=0.0, should not appear in violations."""
        violations = find_sap_violations(java_store, min_distance=0.5)
        flagged = {v["scope_key"] for v in violations}
        assert _scope("api") not in flagged

    def test_violations_sorted_by_distance_descending(self, java_store):
        violations = find_sap_violations(java_store, min_distance=0.0)
        distances = [v["distance"] for v in violations]
        assert distances == sorted(distances, reverse=True)


# ---------------------------------------------------------------------------
# SDP consistency
# ---------------------------------------------------------------------------


class TestSdpConsistency:
    def test_sdp_instability_matches_sap(self, java_store):
        """SDP and SAP compute the same instability for every shared scope."""
        sap = {m["scope_key"]: m["instability"] for m in compute_sap_metrics(java_store)}
        sdp = {m["name"]: m["instability"] for m in compute_sdp_metrics(java_store)}
        common = set(sap) & set(sdp)
        assert common, "No shared scopes between SDP and SAP"
        for sk in common:
            assert abs(sap[sk] - sdp[sk]) < 1e-6, (
                f"Instability mismatch for {Path(sk).name}: SAP={sap[sk]:.4f}, SDP={sdp[sk]:.4f}"
            )

    def test_sdp_sees_api_as_stable(self, java_store):
        sdp = compute_sdp_metrics(java_store)
        api_entry = next((m for m in sdp if m["name"] == _scope("api")), None)
        assert api_entry is not None
        assert api_entry["instability"] == 0.0

    def test_sdp_sees_impl_as_unstable(self, java_store):
        sdp = compute_sdp_metrics(java_store)
        impl_entry = next((m for m in sdp if m["name"] == _scope("impl")), None)
        assert impl_entry is not None
        assert impl_entry["instability"] == 1.0
