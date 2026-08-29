"""Integration tests for Markdown CROSS_ARTIFACT resolution.

Each test exercises a state transition for resolved references and verifies
that unresolved implicit Markdown code-span candidates do not persist as graph
data.
"""

from __future__ import annotations

import tempfile
from pathlib import Path

from dagayn.graph import GraphStore
from dagayn.parser.types import EdgeInfo, NodeInfo
from dagayn.postprocessing import _resolve_markdown_artifact_refs
from dagayn.state_types import PostprocessResult
from tests.store_sql import store_conn


def _make_store() -> tuple[GraphStore, Path]:
    tmp = tempfile.NamedTemporaryFile(suffix=".db", delete=False)
    store = GraphStore(tmp.name)
    return store, Path(tmp.name)


def _ca_edge(sym: str, target: str | None = None, line: int = 5) -> EdgeInfo:
    return EdgeInfo(
        kind="CROSS_ARTIFACT",
        source="/repo/docs/spec.md::section",
        target=target or f"<unresolved:{sym}>",
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


def _directive_edge(sym: str, target: str | None = None, line: int = 5) -> EdgeInfo:
    edge = _ca_edge(sym, target=target, line=line)
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


def _py_node(name: str, fp: str) -> NodeInfo:
    return NodeInfo(
        kind="Function", name=name, file_path=fp, line_start=1, line_end=5, language="python"
    )


class TestResolveToDemote:
    """Resolved code-span edge is pruned when its target symbol is removed."""

    def setup_method(self):
        self.store, self.tmp = _make_store()

    def teardown_method(self):
        self.store.close()
        Path(self.tmp).unlink(missing_ok=True)

    def test_resolve_then_demote(self):
        self.store.upsert_node(_py_node("compute_foo", "/repo/foo.py"))
        # Edge starts resolved (as if a previous run resolved it)
        resolved_edge = _ca_edge("compute_foo", target="/repo/foo.py::compute_foo")
        resolved_edge = EdgeInfo(
            kind=resolved_edge.kind,
            source=resolved_edge.source,
            target="/repo/foo.py::compute_foo",
            file_path=resolved_edge.file_path,
            line=resolved_edge.line,
            extra={
                **resolved_edge.extra,
                "confidence": 0.8,
                "confidence_tier": "HIGH",
                "target_language": "python",
            },
        )
        self.store.upsert_edge(resolved_edge)
        self.store.commit()

        # Symbol still exists — no change
        result = PostprocessResult()
        _resolve_markdown_artifact_refs(self.store, result, [])
        assert result.markdown_artifact_refs_resolved == 0
        assert result.markdown_artifact_refs_dropped == 0

        # Now remove the symbol (simulate rename/delete in Python)
        store_conn(self.store).execute("DELETE FROM nodes WHERE name='compute_foo'")
        self.store.commit()

        result2 = PostprocessResult()
        _resolve_markdown_artifact_refs(self.store, result2, [])
        assert result2.markdown_artifact_refs_dropped == 1

        count = (
            store_conn(self.store)
            .execute("SELECT COUNT(*) FROM edges WHERE kind='CROSS_ARTIFACT'")
            .fetchone()[0]
        )
        assert count == 0


class TestUnresolvedToResolved:
    """Unresolved implicit candidates are pruned if they cannot resolve immediately."""

    def setup_method(self):
        self.store, self.tmp = _make_store()

    def teardown_method(self):
        self.store.close()
        Path(self.tmp).unlink(missing_ok=True)

    def test_unresolved_code_span_is_pruned(self):
        self.store.upsert_edge(_ca_edge("new_bar"))
        self.store.commit()

        result1 = PostprocessResult()
        _resolve_markdown_artifact_refs(self.store, result1, [])
        assert result1.markdown_artifact_refs_resolved == 0
        assert result1.markdown_artifact_refs_still_unresolved == 0
        assert result1.markdown_artifact_refs_dropped == 1

        count = (
            store_conn(self.store)
            .execute("SELECT COUNT(*) FROM edges WHERE kind='CROSS_ARTIFACT'")
            .fetchone()[0]
        )
        assert count == 0


class TestAmbiguousToUnique:
    """Ambiguous implicit candidates are pruned instead of waiting for uniqueness."""

    def setup_method(self):
        self.store, self.tmp = _make_store()

    def teardown_method(self):
        self.store.close()
        Path(self.tmp).unlink(missing_ok=True)

    def test_ambiguous_code_span_is_pruned(self):
        self.store.upsert_node(_py_node("Quux", "/repo/a.py"))
        self.store.upsert_node(_py_node("Quux", "/repo/b.py"))
        self.store.upsert_edge(_ca_edge("Quux"))
        self.store.commit()

        result1 = PostprocessResult()
        _resolve_markdown_artifact_refs(self.store, result1, [])
        assert result1.markdown_artifact_refs_resolved == 0
        assert result1.markdown_artifact_refs_still_unresolved == 0
        assert result1.markdown_artifact_refs_dropped == 1

        count = (
            store_conn(self.store)
            .execute("SELECT COUNT(*) FROM edges WHERE kind='CROSS_ARTIFACT'")
            .fetchone()[0]
        )
        assert count == 0


class TestUniqueToAmbiguous:
    """Resolved code-span edge is pruned when a duplicate makes it ambiguous."""

    def setup_method(self):
        self.store, self.tmp = _make_store()

    def teardown_method(self):
        self.store.close()
        Path(self.tmp).unlink(missing_ok=True)

    def test_resolved_demoted_when_duplicate_added(self):
        self.store.upsert_node(_py_node("Zap", "/repo/a.py"))
        self.store.upsert_edge(_ca_edge("Zap"))
        self.store.commit()

        result1 = PostprocessResult()
        _resolve_markdown_artifact_refs(self.store, result1, [])
        assert result1.markdown_artifact_refs_resolved == 1

        # Add a duplicate in another file
        self.store.upsert_node(_py_node("Zap", "/repo/b.py"))
        self.store.commit()

        result2 = PostprocessResult()
        _resolve_markdown_artifact_refs(self.store, result2, [])
        assert result2.markdown_artifact_refs_dropped == 1

        count = (
            store_conn(self.store)
            .execute("SELECT COUNT(*) FROM edges WHERE kind='CROSS_ARTIFACT'")
            .fetchone()[0]
        )
        assert count == 0


class TestIdempotence:
    """Running the resolver twice in a row produces no changes on the second run."""

    def setup_method(self):
        self.store, self.tmp = _make_store()

    def teardown_method(self):
        self.store.close()
        Path(self.tmp).unlink(missing_ok=True)

    def test_double_run_no_changes(self):
        self.store.upsert_node(_py_node("steady_fn", "/repo/lib.py"))
        self.store.upsert_edge(_ca_edge("steady_fn"))
        self.store.commit()

        result1 = PostprocessResult()
        _resolve_markdown_artifact_refs(self.store, result1, [])
        assert result1.markdown_artifact_refs_resolved == 1

        result2 = PostprocessResult()
        _resolve_markdown_artifact_refs(self.store, result2, [])
        assert result2.markdown_artifact_refs_resolved == 0
        assert result2.markdown_artifact_refs_dropped == 0
        assert result2.markdown_artifact_refs_re_resolved == 0

    def test_double_run_unresolved_code_span_stays_pruned(self):
        self.store.upsert_edge(_ca_edge("missing_sym"))
        self.store.commit()

        result1 = PostprocessResult()
        _resolve_markdown_artifact_refs(self.store, result1, [])
        assert result1.markdown_artifact_refs_still_unresolved == 0
        assert result1.markdown_artifact_refs_dropped == 1

        result2 = PostprocessResult()
        _resolve_markdown_artifact_refs(self.store, result2, [])
        assert result2.markdown_artifact_refs_still_unresolved == 0
        assert result2.markdown_artifact_refs_resolved == 0

    def test_explicit_directive_unresolved_no_changes(self):
        self.store.upsert_edge(_directive_edge("missing_sym"))
        self.store.commit()

        result1 = PostprocessResult()
        _resolve_markdown_artifact_refs(self.store, result1, [])
        assert result1.markdown_artifact_refs_still_unresolved == 1
        assert result1.markdown_artifact_refs_dropped == 0

        result2 = PostprocessResult()
        _resolve_markdown_artifact_refs(self.store, result2, [])
        assert result2.markdown_artifact_refs_still_unresolved == 1
        assert result2.markdown_artifact_refs_resolved == 0
