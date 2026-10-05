"""Phase 4 analysis integration tests for CROSS_ARTIFACT bridges."""

from __future__ import annotations

import subprocess
from pathlib import Path

import pytest

from dagayn.contracts.cross_artifact import (
    annotate_flow_steps_with_bridges,
    is_low_confidence_bridge,
    is_reportable_bridge,
)
from dagayn.flows import _hydrate_flow_rows, get_affected_flows, store_flows, trace_flows
from dagayn.graph import GraphStore
from dagayn.parser._base.types import EdgeInfo, NodeInfo
from dagayn.tools import query as query_module
from dagayn.tools.review_dispatcher import review_func
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

    def test_impact_tool_surfaces_explainable_bridge_path(self, bridge_store, monkeypatch):
        store, paths = bridge_store
        monkeypatch.setattr(
            query_module,
            "_get_store",
            lambda repo_root: (store, paths["root"]),
        )
        store.close = lambda: None

        result = query_module.get_impact_radius(
            changed_files=[paths["wrapper"]],
            repo_root=str(paths["root"]),
            max_depth=2,
        )
        assert result["status"] == "ok"
        assert result["bridge_transitions"]
        assert any(
            item.get("reason_codes") == ["cross_artifact_bridge_impact"]
            for item in result.get("guidance", [])
        )
        assert any(
            item.get("reason_code") == "low_confidence_cross_artifact_bridge"
            for item in result.get("missingness", [])
        )


class TestCrossArtifactFlows:
    def test_flow_trace_crosses_reportable_bridge_and_marks_steps(self, bridge_store):
        store, paths = bridge_store
        store.upsert_node(
            NodeInfo(
                kind="Function",
                name="main",
                file_path=paths["wrapper"],
                line_start=20,
                line_end=30,
                language="python",
            )
        )
        main_qn = f"{paths['wrapper']}::main"
        store.upsert_edge(
            EdgeInfo(
                kind="CALLS",
                source=main_qn,
                target=paths["wrapper_qn"],
                file_path=paths["wrapper"],
                line=21,
            )
        )
        store.commit()

        flows = trace_flows(store)
        assert flows
        count = store_flows(store, flows)
        assert count >= 1

        rows = store_conn(store).execute("SELECT * FROM flows").fetchall()
        hydrated = _hydrate_flow_rows(store, rows)
        bridge_flows = [
            flow
            for flow in hydrated
            if any(step.get("qualified_name") == paths["native_qn"] for step in flow["steps"])
        ]
        assert bridge_flows, "expected a flow that reaches the bridge target"
        bridge_steps = [
            step for flow in bridge_flows for step in flow["steps"] if step.get("is_bridge_step")
        ]
        assert bridge_steps
        assert all(step.get("step_kind") == "bridge" for step in bridge_steps)
        assert all(
            step.get("transition", {}).get("kind") == "CROSS_ARTIFACT" for step in bridge_steps
        )

    def test_get_affected_flows_annotates_bridge_steps(self, bridge_store):
        """Rust get_affected_flows_json path must hydrate bridge annotations."""
        store, paths = bridge_store
        store.upsert_node(
            NodeInfo(
                kind="Function",
                name="main",
                file_path=paths["wrapper"],
                line_start=20,
                line_end=30,
                language="python",
            )
        )
        main_qn = f"{paths['wrapper']}::main"
        store.upsert_edge(
            EdgeInfo(
                kind="CALLS",
                source=main_qn,
                target=paths["wrapper_qn"],
                file_path=paths["wrapper"],
                line=21,
            )
        )
        store.commit()
        flows = trace_flows(store)
        assert store_flows(store, flows) >= 1

        # Simulate native store: JSON without bridge annotations.
        import json

        rows = store_conn(store).execute("SELECT * FROM flows").fetchall()
        bare = _hydrate_flow_rows(store, rows)
        for flow in bare:
            for step in flow["steps"]:
                step.pop("step_kind", None)
                step.pop("transition", None)
                step.pop("is_bridge_step", None)
            flow.pop("bridge_step_count", None)

        store.get_affected_flows_json = lambda _files: json.dumps(bare)

        result = get_affected_flows(store, [paths["wrapper"]])
        bridge_flows = [
            flow
            for flow in result["affected_flows"]
            if any(step.get("qualified_name") == paths["native_qn"] for step in flow["steps"])
        ]
        assert bridge_flows, "expected affected flow reaching bridge target"
        for flow in bridge_flows:
            assert flow.get("bridge_step_count", 0) >= 1
            bridge_steps = [step for step in flow["steps"] if step.get("is_bridge_step")]
            assert bridge_steps
            assert all(step.get("step_kind") == "bridge" for step in bridge_steps)
            assert all(
                step.get("transition", {}).get("kind") == "CROSS_ARTIFACT" for step in bridge_steps
            )

    def test_annotate_flow_steps_marks_bridge_arrival(self):
        steps = [
            {"qualified_name": "a.py::main", "name": "main"},
            {"qualified_name": "a.py::launch", "name": "launch"},
            {"qualified_name": "b.py::native", "name": "native"},
        ]
        annotated = annotate_flow_steps_with_bridges(
            steps,
            [
                _EdgeView(
                    _bridge(
                        source="a.py::launch",
                        target="b.py::native",
                        file_path="a.py",
                        tier="HIGH",
                    )
                )
            ],
        )
        assert annotated[0]["step_kind"] == "entry"
        assert annotated[2]["step_kind"] == "bridge"
        assert annotated[2]["is_bridge_step"] is True
        assert annotated[2]["transition"]["bridge_kind"] == "subprocess"


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


class TestCrossArtifactReviewGuidance:
    def test_review_guidance_recommends_docs_for_and_bridge_followups(self, bridge_store):
        store, paths = bridge_store
        root = paths["root"]
        _git_init(root)

        result = review_func(
            mode="changes", base="HEAD", changed_files=["wrapper.py"], repo_root=str(root)
        )

        assert result["status"] == "ok", result
        summary = result["analysis_summary"]
        proximity = summary["cross_artifact_proximity"]
        assert proximity["counts"]["reportable"] >= 1
        assert proximity["counts"]["low_confidence"] >= 1
        assert proximity["reportable_bridges"]
        assert any("docs_for" in item for item in proximity["follow_ups"])
        assert any("implementations_of" in item for item in proximity["follow_ups"])
        assert "cross_artifact_proximity" in summary["reason_codes"]
        assert "low_confidence_cross_artifact_bridge" in summary["reason_codes"]
        guidance_actions = [str(item.get("action")) for item in summary["guidance"]]
        assert any("docs_for" in action for action in guidance_actions)
        assert any("implementations_of" in action for action in guidance_actions)

    def test_review_guidance_flags_a_lone_low_confidence_bridge(self, tmp_path):
        """With no reportable bridge, the caveat is the guidance item."""
        (tmp_path / ".dagayn").mkdir()
        store = GraphStore(str(tmp_path / ".dagayn" / "graph.db"))
        try:
            wrapper = str(tmp_path / "wrapper.py")
            wrapper_qn = _add_func(store, "launch", wrapper)
            cli_qn = _add_func(store, "cli_main", str(tmp_path / "cli.py"))
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
            _git_init(tmp_path)

            result = review_func(
                mode="changes", base="HEAD", changed_files=["wrapper.py"], repo_root=str(tmp_path)
            )
        finally:
            store.close()

        summary = result["analysis_summary"]
        assert summary["cross_artifact_proximity"]["counts"] == {
            "reportable": 0,
            "low_confidence": 1,
        }
        assert any(
            item["reason_codes"] == ["low_confidence_cross_artifact_bridge"]
            and item["confidence"] == "low"
            for item in summary["guidance"]
        ), summary["guidance"]


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
