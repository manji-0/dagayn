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


def test_a_cli_setup_runs_its_dagayn_command_from_path(tmp_path: Path) -> None:
    bin_dir = harness.cli_bin(tmp_path, "candidate", "/bin/echo dagayn-was-called")

    out = subprocess.run(
        ["dagayn", "tool", "query_graph_tool"],
        env={"PATH": f"{bin_dir}:/usr/bin:/bin"},
        capture_output=True,
        text=True,
        check=True,
    ).stdout
    assert out == "dagayn-was-called tool query_graph_tool\n"


def test_a_shell_call_of_dagayn_tool_counts_as_a_dagayn_call(tmp_path: Path) -> None:
    def tool_use(i: str, name: str, command: str = "") -> dict:
        return {"type": "tool_use", "id": i, "name": name, "input": {"command": command}}

    def tool_result(i: str, text: str) -> dict:
        return {"type": "tool_result", "tool_use_id": i, "content": text}

    events = [
        {"type": "assistant", "message": {"content": [
            tool_use("a", "Bash", "dagayn tool query_graph_tool --arg pattern=callers_of"),
            tool_use("b", "Bash", "grep -rn IsBool ."),
        ]}},
        {"type": "user", "message": {"content": [
            tool_result("a", '{"status":"ok"}'), tool_result("b", "model.go:1"),
        ]}},
        {"type": "result", "result": '```json\n{"answer": ["x"]}\n```', "modelUsage": {}},
    ]  # fmt: skip
    (tmp_path / "001-T-candidate.jsonl").write_text(
        "\n".join(json.dumps(e) for e in events), encoding="utf-8"
    )
    spec = {"tasks": {"T": {"kind": "set", "truth": ["x"]}}}
    meta = {"id": "001-T-candidate", "setup": "candidate", "task": "T"}

    row = harness.score(meta, tmp_path, spec)

    assert row["dagayn_calls"] == 1
    assert row["dagayn_tools"] == "query_graph_tool"
    assert row["dagayn_reply_max"] == len('{"status":"ok"}')
    assert row["tool_calls"] == 2


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


def test_a_go_method_is_read_in_every_receiver_spelling() -> None:
    answer = [
        "zstd/x.go::(*Digest).Reset",
        "zstd/x.go::*Digest.Reset()",
        "zstd/x.go::(Digest).Sum",
        "zstd/x.go::List[T].Push",
        "zstd/x_test.go::TestReset/zero length",
    ]

    assert harness.answer_set("symbols", answer) == {
        "zstd/x.go::Digest.Reset",
        "zstd/x.go::Digest.Sum",
        "zstd/x.go::List.Push",
        "zstd/x_test.go::TestReset",
    }


def test_a_python_symbol_keeps_parentheses_and_slashes_inside_names() -> None:
    assert harness.answer_set("symbols", ["a/b.py::(*C).d"]) == {"a/b.py::(*C).d"}


def test_a_path_answer_keeps_its_spelling() -> None:
    assert harness.answer_set("paths", ["a/b.py::C::d"]) == {"a/b.py::C::d"}
