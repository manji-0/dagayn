"""Tools that open the graph release it before they return: a store left
open keeps its lease and the shared read lock, so the next writer waits."""

from __future__ import annotations

import subprocess
import sys
from functools import partial
from pathlib import Path

import pytest

from dagayn.tools.architecture_analysis import architecture_analysis_func
from dagayn.tools.docs import get_wiki_page_func
from dagayn.write_lock import graph_lock_is_held

DAGAYN = Path(sys.executable).with_name("dagayn")


@pytest.fixture
def repo(tmp_path: Path) -> Path:
    root = tmp_path / "repo"
    (root / ".git").mkdir(parents=True)
    (root / "pkg").mkdir()
    (root / "pkg" / "a.py").write_text("from pkg import b\n\n\ndef f():\n    return b.g()\n")
    (root / "pkg" / "b.py").write_text("def g():\n    return 1\n")
    subprocess.run([DAGAYN, "build", "--repo", root], check=True, capture_output=True)
    return root


@pytest.mark.parametrize(
    "tool",
    [
        *(
            partial(architecture_analysis_func, mode=mode)
            for mode in (
                "overview",
                "adp_violations",
                "sdp_metrics",
                "sdp_violations",
                "sap_metrics",
                "sap_violations",
            )
        ),
        lambda repo_root: get_wiki_page_func("pkg", repo_root=repo_root),
    ],
)
def test_the_graph_is_released_after_the_call(repo: Path, tool) -> None:
    db_path = repo / ".dagayn" / "graph.db"
    tool(repo_root=str(repo))
    assert not graph_lock_is_held(db_path)
