"""Tests for entry-point detection."""

import tempfile
from pathlib import Path

from dagayn.flows import detect_entry_points
from dagayn.graph import GraphStore
from dagayn.parser import EdgeInfo, NodeInfo


class TestFlows:
    def setup_method(self):
        self.tmp = tempfile.NamedTemporaryFile(suffix=".db", delete=False)
        self.store = GraphStore(self.tmp.name)

    def teardown_method(self):
        self.store.close()
        Path(self.tmp.name).unlink(missing_ok=True)

    # -- helpers --

    def _add_func(
        self,
        name: str,
        path: str = "app.py",
        parent: str | None = None,
        is_test: bool = False,
        extra: dict | None = None,
    ) -> int:
        node = NodeInfo(
            kind="Test" if is_test else "Function",
            name=name,
            file_path=path,
            line_start=1,
            line_end=10,
            language="python",
            parent_name=parent,
            is_test=is_test,
            extra=extra or {},
        )
        nid = self.store.upsert_node(node, file_hash="abc")
        self.store.commit()
        return nid

    def _add_call(self, source_qn: str, target_qn: str, path: str = "app.py") -> None:
        edge = EdgeInfo(
            kind="CALLS",
            source=source_qn,
            target=target_qn,
            file_path=path,
            line=5,
        )
        self.store.upsert_edge(edge)
        self.store.commit()

    def _add_tested_by(self, source_qn: str, test_qn: str, path: str) -> None:
        edge = EdgeInfo(
            kind="TESTED_BY",
            source=source_qn,
            target=test_qn,
            file_path=path,
            line=1,
        )
        self.store.upsert_edge(edge)
        self.store.commit()

    # ---------------------------------------------------------------
    # detect_entry_points
    # ---------------------------------------------------------------

    def test_detect_entry_points_no_callers(self):
        """Functions with no incoming CALLS edges are entry points."""
        self._add_func("entry_func")
        self._add_func("helper")
        # entry_func calls helper, so helper has an incoming CALLS.
        self._add_call("app.py::entry_func", "app.py::helper")

        eps = detect_entry_points(self.store)
        ep_names = {ep.name for ep in eps}
        assert "entry_func" in ep_names
        assert "helper" not in ep_names

    def test_detect_entry_points_framework_pattern(self):
        """Decorated functions are entry points even if they have callers."""
        self._add_func("get_users", extra={"decorators": ["app.get('/users')"]})
        self._add_func("caller")
        # caller -> get_users, so get_users has an incoming CALLS.
        self._add_call("app.py::caller", "app.py::get_users")

        eps = detect_entry_points(self.store)
        ep_names = {ep.name for ep in eps}
        # Even though get_users is called by someone, its decorator marks it.
        assert "get_users" in ep_names

    def test_detect_entry_points_nestjs_and_angular_decorators(self):
        """NestJS handlers and Angular host listeners stay entry points when called."""
        for name, decorators in (
            ("findAll", ["Get"]),
            ("create", ["Post", "UseGuards"]),
            ("onUserCreated", ["EventPattern"]),
            ("nightly", ["Cron"]),
            ("onClick", ["HostListener"]),
            ("lowercase_get", ["get"]),
        ):
            self._add_func(
                name,
                path="users.controller.ts",
                parent="UsersController",
                extra={"decorators": decorators},
            )
            self._add_func(f"call_{name}", path="users.controller.ts")
            self._add_call(
                f"users.controller.ts::call_{name}",
                f"users.controller.ts::UsersController.{name}",
                path="users.controller.ts",
            )

        ep_names = {ep.name for ep in detect_entry_points(self.store)}

        for name in ("findAll", "create", "onUserCreated", "nightly", "onClick"):
            assert name in ep_names, name
        # Exact, case-sensitive: Python-style lowercase decorators do not match.
        assert "lowercase_get" not in ep_names

    def test_framework_decorator_patterns_match_the_native_list(self):
        """`entry_point_heuristics` and `flow_trace.rs` must list the same patterns."""
        import re

        from dagayn.entry_point_heuristics import _FRAMEWORK_DECORATOR_PATTERNS

        source = (
            Path(__file__).resolve().parent.parent
            / "crates"
            / "dagayn-graph"
            / "src"
            / "flow_trace.rs"
        ).read_text(encoding="utf-8")
        body = source.split("fn decorator_res()", 1)[1].split(".into_iter()", 1)[0]
        native = re.findall(r'r"((?:[^"\\]|\\.)*)"', body)
        python = [
            ("(?i)" if pattern.flags & re.IGNORECASE else "") + pattern.pattern
            for pattern in _FRAMEWORK_DECORATOR_PATTERNS
        ]
        assert native == python

    def test_detect_entry_points_name_pattern(self):
        """Functions matching name patterns (main, test_*, on_*) are entry points."""
        self._add_func("main")
        self._add_func("test_something")
        self._add_func("on_message")
        self._add_func("handle_request")
        self._add_func("regular_func")

        # Make regular_func called so it's not a root either
        self._add_func("another")
        self._add_call("app.py::another", "app.py::regular_func")

        eps = detect_entry_points(self.store)
        ep_names = {ep.name for ep in eps}
        assert "main" in ep_names
        assert "test_something" in ep_names
        assert "on_message" in ep_names
        assert "handle_request" in ep_names
        assert "regular_func" not in ep_names

    # ---------------------------------------------------------------
    # detect_entry_points -- expanded decorator patterns
    # ---------------------------------------------------------------

    def test_detect_entry_points_pytest_fixture(self):
        """pytest.fixture decorator marks function as entry point."""
        self._add_func("my_fixture", extra={"decorators": ["pytest.fixture"]})
        eps = detect_entry_points(self.store)
        ep_names = {ep.name for ep in eps}
        assert "my_fixture" in ep_names

    def test_detect_entry_points_django_receiver(self):
        """Django signal receiver decorator marks function as entry point."""
        self._add_func("on_save", extra={"decorators": ["receiver(post_save)"]})
        eps = detect_entry_points(self.store)
        ep_names = {ep.name for ep in eps}
        assert "on_save" in ep_names

    def test_detect_entry_points_spring_scheduled(self):
        """Java Spring @Scheduled marks function as entry point."""
        self._add_func("cleanup_job", extra={"decorators": ["Scheduled(cron='0 0 * * *')"]})
        eps = detect_entry_points(self.store)
        ep_names = {ep.name for ep in eps}
        assert "cleanup_job" in ep_names

    def test_detect_entry_points_celery_task(self):
        """Bare @task decorator marks function as entry point."""
        self._add_func("process_data", extra={"decorators": ["task"]})
        eps = detect_entry_points(self.store)
        ep_names = {ep.name for ep in eps}
        assert "process_data" in ep_names

    def test_detect_entry_points_agent_tool(self):
        """@agent.tool decorator marks function as entry point."""
        self._add_func("query_health", extra={"decorators": ["health_agent.tool"]})
        eps = detect_entry_points(self.store)
        ep_names = {ep.name for ep in eps}
        assert "query_health" in ep_names

    def test_detect_entry_points_alembic(self):
        """upgrade/downgrade functions are entry points."""
        self._add_func("upgrade")
        self._add_func("downgrade")
        eps = detect_entry_points(self.store)
        ep_names = {ep.name for ep in eps}
        assert "upgrade" in ep_names
        assert "downgrade" in ep_names

    def test_detect_entry_points_lifespan(self):
        """FastAPI lifespan function is an entry point."""
        self._add_func("lifespan")
        eps = detect_entry_points(self.store)
        ep_names = {ep.name for ep in eps}
        assert "lifespan" in ep_names

    def test_detect_entry_points_excludes_tests_by_default(self):
        """Test nodes are excluded from entry points by default."""
        self._add_func("production_handler")
        self._add_func("it:should do something", is_test=True)
        self.store.commit()

        eps = detect_entry_points(self.store)
        ep_names = {ep.name for ep in eps}
        assert "production_handler" in ep_names
        assert "it:should do something" not in ep_names

        # With include_tests=True, both appear
        eps_all = detect_entry_points(self.store, include_tests=True)
        ep_names_all = {ep.name for ep in eps_all}
        assert "production_handler" in ep_names_all
        assert "it:should do something" in ep_names_all

    def test_detect_entry_points_excludes_test_files(self):
        """Functions in test files (*.spec.ts, *.test.ts) are excluded by default."""
        self._add_func("production_func", path="src/handler.ts")
        self._add_func("describe_block", path="src/handler.spec.ts")
        self._add_func("test_helper", path="tests/__tests__/utils.ts")

        eps = detect_entry_points(self.store)
        ep_files = {ep.file_path for ep in eps}
        assert "src/handler.ts" in ep_files
        assert "src/handler.spec.ts" not in ep_files
        assert "tests/__tests__/utils.ts" not in ep_files

        # With include_tests=True, they appear
        eps_all = detect_entry_points(self.store, include_tests=True)
        ep_files_all = {ep.file_path for ep in eps_all}
        assert "src/handler.spec.ts" in ep_files_all
