"""Phase 4 analysis integration tests for CROSS_ARTIFACT bridges."""

from __future__ import annotations

import subprocess
from pathlib import Path

import pytest

from dagayn.contracts.cross_artifact import (
    is_low_confidence_bridge,
    is_reportable_bridge,
)
from dagayn.graph import GraphStore
from dagayn.parser._base.types import EdgeInfo, NodeInfo
from tests.store_sql import store_conn


def _add_func(store: GraphStore, name: str, path: str, *, line: int = 1) -> str:
    store.upsert_node(
        NodeInfo(
            kind="Function",
            name=name,
            file_path=path,
            line_start=line,
            line_end=line + 5,
            language="python",
        )
    )
    return f"{path}::{name}"


def _bridge(
    *,
    source: str,
    target: str,
    file_path: str,
    role: str = "invokes_binary",
    bridge_kind: str = "subprocess",
    tier: str = "HIGH",
    confidence: float = 0.8,
) -> EdgeInfo:
    return EdgeInfo(
        kind="CROSS_ARTIFACT",
        source=source,
        target=target,
        file_path=file_path,
        line=2,
        extra={
            "relationship_role": role,
            "bridge_kind": bridge_kind,
            "evidence_kind": "syntax",
            "confidence_tier": tier,
            "confidence": confidence,
        },
    )


class _EdgeView:
    """GraphEdge-like adapter over EdgeInfo for helper unit tests."""

    def __init__(self, info: EdgeInfo):
        self.kind = info.kind
        self.source_qualified = info.source
        self.target_qualified = info.target
        self.file_path = info.file_path
        self.line = info.line
        self.extra = info.extra
        self.confidence = float(info.extra.get("confidence", 1.0))
        self.confidence_tier = str(info.extra.get("confidence_tier", "EXTRACTED")).upper()


@pytest.fixture
def bridge_store(tmp_path: Path):
    # The repository's own graph, so review_tool can read it too.
    (tmp_path / ".dagayn").mkdir()
    db = tmp_path / ".dagayn" / "graph.db"
    store = GraphStore(str(db))
    wrapper = str(tmp_path / "wrapper.py")
    native = str(tmp_path / "native_entry.py")
    doc = str(tmp_path / "docs" / "contract.md")

    wrapper_qn = _add_func(store, "launch_native", wrapper)
    native_qn = _add_func(store, "native_main", native)
    store.upsert_node(
        NodeInfo(
            kind="DocSection",
            name="native-contract",
            file_path=doc,
            line_start=1,
            line_end=4,
            language="markdown",
        )
    )
    doc_qn = f"{doc}::native-contract"

    store.upsert_edge(
        _bridge(
            source=wrapper_qn,
            target=native_qn,
            file_path=wrapper,
            role="invokes_binary",
            bridge_kind="subprocess",
            tier="HIGH",
        )
    )
    store.upsert_edge(
        _bridge(
            source=doc_qn,
            target=wrapper_qn,
            file_path=doc,
            role="implemented_by",
            bridge_kind="documentation",
            tier="HIGH",
        )
    )
    store.upsert_edge(
        _bridge(
            source=wrapper_qn,
            target="<unresolved:maybe_cli>",
            file_path=wrapper,
            role="invokes_binary",
            bridge_kind="subprocess",
            tier="LOW",
            confidence=0.2,
        )
    )
    store.commit()
    yield (
        store,
        {
            "root": tmp_path,
            "wrapper": wrapper,
            "native": native,
            "doc": doc,
            "wrapper_qn": wrapper_qn,
            "native_qn": native_qn,
            "doc_qn": doc_qn,
        },
    )
    store.close()


class TestCrossArtifactImpact:
    def test_impact_includes_other_side_of_reportable_bridge(self, bridge_store):
        store, paths = bridge_store
        result = store.get_impact_radius([paths["wrapper"]], max_depth=2)

        impacted_qns = {n.qualified_name for n in result["impacted_nodes"]}
        assert paths["native_qn"] in impacted_qns
        assert paths["doc_qn"] in impacted_qns
        assert result["bridge_transitions"]
        assert any(
            item["source"] == paths["wrapper_qn"] and item["target"] == paths["native_qn"]
            for item in result["bridge_transitions"]
        )
        assert result["low_confidence_bridges"]
        assert all(
            item["reason_code"] == "low_confidence_cross_artifact_bridge"
            for item in result["low_confidence_bridges"]
        )

    def test_medium_non_code_span_bridge_is_a_caveat_not_a_claim(self, tmp_path):
        """MEDIUM bridges without code-span evidence must not vanish from both lists."""
        store = GraphStore(str(tmp_path / "medium.db"))
        try:
            wrapper = str(tmp_path / "wrapper.py")
            cli = str(tmp_path / "cli.py")
            wrapper_qn = _add_func(store, "launch", wrapper)
            cli_qn = _add_func(store, "cli_main", cli)
            store.upsert_edge(
                _bridge(
                    source=wrapper_qn,
                    target=cli_qn,
                    file_path=wrapper,
                    tier="MEDIUM",
                    confidence=0.4,
                )
            )
            store.commit()

            result = store.get_impact_radius([wrapper], max_depth=2)
            assert cli_qn not in {n.qualified_name for n in result["impacted_nodes"]}
            assert not result["bridge_transitions"]
            assert [
                (item["bridge"]["target"], item["bridge"]["confidence_tier"])
                for item in result["low_confidence_bridges"]
            ] == [(cli_qn, "MEDIUM")]
        finally:
            store.close()


class TestCrossArtifactImpactNetworkX:
    def test_networkx_impact_skips_low_confidence_bridges(self, bridge_store):
        store, paths = bridge_store
        # Only reachable via the LOW bridge — must not expand.
        orphan = _add_func(store, "orphan_cli", str(paths["root"] / "orphan.py"))
        store_conn(store).execute(
            """
            UPDATE edges
            SET target_qualified = ?, confidence_tier = 'LOW'
            WHERE kind = 'CROSS_ARTIFACT'
              AND target_qualified LIKE '<unresolved:%'
            """,
            (orphan,),
        )
        store.commit()
        store._nxg_cache = None

        result = store.get_impact_radius([paths["wrapper"]], max_depth=2)
        impacted_qns = {n.qualified_name for n in result["impacted_nodes"]}
        assert paths["native_qn"] in impacted_qns  # reportable HIGH bridge still expands
        assert orphan not in impacted_qns  # LOW bridge must not expand
        assert result["low_confidence_bridges"]


def _git_init(root: Path) -> None:
    """A git repository with one (empty) commit, so ``HEAD`` resolves."""
    git = ["git", "-c", "user.name=t", "-c", "user.email=t@example.invalid"]
    subprocess.run([*git, "init", "-q"], cwd=root, check=True)
    subprocess.run([*git, "commit", "-q", "--allow-empty", "-m", "i"], cwd=root, check=True)


class TestCrossArtifactHelpers:
    def test_unresolved_high_target_is_low_confidence(self):
        edge = _EdgeView(
            _bridge(
                source="infra/main.tf::resource.aws_lambda_function.auth",
                target="<unresolved:serve>",
                file_path="infra/main.tf",
                tier="HIGH",
            )
        )
        assert not is_reportable_bridge(edge)
        assert is_low_confidence_bridge(edge)

    def test_reportable_vs_low_confidence(self):
        high = _EdgeView(
            _bridge(
                source="a::x",
                target="b::y",
                file_path="a.py",
                tier="HIGH",
            )
        )
        low = _EdgeView(
            _bridge(
                source="a::x",
                target="<unresolved:z>",
                file_path="a.py",
                tier="LOW",
                confidence=0.2,
            )
        )
        assert is_reportable_bridge(high)
        assert not is_low_confidence_bridge(high)
        assert not is_reportable_bridge(low)
        assert is_low_confidence_bridge(low)

    def test_resolved_implicit_markdown_code_span_is_low_confidence(self):
        edge = _EdgeView(
            _bridge(
                source="docs/api.md::Api",
                target="app.py::handler",
                file_path="docs/api.md",
                tier="MEDIUM",
                confidence=0.4,
            )
        )
        edge.extra = {
            **edge.extra,
            "relationship_role": "describes_symbol",
            "evidence_kind": "markdown_code_span",
            "evidence_source": "code_span",
        }
        assert not is_reportable_bridge(edge)
        assert is_low_confidence_bridge(edge)

    @pytest.mark.parametrize("tier", ["EXACT", "EXTRACTED", "HIGH", "MEDIUM", "LOW", "UNKNOWN", ""])
    @pytest.mark.parametrize("target", ["b::y", "<unresolved:y>"])
    def test_claim_and_caveat_partition_is_total(self, tier, target):
        edge = _EdgeView(_bridge(source="a::x", target=target, file_path="a.py", tier=tier))
        edge.confidence_tier = tier
        assert is_reportable_bridge(edge) != is_low_confidence_bridge(edge)

    def test_column_tier_preferred_over_extra(self):
        """Parity with Rust: non-reportable column tier must not be overridden by extra."""
        edge = _EdgeView(
            _bridge(
                source="a::x",
                target="b::y",
                file_path="a.py",
                tier="HIGH",
            )
        )
        edge.confidence_tier = "LOW"
        edge.extra = {**edge.extra, "confidence_tier": "HIGH"}
        assert not is_reportable_bridge(edge)
        assert is_low_confidence_bridge(edge)
