"""``architecture_analysis_func`` answers every mode through the Rust tool
once ``_get_store`` has resolved and opened the graph."""

from __future__ import annotations

import inspect
import subprocess
import sys
from pathlib import Path
from typing import Literal, cast

import pytest

from dagayn.contracts.state_types import ArchitectureAnalysisMode
from dagayn.server import main as crg_main
from dagayn.tools import architecture_analysis

DAGAYN = Path(sys.executable).with_name("dagayn")

SUBTOOLS: dict[str, str] = {
    "overview": "get_architecture_overview_func",
    "import_cycles": "find_import_cycles_func",
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

    assert list(params) == [
        "mode",
        "detail_level",
        "scope_kind",
        "unit_filter",
        "repo_root",
        "artifact_scope",
        "import_kind",
    ]
    assert params["mode"].default == "overview"
    assert params["detail_level"].default == "minimal"
    assert params["artifact_scope"].default == "code"


def test_architecture_analysis_routes_every_mode(repo: Path) -> None:
    for mode, subtool in SUBTOOLS.items():
        result = architecture_analysis.architecture_analysis_func(
            mode=cast(ArchitectureAnalysisMode, mode),
            repo_root=str(repo),
            artifact_scope="all",
            # verbose keeps the subtool's name, which the agent's envelope drops.
            detail_level="verbose",
        )

        assert result["mode"] == mode
        assert result["called_subtool"] == subtool
        assert result["status"] == "ok", (mode, result.get("summary"))


def _build_repo(root: Path, files: dict[str, str]) -> Path:
    (root / ".git").mkdir(parents=True)
    for name, text in files.items():
        (root / name).parent.mkdir(parents=True, exist_ok=True)
        (root / name).write_text(text)
    subprocess.run([DAGAYN, "build", "--repo", root], check=True, capture_output=True)
    return root


def _cycles(
    root: Path,
    scope_kind: Literal["file", "package", "directory"] = "package",
    artifact_scope: Literal["code", "docs", "all"] = "code",
) -> list[list[str]]:
    result = architecture_analysis.architecture_analysis_func(
        mode="import_cycles",
        repo_root=str(root),
        scope_kind=scope_kind,
        artifact_scope=artifact_scope,
    )
    assert result["status"] == "ok", result
    assert result["results_complete"] is True, result
    assert result["cycle_count"] == len(result["cycles"]), result
    return result["cycles"]


def test_import_cycles_are_empty_without_a_cycle(tmp_path: Path) -> None:
    root = _build_repo(
        tmp_path / "linear",
        {
            "a.py": "from b import g\n\n\ndef f():\n    return g()\n",
            "b.py": "from c import h\n\n\ndef g():\n    return h()\n",
            "c.py": "def h():\n    return 1\n",
        },
    )
    assert _cycles(root, scope_kind="file") == []


def test_import_cycles_are_empty_for_an_empty_repository(tmp_path: Path) -> None:
    assert _cycles(_build_repo(tmp_path / "empty", {})) == []


def test_import_cycles_name_a_two_file_cycle(tmp_path: Path) -> None:
    root = _build_repo(
        tmp_path / "two",
        {
            "a.py": "from b import g\n\n\ndef f():\n    return g()\n",
            "b.py": "from a import f\n\n\ndef g():\n    return f()\n",
        },
    )
    assert _cycles(root, scope_kind="file") == [["a.py", "b.py"]]


def test_mutual_imports_inside_one_directory_are_no_directory_cycle(tmp_path: Path) -> None:
    root = _build_repo(
        tmp_path / "samedir",
        {
            "pkg/a.py": "from pkg.b import g\n\n\ndef f():\n    return g()\n",
            "pkg/b.py": "from pkg.a import f\n\n\ndef g():\n    return f()\n",
        },
    )
    assert _cycles(root, scope_kind="directory") == []
    assert _cycles(root, scope_kind="file") == [["pkg/a.py", "pkg/b.py"]]


def test_import_cycles_between_documents_need_the_docs_scope(tmp_path: Path) -> None:
    root = _build_repo(
        tmp_path / "docs",
        {
            "docs/a.md": "# A\n\nSee [b](b.md).\n",
            "docs/b.md": "# B\n\nSee [a](a.md).\n",
            "src/x.py": "def f():\n    return 1\n",
        },
    )
    assert _cycles(root, scope_kind="file") == []
    assert _cycles(root, scope_kind="file", artifact_scope="docs") == [["docs/a.md", "docs/b.md"]]
