"""Tests for graph-powered refactoring operations."""

import tempfile
import threading
import time
from pathlib import Path

from dagayn.communities import store_communities
from dagayn.graph import GraphStore
from dagayn.parser import CodeParser, EdgeInfo, NodeInfo
from dagayn.refactor import (
    REFACTOR_EXPIRY_SECONDS,
    _pending_refactors,
    _refactor_lock,
    apply_refactor,
    find_dead_code,
    rename_preview,
    suggest_refactorings,
)


class TestRenamePreview:
    """Tests for rename_preview."""

    def setup_method(self):
        self.tmp = tempfile.NamedTemporaryFile(suffix=".db", delete=False)
        self.store = GraphStore(self.tmp.name)
        self._seed()

    def teardown_method(self):
        self.store.close()
        Path(self.tmp.name).unlink(missing_ok=True)
        # Clean up pending refactors.
        with _refactor_lock:
            _pending_refactors.clear()

    def _seed(self):
        """Seed the store with test data for rename tests."""
        # File nodes
        self.store.upsert_node(
            NodeInfo(
                kind="File",
                name="/repo/utils.py",
                file_path="/repo/utils.py",
                line_start=1,
                line_end=50,
                language="python",
            )
        )
        self.store.upsert_node(
            NodeInfo(
                kind="File",
                name="/repo/main.py",
                file_path="/repo/main.py",
                line_start=1,
                line_end=30,
                language="python",
            )
        )
        # Function to rename
        self.store.upsert_node(
            NodeInfo(
                kind="Function",
                name="helper",
                file_path="/repo/utils.py",
                line_start=10,
                line_end=20,
                language="python",
            )
        )
        # Caller function
        self.store.upsert_node(
            NodeInfo(
                kind="Function",
                name="run",
                file_path="/repo/main.py",
                line_start=5,
                line_end=15,
                language="python",
            )
        )
        # CALLS edge: run -> helper
        self.store.upsert_edge(
            EdgeInfo(
                kind="CALLS",
                source="/repo/main.py::run",
                target="/repo/utils.py::helper",
                file_path="/repo/main.py",
                line=10,
            )
        )
        # IMPORTS_FROM edge: main.py imports helper
        self.store.upsert_edge(
            EdgeInfo(
                kind="IMPORTS_FROM",
                source="/repo/main.py",
                target="/repo/utils.py::helper",
                file_path="/repo/main.py",
                line=1,
            )
        )
        self.store.commit()

    def test_rename_preview_returns_edits_with_refactor_id(self):
        """rename_preview returns a dict with refactor_id and edits."""
        result = rename_preview(self.store, "helper", "new_helper")
        assert result is not None
        assert "refactor_id" in result
        assert len(result["refactor_id"]) == 8
        assert result["type"] == "rename"
        assert result["old_name"] == "helper"
        assert result["new_name"] == "new_helper"
        assert result["target"]["qualified_name"] == "/repo/utils.py::helper"
        assert result["target"]["kind"] == "Function"
        assert result["ambiguous"] is False
        assert result["candidate_count"] == 1
        assert isinstance(result["edits"], list)
        assert len(result["edits"]) > 0
        assert "stats" in result
        assert result["stats"]["high"] > 0
        assert all("source" in edit for edit in result["edits"])

    def test_rename_finds_callers(self):
        """rename_preview finds definition + call sites."""
        result = rename_preview(self.store, "helper", "new_helper")
        assert result is not None
        edits = result["edits"]
        # Should have at least: 1 definition + 1 call + 1 import = 3
        assert len(edits) >= 3
        files = {e["file"] for e in edits}
        assert "/repo/utils.py" in files  # definition
        assert "/repo/main.py" in files  # call site + import site

    def test_rename_not_found(self):
        """rename_preview returns None if symbol not found."""
        result = rename_preview(self.store, "nonexistent_function", "new_name")
        assert result is None

    def test_rename_stores_in_pending(self):
        """rename_preview stores the preview in _pending_refactors."""
        result = rename_preview(self.store, "helper", "new_helper")
        assert result is not None
        rid = result["refactor_id"]
        with _refactor_lock:
            assert rid in _pending_refactors


class TestFindDeadCode:
    """Tests for find_dead_code."""

    def setup_method(self):
        self.tmp = tempfile.NamedTemporaryFile(suffix=".db", delete=False)
        self.store = GraphStore(self.tmp.name)
        self._seed()

    def teardown_method(self):
        self.store.close()
        Path(self.tmp.name).unlink(missing_ok=True)

    def _seed(self):
        """Seed with a mix of used and unused functions."""
        # File
        self.store.upsert_node(
            NodeInfo(
                kind="File",
                name="/repo/app.py",
                file_path="/repo/app.py",
                line_start=1,
                line_end=100,
                language="python",
            )
        )
        # A function that IS called
        self.store.upsert_node(
            NodeInfo(
                kind="Function",
                name="used_func",
                file_path="/repo/app.py",
                line_start=10,
                line_end=20,
                language="python",
            )
        )
        # A function that is NOT called (dead code)
        self.store.upsert_node(
            NodeInfo(
                kind="Function",
                name="dead_func",
                file_path="/repo/app.py",
                line_start=30,
                line_end=40,
                language="python",
            )
        )
        # An entry point function (should be excluded)
        self.store.upsert_node(
            NodeInfo(
                kind="Function",
                name="main",
                file_path="/repo/app.py",
                line_start=50,
                line_end=60,
                language="python",
            )
        )
        # A test function (should be excluded)
        self.store.upsert_node(
            NodeInfo(
                kind="Test",
                name="test_something",
                file_path="/repo/test_app.py",
                line_start=1,
                line_end=10,
                language="python",
                is_test=True,
            )
        )

        # Caller for used_func
        self.store.upsert_node(
            NodeInfo(
                kind="Function",
                name="caller",
                file_path="/repo/app.py",
                line_start=70,
                line_end=80,
                language="python",
            )
        )
        self.store.upsert_edge(
            EdgeInfo(
                kind="CALLS",
                source="/repo/app.py::caller",
                target="/repo/app.py::used_func",
                file_path="/repo/app.py",
                line=75,
            )
        )
        self.store.commit()

    def test_find_dead_code(self):
        """find_dead_code detects unreferenced functions."""
        dead = find_dead_code(self.store)
        dead_names = {d["name"] for d in dead}
        assert "dead_func" in dead_names

    def test_find_dead_code_excludes_called(self):
        """find_dead_code does NOT include functions with callers."""
        dead = find_dead_code(self.store)
        dead_names = {d["name"] for d in dead}
        assert "used_func" not in dead_names

    def test_find_dead_code_excludes_entry_points(self):
        """Entry points (like 'main') are not flagged as dead code."""
        dead = find_dead_code(self.store)
        dead_names = {d["name"] for d in dead}
        assert "main" not in dead_names

    def test_find_dead_code_excludes_tests(self):
        """Test nodes are not flagged as dead code."""
        dead = find_dead_code(self.store)
        dead_names = {d["name"] for d in dead}
        assert "test_something" not in dead_names

    def test_find_dead_code_kind_filter(self):
        """kind filter restricts results."""
        dead = find_dead_code(self.store, kind="Class")
        # We have no Class nodes, so should be empty
        assert len(dead) == 0

    def test_find_dead_code_file_pattern(self):
        """file_pattern filter works."""
        dead = find_dead_code(self.store, file_pattern="nonexistent")
        assert len(dead) == 0

    def test_find_dead_code_excludes_dunder(self):
        """Dunder methods are not flagged as dead code."""
        self.store.upsert_node(
            NodeInfo(
                kind="Function",
                name="__init__",
                file_path="/repo/app.py",
                line_start=90,
                line_end=95,
                language="python",
                parent_name="MyClass",
            )
        )
        self.store.commit()
        dead = find_dead_code(self.store)
        dead_names = {d["name"] for d in dead}
        assert "__init__" not in dead_names

    def test_find_dead_code_excludes_constructor(self):
        """JS/TS constructors are not flagged as dead code."""
        self.store.upsert_node(
            NodeInfo(
                kind="Function",
                name="constructor",
                file_path="/repo/component.ts",
                line_start=10,
                line_end=15,
                language="typescript",
                parent_name="MyComponent",
            )
        )
        self.store.commit()
        dead = find_dead_code(self.store)
        dead_names = {d["name"] for d in dead}
        assert "constructor" not in dead_names

    def test_find_dead_code_excludes_angular_lifecycle(self):
        """Angular lifecycle hooks are not flagged as dead code."""
        for name in (
            "ngOnInit",
            "ngOnChanges",
            "ngOnDestroy",
            "transform",
            "writeValue",
            "canActivate",
        ):
            self.store.upsert_node(
                NodeInfo(
                    kind="Function",
                    name=name,
                    file_path="/repo/component.ts",
                    line_start=10,
                    line_end=15,
                    language="typescript",
                    parent_name="MyComponent",
                )
            )
        self.store.commit()
        dead = find_dead_code(self.store)
        dead_names = {d["name"] for d in dead}
        for name in (
            "ngOnInit",
            "ngOnChanges",
            "ngOnDestroy",
            "transform",
            "writeValue",
            "canActivate",
        ):
            assert name not in dead_names, f"{name} should not be dead"

    def test_find_dead_code_excludes_decorated_entry(self):
        """Functions with framework decorators are not flagged as dead code."""
        self.store.upsert_node(
            NodeInfo(
                kind="Function",
                name="get_users",
                file_path="/repo/app.py",
                line_start=90,
                line_end=95,
                language="python",
                extra={"decorators": ["app.get('/users')"]},
            )
        )
        self.store.commit()
        dead = find_dead_code(self.store)
        dead_names = {d["name"] for d in dead}
        assert "get_users" not in dead_names

    def test_find_dead_code_excludes_type_referenced_class(self):
        """Classes referenced in function type annotations are not dead code."""
        self.store.upsert_node(
            NodeInfo(
                kind="Class",
                name="UserSchema",
                file_path="/repo/app.py",
                line_start=5,
                line_end=15,
                language="python",
            )
        )
        # A function that uses UserSchema in its params
        self.store.upsert_node(
            NodeInfo(
                kind="Function",
                name="create_user",
                file_path="/repo/app.py",
                line_start=20,
                line_end=30,
                language="python",
                params="body: UserSchema",
            )
        )
        self.store.commit()
        dead = find_dead_code(self.store)
        dead_names = {d["name"] for d in dead}
        assert "UserSchema" not in dead_names

    def test_find_dead_code_excludes_return_type_reference(self):
        """Classes referenced in return types are not dead code."""
        self.store.upsert_node(
            NodeInfo(
                kind="Class",
                name="UserResponse",
                file_path="/repo/app.py",
                line_start=5,
                line_end=15,
                language="python",
            )
        )
        self.store.upsert_node(
            NodeInfo(
                kind="Function",
                name="get_user",
                file_path="/repo/app.py",
                line_start=20,
                line_end=30,
                language="python",
                return_type="Optional[UserResponse]",
            )
        )
        self.store.commit()
        dead = find_dead_code(self.store)
        dead_names = {d["name"] for d in dead}
        assert "UserResponse" not in dead_names

    def test_find_dead_code_excludes_orm_model(self):
        """Classes inheriting from known ORM bases are not dead code."""
        self.store.upsert_node(
            NodeInfo(
                kind="Class",
                name="User",
                file_path="/repo/app.py",
                line_start=5,
                line_end=20,
                language="python",
            )
        )
        self.store.upsert_edge(
            EdgeInfo(
                kind="INHERITS",
                source="/repo/app.py::User",
                target="Base",
                file_path="/repo/app.py",
                line=5,
            )
        )
        self.store.commit()
        dead = find_dead_code(self.store)
        dead_names = {d["name"] for d in dead}
        assert "User" not in dead_names

    def test_find_dead_code_excludes_pydantic_settings(self):
        """Classes inheriting from BaseSettings are not dead code."""
        self.store.upsert_node(
            NodeInfo(
                kind="Class",
                name="AppConfig",
                file_path="/repo/app.py",
                line_start=5,
                line_end=15,
                language="python",
            )
        )
        self.store.upsert_edge(
            EdgeInfo(
                kind="INHERITS",
                source="/repo/app.py::AppConfig",
                target="BaseSettings",
                file_path="/repo/app.py",
                line=5,
            )
        )
        self.store.commit()
        dead = find_dead_code(self.store)
        dead_names = {d["name"] for d in dead}
        assert "AppConfig" not in dead_names

    def test_find_dead_code_excludes_agent_tool(self):
        """Functions with @agent.tool decorator are not dead code."""
        self.store.upsert_node(
            NodeInfo(
                kind="Function",
                name="query_data",
                file_path="/repo/app.py",
                line_start=10,
                line_end=20,
                language="python",
                extra={"decorators": ["health_agent.tool"]},
            )
        )
        self.store.commit()
        dead = find_dead_code(self.store)
        dead_names = {d["name"] for d in dead}
        assert "query_data" not in dead_names

    def test_find_dead_code_excludes_alembic_upgrade(self):
        """upgrade() and downgrade() in alembic files are not dead code."""
        self.store.upsert_node(
            NodeInfo(
                kind="Function",
                name="upgrade",
                file_path="/repo/alembic/versions/001.py",
                line_start=5,
                line_end=15,
                language="python",
            )
        )
        self.store.upsert_node(
            NodeInfo(
                kind="Function",
                name="downgrade",
                file_path="/repo/alembic/versions/001.py",
                line_start=20,
                line_end=30,
                language="python",
            )
        )
        self.store.commit()
        dead = find_dead_code(self.store)
        dead_names = {d["name"] for d in dead}
        assert "upgrade" not in dead_names
        assert "downgrade" not in dead_names

    def test_find_dead_code_excludes_subclassed_class(self):
        """Classes with subclasses (INHERITS edges) are not dead code."""
        self.store.upsert_node(
            NodeInfo(
                kind="Class",
                name="BaseConnector",
                file_path="/repo/connectors.py",
                line_start=5,
                line_end=50,
                language="python",
            )
        )
        # A subclass inherits from BaseConnector (bare-name target)
        self.store.upsert_edge(
            EdgeInfo(
                kind="INHERITS",
                source="/repo/connectors.py::GarminConnector",
                target="BaseConnector",
                file_path="/repo/connectors.py",
                line=60,
            )
        )
        self.store.commit()
        dead = find_dead_code(self.store)
        dead_names = {d["name"] for d in dead}
        assert "BaseConnector" not in dead_names

    def test_find_dead_code_bare_name_not_tricked_by_unrelated_caller(self):
        """Bare-name CALLS from unrelated files don't save a dead function
        when there are multiple definitions with the same name."""
        # Two unrelated functions named "processor" in different files
        self.store.upsert_node(
            NodeInfo(
                kind="Function",
                name="processor",
                file_path="/repo/api/routes.py",
                line_start=10,
                line_end=20,
                language="python",
            )
        )
        self.store.upsert_node(
            NodeInfo(
                kind="Function",
                name="processor",
                file_path="/repo/worker/tasks.py",
                line_start=10,
                line_end=20,
                language="python",
            )
        )
        # A bare CALLS edge from a third file that imports only routes.py
        self.store.upsert_edge(
            EdgeInfo(
                kind="IMPORTS_FROM",
                source="/repo/main.py",
                target="/repo/api/routes.py",
                file_path="/repo/main.py",
                line=1,
            )
        )
        self.store.upsert_edge(
            EdgeInfo(
                kind="CALLS",
                source="/repo/main.py::start",
                target="processor",
                file_path="/repo/main.py",
                line=10,
            )
        )
        self.store.commit()
        dead = find_dead_code(self.store)
        dead_qnames = {d["qualified_name"] for d in dead}
        # routes.py processor is saved (caller imports its file)
        assert "/repo/api/routes.py::processor" not in dead_qnames
        # worker/tasks.py processor is dead (no relationship with caller)
        assert "/repo/worker/tasks.py::processor" in dead_qnames

    def test_find_dead_code_excludes_mock_variables(self):
        """Mock/stub variables in test files are not flagged as dead code."""
        for name in ("mockDynamoClient", "s3ClientMock", "MockService", "createMockRequest"):
            self.store.upsert_node(
                NodeInfo(
                    kind="Function",
                    name=name,
                    file_path="/repo/tests/handler.spec.ts",
                    line_start=10,
                    line_end=15,
                    language="typescript",
                )
            )
        self.store.commit()
        dead = find_dead_code(self.store)
        dead_names = {d["name"] for d in dead}
        for name in ("mockDynamoClient", "s3ClientMock", "MockService", "createMockRequest"):
            assert name not in dead_names, f"{name} should not be dead (mock pattern)"

    def test_find_dead_code_excludes_angular_decorated_class(self):
        """Angular @Component classes are not flagged as dead code."""
        self.store.upsert_node(
            NodeInfo(
                kind="Class",
                name="ClipboardButtonComponent",
                file_path="/repo/src/app/clipboard.component.ts",
                line_start=5,
                line_end=50,
                language="typescript",
                extra={"decorators": ["Component({selector: 'app-clipboard'})"]},
            )
        )
        self.store.commit()
        dead = find_dead_code(self.store)
        dead_names = {d["name"] for d in dead}
        assert "ClipboardButtonComponent" not in dead_names

    def test_find_dead_code_excludes_property(self):
        """Functions decorated with @property are not dead code."""
        self.store.upsert_node(
            NodeInfo(
                kind="Function",
                name="db",
                file_path="/repo/deps.py",
                line_start=10,
                line_end=15,
                language="python",
                extra={"decorators": ["property"]},
            )
        )
        self.store.commit()
        dead = find_dead_code(self.store)
        dead_names = {d["name"] for d in dead}
        assert "db" not in dead_names

    def test_find_dead_code_excludes_markdown_sections(self):
        """Markdown heading nodes are documentation structure, not dead classes."""
        self.store.upsert_node(
            NodeInfo(
                kind="Class",
                name="getting-started",
                file_path="/repo/README.md",
                line_start=1,
                line_end=1,
                language="markdown",
                extra={"markdown_kind": "section"},
            )
        )
        self.store.commit()
        dead = find_dead_code(self.store)
        dead_qnames = {d["qualified_name"] for d in dead}
        assert "/repo/README.md::getting-started" not in dead_qnames

    def test_find_dead_code_excludes_rust_cfg_test_functions(self):
        """Rust functions under a tests module are test helpers, not dead code."""
        self.store.upsert_node(
            NodeInfo(
                kind="Function",
                name="loads_markdown_language",
                file_path="/repo/src/lib.rs",
                line_start=224,
                line_end=231,
                language="rust",
                parent_name="/repo/src/lib.rs::tests",
            )
        )
        self.store.commit()
        dead = find_dead_code(self.store)
        dead_names = {d["name"] for d in dead}
        assert "loads_markdown_language" not in dead_names

    def test_find_dead_code_includes_evidence(self):
        """Dead-code output includes enough evidence for review decisions."""
        dead = find_dead_code(self.store)
        orphan = next(d for d in dead if d["name"] == "dead_func")
        assert orphan["confidence"] == "medium"
        assert "no_callers" in orphan["reason_codes"]
        assert orphan["evidence"]["caller_count"] == 0
        assert orphan["caveats"]


class TestSuggestRefactorings:
    """Tests for suggest_refactorings."""

    def setup_method(self):
        self.tmp = tempfile.NamedTemporaryFile(suffix=".db", delete=False)
        self.store = GraphStore(self.tmp.name)
        self._seed()

    def teardown_method(self):
        self.store.close()
        Path(self.tmp.name).unlink(missing_ok=True)

    def _seed(self):
        """Seed with dead code to generate suggestions."""
        self.store.upsert_node(
            NodeInfo(
                kind="File",
                name="/repo/lib.py",
                file_path="/repo/lib.py",
                line_start=1,
                line_end=50,
                language="python",
            )
        )
        # Unreferenced function -> removal suggestion
        self.store.upsert_node(
            NodeInfo(
                kind="Function",
                name="orphan_func",
                file_path="/repo/lib.py",
                line_start=10,
                line_end=20,
                language="python",
            )
        )
        self.store.commit()

    def test_suggest_refactorings(self):
        """suggest_refactorings returns a list of suggestions."""
        suggestions = suggest_refactorings(self.store)
        assert isinstance(suggestions, list)
        # Should have at least the dead-code removal suggestion
        assert len(suggestions) >= 1
        types = {s["type"] for s in suggestions}
        assert "remove" in types

    def test_suggestion_structure(self):
        """Each suggestion has the required fields."""
        suggestions = suggest_refactorings(self.store)
        for s in suggestions:
            assert "type" in s
            assert "description" in s
            assert "symbols" in s
            assert "rationale" in s
            assert "priority" in s
            assert "confidence" in s
            assert "category" in s
            assert "estimated_risk" in s
            assert "affected_files" in s
            assert "verification_steps" in s
            assert "execution_plan" in s
            assert "minimum_steps" in s["execution_plan"]
            assert "required_tests" in s["execution_plan"]
            assert s["type"] in ("move", "remove", "split", "document")

    def test_remove_suggestions_prioritize_executable_code(self):
        """Executable dead-code suggestions rank ahead of docs and fixtures."""
        self.store.upsert_node(
            NodeInfo(
                kind="Function",
                name="fixture_helper",
                file_path="tests/fixtures/helpers.py",
                line_start=1,
                line_end=3,
                language="python",
            )
        )
        self.store.commit()

        suggestions = [s for s in suggest_refactorings(self.store) if s["type"] == "remove"]

        assert suggestions[0]["symbols"] == ["/repo/lib.py::orphan_func"]
        assert suggestions[0]["category"] == "executable"
        assert any(s["category"] == "fixture" for s in suggestions)

    def test_remove_suggestions_demote_exported_api_candidates(self, tmp_path):
        """Exported API candidates are high-risk even when graph-unreferenced."""
        lib_rs = tmp_path / "crates" / "api" / "src" / "lib.rs"
        py_lib_rs = tmp_path / "crates" / "api-py" / "src" / "lib.rs"
        ts_api = tmp_path / "packages" / "api" / "index.ts"
        lib_rs.parent.mkdir(parents=True)
        py_lib_rs.parent.mkdir(parents=True)
        ts_api.parent.mkdir(parents=True)
        lib_rs.write_text("pub fn exported_api() {}\n", encoding="utf-8")
        py_lib_rs.write_text(
            "#[pymethods]\nimpl PyGraphStore {\n    fn close(&self) {}\n}\n",
            encoding="utf-8",
        )
        ts_api.write_text("export function publicApi() {}\n", encoding="utf-8")
        self.store.upsert_node(
            NodeInfo(
                kind="Function",
                name="exported_api",
                file_path=str(lib_rs),
                line_start=1,
                line_end=1,
                language="rust",
            )
        )
        self.store.upsert_node(
            NodeInfo(
                kind="Function",
                name="close",
                file_path=str(py_lib_rs),
                line_start=3,
                line_end=3,
                language="rust",
                parent_name="PyGraphStore",
            )
        )
        self.store.upsert_node(
            NodeInfo(
                kind="Function",
                name="publicApi",
                file_path=str(ts_api),
                line_start=1,
                line_end=1,
                language="typescript",
            )
        )
        self.store.commit()

        suggestions = [s for s in suggest_refactorings(self.store) if s["type"] == "remove"]
        public_api = next(s for s in suggestions if s["symbols"] == [f"{lib_rs}::exported_api"])
        pyo3_api = next(
            s for s in suggestions if s["symbols"] == [f"{py_lib_rs}::PyGraphStore.close"]
        )
        ts_public_api = next(s for s in suggestions if s["symbols"] == [f"{ts_api}::publicApi"])

        assert public_api["category"] == "public_api"
        assert public_api["confidence"] == "low"
        assert public_api["estimated_risk"] == "high"
        assert "public_api_candidate" in public_api["reason_codes"]
        assert pyo3_api["category"] == "public_api"
        assert pyo3_api["confidence"] == "low"
        assert ts_public_api["category"] == "public_api"
        assert ts_public_api["confidence"] == "low"
        assert suggestions.index(public_api) > suggestions.index(
            next(s for s in suggestions if s["symbols"] == ["/repo/lib.py::orphan_func"])
        )

    def test_remove_suggestions_skip_rust_cfg_test_module_functions(self, tmp_path):
        """Rust functions inside #[cfg(test)] mod tests are not removal suggestions."""
        lib_rs = tmp_path / "src" / "lib.rs"
        lib_rs.parent.mkdir(parents=True)
        tests_rs = tmp_path / "src" / "tests.rs"
        core_tests_rs = tmp_path / "src" / "core_tests.rs"
        lib_rs.write_text(
            "pub fn production() {}\n\n#[cfg(test)]\nmod tests {\n    fn loads_language() {}\n}\n",
            encoding="utf-8",
        )
        tests_rs.write_text("fn integration_style_helper() {}\n", encoding="utf-8")
        core_tests_rs.write_text("fn parser_fixture_helper() {}\n", encoding="utf-8")
        self.store.upsert_node(
            NodeInfo(
                kind="Function",
                name="loads_language",
                file_path=str(lib_rs),
                line_start=5,
                line_end=5,
                language="rust",
            )
        )
        self.store.upsert_node(
            NodeInfo(
                kind="Function",
                name="integration_style_helper",
                file_path=str(tests_rs),
                line_start=1,
                line_end=1,
                language="rust",
            )
        )
        self.store.upsert_node(
            NodeInfo(
                kind="Function",
                name="parser_fixture_helper",
                file_path=str(core_tests_rs),
                line_start=1,
                line_end=1,
                language="rust",
            )
        )
        self.store.commit()

        symbols = {
            symbol
            for suggestion in suggest_refactorings(self.store)
            for symbol in suggestion["symbols"]
        }

        assert f"{lib_rs}::loads_language" not in symbols
        assert f"{tests_rs}::integration_style_helper" not in symbols
        assert f"{core_tests_rs}::parser_fixture_helper" not in symbols

    def test_move_suggestions_require_multiple_known_callers(self):
        """A single cross-community caller is too weak for a move suggestion."""
        self.store.upsert_node(
            NodeInfo(
                kind="Function",
                name="owned_by_source",
                file_path="/repo/source.py",
                line_start=1,
                line_end=2,
                language="python",
            )
        )
        self.store.upsert_node(
            NodeInfo(
                kind="Function",
                name="caller_one",
                file_path="/repo/target.py",
                line_start=1,
                line_end=2,
                language="python",
            )
        )
        self.store.upsert_edge(
            EdgeInfo(
                kind="CALLS",
                source="/repo/target.py::caller_one",
                target="/repo/source.py::owned_by_source",
                file_path="/repo/target.py",
                line=1,
            )
        )
        self.store.commit()
        store_communities(
            self.store,
            [
                {
                    "name": "source",
                    "size": 1,
                    "members": ["/repo/source.py::owned_by_source"],
                },
                {
                    "name": "target",
                    "size": 1,
                    "members": ["/repo/target.py::caller_one"],
                },
            ],
        )

        moves = [s for s in suggest_refactorings(self.store) if s["type"] == "move"]
        assert not any(s["symbols"] == ["/repo/source.py::owned_by_source"] for s in moves)

        self.store.upsert_node(
            NodeInfo(
                kind="Function",
                name="caller_two",
                file_path="/repo/target.py",
                line_start=3,
                line_end=4,
                language="python",
            )
        )
        self.store.upsert_edge(
            EdgeInfo(
                kind="CALLS",
                source="/repo/target.py::caller_two",
                target="/repo/source.py::owned_by_source",
                file_path="/repo/target.py",
                line=3,
            )
        )
        self.store.commit()
        store_communities(
            self.store,
            [
                {
                    "name": "source",
                    "size": 1,
                    "members": ["/repo/source.py::owned_by_source"],
                },
                {
                    "name": "target",
                    "size": 2,
                    "members": [
                        "/repo/target.py::caller_one",
                        "/repo/target.py::caller_two",
                    ],
                },
            ],
        )

        moves = [s for s in suggest_refactorings(self.store) if s["type"] == "move"]
        move = next(s for s in moves if s["symbols"] == ["/repo/source.py::owned_by_source"])
        assert move["confidence"] == "low"
        assert move["evidence"]["incoming_call_count"] == 2
        assert move["evidence"]["minimum_call_threshold"] == 2

    def test_split_and_document_suggestions_for_large_complex_units(self, tmp_path):
        """Large live code with little explanation gets split/document suggestions."""
        service = tmp_path / "service.py"
        body = ["def complex_service(value):"]
        for idx in range(1, 90):
            body.append(f"    if value == {idx}:")
            body.append(f"        helper_{idx % 3}()")
        service.write_text("\n".join(body) + "\n", encoding="utf-8")
        self.store.upsert_node(
            NodeInfo(
                kind="Function",
                name="complex_service",
                file_path=str(service),
                line_start=1,
                line_end=len(body),
                language="python",
            )
        )
        self.store.upsert_node(
            NodeInfo(
                kind="Function",
                name="caller",
                file_path=str(service),
                line_start=len(body) + 1,
                line_end=len(body) + 2,
                language="python",
            )
        )
        self.store.upsert_edge(
            EdgeInfo(
                kind="CALLS",
                source=f"{service}::caller",
                target=f"{service}::complex_service",
                file_path=str(service),
                line=len(body) + 1,
            )
        )
        self.store.commit()

        suggestions = suggest_refactorings(self.store)
        split = next(
            s
            for s in suggestions
            if s["type"] == "split" and s["symbols"] == [f"{service}::complex_service"]
        )
        document = next(
            s
            for s in suggestions
            if s["type"] == "document" and s["symbols"] == [f"{service}::complex_service"]
        )

        assert split["evidence"]["line_threshold"] == 60
        assert split["evidence"]["branch_threshold"] == 12
        assert split["evidence"]["line_count"] >= 60
        assert split["evidence"]["branch_count"] >= 12
        assert "branch_heavy" in split["reason_codes"]
        assert document["evidence"]["comment_ratio"] <= 0.01
        assert document["evidence"]["line_threshold"] == 60
        assert document["evidence"]["comment_ratio_threshold"] == 0.01
        assert "low_explanation_density" in document["reason_codes"]

    def test_document_suggestions_require_enough_lines(self, tmp_path):
        """Low comments on short public code is too weak for a document suggestion."""
        module = tmp_path / "api.ts"
        lines = ["export function publicApi() {"]
        lines.extend(f"  call{idx}();" for idx in range(1, 45))
        lines.append("}")
        module.write_text("\n".join(lines) + "\n", encoding="utf-8")
        self.store.upsert_node(
            NodeInfo(
                kind="Function",
                name="publicApi",
                file_path=str(module),
                line_start=1,
                line_end=len(lines),
                language="typescript",
            )
        )
        self.store.commit()

        docs = [s for s in suggest_refactorings(self.store) if s["type"] == "document"]

        assert not any(s["symbols"] == [f"{module}::publicApi"] for s in docs)

    def test_split_suggestions_do_not_flag_large_simple_functions(self, tmp_path):
        """Size alone is not enough for a function split suggestion."""
        module = tmp_path / "simple.py"
        lines = ["def long_but_linear():"]
        lines.extend(f"    value_{idx} = {idx}" for idx in range(1, 90))
        module.write_text("\n".join(lines) + "\n", encoding="utf-8")
        self.store.upsert_node(
            NodeInfo(
                kind="Function",
                name="long_but_linear",
                file_path=str(module),
                line_start=1,
                line_end=len(lines),
                language="python",
            )
        )
        self.store.upsert_node(
            NodeInfo(
                kind="Function",
                name="caller",
                file_path=str(module),
                line_start=len(lines) + 1,
                line_end=len(lines) + 2,
                language="python",
            )
        )
        self.store.upsert_edge(
            EdgeInfo(
                kind="CALLS",
                source=f"{module}::caller",
                target=f"{module}::long_but_linear",
                file_path=str(module),
                line=len(lines) + 1,
            )
        )
        self.store.commit()

        splits = [s for s in suggest_refactorings(self.store) if s["type"] == "split"]

        assert not any(s["symbols"] == [f"{module}::long_but_linear"] for s in splits)


class TestApplyRefactor:
    """Tests for apply_refactor."""

    def setup_method(self):
        with _refactor_lock:
            _pending_refactors.clear()

    def teardown_method(self):
        with _refactor_lock:
            _pending_refactors.clear()

    def test_apply_refactor_validates_id(self):
        """apply_refactor rejects nonexistent refactor_id."""
        # Use a real temp dir as repo_root (needs .git or .dagayn)
        tmp_dir = Path(tempfile.mkdtemp())
        (tmp_dir / ".git").mkdir()
        try:
            result = apply_refactor("nonexistent_id", tmp_dir)
            assert result["status"] == "error"
            assert "not found" in result["error"].lower() or "expired" in result["error"].lower()
        finally:
            (tmp_dir / ".git").rmdir()
            tmp_dir.rmdir()

    def test_apply_refactor_expiry(self):
        """apply_refactor rejects expired previews."""
        tmp_dir = Path(tempfile.mkdtemp())
        (tmp_dir / ".git").mkdir()
        try:
            # Insert a preview that is already expired.
            rid = "expired1"
            with _refactor_lock:
                _pending_refactors[rid] = {
                    "refactor_id": rid,
                    "type": "rename",
                    "old_name": "old",
                    "new_name": "new",
                    "edits": [],
                    "stats": {"high": 0, "medium": 0, "low": 0},
                    "created_at": time.time() - REFACTOR_EXPIRY_SECONDS - 10,
                }
            result = apply_refactor(rid, tmp_dir)
            assert result["status"] == "error"
            assert "expired" in result["error"].lower()
        finally:
            (tmp_dir / ".git").rmdir()
            tmp_dir.rmdir()

    def test_apply_refactor_path_traversal(self):
        """apply_refactor blocks edits outside repo root."""
        tmp_dir = Path(tempfile.mkdtemp())
        (tmp_dir / ".git").mkdir()
        try:
            rid = "traversal"
            with _refactor_lock:
                _pending_refactors[rid] = {
                    "refactor_id": rid,
                    "type": "rename",
                    "old_name": "old",
                    "new_name": "new",
                    "edits": [
                        {
                            "file": "/etc/passwd",
                            "line": 1,
                            "old": "old",
                            "new": "new",
                            "confidence": "high",
                        }
                    ],
                    "stats": {"high": 1, "medium": 0, "low": 0},
                    "created_at": time.time(),
                }
            result = apply_refactor(rid, tmp_dir)
            assert result["status"] == "error"
            assert "outside repo root" in result["error"].lower()
        finally:
            (tmp_dir / ".git").rmdir()
            tmp_dir.rmdir()

    def test_apply_refactor_success(self):
        """apply_refactor applies string replacement to a real file."""
        tmp_dir = Path(tempfile.mkdtemp())
        (tmp_dir / ".git").mkdir()
        target_file = tmp_dir / "example.py"
        target_file.write_text("def old_func():\n    pass\n", encoding="utf-8")
        try:
            rid = "success1"
            with _refactor_lock:
                _pending_refactors[rid] = {
                    "refactor_id": rid,
                    "type": "rename",
                    "old_name": "old_func",
                    "new_name": "new_func",
                    "edits": [
                        {
                            "file": str(target_file),
                            "line": 1,
                            "old": "old_func",
                            "new": "new_func",
                            "confidence": "high",
                        }
                    ],
                    "stats": {"high": 1, "medium": 0, "low": 0},
                    "created_at": time.time(),
                }
            result = apply_refactor(rid, tmp_dir)
            assert result["status"] == "ok"
            assert result["edits_applied"] == 1
            assert len(result["files_modified"]) == 1
            # Verify file content was changed.
            content = target_file.read_text(encoding="utf-8")
            assert "new_func" in content
            assert "old_func" not in content
        finally:
            target_file.unlink(missing_ok=True)
            (tmp_dir / ".git").rmdir()
            tmp_dir.rmdir()

    def test_apply_refactor_relative_paths_use_repo_root(self):
        """Relative edit paths resolve against repo_root for both reads and writes."""
        tmp_dir = Path(tempfile.mkdtemp())
        (tmp_dir / ".git").mkdir()
        (tmp_dir / "pkg").mkdir()
        target_file = tmp_dir / "pkg" / "example.py"
        target_file.write_text("def old_func():\n    pass\n", encoding="utf-8")
        try:
            rid = "relative1"
            with _refactor_lock:
                _pending_refactors[rid] = {
                    "refactor_id": rid,
                    "type": "rename",
                    "old_name": "old_func",
                    "new_name": "new_func",
                    "edits": [
                        {
                            "file": "pkg/example.py",
                            "line": 1,
                            "old": "old_func",
                            "new": "new_func",
                            "confidence": "high",
                        }
                    ],
                    "stats": {"high": 1, "medium": 0, "low": 0},
                    "created_at": time.time(),
                }
            result = apply_refactor(rid, tmp_dir)
            assert result["status"] == "ok"
            assert result["edits_applied"] == 1
            assert result["files_modified"] == [str(target_file.resolve())]
            content = target_file.read_text(encoding="utf-8")
            assert "new_func" in content
            assert "old_func" not in content
        finally:
            target_file.unlink(missing_ok=True)
            (tmp_dir / "pkg").rmdir()
            (tmp_dir / ".git").rmdir()
            tmp_dir.rmdir()

    def test_apply_refactor_dry_run_returns_diff_without_writing(self):
        """dry_run=True returns a unified diff without touching disk and
        keeps the refactor_id valid for a follow-up write (#176)."""
        tmp_dir = Path(tempfile.mkdtemp())
        (tmp_dir / ".git").mkdir()
        target_file = tmp_dir / "example.py"
        original = "def old_func():\n    pass\n"
        target_file.write_text(original, encoding="utf-8")
        try:
            rid = "dryrun1"
            with _refactor_lock:
                _pending_refactors[rid] = {
                    "refactor_id": rid,
                    "type": "rename",
                    "old_name": "old_func",
                    "new_name": "new_func",
                    "edits": [
                        {
                            "file": str(target_file),
                            "line": 1,
                            "old": "old_func",
                            "new": "new_func",
                            "confidence": "high",
                        }
                    ],
                    "stats": {"high": 1, "medium": 0, "low": 0},
                    "created_at": time.time(),
                }

            # Step 1: dry_run — no writes, returns diff
            result = apply_refactor(rid, tmp_dir, dry_run=True)
            assert result["status"] == "ok"
            assert result["dry_run"] is True
            assert result["edits_applied"] == 1
            assert len(result["would_modify"]) == 1
            assert result["files_modified"] == []  # nothing written yet
            assert str(target_file) in result["would_modify"]
            # Diff should mention both the old and new name
            diff = result["diffs"][str(target_file)]
            assert "-def old_func():" in diff
            assert "+def new_func():" in diff
            # File on disk must be unchanged
            assert target_file.read_text(encoding="utf-8") == original

            # Step 2: refactor_id should still be valid — dry_run doesn't consume it
            with _refactor_lock:
                assert rid in _pending_refactors

            # Step 3: real apply — uses same refactor_id
            real_result = apply_refactor(rid, tmp_dir, dry_run=False)
            assert real_result["status"] == "ok"
            assert real_result.get("dry_run") is None  # not set on the real path
            assert real_result["edits_applied"] == 1
            assert len(real_result["files_modified"]) == 1
            # File content changed
            new_content = target_file.read_text(encoding="utf-8")
            assert "new_func" in new_content
            assert "old_func" not in new_content

            # refactor_id consumed after real apply
            with _refactor_lock:
                assert rid not in _pending_refactors
        finally:
            target_file.unlink(missing_ok=True)
            (tmp_dir / ".git").rmdir()
            tmp_dir.rmdir()

    def test_apply_refactor_dry_run_no_edits(self):
        """dry_run with an empty edit list returns an empty diff dict."""
        tmp_dir = Path(tempfile.mkdtemp())
        (tmp_dir / ".git").mkdir()
        try:
            rid = "dryrun-empty"
            with _refactor_lock:
                _pending_refactors[rid] = {
                    "refactor_id": rid,
                    "type": "rename",
                    "old_name": "x",
                    "new_name": "y",
                    "edits": [],
                    "stats": {"high": 0, "medium": 0, "low": 0},
                    "created_at": time.time(),
                }
            result = apply_refactor(rid, tmp_dir, dry_run=True)
            assert result["status"] == "ok"
            assert result["dry_run"] is True
            assert result["would_modify"] == []
            assert result["diffs"] == {}
        finally:
            with _refactor_lock:
                _pending_refactors.pop("dryrun-empty", None)
            (tmp_dir / ".git").rmdir()
            tmp_dir.rmdir()


class TestPendingRefactorsThreadSafe:
    """Tests for thread-safety of the pending refactors storage."""

    def test_pending_refactors_thread_safe(self):
        """The _refactor_lock is a threading.Lock instance."""
        assert isinstance(_refactor_lock, type(threading.Lock()))

    def test_concurrent_access(self):
        """Multiple threads can safely access _pending_refactors."""
        results = []

        def writer(rid: str):
            with _refactor_lock:
                _pending_refactors[rid] = {
                    "refactor_id": rid,
                    "created_at": time.time(),
                }
                results.append(rid)

        threads = [threading.Thread(target=writer, args=(f"t{i}",)) for i in range(10)]
        for t in threads:
            t.start()
        for t in threads:
            t.join()

        with _refactor_lock:
            assert len(results) == 10
            assert len(_pending_refactors) >= 10
            # Clean up
            _pending_refactors.clear()


class TestFindDeadCodeWithReferences:
    """Tests for REFERENCES-aware dead code detection."""

    def setup_method(self):
        self.tmp = tempfile.NamedTemporaryFile(suffix=".db", delete=False)
        self.store = GraphStore(self.tmp.name)
        self._seed()

    def teardown_method(self):
        self.store.close()
        Path(self.tmp.name).unlink(missing_ok=True)

    def _seed(self):
        """Seed with functions that have REFERENCES edges (map dispatch pattern)."""
        # File
        self.store.upsert_node(
            NodeInfo(
                kind="File",
                name="/repo/handlers.ts",
                file_path="/repo/handlers.ts",
                line_start=1,
                line_end=100,
                language="typescript",
            )
        )
        # A function referenced in a map (should NOT be dead)
        self.store.upsert_node(
            NodeInfo(
                kind="Function",
                name="handleCreate",
                file_path="/repo/handlers.ts",
                line_start=10,
                line_end=20,
                language="typescript",
            )
        )
        # A function with CALLS edge (should NOT be dead)
        self.store.upsert_node(
            NodeInfo(
                kind="Function",
                name="calledFunc",
                file_path="/repo/handlers.ts",
                line_start=30,
                line_end=40,
                language="typescript",
            )
        )
        # A truly dead function (no edges at all)
        self.store.upsert_node(
            NodeInfo(
                kind="Function",
                name="deadFunc",
                file_path="/repo/handlers.ts",
                line_start=50,
                line_end=60,
                language="typescript",
            )
        )
        # Caller
        self.store.upsert_node(
            NodeInfo(
                kind="Function",
                name="dispatch",
                file_path="/repo/handlers.ts",
                line_start=70,
                line_end=80,
                language="typescript",
            )
        )
        # REFERENCES edge: dispatch -> handleCreate (map dispatch pattern)
        self.store.upsert_edge(
            EdgeInfo(
                kind="REFERENCES",
                source="/repo/handlers.ts::dispatch",
                target="/repo/handlers.ts::handleCreate",
                file_path="/repo/handlers.ts",
                line=75,
            )
        )
        # CALLS edge: dispatch -> calledFunc
        self.store.upsert_edge(
            EdgeInfo(
                kind="CALLS",
                source="/repo/handlers.ts::dispatch",
                target="/repo/handlers.ts::calledFunc",
                file_path="/repo/handlers.ts",
                line=76,
            )
        )
        self.store.commit()

    def test_referenced_function_not_dead(self):
        """Functions with REFERENCES edges should NOT be flagged as dead code."""
        dead = find_dead_code(self.store)
        dead_names = {d["name"] for d in dead}
        assert "handleCreate" not in dead_names

    def test_called_function_not_dead(self):
        """Functions with CALLS edges remain excluded (existing behavior)."""
        dead = find_dead_code(self.store)
        dead_names = {d["name"] for d in dead}
        assert "calledFunc" not in dead_names

    def test_truly_dead_function_still_reported(self):
        """Functions with no edges at all should still be flagged as dead code."""
        dead = find_dead_code(self.store)
        dead_names = {d["name"] for d in dead}
        assert "deadFunc" in dead_names

    def test_only_references_edge_sufficient(self):
        """A function with ONLY a REFERENCES edge (no CALLS/IMPORTS) is not dead."""
        dead = find_dead_code(self.store)
        dead_names = {d["name"] for d in dead}
        # handleCreate has only a REFERENCES edge, no CALLS targeting it
        assert "handleCreate" not in dead_names


class TestTransitiveImportResolution:
    """Tests for 2-hop transitive import resolution in plausible caller."""

    def setup_method(self):
        self.store = GraphStore(":memory:")
        for f in ("/repo/consumer.ts", "/repo/lib/index.ts", "/repo/lib/utils.ts"):
            self.store.upsert_node(
                NodeInfo(
                    kind="File",
                    name=f,
                    file_path=f,
                    line_start=1,
                    line_end=50,
                    language="typescript",
                )
            )

    def test_transitive_import_via_barrel_file(self):
        """consumer.ts imports index.ts which re-exports from utils.ts.
        A bare-name CALLS from consumer.ts should be plausible for utils.ts functions."""
        # Function defined in utils.ts
        self.store.upsert_node(
            NodeInfo(
                kind="Function",
                name="safeJsonParse",
                file_path="/repo/lib/utils.ts",
                line_start=10,
                line_end=20,
                language="typescript",
            )
        )
        # Import chain: consumer -> index -> utils
        self.store.upsert_edge(
            EdgeInfo(
                kind="IMPORTS_FROM",
                source="/repo/consumer.ts",
                target="/repo/lib/index.ts",
                file_path="/repo/consumer.ts",
                line=1,
            )
        )
        self.store.upsert_edge(
            EdgeInfo(
                kind="IMPORTS_FROM",
                source="/repo/lib/index.ts",
                target="/repo/lib/utils.ts",
                file_path="/repo/lib/index.ts",
                line=1,
            )
        )
        # Bare-name CALLS from consumer
        self.store.upsert_edge(
            EdgeInfo(
                kind="CALLS",
                source="/repo/consumer.ts::processData",
                target="safeJsonParse",
                file_path="/repo/consumer.ts",
                line=5,
            )
        )
        self.store.commit()
        dead = find_dead_code(self.store)
        dead_names = {d["name"] for d in dead}
        assert "safeJsonParse" not in dead_names, (
            "2-hop import chain should make consumer a plausible caller"
        )


class TestFindDeadCodeModuleScope:
    """End-to-end regression: parse → store → find_dead_code.

    Pins the contract that functions invoked only from module scope are not
    flagged as dead. Bypasses the hand-built graph fixtures used elsewhere in
    this file so that a regression in any of the parser's 5 module-scope
    CALLS paths is caught.
    """

    def setup_method(self):
        self.tmp = tempfile.NamedTemporaryFile(suffix=".db", delete=False)
        self.store = GraphStore(self.tmp.name)
        self.parser = CodeParser()

    def teardown_method(self):
        self.store.close()
        Path(self.tmp.name).unlink(missing_ok=True)

    def _store_parsed(self, path: Path, source: bytes) -> None:
        nodes, edges = self.parser.parse_bytes(path, source)
        for n in nodes:
            self.store.upsert_node(n)
        for e in edges:
            self.store.upsert_edge(e)
        self.store.commit()

    def test_module_scope_caller_prevents_dead_code_flag(self, tmp_path):
        """A function called only from top-level script glue is not dead."""
        # ``run_job`` has no non-dunder name match and no framework decorator,
        # so without the module-scope CALLS fix it would be flagged dead.
        path = tmp_path / "script.py"
        path.write_bytes(b"def run_job():\n    return 1\n\nrun_job()\n")
        self._store_parsed(path, path.read_bytes())

        dead = find_dead_code(self.store)
        dead_names = {d["name"] for d in dead}
        assert "run_job" not in dead_names, (
            "module-scope caller should prevent run_job from being flagged dead"
        )

    def test_if_main_block_caller_prevents_dead_code_flag(self, tmp_path):
        """A function called only inside ``if __name__ == '__main__'`` is not dead."""
        path = tmp_path / "cli.py"
        path.write_bytes(
            b"def launch():\n    return 1\n\nif __name__ == '__main__':\n    launch()\n"
        )
        self._store_parsed(path, path.read_bytes())

        dead = find_dead_code(self.store)
        dead_names = {d["name"] for d in dead}
        assert "launch" not in dead_names
