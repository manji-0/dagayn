"""The Rust stdio front end of ``dagayn serve`` (``dagayn.server.proxy``).

The MCP snapshots prove what it answers; these prove when it loads Python:
never for a session that only lists, once for the first call, and a session
still ends cleanly when stdin closes.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import time
from pathlib import Path
from typing import Any

import pytest

DAGAYN = Path(sys.executable).with_name("dagayn")
BOOT_TRACE = "starting the Python MCP server"
SURFACE = Path(__file__).parent.parent / "dagayn" / "server" / "mcp_surface.json"

pytestmark = pytest.mark.skipif(not DAGAYN.exists(), reason="dagayn console script not installed")


def _env(**extra: str) -> dict[str, str]:
    env = {
        key: value
        for key, value in os.environ.items()
        if not key.startswith(("DAGAYN_", "CRG_"))
        and key not in ("CLAUDE_PROJECT_DIR", "CURSOR_PROJECT_DIR", "WORKSPACE_FOLDER_PATHS")
    }
    env.update({"DAGAYN_MCP_TRACE": "1", **extra})
    return env


@pytest.fixture
def repo(tmp_path: Path) -> Path:
    root = tmp_path / "repo"
    (root / ".git").mkdir(parents=True)
    (root / "app.py").write_text("def main():\n    return helper()\n\n\ndef helper():\n    pass\n")
    subprocess.run([DAGAYN, "build", "--repo", root], env=_env(), check=True, capture_output=True)
    return root


class Session:
    def __init__(self, repo: Path, **env: str) -> None:
        self.proc = subprocess.Popen(  # noqa: S603 - fixed argv, no shell
            [DAGAYN, "serve", "--repo", repo, "--tools", "all"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=_env(**env),
            text=True,
        )

    def send(self, message: dict[str, Any]) -> None:
        assert self.proc.stdin is not None
        self.proc.stdin.write(json.dumps({"jsonrpc": "2.0", **message}) + "\n")
        self.proc.stdin.flush()

    def read(self) -> dict[str, Any]:
        assert self.proc.stdout is not None
        line = self.proc.stdout.readline()
        assert line, "server closed stdout"
        return json.loads(line)

    def open(self) -> dict[str, Any]:
        self.send(
            {
                "id": 0,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-06-18",
                    "capabilities": {},
                    "clientInfo": {"name": "test", "version": "0"},
                },
            }
        )
        reply = self.read()
        self.send({"method": "notifications/initialized"})
        return reply

    def close(self) -> tuple[int, float, str]:
        """Close stdin; the exit status, seconds until exit, and stderr."""
        started = time.monotonic()
        # communicate() closes stdin first.
        _, stderr = self.proc.communicate(timeout=30)
        return self.proc.returncode, time.monotonic() - started, stderr


def test_a_listing_session_never_loads_the_python_server(repo: Path) -> None:
    session = Session(repo)
    reply = session.open()
    assert reply["result"]["serverInfo"] == {"name": "dagayn", "version": _version()}
    session.send({"id": 1, "method": "tools/list", "params": {}})
    surface = json.loads(SURFACE.read_text(encoding="utf-8"))
    assert len(session.read()["result"]["tools"]) == len(surface["tools"])
    session.send({"id": 2, "method": "ping", "params": {}})
    assert session.read() == {"jsonrpc": "2.0", "id": 2, "result": {}}

    status, seconds, stderr = session.close()
    assert status == 0
    assert seconds < 5
    assert BOOT_TRACE not in stderr


def test_the_first_call_loads_it_and_local_replies_do_not_wait(repo: Path) -> None:
    session = Session(repo)
    session.open()
    session.send(
        {
            "id": 1,
            "method": "tools/call",
            # Not answered in Rust yet.
            "params": {
                "name": "query_graph_tool",
                "arguments": {"pattern": "callers_of", "target": "helper"},
            },
        }
    )
    # Sent while the call boots fastmcp and the tools, answered first.
    session.send({"id": 2, "method": "ping", "params": {}})
    first, second = session.read(), session.read()
    assert first == {"jsonrpc": "2.0", "id": 2, "result": {}}
    assert second["id"] == 1
    callers = second["result"]["structuredContent"]["results"]
    assert [node["name"] for node in callers] == ["main"]

    status, _, stderr = session.close()
    assert status == 0
    assert stderr.count(BOOT_TRACE) == 1


@pytest.mark.parametrize(
    ("name", "arguments"),
    [
        ("list_graph_stats_tool", {}),
        ("get_docs_section_tool", {"section_name": "trust"}),
        ("get_docs_section_tool", {"section_name": "trust", "max_chars": 50}),
    ],
)
def test_native_tools_answer_without_python_and_as_python_does(
    repo: Path, name: str, arguments: dict[str, Any]
) -> None:
    replies = []
    for env in ({}, {"DAGAYN_PYTHON_CLI": "1"}):
        session = Session(repo, **env)
        session.open()
        session.send(
            {"id": 1, "method": "tools/call", "params": {"name": name, "arguments": arguments}}
        )
        replies.append(session.read()["result"])
        status, _, stderr = session.close()
        assert status == 0
        assert BOOT_TRACE not in stderr
    rust, python = replies
    assert rust["isError"] is False
    assert json.loads(rust["content"][0]["text"]) == rust["structuredContent"]
    assert rust["structuredContent"] == python["structuredContent"]


def test_a_request_larger_than_a_pipe_buffer_is_relayed(repo: Path) -> None:
    """A 1 MiB argument crosses the request pipe and gets fastmcp's reply."""
    call = {
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "query_graph_tool",
            "arguments": {"pattern": "callers_of", "target": "x" * (1 << 20)},
        },
    }
    replies = []
    for env in ({}, {"DAGAYN_PYTHON_CLI": "1"}):
        session = Session(repo, **env)
        session.open()
        session.send(call)
        replies.append(session.read())
        status, _, _ = session.close()
        assert status == 0
    rust, python = replies
    assert rust["id"] == 1
    assert rust["result"]["structuredContent"]["status"] == "error"
    assert rust["result"]["structuredContent"] == python["result"]["structuredContent"]


def test_the_python_cli_keeps_fastmcps_own_loop(repo: Path) -> None:
    session = Session(repo, DAGAYN_PYTHON_CLI="1")
    reply = session.open()
    assert reply["result"]["serverInfo"]["version"] != _version()
    session.send(
        {
            "id": 1,
            "method": "tools/call",
            "params": {"name": "list_graph_stats_tool", "arguments": {}},
        }
    )
    assert session.read()["id"] == 1
    status, _, stderr = session.close()
    assert status == 0
    assert BOOT_TRACE not in stderr


def _version() -> str:
    from dagayn import __version__

    return __version__


@pytest.fixture
def git_repo(tmp_path: Path) -> Path:
    """A committed git repository with a built graph."""
    root = tmp_path / "gitrepo"
    root.mkdir()
    (root / "app.py").write_text("def main():\n    return helper()\n\n\ndef helper():\n    pass\n")
    (root / "test_app.py").write_text("from app import main\n\n\ndef test_main():\n    main()\n")
    identity = {
        "GIT_AUTHOR_NAME": "t",
        "GIT_AUTHOR_EMAIL": "t@example.invalid",
        "GIT_COMMITTER_NAME": "t",
        "GIT_COMMITTER_EMAIL": "t@example.invalid",
    }
    for args in (["init", "-q", "-b", "main"], ["add", "-A"], ["commit", "-q", "-m", "init"]):
        subprocess.run(["git", *args], cwd=root, env={**_env(), **identity}, check=True)
    subprocess.run([DAGAYN, "build", "--repo", root], env=_env(), check=True, capture_output=True)
    return root


def _call_both(repo: Path, name: str, arguments: dict[str, Any]) -> tuple[Any, Any, str]:
    """The Rust front end's and fastmcp's results, and the front end's stderr."""
    results = []
    rust_stderr = ""
    for env in ({}, {"DAGAYN_PYTHON_CLI": "1"}):
        session = Session(repo, **env)
        session.open()
        session.send(
            {"id": 1, "method": "tools/call", "params": {"name": name, "arguments": arguments}}
        )
        results.append(session.read()["result"])
        status, _, stderr = session.close()
        assert status == 0
        if not env:
            rust_stderr = stderr
    return results[0], results[1], rust_stderr


@pytest.mark.parametrize(
    "task",
    ["", "review PR #42", "fix the login bug", "レビューして", "explore the architecture", "x"],
)
@pytest.mark.parametrize("dirty", [False, True])
def test_minimal_context_answers_at_head_as_python_does(
    git_repo: Path, task: str, dirty: bool
) -> None:
    if dirty:
        (git_repo / "app.py").write_text("def main():\n    return 2\n")
    rust, python, stderr = _call_both(git_repo, "get_minimal_context_tool", {"task": task})
    assert BOOT_TRACE not in stderr
    assert rust["structuredContent"] == python["structuredContent"]
    assert json.loads(rust["content"][0]["text"]) == rust["structuredContent"]
    expected = "worktree_behind" if dirty else "commit_synced"
    assert rust["structuredContent"]["sync"]["state"] == expected


def test_minimal_context_leaves_repair_to_python(git_repo: Path) -> None:
    """A graph behind HEAD queues a prepare, which is Python's."""
    (git_repo / "app.py").write_text("def main():\n    return 3\n")
    subprocess.run(
        ["git", "commit", "-qam", "next"],
        cwd=git_repo,
        env={
            **_env(),
            "GIT_AUTHOR_NAME": "t",
            "GIT_AUTHOR_EMAIL": "t@x",
            "GIT_COMMITTER_NAME": "t",
            "GIT_COMMITTER_EMAIL": "t@x",
        },
        check=True,
    )
    session = Session(git_repo, DAGAYN_HOOK_UPDATE="0")
    session.open()
    session.send(
        {
            "id": 1,
            "method": "tools/call",
            "params": {"name": "get_minimal_context_tool", "arguments": {}},
        }
    )
    reply = session.read()["result"]["structuredContent"]
    status, _, stderr = session.close()
    assert status == 0
    assert reply["sync"]["state"] == "commit_drift"
    assert stderr.count(BOOT_TRACE) == 1
