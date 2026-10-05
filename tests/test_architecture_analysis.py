"""``architecture_analysis_func`` answers every mode through the Rust tool
once ``_get_store`` has resolved and opened the graph."""

from __future__ import annotations

import inspect
import subprocess
import sys
from pathlib import Path
from typing import cast

import pytest

from dagayn.contracts.dependency_profiles import DependencyProfile
from dagayn.contracts.state_types import ArchitectureAnalysisMode
from dagayn.server import main as crg_main
from dagayn.tools import architecture_analysis

DAGAYN = Path(sys.executable).with_name("dagayn")

SUBTOOLS: dict[str, str] = {
    "overview": "get_architecture_overview_func",
    "communities": "list_communities_func",
    "community": "get_community_func",
    "hubs": "get_hub_nodes_func",
    "bridges": "get_bridge_nodes_func",
    "knowledge_gaps": "get_knowledge_gaps_func",
    "surprising_connections": "get_surprising_connections_func",
    "adp_violations": "detect_adp_violations_func",
    "sdp_metrics": "compute_sdp_metrics_func",
    "sdp_violations": "detect_sdp_violations_func",
    "sap_metrics": "compute_sap_metrics_func",
    "sap_violations": "detect_sap_violations_func",
}


@pytest.fixture(scope="module")
def repo(tmp_path_factory: pytest.TempPathFactory) -> Path:
    root = tmp_path_factory.mktemp("arch") / "repo"
    (root / ".git").mkdir(parents=True)
    (root / "pkg").mkdir()
    (root / "tests").mkdir()
    (root / "pkg" / "__init__.py").write_text("")
    (root / "pkg" / "core.py").write_text(
        "from app import main\n\n\nclass Base:\n    def run(self):\n        return main()\n"
    )
    (root / "app.py").write_text(
        "from pkg.core import Base\n\n\ndef main():\n    return helper()\n\n\n"
        "def helper():\n    return Base()\n"
    )
    (root / "tests" / "test_app.py").write_text(
        "from app import main\n\n\ndef test_main():\n    main()\n"
    )
    subprocess.run([DAGAYN, "build", "--repo", root], check=True, capture_output=True)
    return root


def test_architecture_analysis_wrapper_exposes_typed_dispatch_args() -> None:
    params = inspect.signature(crg_main.architecture_analysis_tool).parameters

    for name in (
        "mode",
        "detail_level",
        "top_n",
        "sort_by",
        "community_name",
        "community_id",
        "granularity",
        "scope_kind",
        "artifact_scope",
        "dependency_profile",
        "min_delta",
        "min_distance",
    ):
        assert name in params
    assert params["mode"].default == "overview"
    assert params["detail_level"].default == "minimal"
    assert params["top_n"].default == 10
    assert params["artifact_scope"].default == "code"


def test_architecture_analysis_routes_every_mode(repo: Path) -> None:
    for mode, subtool in SUBTOOLS.items():
        result = architecture_analysis.architecture_analysis_func(
            mode=cast(ArchitectureAnalysisMode, mode),
            repo_root=str(repo),
            community_name="zz-none" if mode == "community" else None,
            artifact_scope="all",
        )

        assert result["mode"] == mode
        assert result["called_subtool"] == subtool
        expected = "not_found" if mode == "community" else "ok"
        assert result["status"] == expected, (mode, result.get("summary"))


def test_architecture_analysis_sap_violations_preserves_exclusion_explanation(
    repo: Path,
) -> None:
    result = architecture_analysis.architecture_analysis_func(
        mode="sap_violations", repo_root=str(repo)
    )

    assert result["status"] == "ok"
    assert result["called_subtool"] == "detect_sap_violations_func"
    assert result["excluded_scope_categories"] == ["test-scope", "fixture-scope"]
    assert "suppresses test and fixture scopes" in result["summary"]


def test_architecture_analysis_thresholds_print_like_python(repo: Path) -> None:
    sdp = architecture_analysis.architecture_analysis_func(
        mode="sdp_violations", repo_root=str(repo), min_delta=1e-05
    )
    sap = architecture_analysis.architecture_analysis_func(
        mode="sap_violations", repo_root=str(repo), min_distance=1.5e16
    )

    assert f"min_delta={1e-05!r})" in sdp["summary"]
    assert f"min_distance={1.5e16!r})" in sap["summary"]


def test_architecture_analysis_rejects_unknown_dependency_profile(repo: Path) -> None:
    # fastmcp rejects the literal before the call; a direct call has no answer.
    with pytest.raises(RuntimeError):
        architecture_analysis.architecture_analysis_func(
            mode="sdp_metrics",
            repo_root=str(repo),
            dependency_profile=cast(DependencyProfile, "typo"),
        )


def test_architecture_analysis_code_scope_excludes_tests_from_structural_modes(
    repo: Path,
) -> None:
    for mode in ("hubs", "bridges", "knowledge_gaps", "surprising_connections"):
        code = architecture_analysis.architecture_analysis_func(
            mode=cast(ArchitectureAnalysisMode, mode),
            repo_root=str(repo),
            artifact_scope="code",
        )
        everything = architecture_analysis.architecture_analysis_func(
            mode=cast(ArchitectureAnalysisMode, mode),
            repo_root=str(repo),
            artifact_scope="all",
        )

        assert code["include_tests"] is False, mode
        assert everything["include_tests"] is True, mode


def test_architecture_analysis_community_requires_selector(repo: Path) -> None:
    result = architecture_analysis.architecture_analysis_func(mode="community", repo_root=str(repo))

    assert result["status"] == "error"
    assert result["mode"] == "community"
    assert result["called_subtool"] is None
    assert "community_id or community_name" in result["summary"]


@pytest.fixture(scope="module")
def layered_repo(tmp_path_factory: pytest.TempPathFactory) -> Path:
    """Three package cycles, and a stable ``core`` that depends on two less
    stable leaves."""
    root = tmp_path_factory.mktemp("layered") / "repo"
    (root / ".git").mkdir(parents=True)

    def module(package: str, name: str, body: str) -> None:
        (root / package).mkdir(exist_ok=True)
        (root / package / "__init__.py").write_text("")
        (root / package / f"{name}.py").write_text(body)

    for left, right in (("a", "b"), ("c", "d"), ("e", "f")):
        for source, target in ((left, right), (right, left)):
            module(
                source,
                "mod",
                f"from {target}.mod import {target}_fn\n\n\n"
                f"def {source}_fn():\n    return {target}_fn\n",
            )
    module("x", "util", "def raw():\n    return 1\n")
    for leaf in ("leaf", "leaf2"):
        module(
            leaf,
            "util",
            "from x.util import raw\n\n\nclass Tool:\n    pass\n\n\n"
            "def tool():\n    return raw()\n",
        )
    module(
        "core",
        "base",
        "from leaf.util import tool\nfrom leaf2.util import tool as tool2\n\n\n"
        "class Base:\n    pass\n\n\ndef base():\n    return tool(), tool2()\n",
    )
    for index in range(3):
        module(
            f"user{index}", "m", "from core.base import base\n\n\ndef go():\n    return base()\n"
        )
    subprocess.run([DAGAYN, "build", "--repo", root], check=True, capture_output=True)
    return root


def test_adp_violations_truncated_suggests_listing_every_cycle(layered_repo: Path) -> None:
    result = architecture_analysis.architecture_analysis_func(
        mode="adp_violations", repo_root=str(layered_repo), top_n=2
    )

    assert result["truncated"] is True
    assert len(result["violations"]) == 2
    total = result["count"]
    assert total >= 3
    assert result["next_tool_suggestions"][0] == (
        f'architecture_analysis_tool mode="adp_violations" top_n={total} -- list every cycle'
    )


def test_sdp_violations_truncate_to_top_n(layered_repo: Path) -> None:
    result = architecture_analysis.architecture_analysis_func(
        mode="sdp_violations", repo_root=str(layered_repo), top_n=1, min_delta=0.0
    )

    assert result["status"] == "ok"
    assert result["total"] >= 2
    assert result["truncated"] is True
    assert len(result["violations"]) == 1
    assert result["_hints"]["next_steps"][0]["tool"] == "architecture_analysis_tool"


def test_sap_metrics_separate_inapplicable_scopes_by_default(layered_repo: Path) -> None:
    result = architecture_analysis.architecture_analysis_func(
        mode="sap_metrics", repo_root=str(layered_repo), top_n=50
    )

    assert result["inapplicable_visibility"] == "separate_bucket"
    assert result["applicable_count"] >= 2
    assert result["inapplicable_count"] >= 1
    assert all(row["sap_applicable"] for row in result["metrics"])
    assert not any(row["sap_applicable"] for row in result["inapplicable_metrics"])
    assert sum(result["inapplicable_by_reason"].values()) == result["inapplicable_count"]
    assert result["truncated"] is False


def test_sap_metrics_report_truncation(layered_repo: Path) -> None:
    result = architecture_analysis.architecture_analysis_func(
        mode="sap_metrics", repo_root=str(layered_repo), top_n=1
    )

    assert result["truncated"] is True
    assert len(result["metrics"]) == 1
    assert "Results truncated." in result["summary"]


def test_sap_metrics_verbose_includes_inapplicable_scopes(layered_repo: Path) -> None:
    result = architecture_analysis.architecture_analysis_func(
        mode="sap_metrics", repo_root=str(layered_repo), top_n=50, detail_level="verbose"
    )

    assert result["inapplicable_visibility"] == "included_in_metrics"
    assert any(not row["sap_applicable"] for row in result["metrics"])


def test_sap_violations_use_a_compact_envelope(layered_repo: Path) -> None:
    result = architecture_analysis.architecture_analysis_func(
        mode="sap_violations", repo_root=str(layered_repo), min_distance=0.0, top_n=1
    )

    assert result["status"] == "ok"
    assert result["artifact_scope"] == "code"
    assert result["total"] >= 2
    assert result["truncated"] is True
    assert "Showing top 1 by distance." in result["summary"]
    assert result["exclusion_reason"] == (
        "test and fixture scopes are retained in sap_metrics notes but omitted from sap_violations"
    )
    [violation] = result["violations"]
    assert set(violation) == {"scope_key", "display_name", "distance", "zone"}
