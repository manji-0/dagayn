"""Tests for the shared post-processing pipeline."""

import json
import tempfile
from pathlib import Path
from unittest.mock import MagicMock, patch

from dagayn.graph import GraphStore
from dagayn.incremental import full_build
from dagayn.parser import EdgeInfo, NodeInfo
from dagayn.postprocessing import (
    _markdown_artifact_resolution,
    _resolve_markdown_artifact_refs,
    run_post_processing,
)
from dagayn.state_types import PostprocessResult


def _get_signature(store, qualified_name):
    row = store._conn.execute(
        "SELECT signature FROM nodes WHERE qualified_name = ?",
        (qualified_name,),
    ).fetchone()
    return row["signature"] if row else None


def test_markdown_artifact_resolution_returns_typed_states():
    resolved = _markdown_artifact_resolution(
        edge_id=1,
        current_target="<unresolved:Service>",
        symbol="Service",
        extra={"evidence_kind": "markdown_code_span", "evidence_source": "code_span"},
        matches=[("/repo/app.py::Service", "python")],
    )
    assert resolved.state == "resolved"
    assert resolved.target_qualified == "/repo/app.py::Service"
    assert resolved.confidence_tier == "MEDIUM"
    assert resolved.confidence == 0.4

    directive_resolved = _markdown_artifact_resolution(
        edge_id=4,
        current_target="<unresolved:Service>",
        symbol="Service",
        extra={
            "evidence_kind": "markdown_directive",
            "evidence_source": "dagayn_directive",
        },
        matches=[("/repo/app.py::Service", "python")],
    )
    assert directive_resolved.confidence_tier == "HIGH"
    assert directive_resolved.confidence == 0.8

    dropped = _markdown_artifact_resolution(
        edge_id=2,
        current_target="<unresolved:Missing>",
        symbol="Missing",
        extra={"evidence_kind": "markdown_code_span", "evidence_source": "code_span"},
        matches=[],
    )
    assert dropped.state == "dropped"
    assert dropped.edge_id == 2
    assert dropped.target_qualified is None

    still_unresolved = _markdown_artifact_resolution(
        edge_id=3,
        current_target="<unresolved:Missing>",
        symbol="Missing",
        extra={"relationship_role": "implemented_by"},
        matches=[],
    )
    assert still_unresolved.state == "still_unresolved"
    assert still_unresolved.confidence_tier == "LOW"


class TestRunPostProcessing:
    def setup_method(self):
        self.tmp = tempfile.NamedTemporaryFile(suffix=".db", delete=False)
        self.store = GraphStore(self.tmp.name)
        self._seed_data()

    def teardown_method(self):
        self.store.close()
        Path(self.tmp.name).unlink(missing_ok=True)

    def _seed_data(self):
        self.store.upsert_node(
            NodeInfo(
                kind="File",
                name="/repo/app.py",
                file_path="/repo/app.py",
                line_start=1,
                line_end=50,
                language="python",
            )
        )
        self.store.upsert_node(
            NodeInfo(
                kind="Class",
                name="Service",
                file_path="/repo/app.py",
                line_start=5,
                line_end=40,
                language="python",
            )
        )
        self.store.upsert_node(
            NodeInfo(
                kind="Function",
                name="handle",
                file_path="/repo/app.py",
                line_start=10,
                line_end=20,
                language="python",
                parent_name="Service",
                params="request",
                return_type="Response",
            )
        )
        self.store.upsert_node(
            NodeInfo(
                kind="Function",
                name="process",
                file_path="/repo/app.py",
                line_start=25,
                line_end=35,
                language="python",
            )
        )
        self.store.upsert_node(
            NodeInfo(
                kind="Test",
                name="test_handle",
                file_path="/repo/test_app.py",
                line_start=1,
                line_end=10,
                language="python",
                is_test=True,
            )
        )

        self.store.upsert_edge(
            EdgeInfo(
                kind="CONTAINS",
                source="/repo/app.py",
                target="/repo/app.py::Service",
                file_path="/repo/app.py",
            )
        )
        self.store.upsert_edge(
            EdgeInfo(
                kind="CONTAINS",
                source="/repo/app.py::Service",
                target="/repo/app.py::Service.handle",
                file_path="/repo/app.py",
            )
        )
        self.store.upsert_edge(
            EdgeInfo(
                kind="CALLS",
                source="/repo/app.py::Service.handle",
                target="/repo/app.py::process",
                file_path="/repo/app.py",
                line=15,
            )
        )
        self.store.commit()

    def test_computes_signatures(self):
        unsigned = self.store.get_nodes_without_signature()
        assert len(unsigned) > 0

        result = run_post_processing(self.store)

        assert (result.signatures_computed or 0) > 0
        remaining = self.store.get_nodes_without_signature()
        assert len(remaining) == 0

    def test_function_signature_format(self):
        run_post_processing(self.store)

        sig = _get_signature(self.store, "/repo/app.py::Service.handle")
        assert sig == "def handle(request) -> Response"

    def test_class_signature_format(self):
        run_post_processing(self.store)

        sig = _get_signature(self.store, "/repo/app.py::Service")
        assert sig == "class Service"

    def test_test_signature_format(self):
        run_post_processing(self.store)

        sig = _get_signature(self.store, "/repo/test_app.py::test_handle")
        assert sig is not None
        assert sig.startswith("def test_handle(")

    def test_rebuilds_fts_index(self):
        result = run_post_processing(self.store)

        assert result.fts_indexed is not None
        assert result.fts_indexed > 0

    def test_fts_search_works_after_post_processing(self):
        run_post_processing(self.store)

        from dagayn.search import hybrid_search

        hits = hybrid_search(self.store, "handle")["results"]
        names = {h["name"] for h in hits}
        assert "handle" in names

    def test_detects_flows(self):
        result = run_post_processing(self.store)

        assert result.flows_detected is not None
        assert result.flows_detected >= 0

    def test_detects_communities(self):
        result = run_post_processing(self.store)

        assert result.communities_detected is not None
        assert result.communities_detected >= 0

    def test_no_warnings_on_healthy_store(self):
        result = run_post_processing(self.store)

        assert not result.warnings

    def test_empty_store_no_crash(self):
        empty_tmp = tempfile.NamedTemporaryFile(suffix=".db", delete=False)
        empty_store = GraphStore(empty_tmp.name)
        try:
            result = run_post_processing(empty_store)
            assert result.signatures_computed == 0
            assert result.fts_indexed == 0
        finally:
            empty_store.close()
            Path(empty_tmp.name).unlink(missing_ok=True)

    def test_idempotent(self):
        first = run_post_processing(self.store)
        second = run_post_processing(self.store)

        assert (second.fts_indexed or 0) == (first.fts_indexed or 0)
        assert second.signatures_computed == 0

    def test_signature_truncated_at_512(self):
        self.store.upsert_node(
            NodeInfo(
                kind="Function",
                name="f",
                file_path="/repo/big.py",
                line_start=1,
                line_end=2,
                language="python",
                params="a" * 600,
            )
        )
        self.store.commit()

        run_post_processing(self.store)
        sig = _get_signature(self.store, "/repo/big.py::f")
        assert sig is not None
        assert len(sig) <= 512


class TestPostProcessingStepIsolation:
    def setup_method(self):
        self.tmp = tempfile.NamedTemporaryFile(suffix=".db", delete=False)
        self.store = GraphStore(self.tmp.name)
        self.store.upsert_node(
            NodeInfo(
                kind="Function",
                name="fn",
                file_path="/repo/a.py",
                line_start=1,
                line_end=5,
                language="python",
            )
        )
        self.store.commit()

    def teardown_method(self):
        self.store.close()
        Path(self.tmp.name).unlink(missing_ok=True)

    def test_fts_failure_does_not_block_flows(self):
        with patch(
            "dagayn.search.rebuild_fts_index",
            side_effect=ImportError("fts boom"),
        ):
            result = run_post_processing(self.store)

        assert result.flows_detected is not None
        assert result.communities_detected is not None
        assert result.warnings
        assert any("FTS" in w for w in result.warnings)

    def test_flow_failure_does_not_block_communities(self):
        with patch(
            "dagayn.flows.trace_flows",
            side_effect=ImportError("flow boom"),
        ):
            result = run_post_processing(self.store)

        assert result.communities_detected is not None
        assert result.warnings
        assert any("Flow" in w for w in result.warnings)

    def test_community_failure_still_has_signatures(self):
        with patch(
            "dagayn.communities.detect_communities",
            side_effect=ImportError("comm boom"),
        ):
            result = run_post_processing(self.store)

        assert (result.signatures_computed or 0) > 0
        assert result.warnings
        assert any("Community" in w for w in result.warnings)


class TestToolBuildUsesSharedPipeline:
    def test_build_tool_runs_post_processing(self, tmp_path):
        py_file = tmp_path / "sample.py"
        py_file.write_text("def hello():\n    pass\n")
        (tmp_path / ".git").mkdir()
        (tmp_path / ".dagayn").mkdir()

        db_path = tmp_path / ".dagayn" / "graph.db"
        store = GraphStore(db_path)
        try:
            mock_target = "dagayn.incremental.get_all_tracked_files"
            with patch(mock_target, return_value=["sample.py"]):
                full_build(tmp_path, store)

            unsigned_before_pp = store.get_nodes_without_signature()
            run_post_processing(store)
            unsigned_after_pp = store.get_nodes_without_signature()

            assert len(unsigned_before_pp) > 0
            assert len(unsigned_after_pp) == 0
        finally:
            store.close()


class TestWatchCallbackIntegration:
    def test_watch_accepts_callback_parameter(self):
        import inspect

        from dagayn.incremental import watch

        sig = inspect.signature(watch)
        assert "on_files_updated" in sig.parameters

    def test_watch_callback_not_called_without_updates(self, tmp_path):
        import threading

        from dagayn.incremental import watch

        (tmp_path / ".git").mkdir()
        db_path = tmp_path / "test.db"
        store = GraphStore(db_path)
        callback = MagicMock()

        try:

            def run_watch():
                try:
                    watch(tmp_path, store, on_files_updated=callback)
                except KeyboardInterrupt:
                    pass

            t = threading.Thread(target=run_watch, daemon=True)
            t.start()

            import time

            time.sleep(0.5)
            callback.assert_not_called()
        finally:
            store.close()


class TestMarkdownArtifactResolver:
    """Unit tests for _resolve_markdown_artifact_refs postprocess step."""

    def setup_method(self):
        self.tmp = tempfile.NamedTemporaryFile(suffix=".db", delete=False)
        self.store = GraphStore(self.tmp.name)

    def teardown_method(self):
        self.store.close()
        Path(self.tmp.name).unlink(missing_ok=True)

    def _unresolved_edge(self, sym: str, line: int = 5) -> EdgeInfo:
        return EdgeInfo(
            kind="CROSS_ARTIFACT",
            source="/repo/docs/spec.md::section",
            target=f"<unresolved:{sym}>",
            file_path="/repo/docs/spec.md",
            line=line,
            extra={
                "relationship_role": "describes_symbol",
                "bridge_kind": "documentation",
                "evidence_kind": "markdown_code_span",
                "evidence_source": "code_span",
                "source_language": "markdown",
                "target_language": "unknown",
                "confidence": 0.2,
                "confidence_tier": "LOW",
                "original_symbol_name": sym,
            },
        )

    def _directive_edge(self, sym: str, line: int = 5) -> EdgeInfo:
        edge = self._unresolved_edge(sym, line=line)
        return EdgeInfo(
            kind=edge.kind,
            source=edge.source,
            target=edge.target,
            file_path=edge.file_path,
            line=edge.line,
            extra={
                **edge.extra,
                "relationship_role": "implemented_by",
                "evidence_kind": "markdown_directive",
                "evidence_source": "dagayn_directive",
                "dagayn_directive_kind": "implemented-by",
            },
        )

    def test_resolves_unique_code_span_match_to_medium(self):
        self.store.upsert_node(
            NodeInfo(
                kind="Class",
                name="BridgePattern",
                file_path="/repo/parser.py",
                line_start=1,
                line_end=10,
                language="python",
            )
        )
        self.store.upsert_edge(self._unresolved_edge("BridgePattern"))
        self.store.commit()

        result = PostprocessResult()
        _resolve_markdown_artifact_refs(self.store, result, [])

        assert result.markdown_artifact_refs_resolved == 1
        assert result.markdown_artifact_refs_dropped == 0

        row = self.store._conn.execute(
            "SELECT target_qualified, target_name, confidence_tier, extra FROM edges "
            "WHERE kind='CROSS_ARTIFACT'"
        ).fetchone()
        assert row["target_qualified"] == "/repo/parser.py::BridgePattern"
        assert row["target_name"] == "BridgePattern"
        assert row["confidence_tier"] == "MEDIUM"
        extra = json.loads(row["extra"])
        assert extra["original_symbol_name"] == "BridgePattern"  # always preserved
        assert extra["target_language"] == "python"
        assert extra["confidence"] == 0.4

    def test_resolves_unique_directive_match_to_high(self):
        self.store.upsert_node(
            NodeInfo(
                kind="Class",
                name="BridgePattern",
                file_path="/repo/parser.py",
                line_start=1,
                line_end=10,
                language="python",
            )
        )
        self.store.upsert_edge(self._directive_edge("BridgePattern"))
        self.store.commit()

        result = PostprocessResult()
        _resolve_markdown_artifact_refs(self.store, result, [])

        assert result.markdown_artifact_refs_resolved == 1
        row = self.store._conn.execute(
            "SELECT target_qualified, target_name, confidence_tier, extra FROM edges "
            "WHERE kind='CROSS_ARTIFACT'"
        ).fetchone()
        assert row["target_qualified"] == "/repo/parser.py::BridgePattern"
        assert row["target_name"] == "BridgePattern"
        assert row["confidence_tier"] == "HIGH"
        extra = json.loads(row["extra"])
        assert extra["confidence"] == 0.8

    def test_prunes_ambiguous_code_span_candidate(self):
        """Ambiguous Markdown code spans are dropped instead of persisted as data."""
        for fp in ("/repo/a.py", "/repo/b.py"):
            self.store.upsert_node(
                NodeInfo(
                    kind="Class",
                    name="Foo",
                    file_path=fp,
                    line_start=1,
                    line_end=5,
                    language="python",
                )
            )
        self.store.upsert_edge(self._unresolved_edge("Foo"))
        self.store.commit()

        result = PostprocessResult()
        _resolve_markdown_artifact_refs(self.store, result, [])

        assert result.markdown_artifact_refs_resolved == 0
        assert result.markdown_artifact_refs_dropped == 1
        assert result.markdown_artifact_refs_still_unresolved == 0
        count = self.store._conn.execute(
            "SELECT COUNT(*) FROM edges WHERE kind='CROSS_ARTIFACT'"
        ).fetchone()[0]
        assert count == 0

    def test_prunes_unmatched_code_span_candidate(self):
        """No-match Markdown code spans are dropped instead of persisted as data."""
        self.store.upsert_edge(self._unresolved_edge("NonexistentSymbolXYZ"))
        self.store.commit()

        result = PostprocessResult()
        _resolve_markdown_artifact_refs(self.store, result, [])

        assert result.markdown_artifact_refs_resolved == 0
        assert result.markdown_artifact_refs_dropped == 1
        assert result.markdown_artifact_refs_still_unresolved == 0
        count = self.store._conn.execute(
            "SELECT COUNT(*) FROM edges WHERE kind='CROSS_ARTIFACT'"
        ).fetchone()[0]
        assert count == 0

    def test_keeps_unmatched_explicit_directive_as_unresolved(self):
        """Explicit dagayn directives are author-declared references, not prose candidates."""
        self.store.upsert_edge(self._directive_edge("NonexistentSymbolXYZ"))
        self.store.commit()

        result = PostprocessResult()
        _resolve_markdown_artifact_refs(self.store, result, [])

        assert result.markdown_artifact_refs_resolved == 0
        assert result.markdown_artifact_refs_dropped == 0
        assert result.markdown_artifact_refs_still_unresolved == 1
        count = self.store._conn.execute(
            "SELECT COUNT(*) FROM edges WHERE kind='CROSS_ARTIFACT'"
        ).fetchone()[0]
        assert count == 1

    def test_does_not_match_markdown_nodes(self):
        self.store.upsert_node(
            NodeInfo(
                kind="Class",
                name="MySection",
                file_path="/repo/docs/spec.md",
                line_start=1,
                line_end=1,
                language="markdown",
                extra={"markdown_kind": "section"},
            )
        )
        self.store.upsert_edge(self._unresolved_edge("MySection"))
        self.store.commit()

        result = PostprocessResult()
        _resolve_markdown_artifact_refs(self.store, result, [])

        assert result.markdown_artifact_refs_still_unresolved == 0
        assert result.markdown_artifact_refs_dropped == 1
        assert result.markdown_artifact_refs_resolved == 0

    def test_idempotent_second_run_no_ops(self):
        self.store.upsert_node(
            NodeInfo(
                kind="Function",
                name="helper",
                file_path="/repo/x.py",
                line_start=1,
                line_end=5,
                language="python",
            )
        )
        self.store.upsert_edge(self._unresolved_edge("helper"))
        self.store.commit()

        result1 = PostprocessResult()
        _resolve_markdown_artifact_refs(self.store, result1, [])
        assert result1.markdown_artifact_refs_resolved == 1

        result2 = PostprocessResult()
        _resolve_markdown_artifact_refs(self.store, result2, [])
        assert result2.markdown_artifact_refs_resolved == 0
        assert result2.markdown_artifact_refs_dropped == 0

    def test_run_post_processing_includes_resolver(self):
        self.store.upsert_node(
            NodeInfo(
                kind="Class",
                name="Widget",
                file_path="/repo/ui.py",
                line_start=1,
                line_end=10,
                language="python",
            )
        )
        self.store.upsert_edge(self._unresolved_edge("Widget"))
        self.store.commit()

        result = run_post_processing(self.store)

        assert result.markdown_artifact_refs_resolved == 1
        row = self.store._conn.execute(
            "SELECT target_qualified FROM edges WHERE kind='CROSS_ARTIFACT'"
        ).fetchone()
        assert row["target_qualified"] == "/repo/ui.py::Widget"


class TestTerraformArtifactResolver:
    """Postprocess resolution for Terraform entrypoint CROSS_ARTIFACT edges."""

    def setup_method(self):
        self.tmp = tempfile.NamedTemporaryFile(suffix=".db", delete=False)
        self.store = GraphStore(self.tmp.name)

    def teardown_method(self):
        self.store.close()
        Path(self.tmp.name).unlink(missing_ok=True)

    def _handler_edge(self, symbol: str = "hello.main") -> EdgeInfo:
        return EdgeInfo(
            kind="CROSS_ARTIFACT",
            source="infra/main.tf::resource.aws_lambda_function.auth",
            target=f"<unresolved:{symbol}>",
            file_path="infra/main.tf",
            line=4,
            extra={
                "relationship_role": "maps_entrypoint",
                "bridge_kind": "manifest_link",
                "evidence_kind": "config",
                "evidence_source": "handler",
                "source_language": "terraform",
                "target_language": "unknown",
                "confidence": 0.8,
                "confidence_tier": "HIGH",
                "original_symbol_name": symbol,
            },
        )

    def test_resolves_unique_module_handler(self):
        self.store.upsert_node(
            NodeInfo(
                kind="Function",
                name="main",
                file_path="app/hello.py",
                line_start=1,
                line_end=2,
                language="python",
            )
        )
        self.store.upsert_edge(self._handler_edge("hello.main"))
        self.store.commit()

        result = PostprocessResult()
        warnings: list[str] = []
        from dagayn.postprocessing import _resolve_terraform_artifact_refs

        _resolve_terraform_artifact_refs(self.store, result, warnings)
        assert warnings == []
        assert result.terraform_artifact_refs_resolved == 1

        row = self.store._conn.execute(
            "SELECT target_qualified, target_name, confidence_tier, extra FROM edges "
            "WHERE kind='CROSS_ARTIFACT'"
        ).fetchone()
        assert row["target_qualified"] == "app/hello.py::main"
        assert row["target_name"] == "main"
        assert row["confidence_tier"] == "HIGH"
        extra = json.loads(row["extra"])
        assert extra["target_language"] == "python"
        assert extra["original_symbol_name"] == "hello.main"

    def test_mixed_fixture_survives_postprocess_and_is_queryable(self, tmp_path):
        import shutil
        from unittest.mock import patch

        from dagayn.incremental import full_build
        from dagayn.postprocessing import run_post_processing
        from dagayn.tools.query import query_graph

        fixture = Path(__file__).parent / "fixtures" / "terraform_cross_artifact"
        for rel in (
            "infra/main.tf",
            "app/hello.py",
            "scripts/bootstrap.py",
        ):
            dest = tmp_path / rel
            dest.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy(fixture / rel, dest)

        (tmp_path / ".git").mkdir()
        (tmp_path / ".dagayn").mkdir()
        db_path = tmp_path / ".dagayn" / "graph.db"
        store = GraphStore(db_path)
        tracked = ["infra/main.tf", "app/hello.py", "scripts/bootstrap.py"]
        try:
            with patch("dagayn.incremental_build.collect_all_files", return_value=tracked):
                full_build(tmp_path, store)
            result = run_post_processing(store)
            assert not result.warnings

            rows = store._conn.execute(
                "SELECT source_qualified, target_qualified, extra FROM edges "
                "WHERE kind='CROSS_ARTIFACT' AND extra LIKE '%\"source_language\": \"terraform\"%'"
            ).fetchall()
            assert len(rows) >= 3
            extras = [json.loads(r["extra"]) for r in rows]
            assert any(e.get("evidence_source") == "filename" for e in extras)
            assert any(e.get("evidence_source") == "provisioner.local-exec.command" for e in extras)
            assert any(e.get("confidence_tier") == "HIGH" for e in extras)

            handler_rows = [
                r for r in rows if json.loads(r["extra"]).get("evidence_source") == "handler"
            ]
            assert handler_rows
            assert handler_rows[0]["target_qualified"].endswith("::main")
        finally:
            store.close()

        q = query_graph(
            pattern="bridges_from",
            target="infra/main.tf::resource.aws_lambda_function.auth",
            repo_root=str(tmp_path),
        )
        assert q["result_count"] >= 1
        roles = {r.get("relationship_role") for r in q["results"]}
        assert "maps_entrypoint" in roles

    def test_markdown_resolver_does_not_hijack_terraform_handler(self):
        """A unique non-Function named like the handler must not steal the edge.

        Markdown resolution binds any unique non-Markdown node by bare name.
        For ``hello.main``, that would wrongly attach to a Class named
        ``hello.main``.  Full postprocess must leave Function matching to the
        Terraform resolver instead.
        """
        self.store.upsert_node(
            NodeInfo(
                kind="Class",
                name="hello.main",
                file_path="app/decoy.py",
                line_start=1,
                line_end=10,
                language="python",
            )
        )
        self.store.upsert_node(
            NodeInfo(
                kind="Function",
                name="main",
                file_path="app/hello.py",
                line_start=1,
                line_end=2,
                language="python",
            )
        )
        self.store.upsert_edge(self._handler_edge("hello.main"))
        self.store.commit()

        result = run_post_processing(self.store)
        assert not result.warnings
        assert (result.markdown_artifact_refs_resolved or 0) == 0
        assert result.terraform_artifact_refs_resolved == 1

        row = self.store._conn.execute(
            "SELECT target_qualified, target_name, confidence_tier, extra FROM edges "
            "WHERE kind='CROSS_ARTIFACT'"
        ).fetchone()
        assert row["target_qualified"] == "app/hello.py::main"
        assert row["target_name"] == "main"
        assert row["confidence_tier"] == "HIGH"
        extra = json.loads(row["extra"])
        assert extra["source_language"] == "terraform"
        assert extra["target_language"] == "python"
        assert extra["original_symbol_name"] == "hello.main"

    def test_markdown_resolver_leaves_terraform_unresolved_without_function(self):
        """Without a Function/Test match, terraform edges stay unresolved."""
        self.store.upsert_node(
            NodeInfo(
                kind="Class",
                name="serve",
                file_path="app/decoy.py",
                line_start=1,
                line_end=10,
                language="python",
            )
        )
        self.store.upsert_edge(
            EdgeInfo(
                kind="CROSS_ARTIFACT",
                source="infra/main.tf::resource.google_cloudfunctions2_function.api",
                target="<unresolved:serve>",
                file_path="infra/main.tf",
                line=10,
                extra={
                    "relationship_role": "maps_entrypoint",
                    "bridge_kind": "manifest_link",
                    "evidence_kind": "config",
                    "evidence_source": "entry_point",
                    "source_language": "terraform",
                    "target_language": "unknown",
                    "confidence": 0.8,
                    "confidence_tier": "HIGH",
                    "original_symbol_name": "serve",
                },
            )
        )
        self.store.commit()

        result = run_post_processing(self.store)
        assert not result.warnings
        assert (result.markdown_artifact_refs_resolved or 0) == 0
        assert (result.terraform_artifact_refs_resolved or 0) == 0
        assert result.terraform_artifact_refs_still_unresolved == 1

        row = self.store._conn.execute(
            "SELECT target_qualified, confidence_tier FROM edges WHERE kind='CROSS_ARTIFACT'"
        ).fetchone()
        assert row["target_qualified"] == "<unresolved:serve>"
        assert row["confidence_tier"] == "HIGH"

    def test_run_postprocess_resolves_terraform_handlers(self):
        from dagayn.tools.build import _run_postprocess

        self.store.upsert_node(
            NodeInfo(
                kind="Function",
                name="main",
                file_path="app/hello.py",
                line_start=1,
                line_end=2,
                language="python",
            )
        )
        self.store.upsert_edge(self._handler_edge("hello.main"))
        self.store.commit()

        build_result: dict = {}
        warnings = _run_postprocess(self.store, build_result, "minimal", full_rebuild=True)
        assert warnings == []
        assert build_result["terraform_artifact_refs_resolved"] == 1

        row = self.store._conn.execute(
            "SELECT target_qualified, target_name, confidence_tier FROM edges "
            "WHERE kind='CROSS_ARTIFACT'"
        ).fetchone()
        assert row["target_qualified"] == "app/hello.py::main"
        assert row["target_name"] == "main"
        assert row["confidence_tier"] == "HIGH"


class TestPostprocessLevelMetadata:
    """A minimal run must record that it was minimal."""

    def test_minimal_postprocess_records_its_level(self):
        from dagayn.tools.build import _run_postprocess

        with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as handle:
            db_path = handle.name
        try:
            store = GraphStore(db_path)
            try:
                store.set_metadata("postprocess_level", "full")
                _run_postprocess(store, {}, "minimal", full_rebuild=True)
                # Returning early used to leave the previous level in place, so
                # a graph whose flows were never computed advertised "full".
                assert store.get_metadata("postprocess_level") == "minimal"
                assert store.get_metadata("last_postprocessed_at")
            finally:
                store.close()
        finally:
            Path(db_path).unlink(missing_ok=True)


class TestNativeStoreStaysOnOneConnection:
    def test_rust_postprocess_does_not_construct_python_graph_store(self, monkeypatch, tmp_path):
        try:
            from dagayn._core import GraphStore as NativeGraphStore
        except ImportError:
            import pytest

            pytest.skip("native extension not built")  # ty: ignore[too-many-positional-arguments]

        from dagayn.graph.core import GraphStore as PyGraphStore
        from dagayn.postprocessing import run_post_processing

        constructed: list[str | Path] = []
        original_init = PyGraphStore.__init__

        def tracking_init(self, *args: object, **kwargs: object) -> None:
            db_path = args[0] if args else kwargs.get("db_path")
            if isinstance(db_path, (str, Path)):
                constructed.append(db_path)
                original_init(self, db_path)

        db = tmp_path / "graph.db"
        store = NativeGraphStore(db)
        store.set_metadata("repo_root", str(tmp_path))
        monkeypatch.setattr(PyGraphStore, "__init__", tracking_init)
        try:
            result = run_post_processing(store)
        finally:
            store.close()

        assert constructed == []
        assert not result.warnings or result.warnings == []
