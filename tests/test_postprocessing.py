"""Tests for the shared post-processing pipeline."""

import json
import tempfile
from pathlib import Path
from unittest.mock import MagicMock, patch

from dagayn.graph import GraphStore
from dagayn.incremental import full_build
from dagayn.parser import EdgeInfo, NodeInfo
from dagayn.postprocessing import _resolve_markdown_artifact_refs, run_post_processing


def _get_signature(store, qualified_name):
    row = store._conn.execute(
        "SELECT signature FROM nodes WHERE qualified_name = ?",
        (qualified_name,),
    ).fetchone()
    return row["signature"] if row else None


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

        assert result["signatures_computed"] > 0
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

        assert "fts_indexed" in result
        assert result["fts_indexed"] > 0

    def test_fts_search_works_after_post_processing(self):
        run_post_processing(self.store)

        from dagayn.search import hybrid_search

        hits = hybrid_search(self.store, "handle")
        names = {h["name"] for h in hits}
        assert "handle" in names

    def test_detects_flows(self):
        result = run_post_processing(self.store)

        assert "flows_detected" in result
        assert result["flows_detected"] >= 0

    def test_detects_communities(self):
        result = run_post_processing(self.store)

        assert "communities_detected" in result
        assert result["communities_detected"] >= 0

    def test_no_warnings_on_healthy_store(self):
        result = run_post_processing(self.store)

        assert "warnings" not in result

    def test_empty_store_no_crash(self):
        empty_tmp = tempfile.NamedTemporaryFile(suffix=".db", delete=False)
        empty_store = GraphStore(empty_tmp.name)
        try:
            result = run_post_processing(empty_store)
            assert result["signatures_computed"] == 0
            assert result["fts_indexed"] == 0
        finally:
            empty_store.close()
            Path(empty_tmp.name).unlink(missing_ok=True)

    def test_idempotent(self):
        first = run_post_processing(self.store)
        second = run_post_processing(self.store)

        assert second["fts_indexed"] == first["fts_indexed"]
        assert second["signatures_computed"] == 0

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

        assert "flows_detected" in result
        assert "communities_detected" in result
        assert "warnings" in result
        assert any("FTS" in w for w in result["warnings"])

    def test_flow_failure_does_not_block_communities(self):
        with patch(
            "dagayn.flows.trace_flows",
            side_effect=ImportError("flow boom"),
        ):
            result = run_post_processing(self.store)

        assert "communities_detected" in result
        assert "warnings" in result
        assert any("Flow" in w for w in result["warnings"])

    def test_community_failure_still_has_signatures(self):
        with patch(
            "dagayn.communities.detect_communities",
            side_effect=ImportError("comm boom"),
        ):
            result = run_post_processing(self.store)

        assert result["signatures_computed"] > 0
        assert "warnings" in result
        assert any("Community" in w for w in result["warnings"])


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
                "unresolved_target_name": sym,
            },
        )

    def test_resolves_unique_match_to_high(self):
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

        result: dict = {}
        _resolve_markdown_artifact_refs(self.store, result, [])

        assert result["markdown_artifact_refs_resolved"] == 1
        assert result["markdown_artifact_refs_dropped"] == 0

        row = self.store._conn.execute(
            "SELECT target_qualified, confidence_tier, extra FROM edges WHERE kind='CROSS_ARTIFACT'"
        ).fetchone()
        assert row["target_qualified"] == "/repo/parser.py::BridgePattern"
        assert row["confidence_tier"] == "HIGH"
        extra = json.loads(row["extra"])
        assert "unresolved_target_name" not in extra
        assert extra["target_language"] == "python"
        assert extra["confidence"] == 0.8

    def test_drops_ambiguous_match(self):
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

        result: dict = {}
        _resolve_markdown_artifact_refs(self.store, result, [])

        assert result["markdown_artifact_refs_resolved"] == 0
        assert result["markdown_artifact_refs_dropped"] == 1
        count = self.store._conn.execute(
            "SELECT COUNT(*) FROM edges WHERE kind='CROSS_ARTIFACT'"
        ).fetchone()[0]
        assert count == 0

    def test_drops_unmatched(self):
        self.store.upsert_edge(self._unresolved_edge("NonexistentSymbolXYZ"))
        self.store.commit()

        result: dict = {}
        _resolve_markdown_artifact_refs(self.store, result, [])

        assert result["markdown_artifact_refs_resolved"] == 0
        assert result["markdown_artifact_refs_dropped"] == 1

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

        result: dict = {}
        _resolve_markdown_artifact_refs(self.store, result, [])

        assert result["markdown_artifact_refs_dropped"] == 1

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

        result1: dict = {}
        _resolve_markdown_artifact_refs(self.store, result1, [])
        assert result1["markdown_artifact_refs_resolved"] == 1

        result2: dict = {}
        _resolve_markdown_artifact_refs(self.store, result2, [])
        assert result2["markdown_artifact_refs_resolved"] == 0
        assert result2["markdown_artifact_refs_dropped"] == 0

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

        assert result.get("markdown_artifact_refs_resolved") == 1
        row = self.store._conn.execute(
            "SELECT target_qualified FROM edges WHERE kind='CROSS_ARTIFACT'"
        ).fetchone()
        assert row["target_qualified"] == "/repo/ui.py::Widget"
