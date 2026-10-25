"""``architecture_analysis_func`` answers every mode through the Rust tool
once ``_get_store`` has resolved and opened the graph."""

from __future__ import annotations

import inspect
import subprocess
import sys
from pathlib import Path
from typing import cast

import pytest

from dagayn.contracts.state_types import ArchitectureAnalysisMode
from dagayn.server import main as crg_main
from dagayn.tools import architecture_analysis

DAGAYN = Path(sys.executable).with_name("dagayn")

SUBTOOLS: dict[str, str] = {
    "overview": "get_architecture_overview_func",
    "communities": "list_communities_func",
    "community": "get_community_func",
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
        "scope_kind",
        "artifact_scope",
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
            # verbose keeps the subtool's name, which the agent's envelope drops.
            detail_level="verbose",
        )

        assert result["mode"] == mode
        assert result["called_subtool"] == subtool
        expected = "not_found" if mode == "community" else "ok"
        assert result["status"] == expected, (mode, result.get("summary"))


def test_architecture_analysis_community_requires_selector(repo: Path) -> None:
    result = architecture_analysis.architecture_analysis_func(mode="community", repo_root=str(repo))

    assert result["status"] == "error"
    assert result["mode"] == "community"
    assert "called_subtool" not in result
    assert "community_id or community_name" in result["summary"]
