"""Phase 4 analysis integration tests for CROSS_ARTIFACT bridges."""

from __future__ import annotations

from pathlib import Path

import pytest

from dagayn.cross_artifact import (
    annotate_flow_steps_with_bridges,
    is_low_confidence_bridge,
    is_reportable_bridge,
)
from dagayn.flows import _hydrate_flow_rows, store_flows, trace_flows
from dagayn.graph import GraphStore
from dagayn.parser.types import EdgeInfo, NodeInfo
from dagayn.tools import query as query_module
from dagayn.tools.review_helpers import (
    _change_analysis_summary,
    _cross_artifact_proximity,
    _review_guidance_items,
)


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
    db = tmp_path / "graph.db"
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

        rows = store._conn.execute("SELECT * FROM flows").fetchall()
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


class TestCrossArtifactReviewGuidance:
    def test_review_guidance_recommends_docs_for_and_bridge_followups(self, bridge_store):
        store, paths = bridge_store
        impact = store.get_impact_radius([paths["wrapper"]], max_depth=2)
        changed_functions = [
            {
                "qualified_name": paths["wrapper_qn"],
                "kind": "Function",
                "file_path": paths["wrapper"],
                "name": "launch_native",
            }
        ]
        proximity = _cross_artifact_proximity(store, impact, changed_functions)
        assert proximity["counts"]["reportable"] >= 1
        assert proximity["counts"]["low_confidence"] >= 1
        assert any("docs_for" in item for item in proximity["follow_ups"])
        assert any("implementations_of" in item for item in proximity["follow_ups"])

        summary = _change_analysis_summary(
            store,
            {
                "risk_score": 0.4,
                "affected_flows": [],
                "test_gaps": [],
                "changed_functions": changed_functions,
            },
            impact,
            [paths["wrapper"]],
        )
        assert "cross_artifact_proximity" in summary["reason_codes"]
        assert "low_confidence_cross_artifact_bridge" in summary["reason_codes"]
        assert summary["cross_artifact_proximity"]["reportable_bridges"]
        guidance_actions = [str(item.get("action")) for item in summary["guidance"]]
        assert any("docs_for" in action for action in guidance_actions)
        assert any("implementations_of" in action for action in guidance_actions)

        items = _review_guidance_items(
            risk="medium",
            risk_score=0.4,
            reason_codes=["low_confidence_cross_artifact_bridge"],
            recommended_tests=[],
            docs=[],
            test_gap_ranking={"counts": {}},
            stability_contracts=[],
            affected_flow_rankings=[],
            hotspots={
                "changed_hubs": [],
                "changed_bridges": [],
                "impacted_hubs": [],
                "impacted_bridges": [],
            },
            architecture_delta={"counts": {}},
            signal_quality={"graph_facts": []},
            cross_artifact_proximity={
                "reportable_bridges": [],
                "low_confidence_bridges": proximity["low_confidence_bridges"],
                "follow_ups": proximity["follow_ups"],
                "counts": proximity["counts"],
            },
        )
        assert any(
            item["reason_codes"] == ["low_confidence_cross_artifact_bridge"]
            and item["confidence"] == "low"
            for item in items
        )


class TestCrossArtifactHelpers:
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
