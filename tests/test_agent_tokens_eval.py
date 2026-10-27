"""Checks for eval/run_agent_tokens_eval.py that need no `claude` CLI."""

from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
from pathlib import Path

_SCRIPT = Path(__file__).resolve().parents[1] / "eval" / "run_agent_tokens_eval.py"


def _load_harness():
    spec = importlib.util.spec_from_file_location("run_agent_tokens_eval", _SCRIPT)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


harness = _load_harness()


def _git(repo: Path, *args: str) -> str:
    return subprocess.run(
        ["git", "-c", "user.name=t", "-c", "user.email=t@example.invalid", *args],
        cwd=repo,
        capture_output=True,
        text=True,
        check=True,
    ).stdout


def _source(tmp_path: Path) -> tuple[Path, str]:
    repo = tmp_path / "upstream"
    repo.mkdir()
    (repo / "app.py").write_text("def greet():\n    return 'hi'\n", encoding="utf-8")
    _git(repo, "init", "-q")
    _git(repo, "add", "-A")
    _git(repo, "commit", "-q", "-m", "one")
    return repo, _git(repo, "rev-parse", "HEAD").strip()


def test_a_change_patch_becomes_the_last_commit(tmp_path: Path) -> None:
    repo, commit = _source(tmp_path)
    patch = tmp_path / "change.patch"
    patch.write_text(
        "--- a/app.py\n+++ b/app.py\n@@ -1,2 +1,2 @@\n"
        " def greet():\n-    return 'hi'\n+    return 'hello'\n",
        encoding="utf-8",
    )

    src = harness.make_target(tmp_path / "work", commit, repo, patch, "Say hello")

    assert _git(src, "log", "--format=%s").splitlines() == ["Say hello", f"archive {commit[:12]}"]
    assert _git(src, "diff", "--name-only", "HEAD~1", "HEAD").split() == ["app.py"]
    assert "hello" in (src / "app.py").read_text(encoding="utf-8")


def test_without_a_change_the_target_is_one_commit(tmp_path: Path) -> None:
    repo, commit = _source(tmp_path)

    src = harness.make_target(tmp_path / "work", commit, repo)

    assert len(_git(src, "log", "--format=%s").splitlines()) == 1


def test_the_mcp_server_gets_the_serve_arguments(tmp_path: Path) -> None:
    cfg = harness.mcp_config(tmp_path, "candidate", "dagayn", "--local-embedding")

    server = json.loads(cfg.read_text())["mcpServers"]["dagayn"]
    assert server["args"] == ["serve", "--local-embedding"]


def test_a_symbol_answer_is_read_in_every_spelling_of_the_same_symbol() -> None:
    answer = [
        "`tests/test_a.py::TestX::test_y[case-1]`",
        "./pkg/mod.py::run()",
        "pkg/mod.py::Cls.method",
    ]

    assert harness.answer_set("symbols", answer) == {
        "tests/test_a.py::TestX.test_y",
        "pkg/mod.py::run",
        "pkg/mod.py::Cls.method",
    }


def test_a_path_answer_keeps_its_spelling() -> None:
    assert harness.answer_set("paths", ["a/b.py::C::d"]) == {"a/b.py::C::d"}
