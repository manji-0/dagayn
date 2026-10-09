"""The SDP/SAP commands answer with ``architecture_analysis_tool``, so they
scope by the same declared units."""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path
from typing import Any

import pytest

from dagayn.tools._native import native_tool

DAGAYN = Path(sys.executable).with_name("dagayn")

AGENT_ONLY_FIELDS = {"_hints", "next_tool_suggestions", "_runtime", "_repo"}


@pytest.fixture(scope="module")
def repo(tmp_path_factory: pytest.TempPathFactory) -> Path:
    # `pkg` and `pkg/sub` import each other, a cycle between directories but
    # not between modules, inside the one unit pyproject.toml declares.
    # `tools/` holds two file cycles.
    root = tmp_path_factory.mktemp("cli-arch") / "repo"
    (root / ".git").mkdir(parents=True)
    (root / "pkg" / "sub").mkdir(parents=True)
    (root / "tools").mkdir()
    (root / "pyproject.toml").write_text('[project]\nname = "pkg"\nversion = "0.1.0"\n')
    (root / "pkg" / "__init__.py").write_text(
        "from pkg.sub.leaf import leaf\n\n\ndef top():\n    return leaf()\n"
    )
    (root / "pkg" / "base.py").write_text("def base():\n    return 1\n")
    (root / "pkg" / "sub" / "__init__.py").write_text("")
    (root / "pkg" / "sub" / "leaf.py").write_text(
        "from pkg.base import base\n\n\ndef leaf():\n    return base()\n"
    )
    for left, right in (("a", "b"), ("c", "d")):
        for one, other in ((left, right), (right, left)):
            (root / "tools" / f"{one}.py").write_text(
                f"from tools.{other} import {other}_fn\n\n\n"
                f"def {one}_fn():\n    return {other}_fn()\n"
            )
    subprocess.run([DAGAYN, "build", "--repo", root], check=True, capture_output=True)
    return root


def _cli(repo: Path, *args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [DAGAYN, *args, "--repo", str(repo)], check=True, capture_output=True, text=True
    )


def _mcp(repo: Path, mode: str, **arguments: Any) -> dict[str, Any]:
    payload = native_tool("architecture_analysis_tool", mode=mode, repo_root=str(repo), **arguments)
    return {key: value for key, value in payload.items() if key not in AGENT_ONLY_FIELDS}


@pytest.mark.parametrize(
    ("command", "mode", "arguments"),
    [
        ("sdp-metrics", "sdp_metrics", {"top_n": 30}),
        ("detect-sdp", "sdp_violations", {"top_n": 2**31 - 1}),
        ("sap-metrics", "sap_metrics", {"top_n": 30}),
        ("detect-sap", "sap_violations", {"top_n": 2**31 - 1}),
    ],
)
def test_cli_json_is_the_mcp_answer(
    repo: Path, command: str, mode: str, arguments: dict[str, Any]
) -> None:
    out = json.loads(_cli(repo, command).stdout)

    assert out == _mcp(repo, mode, **arguments)
    assert not AGENT_ONLY_FIELDS & out.keys()


def test_detect_adp_is_gone(repo: Path) -> None:
    # Import cycles are the overview's `import_cycle` findings.
    result = subprocess.run(
        [DAGAYN, "detect-adp", "--repo", str(repo)], capture_output=True, text=True
    )

    assert result.returncode != 0


def test_sap_scope_kind_directory_keeps_directory_scopes(repo: Path) -> None:
    out = json.loads(_cli(repo, "sap-metrics", "--scope-kind", "directory").stdout)
    scopes = {
        metric["scope_key"] for metric in out["metrics"] + out.get("inapplicable_metrics", [])
    }

    assert out["scope_kind"] == "directory"
    assert {"pkg", "pkg/sub"} <= scopes
