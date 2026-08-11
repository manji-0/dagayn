"""Tests for heuristic test coverage inference."""

import tempfile
from pathlib import Path

from dagayn.coverage import infer_tests_for_node
from dagayn.graph import GraphStore
from dagayn.parser import EdgeInfo, NodeInfo


class TestCoverageInference:
    def setup_method(self):
        self.tmp = tempfile.NamedTemporaryFile(suffix=".db", delete=False)
        self.store = GraphStore(self.tmp.name)

    def teardown_method(self):
        self.store.close()
        Path(self.tmp.name).unlink(missing_ok=True)

    def _add_func(
        self,
        name: str,
        path: str,
        *,
        is_test: bool = False,
        line_start: int = 1,
        line_end: int = 20,
    ) -> None:
        self.store.upsert_node(
            NodeInfo(
                kind="Test" if is_test else "Function",
                name=name,
                file_path=path,
                line_start=line_start,
                line_end=line_end,
                language="python",
                is_test=is_test,
            )
        )

    def _add_tested_by(self, source: str, target: str, file_path: str) -> None:
        self.store.upsert_edge(
            EdgeInfo(
                kind="TESTED_BY",
                source=source,
                target=target,
                file_path=file_path,
                line=1,
            )
        )

    def test_infer_tests_for_node_rejects_cross_module_name_collision(self):
        """Same symbol name in another module must not inherit heuristic coverage."""
        self._add_func("process", path="pkg/alpha.py")
        self._add_func("process", path="pkg/beta.py")
        self._add_func("test_process_alpha", path="tests/test_alpha.py", is_test=True)
        self._add_tested_by(
            "pkg/alpha.py::process",
            "tests/test_alpha.py::test_process_alpha",
            "tests/test_alpha.py",
        )
        self.store.commit()

        alpha = self.store.get_node("pkg/alpha.py::process")
        beta = self.store.get_node("pkg/beta.py::process")
        assert alpha is not None
        assert beta is not None

        alpha_tests = infer_tests_for_node(self.store, alpha)
        beta_tests = infer_tests_for_node(self.store, beta)

        assert [item["qualified_name"] for item in alpha_tests] == [
            "tests/test_alpha.py::test_process_alpha"
        ]
        assert beta_tests == []

    def test_infer_tests_for_node_rejects_prefix_name_collision(self):
        """get_user must not match test_get_user_profile without exact symbol evidence."""
        self._add_func("get_user", path="pkg/users.py")
        self._add_func("get_user_profile", path="pkg/users.py")
        self._add_func("test_get_user_profile", path="tests/test_users.py", is_test=True)
        self.store.commit()

        get_user = self.store.get_node("pkg/users.py::get_user")
        assert get_user is not None

        inferred = infer_tests_for_node(self.store, get_user)
        assert inferred == []

    def test_infer_tests_for_node_keeps_module_linked_name_match(self):
        """Module-linked naming heuristics still surface medium-confidence coverage."""
        self._add_func("process", path="pkg/alpha.py")
        self._add_func("test_process_alpha", path="tests/test_alpha.py", is_test=True)
        self.store.commit()

        target = self.store.get_node("pkg/alpha.py::process")
        assert target is not None

        inferred = infer_tests_for_node(self.store, target)

        assert inferred[0]["qualified_name"] == "tests/test_alpha.py::test_process_alpha"
        assert inferred[0]["confidence"] == "medium"
        assert inferred[0]["coverage_source"] == "heuristic"

    def test_infer_tests_for_node_skips_bare_tested_by_when_qualified_exists(self):
        """Qualified TESTED_BY edges must not be augmented by bare-name collisions."""
        self._add_func("process", path="pkg/alpha.py")
        self._add_func("process", path="pkg/beta.py")
        self._add_func("test_process_alpha", path="tests/test_alpha.py", is_test=True)
        self._add_func("test_process_beta", path="tests/test_beta.py", is_test=True)
        self._add_tested_by(
            "pkg/alpha.py::process",
            "tests/test_alpha.py::test_process_alpha",
            "tests/test_alpha.py",
        )
        self._add_tested_by(
            "process",
            "tests/test_beta.py::test_process_beta",
            "tests/test_beta.py",
        )
        self.store.commit()

        alpha = self.store.get_node("pkg/alpha.py::process")
        assert alpha is not None

        inferred = infer_tests_for_node(self.store, alpha)

        assert [item["qualified_name"] for item in inferred] == [
            "tests/test_alpha.py::test_process_alpha"
        ]

    def test_get_transitive_tests_skips_bare_fallback_when_qualified_exists(self):
        """Bare TESTED_BY fallback is skipped once qualified coverage is found."""
        self._add_func("process", path="pkg/alpha.py")
        self._add_func("test_process_alpha", path="tests/test_alpha.py", is_test=True)
        self._add_func("test_process_other", path="tests/test_other.py", is_test=True)
        self._add_tested_by(
            "pkg/alpha.py::process",
            "tests/test_alpha.py::test_process_alpha",
            "tests/test_alpha.py",
        )
        self._add_tested_by(
            "process",
            "tests/test_other.py::test_process_other",
            "tests/test_other.py",
        )
        self.store.commit()

        tests = self.store.get_transitive_tests("pkg/alpha.py::process")

        assert [item["qualified_name"] for item in tests] == [
            "tests/test_alpha.py::test_process_alpha"
        ]

    def test_get_transitive_tests_keeps_bare_tested_by_when_qualified_missing(self):
        """Bare TESTED_BY sources remain available when qualified lookup is empty."""
        self._add_func("store_file_batch_tx", path="crates/dagayn-graph/src/helpers.rs")
        self._add_func(
            "stores_file_batch_edge_metadata_once_per_call_site",
            path="crates/dagayn-graph/src/tests.rs",
            is_test=True,
        )
        self._add_tested_by(
            "store_file_batch_tx",
            "crates/dagayn-graph/src/tests.rs::stores_file_batch_edge_metadata_once_per_call_site",
            "crates/dagayn-graph/src/tests.rs",
        )
        self.store.commit()

        node = self.store.get_node("crates/dagayn-graph/src/helpers.rs::store_file_batch_tx")
        assert node is not None

        tests = self.store.get_transitive_tests(node.qualified_name)

        assert tests[0]["qualified_name"] == (
            "crates/dagayn-graph/src/tests.rs::stores_file_batch_edge_metadata_once_per_call_site"
        )
