"""Tests for execution flow detection, tracing, and scoring."""

import json
import tempfile
from pathlib import Path

from dagayn.flows import (
    _hydrate_flow_rows,
    detect_entry_points,
    get_affected_flows,
    get_flow_by_id,
    get_flows,
    incremental_trace_flows,
    store_flows,
    trace_flows,
)
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

    # ---------------------------------------------------------------
    # trace_flows
    # ---------------------------------------------------------------

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

    def test_detect_entry_points_module_scope_caller_is_still_root(self):
        """A function called only from module scope (File-sourced CALLS) is a root.

        Regression guard: the parser attributes module-scope calls to the File
        node. Without filtering File-sourced callers, ``run_job`` here would
        look "called" by ``script.py`` and be excluded from flow analysis,
        even though in practice it IS an entry point (the script itself is
        invoked externally).
        """
        self._add_func("run_job", path="script.py")
        # Ensure the File node exists so its qualified_name resolves cleanly
        # (production code creates this automatically during parsing).
        self.store.upsert_node(
            NodeInfo(
                kind="File",
                name="script.py",
                file_path="script.py",
                line_start=1,
                line_end=10,
                language="python",
            )
        )
        self.store.commit()
        # Module-scope call: source is the File node's qualified_name.
        self._add_call("script.py", "script.py::run_job", path="script.py")

        eps = detect_entry_points(self.store)
        ep_names = {ep.name for ep in eps}
        assert "run_job" in ep_names

    def test_trace_simple_flow(self):
        """BFS traces a linear call chain: A -> B -> C."""
        self._add_func("entry")
        self._add_func("middle")
        self._add_func("leaf")

        self._add_call("app.py::entry", "app.py::middle")
        self._add_call("app.py::middle", "app.py::leaf")

        flows = trace_flows(self.store)
        # entry should produce a flow with 3 nodes.
        entry_flows = [f for f in flows if f["entry_point"] == "app.py::entry"]
        assert len(entry_flows) == 1
        assert entry_flows[0]["node_count"] == 3
        assert entry_flows[0]["depth"] >= 1

    def test_trace_flow_cycle_detection(self):
        """Cycles don't cause infinite loops."""
        # main is an entry point (name pattern), calls a, which calls b,
        # which calls a again (cycle).
        self._add_func("main")
        self._add_func("a")
        self._add_func("b")
        self._add_call("app.py::main", "app.py::a")
        self._add_call("app.py::a", "app.py::b")
        self._add_call("app.py::b", "app.py::a")  # cycle back to a

        # Should complete without hanging.
        flows = trace_flows(self.store)
        main_flows = [f for f in flows if f["entry_point"] == "app.py::main"]
        assert len(main_flows) == 1
        # main -> a -> b (a already visited, cycle skipped)
        assert main_flows[0]["node_count"] == 3

    def test_trace_flow_max_depth(self):
        """Respects max_depth limit."""
        # Create a chain of 20 functions.
        for i in range(20):
            self._add_func(f"func_{i}")
        for i in range(19):
            self._add_call(f"app.py::func_{i}", f"app.py::func_{i + 1}")

        flows_shallow = trace_flows(self.store, max_depth=3)
        entry_flow = [f for f in flows_shallow if f["entry_point"] == "app.py::func_0"]
        assert len(entry_flow) == 1
        # With max_depth=3, we should see at most 4 nodes (entry + 3 levels).
        assert entry_flow[0]["node_count"] <= 4

    def test_trace_flow_skips_trivial(self):
        """Flows with only a single node (no outgoing calls leading to graph nodes)
        are excluded."""
        self._add_func("lonely")
        flows = trace_flows(self.store)
        lonely_flows = [f for f in flows if f["entry_point"] == "app.py::lonely"]
        assert len(lonely_flows) == 0

    def test_trace_flow_multi_file(self):
        """Flows spanning multiple files track all files."""
        self._add_func("api_handler", path="routes.py")
        self._add_func("service_call", path="services.py")
        self._add_func("db_query", path="db.py")
        self._add_call("routes.py::api_handler", "services.py::service_call", "routes.py")
        self._add_call("services.py::service_call", "db.py::db_query", "services.py")

        flows = trace_flows(self.store)
        handler_flows = [f for f in flows if f["entry_point"] == "routes.py::api_handler"]
        assert len(handler_flows) == 1
        assert handler_flows[0]["file_count"] == 3
        assert set(handler_flows[0]["files"]) == {"routes.py", "services.py", "db.py"}

    # ---------------------------------------------------------------
    # compute_criticality
    # ---------------------------------------------------------------

    def test_criticality_scoring(self):
        """Criticality scores are between 0 and 1."""
        self._add_func("entry")
        self._add_func("helper")
        self._add_call("app.py::entry", "app.py::helper")

        flows = trace_flows(self.store)
        for flow in flows:
            assert 0.0 <= flow["criticality"] <= 1.0

    def test_criticality_security_keywords_boost(self):
        """Flows touching security-sensitive functions score higher."""
        # Non-security flow.
        self._add_func("start")
        self._add_func("process")
        self._add_call("app.py::start", "app.py::process")

        # Security flow.
        self._add_func("login_handler", path="auth.py")
        self._add_func("check_password", path="auth.py")
        self._add_call("auth.py::login_handler", "auth.py::check_password", "auth.py")

        flows = trace_flows(self.store)
        normal_flows = [f for f in flows if f["entry_point"] == "app.py::start"]
        secure_flows = [f for f in flows if f["entry_point"] == "auth.py::login_handler"]

        assert len(normal_flows) == 1
        assert len(secure_flows) == 1
        # The security flow should have a higher criticality.
        assert secure_flows[0]["criticality"] >= normal_flows[0]["criticality"]

    def test_criticality_file_spread_boost(self):
        """Flows spanning more files score higher on file-spread."""
        # Single-file flow.
        self._add_func("single_a", path="one.py")
        self._add_func("single_b", path="one.py")
        self._add_call("one.py::single_a", "one.py::single_b", "one.py")

        # Multi-file flow.
        self._add_func("multi_a", path="a.py")
        self._add_func("multi_b", path="b.py")
        self._add_func("multi_c", path="c.py")
        self._add_call("a.py::multi_a", "b.py::multi_b", "a.py")
        self._add_call("b.py::multi_b", "c.py::multi_c", "b.py")

        flows = trace_flows(self.store)
        single = [f for f in flows if f["entry_point"] == "one.py::single_a"]
        multi = [f for f in flows if f["entry_point"] == "a.py::multi_a"]

        assert len(single) == 1
        assert len(multi) == 1
        assert multi[0]["criticality"] >= single[0]["criticality"]

    # ---------------------------------------------------------------
    # store_flows + get_flows roundtrip
    # ---------------------------------------------------------------

    def test_store_and_retrieve_flows(self):
        """store_flows + get_flows roundtrip works correctly."""
        self._add_func("ep")
        self._add_func("callee")
        self._add_call("app.py::ep", "app.py::callee")

        flows = trace_flows(self.store)
        assert len(flows) >= 1

        count = store_flows(self.store, flows)
        assert count == len(flows)

        retrieved = get_flows(self.store)
        assert len(retrieved) >= 1

        # Check that all expected fields are present.
        flow = retrieved[0]
        assert "id" in flow
        assert "name" in flow
        assert "criticality" in flow
        assert "path" in flow
        assert isinstance(flow["path"], list)

    def test_store_flows_clears_old(self):
        """Calling store_flows replaces all previous flow data."""
        self._add_func("ep1")
        self._add_func("callee1")
        self._add_call("app.py::ep1", "app.py::callee1")

        flows_v1 = trace_flows(self.store)
        store_flows(self.store, flows_v1)
        assert len(get_flows(self.store)) >= 1

        # Store an empty list — should clear everything.
        store_flows(self.store, [])
        assert len(get_flows(self.store)) == 0

    def test_get_flow_by_id(self):
        """get_flow_by_id returns full step details."""
        self._add_func("ep")
        self._add_func("step1")
        self._add_call("app.py::ep", "app.py::step1")

        flows = trace_flows(self.store)
        store_flows(self.store, flows)

        stored = get_flows(self.store)
        assert len(stored) >= 1
        flow_id = stored[0]["id"]

        detail = get_flow_by_id(self.store, flow_id)
        assert detail is not None
        assert "steps" in detail
        assert len(detail["steps"]) >= 2
        # Each step should have name, kind, file.
        step = detail["steps"][0]
        assert "name" in step
        assert "kind" in step
        assert "file" in step

    def test_get_flow_by_id_not_found(self):
        """get_flow_by_id returns None for nonexistent flow."""
        result = get_flow_by_id(self.store, 99999)
        assert result is None

    # ---------------------------------------------------------------
    # get_affected_flows
    # ---------------------------------------------------------------

    def test_get_affected_flows(self):
        """Finds flows through changed files."""
        self._add_func("handler", path="routes.py")
        self._add_func("service", path="services.py")
        self._add_func("repo", path="repo.py")
        self._add_call("routes.py::handler", "services.py::service", "routes.py")
        self._add_call("services.py::service", "repo.py::repo", "services.py")

        flows = trace_flows(self.store)
        store_flows(self.store, flows)

        # Changing services.py should affect the handler flow.
        result = get_affected_flows(self.store, ["services.py"])
        assert result["total"] >= 1
        affected_entries = {f["entry_point_id"] for f in result["affected_flows"]}
        handler_node = self.store.get_node("routes.py::handler")
        assert handler_node is not None
        assert handler_node.id in affected_entries

    def test_get_affected_flows_empty(self):
        """No affected flows when no files match."""
        self._add_func("ep")
        self._add_func("callee")
        self._add_call("app.py::ep", "app.py::callee")

        flows = trace_flows(self.store)
        store_flows(self.store, flows)

        result = get_affected_flows(self.store, ["nonexistent.py"])
        assert result["total"] == 0
        assert result["affected_flows"] == []

    def test_get_affected_flows_no_files(self):
        """Empty changed_files list returns no results."""
        result = get_affected_flows(self.store, [])
        assert result["total"] == 0

    def test_get_affected_flows_absolute_path(self, tmp_path):
        """Absolute changed-file paths resolve against repo_root like relative paths."""
        repo = tmp_path / "repo"
        repo.mkdir()
        self.store.set_metadata("repo_root", str(repo.resolve()))

        self._add_func("handler", path="routes.py")
        self._add_func("service", path="services.py")
        self._add_func("repo", path="repo.py")
        self._add_call("routes.py::handler", "services.py::service", "routes.py")
        self._add_call("services.py::service", "repo.py::repo", "services.py")

        flows = trace_flows(self.store)
        store_flows(self.store, flows)

        result_rel = get_affected_flows(self.store, ["services.py"])
        abs_path = str((repo / "services.py").resolve())
        result_abs = get_affected_flows(self.store, [abs_path])

        assert result_rel["total"] >= 1
        assert result_abs["total"] == result_rel["total"]
        rel_entries = {f["entry_point_id"] for f in result_rel["affected_flows"]}
        abs_entries = {f["entry_point_id"] for f in result_abs["affected_flows"]}
        assert abs_entries == rel_entries

    def test_get_affected_flows_ignores_unrelated_dangling_memberships(self):
        """Dangling rows on other flows must not mark every query as affected."""
        self._add_func("main_fn", path="a.py")
        self._add_func("other_fn", path="b.py")
        self._add_call("a.py::main_fn", "b.py::other_fn", "a.py")

        main_flows = trace_flows(self.store)
        store_flows(self.store, main_flows)
        main_flow = get_flows(self.store)[0]

        # Corrupt a second flow with a dangling membership unrelated to a.py.
        self.store._conn.execute(
            "INSERT INTO flows (name, entry_point_id, path_json, node_count, depth, file_count, criticality) "
            "VALUES (?, ?, ?, ?, ?, ?, ?)",
            ("xmain", 999999, "[]", 0, 0, 0, 0.0),
        )
        orphan_flow_id = self.store._conn.execute("SELECT last_insert_rowid()").fetchone()[0]
        self.store._conn.execute(
            "INSERT INTO flow_memberships (flow_id, node_id, position) VALUES (?, ?, ?)",
            (orphan_flow_id, 999999, 0),
        )
        self.store.commit()

        result = get_affected_flows(self.store, ["a.py"])
        affected_ids = {flow["id"] for flow in result["affected_flows"]}

        assert main_flow["id"] in affected_ids
        assert orphan_flow_id not in affected_ids

    def test_get_node_ids_by_files_absolute_path(self, tmp_path):
        """get_node_ids_by_files accepts absolute paths when repo_root is set."""
        repo = tmp_path / "repo"
        repo.mkdir()
        self.store.set_metadata("repo_root", str(repo.resolve()))

        self._add_func("service", path="services.py")
        self._add_func("other", path="other.py")

        rel_ids = self.store.get_node_ids_by_files(["services.py"])
        abs_ids = self.store.get_node_ids_by_files([str((repo / "services.py").resolve())])

        assert rel_ids == abs_ids
        assert len(rel_ids) == 1

    # ---------------------------------------------------------------
    # get_flows sorting
    # ---------------------------------------------------------------

    def test_get_flows_sorting(self):
        """get_flows respects sort_by parameter."""
        self._add_func("shallow_ep", path="a.py")
        self._add_func("shallow_callee", path="a.py")
        self._add_call("a.py::shallow_ep", "a.py::shallow_callee", "a.py")

        self._add_func("deep_ep", path="b.py")
        self._add_func("deep_mid", path="c.py")
        self._add_func("deep_end", path="d.py")
        self._add_call("b.py::deep_ep", "c.py::deep_mid", "b.py")
        self._add_call("c.py::deep_mid", "d.py::deep_end", "c.py")

        flows = trace_flows(self.store)
        store_flows(self.store, flows)

        by_depth = get_flows(self.store, sort_by="depth")
        assert len(by_depth) >= 2
        # Deepest flow first.
        assert by_depth[0]["depth"] >= by_depth[-1]["depth"]

    # ---------------------------------------------------------------
    # incremental_trace_flows
    # ---------------------------------------------------------------

    def test_incremental_trace_flows_no_changed_files(self):
        """Empty changed_files returns 0 and does nothing."""
        assert incremental_trace_flows(self.store, []) == 0

    def test_incremental_trace_flows_preserves_unrelated(self):
        """Flows not touching changed files survive an incremental update."""
        # Flow A: routes.py -> services.py
        self._add_func("handler", path="routes.py")
        self._add_func("service", path="services.py")
        self._add_call("routes.py::handler", "services.py::service", "routes.py")

        # Flow B: cli.py -> utils.py (unrelated to routes/services)
        self._add_func("main", path="cli.py")
        self._add_func("helper", path="utils.py")
        self._add_call("cli.py::main", "utils.py::helper", "cli.py")

        # Store both flows
        flows = trace_flows(self.store)
        store_flows(self.store, flows)
        initial = get_flows(self.store)
        initial_count = len(initial)
        assert initial_count >= 2

        # Incrementally update only services.py — Flow A gets re-traced,
        # Flow B stays untouched.
        incremental_trace_flows(self.store, ["services.py"])

        after = get_flows(self.store)
        # Flow B should still be present.
        cli_flows = [f for f in after if f["name"] == "main"]
        assert len(cli_flows) == 1

    def test_incremental_trace_flows_retraces_affected(self):
        """Affected flows are deleted and re-traced."""
        self._add_func("handler", path="routes.py")
        self._add_func("service", path="services.py")
        self._add_func("repo", path="repo.py")
        self._add_call("routes.py::handler", "services.py::service", "routes.py")
        self._add_call("services.py::service", "repo.py::repo", "services.py")

        flows = trace_flows(self.store)
        store_flows(self.store, flows)

        # Change services.py — the handler flow should be re-traced.
        count = incremental_trace_flows(self.store, ["services.py"])
        assert count >= 1

        after = get_flows(self.store)
        handler_flows = [f for f in after if f["name"] == "handler"]
        assert len(handler_flows) == 1
        assert handler_flows[0]["node_count"] == 3

    def test_incremental_trace_flows_new_entry_point(self):
        """New entry points in changed files are discovered."""
        # Start with one flow.
        self._add_func("old_entry", path="a.py")
        self._add_func("old_callee", path="a.py")
        self._add_call("a.py::old_entry", "a.py::old_callee", "a.py")

        flows = trace_flows(self.store)
        store_flows(self.store, flows)

        # Now add a new entry point in b.py.
        self._add_func("new_entry", path="b.py")
        self._add_func("new_callee", path="b.py")
        self._add_call("b.py::new_entry", "b.py::new_callee", "b.py")

        count = incremental_trace_flows(self.store, ["b.py"])
        assert count >= 1

        after = get_flows(self.store)
        new_flows = [f for f in after if f["name"] == "new_entry"]
        assert len(new_flows) == 1

    def test_incremental_trace_flows_no_affected_flows(self):
        """When changed files have no existing flows, only new entry points are checked."""
        self._add_func("handler", path="routes.py")
        self._add_func("service", path="services.py")
        self._add_call("routes.py::handler", "services.py::service", "routes.py")

        flows = trace_flows(self.store)
        store_flows(self.store, flows)
        initial_count = len(get_flows(self.store))

        # Change a file with no existing flow involvement and no entry points.
        count = incremental_trace_flows(self.store, ["nonexistent.py"])
        assert count == 0
        # Original flows unchanged.
        assert len(get_flows(self.store)) == initial_count

    def test_incremental_trace_flows_delete_is_atomic(self):
        """Regression test for #258: the DELETE loop in incremental_trace_flows
        must be wrapped in a transaction so a crash mid-loop cannot leave
        orphaned flow_memberships rows."""
        self._add_func("handler", path="routes.py")
        self._add_func("service", path="services.py")
        self._add_call("routes.py::handler", "services.py::service", "routes.py")

        flows = trace_flows(self.store)
        store_flows(self.store, flows)
        assert len(get_flows(self.store)) > 0

        # Incremental trace touching routes.py should delete old flows and
        # re-trace them.  The key assertion is that this does NOT raise
        # "cannot start a transaction within a transaction" and that the
        # DB ends in a consistent state.
        count = incremental_trace_flows(self.store, ["routes.py"])
        # The re-trace should find the same entry points.
        assert count >= 0
        # No orphaned memberships: every membership references a valid flow.
        conn = self.store._conn
        orphans = conn.execute(
            "SELECT fm.flow_id FROM flow_memberships fm "
            "LEFT JOIN flows f ON f.id = fm.flow_id "
            "WHERE f.id IS NULL"
        ).fetchall()
        assert len(orphans) == 0, f"found {len(orphans)} orphaned memberships"

    def test_incremental_trace_flows_deletes_snapshots_before_flows(self):
        """Affected flow snapshots must be removed before their parent flows."""
        conn = self.store._conn
        conn.execute("PRAGMA foreign_keys = ON")

        self._add_func("handler", path="routes.py")
        self._add_func("service", path="services.py")
        self._add_call("routes.py::handler", "services.py::service", "routes.py")

        flows = trace_flows(self.store)
        store_flows(self.store, flows)
        stored = [flow for flow in get_flows(self.store) if flow["name"] == "handler"]
        assert len(stored) == 1
        flow_id = stored[0]["id"]

        conn.execute(
            """INSERT INTO flow_snapshots
               (flow_id, name, entry_point, critical_path, criticality, node_count, file_count)
               VALUES (?, ?, ?, ?, ?, ?, ?)""",
            (
                flow_id,
                "handler",
                "routes.py::handler",
                "[]",
                0.1,
                2,
                2,
            ),
        )

        count = incremental_trace_flows(self.store, ["routes.py"])
        assert count >= 1
        violations = conn.execute("PRAGMA foreign_key_check").fetchall()
        assert len(violations) == 0

    def test_incremental_retrace_after_reparse_middle_file(self):
        """Regression for #29: changed middle file must re-trace the flow."""
        self._add_func("main", path="pkg/a.py")
        self._add_func("helper", path="pkg/b.py")
        self._add_func("leaf", path="pkg/c.py")
        self._add_call("pkg/a.py::main", "pkg/b.py::helper", "pkg/a.py")
        self._add_call("pkg/b.py::helper", "pkg/c.py::leaf", "pkg/b.py")

        store_flows(self.store, trace_flows(self.store))

        self._add_func("extra", path="pkg/b.py")
        self._add_call("pkg/b.py::helper", "pkg/b.py::extra", "pkg/b.py")
        self.store.store_file_nodes_edges(
            "pkg/b.py",
            [
                NodeInfo(
                    kind="Function",
                    name="helper",
                    file_path="pkg/b.py",
                    line_start=1,
                    line_end=10,
                    language="python",
                ),
                NodeInfo(
                    kind="Function",
                    name="extra",
                    file_path="pkg/b.py",
                    line_start=11,
                    line_end=20,
                    language="python",
                ),
            ],
            [
                EdgeInfo(
                    kind="CALLS",
                    source="pkg/b.py::helper",
                    target="pkg/b.py::extra",
                    file_path="pkg/b.py",
                    line=12,
                ),
                EdgeInfo(
                    kind="CALLS",
                    source="pkg/b.py::helper",
                    target="pkg/c.py::leaf",
                    file_path="pkg/b.py",
                    line=5,
                ),
            ],
            fhash="new",
        )

        count = incremental_trace_flows(self.store, ["pkg/b.py"])
        assert count >= 1

        main_flows = [f for f in get_flows(self.store) if f["name"] == "main"]
        assert len(main_flows) == 1
        detail = get_flow_by_id(self.store, main_flows[0]["id"])
        assert "extra" in {step["name"] for step in detail["steps"]}

    def test_incremental_retrace_entry_file_no_duplicate_flows(self):
        """Regression for #29: re-parsed entry file must not leave duplicate flows."""
        self._add_func("main", path="pkg/a.py")
        self._add_func("helper", path="pkg/b.py")
        self._add_call("pkg/a.py::main", "pkg/b.py::helper", "pkg/a.py")

        store_flows(self.store, trace_flows(self.store))
        assert len([f for f in get_flows(self.store) if f["name"] == "main"]) == 1

        self.store.store_file_nodes_edges(
            "pkg/a.py",
            [
                NodeInfo(
                    kind="Function",
                    name="main",
                    file_path="pkg/a.py",
                    line_start=1,
                    line_end=10,
                    language="python",
                )
            ],
            [
                EdgeInfo(
                    kind="CALLS",
                    source="pkg/a.py::main",
                    target="pkg/b.py::helper",
                    file_path="pkg/a.py",
                    line=5,
                )
            ],
            fhash="new",
        )

        count = incremental_trace_flows(self.store, ["pkg/a.py"])
        assert count >= 1
        assert len([f for f in get_flows(self.store) if f["name"] == "main"]) == 1


class TestOrphanedStructurePruning:
    """Re-parsing a file must not leave flows pointing at deleted nodes."""

    def setup_method(self):
        self.tmp = tempfile.NamedTemporaryFile(suffix=".db", delete=False)
        self.store = GraphStore(self.tmp.name)

    def teardown_method(self):
        self.store.close()
        Path(self.tmp.name).unlink(missing_ok=True)

    def _flow_graph(self) -> None:
        for name in ("entry", "callee"):
            self.store.upsert_node(
                NodeInfo(
                    kind="Function",
                    name=name,
                    file_path="app.py",
                    line_start=1,
                    line_end=10,
                    language="python",
                ),
                file_hash="abc",
            )
        self.store.upsert_edge(
            EdgeInfo(
                kind="CALLS",
                source="app.py::entry",
                target="app.py::callee",
                file_path="app.py",
                line=5,
            )
        )
        self.store.commit()
        store_flows(self.store, trace_flows(self.store))
        assert len(get_flows(self.store)) >= 1

    def _dangling_memberships(self) -> int:
        row = self.store._conn.execute(
            "SELECT COUNT(*) FROM flow_memberships m "
            "LEFT JOIN nodes n ON n.id = m.node_id WHERE n.id IS NULL"
        ).fetchone()
        return row[0]

    def test_replacing_a_file_leaves_no_dangling_memberships(self):
        self._flow_graph()

        # Same content, new node ids — what every re-parse does.
        self.store.store_file_nodes_edges(
            "app.py",
            [
                NodeInfo(
                    kind="Function",
                    name="entry",
                    file_path="app.py",
                    line_start=1,
                    line_end=10,
                    language="python",
                )
            ],
            [],
            fhash="def",
        )

        assert self._dangling_memberships() == 0

    def test_prune_sweeps_flows_that_lost_every_member(self):
        self._flow_graph()

        # Simulate a graph that was rebuilt by a path which bypasses the Python
        # store (the Rust backend), leaving the derived tables behind.
        self.store._conn.execute("DELETE FROM nodes")
        self.store.commit()
        assert self._dangling_memberships() > 0

        pruned = self.store.prune_orphaned_graph_structures()
        self.store.commit()

        assert pruned["flow_memberships"] > 0
        assert self._dangling_memberships() == 0
        assert get_flows(self.store) == []

    def test_prune_rewrites_stale_path_json(self):
        self._flow_graph()
        flow = get_flows(self.store)[0]
        flow_id = flow["id"]
        stale_path = json.loads(self.store._conn.execute(
            "SELECT path_json FROM flows WHERE id = ?", (flow_id,)
        ).fetchone()[0])

        self.store._conn.execute(
            "UPDATE flows SET path_json = ? WHERE id = ?",
            (json.dumps(stale_path + [999999]), flow_id),
        )
        self.store.commit()

        pruned = self.store.prune_orphaned_graph_structures()
        assert pruned.get("flows_repaired", 0) >= 1

        row = self.store._conn.execute(
            "SELECT path_json, node_count FROM flows WHERE id = ?", (flow_id,)
        ).fetchone()
        live_path = json.loads(row[0])
        assert 999999 not in live_path
        assert row[1] == len(live_path)


class TestStaleFlowHydration:
    """Stored flow paths must report unresolved step counts."""

    def setup_method(self):
        self.tmp = tempfile.NamedTemporaryFile(suffix=".db", delete=False)
        self.store = GraphStore(self.tmp.name)

    def teardown_method(self):
        self.store.close()
        Path(self.tmp.name).unlink(missing_ok=True)

    def test_hydrate_flow_rows_reports_missing_steps(self):
        for name in ("entry", "middle", "leaf"):
            self.store.upsert_node(
                NodeInfo(
                    kind="Function",
                    name=name,
                    file_path="app.py",
                    line_start=1,
                    line_end=10,
                    language="python",
                ),
                file_hash="abc",
            )
        self.store.upsert_edge(
            EdgeInfo(
                kind="CALLS",
                source="app.py::entry",
                target="app.py::middle",
                file_path="app.py",
                line=2,
            )
        )
        self.store.upsert_edge(
            EdgeInfo(
                kind="CALLS",
                source="app.py::middle",
                target="app.py::leaf",
                file_path="app.py",
                line=3,
            )
        )
        self.store.commit()
        store_flows(self.store, trace_flows(self.store))

        row = self.store._conn.execute("SELECT * FROM flows WHERE name = 'entry'").fetchone()
        assert row is not None
        path_ids = json.loads(row["path_json"])
        assert len(path_ids) == 3

        for node_id in path_ids[1:]:
            self.store._conn.execute("DELETE FROM nodes WHERE id = ?", (node_id,))
        self.store.commit()

        hydrated = _hydrate_flow_rows(self.store, [row])[0]
        assert hydrated["node_count"] == 3
        assert hydrated["resolved_step_count"] == 1
        assert hydrated["missing_step_count"] == 2
        assert len(hydrated["steps"]) == 1
