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
    def __init__(self, repo: Path | None, cwd: Path | None = None, /, **env: str) -> None:
        """``dagayn serve --repo repo``, or without ``--repo`` from *cwd*."""
        pin = ["--repo", str(repo)] if repo is not None else []
        self.proc = subprocess.Popen(  # noqa: S603 - fixed argv, no shell
            [DAGAYN, "serve", *pin, "--tools", "all"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=_env(**env),
            cwd=cwd,
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
            # An unknown tool is never answered in Rust: fastmcp reports it.
            "params": {"name": "no_such_tool", "arguments": {}},
        }
    )
    # Sent while the call boots fastmcp and the tools, answered first.
    session.send({"id": 2, "method": "ping", "params": {}})
    first, second = session.read(), session.read()
    assert first == {"jsonrpc": "2.0", "id": 2, "result": {}}
    assert second["id"] == 1
    assert second["result"]["isError"] is True

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
    session.open()
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
    # The front end would have answered it natively and said so.
    assert "answered list_graph_stats_tool in Rust" not in stderr
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


def _call_both(
    repo: Path | None,
    name: str,
    arguments: dict[str, Any],
    *,
    cwd: Path | None = None,
    **extra_env: str,
) -> tuple[Any, Any, str]:
    """The Rust front end's and fastmcp's results, and the front end's stderr."""
    results = []
    rust_stderr = ""
    for env in ({}, {"DAGAYN_PYTHON_CLI": "1"}):
        session = Session(repo, cwd, **extra_env, **env)
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


NATIVE_TRACE = "answered query_graph_tool in Rust"


@pytest.mark.parametrize(
    "arguments",
    [
        {"pattern": "callers_of", "target": "app.py::helper"},
        {"pattern": "callers_of", "target": "app.py::main", "detail_level": "minimal"},
        {"pattern": "callees_of", "target": "app.py::main"},
        {"pattern": "callees_of", "target": "app.py::helper", "detail_level": "minimal"},
        {"pattern": "callers_of", "target": "helper"},  # resolved by name search
        {"pattern": "callees_of", "target": "no_such_symbol"},
        {"pattern": "source_of", "target": "app.py::main"},
        {"pattern": "source_of", "target": "main", "detail_level": "minimal"},
        # Nothing calls it by its qualified name: the bare-name fallback.
        {"pattern": "callers_of", "target": "test_app.py::test_main"},
        {"pattern": "callers_of", "target": "app.py::helper", "depth": 3},
        {"pattern": "callers_of", "target": "app.py::helper", "detail_level": "full"},
        {"pattern": "children_of", "target": "app.py"},
        {"pattern": "imports_of", "target": "test_app.py"},
        {"pattern": "importers_of", "target": "app.py", "depth": 2},
        {"pattern": "file_summary", "target": "app.py", "detail_level": "minimal"},
        {"pattern": "file_summary", "target": "missing.py"},
        {"pattern": "inheritors_of", "target": "app.py::main"},
        {"pattern": "docs_for", "target": "app.py::main", "detail_level": "full"},
        {"pattern": "callers_of", "target": "map"},
        {"pattern": "tests_for", "target": "app.py::main"},
        {"pattern": "tests_for", "target": "helper", "detail_level": "full"},
        {"pattern": "tests_for", "target": "app.py::helper", "detail_level": "minimal"},
    ],
)
def test_query_graph_answers_in_rust_as_python_does(
    git_repo: Path, arguments: dict[str, Any]
) -> None:
    rust, python, stderr = _call_both(git_repo, "query_graph_tool", arguments)
    assert NATIVE_TRACE in stderr
    assert BOOT_TRACE not in stderr
    assert rust["structuredContent"] == python["structuredContent"]
    assert json.loads(rust["content"][0]["text"]) == rust["structuredContent"]


def _second_repo(tmp_path: Path) -> Path:
    other = tmp_path / "other"
    (other / ".git").mkdir(parents=True)
    return other


@pytest.mark.parametrize(
    ("where", "hints", "native"),
    [
        # The server's working directory is the checkout, or below it.
        ("repo", {}, True),
        ("repo/sub", {}, True),
        # An editor that starts the server elsewhere names the project.
        ("outside", {"CLAUDE_PROJECT_DIR": "repo"}, True),
        ("outside", {"WORKSPACE_FOLDER_PATHS": "repo"}, True),
        ("repo", {"CURSOR_PROJECT_DIR": "repo"}, True),
        ("outside", {"CRG_REPO_ROOT": "repo"}, True),
        # Two unrelated workspaces: Python explains the ambiguity.
        ("outside", {"WORKSPACE_FOLDER_PATHS": "repo,other"}, False),
        # No checkout and no hint: Python decides (and refuses $HOME).
        ("outside", {}, False),
    ],
)
def test_an_omitted_repo_root_is_auto_detected_as_python_does(
    git_repo: Path, tmp_path: Path, where: str, hints: dict[str, str], native: bool
) -> None:
    places = {
        "repo": git_repo,
        "repo/sub": git_repo / "sub",
        "outside": tmp_path / "outside",
        "other": _second_repo(tmp_path),
    }
    for path in places.values():
        path.mkdir(exist_ok=True)
    env = {
        var: ",".join(str(places[name]) for name in value.split(","))
        for var, value in hints.items()
    }
    arguments = {"pattern": "callers_of", "target": "app.py::helper"}
    rust, python, stderr = _call_both(None, "query_graph_tool", arguments, cwd=places[where], **env)
    assert (NATIVE_TRACE in stderr) is native
    assert rust["structuredContent"] == python["structuredContent"]
    assert json.loads(rust["content"][0]["text"]) == rust["structuredContent"]
    if native:
        assert rust["structuredContent"]["_repo"]["source"] == "auto"
        assert rust["structuredContent"]["_repo"]["repo_root"] == str(git_repo.resolve())


def _call_while_written(repo: Path, hold_seconds: float, **env: str) -> tuple[dict[str, Any], str]:
    """A ``query_graph_tool`` call sent while another process writes the graph
    for *hold_seconds*: its result and the front end's stderr."""
    holder = subprocess.Popen(  # noqa: S603 - fixed argv, no shell
        [
            sys.executable,
            "-c",
            "import sys, time; from dagayn.write_lock import graph_write_lock\n"
            "with graph_write_lock(sys.argv[1]):\n"
            "    print('held', flush=True); time.sleep(float(sys.argv[2]))",
            str(repo / ".dagayn" / "graph.db"),
            str(hold_seconds),
        ],
        stdout=subprocess.PIPE,
        text=True,
    )
    try:
        assert holder.stdout is not None
        assert holder.stdout.readline().strip() == "held"
        session = Session(repo, **env)
        session.open()
        arguments = {"pattern": "callers_of", "target": "app.py::helper"}
        session.send(
            {
                "id": 1,
                "method": "tools/call",
                "params": {"name": "query_graph_tool", "arguments": arguments},
            }
        )
        result = session.read()["result"]
        status, _, stderr = session.close()
        assert status == 0
    finally:
        holder.wait(timeout=30)
    return result, stderr


def test_a_graph_written_briefly_is_waited_for_in_rust(git_repo: Path) -> None:
    result, stderr = _call_while_written(git_repo, 1.0)
    assert NATIVE_TRACE in stderr
    assert BOOT_TRACE not in stderr
    _, python, _ = _call_both(
        git_repo, "query_graph_tool", {"pattern": "callers_of", "target": "app.py::helper"}
    )
    assert result["structuredContent"] == python["structuredContent"]


def test_a_graph_written_past_the_wait_goes_to_python(git_repo: Path) -> None:
    result, stderr = _call_while_written(git_repo, 3.0, DAGAYN_READ_LOCK_TIMEOUT="0.5")
    assert NATIVE_TRACE not in stderr
    assert BOOT_TRACE in stderr
    assert result["structuredContent"]["status"] == "error"
    assert "is being written" in result["structuredContent"]["error"]


def test_a_graph_under_crg_data_dir_is_answered_in_rust(tmp_path: Path) -> None:
    """``CRG_DATA_DIR`` keeps each checkout's graph in ``<dir>/<name>-<digest>``;
    the name is made filesystem-safe and, on a case-folding filesystem, lower."""
    root = tmp_path / "My Repo!"
    (root / ".git").mkdir(parents=True)
    (root / "app.py").write_text("def main():\n    return helper()\n\n\ndef helper():\n    pass\n")
    shared = tmp_path / "graphs"
    env = {"CRG_DATA_DIR": str(shared)}
    subprocess.run(
        [DAGAYN, "build", "--repo", root],
        env=_env(DAGAYN_PYTHON_CLI="1", **env),
        check=True,
        capture_output=True,
    )
    assert not (root / ".dagayn").exists()
    (data_dir,) = shared.iterdir()
    arguments = {"pattern": "callers_of", "target": "app.py::helper"}
    rust, python, stderr = _call_both(root, "query_graph_tool", arguments, **env)
    assert NATIVE_TRACE in stderr
    assert rust["structuredContent"] == python["structuredContent"]
    assert rust["structuredContent"]["_repo"]["db_path"] == str(data_dir / "graph.db")


MODERN_META: dict[str, Any] = {
    "io.modelcontextprotocol/protocolVersion": "2026-07-28",
    "io.modelcontextprotocol/clientInfo": {"name": "claude-code", "version": "2.1.289"},
    "io.modelcontextprotocol/clientCapabilities": {"roots": {"listChanged": True}},
}


def _modern_session(
    repo: Path, requests: list[tuple[str, dict[str, Any]]], **env: str
) -> tuple[list[dict[str, Any]], str]:
    """Send *requests* as Claude Code does over the 2026-07-28 protocol: no
    ``initialize``, every request with the ``_meta`` envelope (a tool call's
    also with a tool-use id and a progress token)."""
    session = Session(repo, **env)
    replies = []
    for request_id, (method, params) in enumerate(requests):
        meta = dict(MODERN_META)
        if method == "tools/call":
            meta.update(
                {"claudecode/toolUseId": f"toolu_{request_id}", "progressToken": request_id}
            )
        session.send({"id": request_id, "method": method, "params": {**params, "_meta": meta}})
        reply = session.read()
        assert reply["id"] == request_id
        replies.append(reply)
    status, _, stderr = session.close()
    assert status == 0
    return replies, stderr


def _comparable(replies: list[dict[str, Any]]) -> list[dict[str, Any]]:
    """*replies* with each tool result's text parsed (its key order is the
    implementation's) and the process id dropped."""
    for reply in replies:
        result = reply.get("result") or {}
        for item in result.get("content") or []:
            if isinstance(result.get("structuredContent"), dict):
                item["text"] = json.loads(item["text"])
                item["text"].get("_runtime", {}).pop("pid", None)
        structured = result.get("structuredContent")
        if isinstance(structured, dict):
            structured.get("_runtime", {}).pop("pid", None)
    return replies


def test_a_modern_session_is_answered_in_rust_as_python_does(git_repo: Path) -> None:
    """Claude Code opens with ``server/discover`` and sends every request in
    the 2026-07-28 envelope; the listings and native calls carry
    ``resultType`` and the ``serverInfo`` stamp as the SDK's runner adds them."""
    requests: list[tuple[str, dict[str, Any]]] = [
        ("server/discover", {}),
        ("prompts/list", {}),
        ("resources/list", {}),
        ("tools/list", {}),
        ("tools/call", {"name": "get_minimal_context_tool", "arguments": {"task": "review"}}),
        (
            "tools/call",
            {
                "name": "query_graph_tool",
                "arguments": {"pattern": "callers_of", "target": "app.py::helper"},
            },
        ),
        (
            "tools/call",
            {"name": "query_graph_tool", "arguments": {"pattern": "nope", "target": "x"}},
        ),
        ("tools/call", {"name": "flow_tool", "arguments": {"mode": "get"}}),
    ]
    rust, stderr = _modern_session(git_repo, requests)
    python, _ = _modern_session(git_repo, requests, DAGAYN_PYTHON_CLI="1")
    assert BOOT_TRACE not in stderr
    assert stderr.count(" in Rust") == 4
    assert _comparable(rust) == _comparable(python)
    stamp = {"io.modelcontextprotocol/serverInfo": {"name": "dagayn", "version": _version()}}
    assert all(reply["result"]["_meta"] == stamp for reply in rust)
    assert rust[0]["result"]["supportedVersions"] == ["2026-07-28"]


def test_a_modern_call_left_to_python_stays_modern(git_repo: Path) -> None:
    """A call the front end declines boots the backend with that very request
    (no ``initialize`` to replay), so the backend serves the modern era too;
    ``initialize`` on the modern connection gets the backend's refusal."""
    requests: list[tuple[str, dict[str, Any]]] = [
        ("server/discover", {}),
        (
            "tools/call",
            {
                "name": "query_graph_tool",
                "arguments": {"pattern": "callers_of", "target": "app.py::main", "x": 1},
            },
        ),
        (
            "tools/call",
            {
                "name": "query_graph_tool",
                "arguments": {"pattern": "callers_of", "target": "app.py::helper"},
            },
        ),
    ]
    rust, stderr = _modern_session(git_repo, requests)
    python, _ = _modern_session(git_repo, requests, DAGAYN_PYTHON_CLI="1")
    assert stderr.count(BOOT_TRACE) == 1
    assert stderr.count(" in Rust") == 1
    assert _comparable(rust) == _comparable(python)
    assert rust[1]["result"]["resultType"] == "complete"

    def opened_then_initialized(**env: str) -> dict[str, Any]:
        session = Session(git_repo, **env)
        session.send({"id": 0, "method": "server/discover", "params": {"_meta": MODERN_META}})
        session.read()
        session.send(
            {
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-06-18",
                    "capabilities": {},
                    "clientInfo": {"name": "test", "version": "0"},
                },
            }
        )
        reply = session.read()
        session.close()
        return reply

    assert opened_then_initialized() == opened_then_initialized(DAGAYN_PYTHON_CLI="1")


def test_ensure_graph_reports_an_auto_detected_root_as_python_does(git_repo: Path) -> None:
    """``session_prepare`` resolves the root before opening the store, so Python
    reports even an auto-detected one as explicit."""
    rust, python, stderr = _call_both(None, "ensure_graph_tool", {}, cwd=git_repo)
    assert "answered ensure_graph_tool in Rust" in stderr
    for result in (rust, python):
        result["structuredContent"].pop("elapsed_seconds")
    assert rust["structuredContent"] == python["structuredContent"]
    assert rust["structuredContent"]["_repo"]["source"] == "explicit"


@pytest.mark.parametrize(
    ("name", "arguments"),
    [
        ("query_graph_tool", {"pattern": "callers_of", "target": "app.py::helper", "depth": 0}),
        ("query_graph_tool", {"pattern": "callees_of", "target": "app.py::helper", "depth": 2}),
        ("query_graph_tool", {"pattern": "no_such_pattern", "target": "app.py::helper"}),
        ("semantic_search_nodes_tool", {"query": "helper", "limit": 0}),
        ("semantic_search_nodes_tool", {"query": "", "limit": -1}),
        ("get_docs_section_tool", {"section_name": "no-such-section"}),
        ("get_docs_section_tool", {"section_name": "trust", "max_chars": 0}),
        ("get_docs_section_tool", {"section_name": "trust", "max_chars": -10}),
        ("refactor_tool", {"mode": "rename", "old_name": "", "new_name": "x"}),
        ("refactor_tool", {"mode": "rename", "new_name": "x"}),
        ("refactor_tool", {"mode": "rename", "old_name": "", "new_name": ""}),
    ],
)
def test_argument_errors_are_answered_in_rust_as_python_does(
    git_repo: Path, name: str, arguments: dict[str, Any]
) -> None:
    rust, python, stderr = _call_both(git_repo, name, arguments)
    assert f"answered {name} in Rust" in stderr
    assert BOOT_TRACE not in stderr
    for result in (rust, python):
        result["structuredContent"].get("_runtime", {}).pop("pid", None)
    assert rust["structuredContent"] == python["structuredContent"]
    assert rust["structuredContent"]["status"] in ("error", "not_found", "ok")


@pytest.mark.parametrize(
    "arguments",
    [
        {"pattern": "callers_of", "target": "app.py::main", "detail_level": "minimal", "x": 1},
    ],
)
def test_query_graph_leaves_the_rest_to_python(git_repo: Path, arguments: dict[str, Any]) -> None:
    rust, python, stderr = _call_both(git_repo, "query_graph_tool", arguments)
    assert NATIVE_TRACE not in stderr
    assert rust == python


SEARCH_TRACE = "answered semantic_search_nodes_tool in Rust"


@pytest.mark.parametrize(
    "arguments",
    [
        {"query": "helper"},
        {"query": "main", "kind": "Function", "limit": 1},
        {"query": "app.main", "detail_level": "minimal"},
        {"query": "test_main"},
        {"query": "no_such_thing_here"},
    ],
)
def test_search_without_embeddings_answers_as_python_does(
    git_repo: Path, arguments: dict[str, Any]
) -> None:
    rust, python, stderr = _call_both(git_repo, "semantic_search_nodes_tool", arguments)
    assert SEARCH_TRACE in stderr
    assert BOOT_TRACE not in stderr
    assert rust["structuredContent"] == python["structuredContent"]


@pytest.mark.parametrize(
    "arguments",
    [
        {"query": "helper", "provider": "openai"},
        {"query": "helper", "model": "m"},
        {"query": "   "},
    ],
)
def test_search_leaves_embedding_and_errors_to_python(
    git_repo: Path, arguments: dict[str, Any]
) -> None:
    rust, python, stderr = _call_both(git_repo, "semantic_search_nodes_tool", arguments)
    assert SEARCH_TRACE not in stderr
    assert rust == python


@pytest.mark.parametrize(
    ("tool", "arguments"),
    [
        ("get_minimal_context", {"task": "review"}),
        ("query_graph", {"pattern": "callers_of", "target": "app.py::helper"}),
        ("query_graph", {"pattern": "source_of", "target": "main"}),
        ("query_graph", {"pattern": "tests_for", "target": "app.py::main"}),
        ("query_graph", {"pattern": "children_of", "target": "app.py", "detail_level": "full"}),
        ("semantic_search_nodes", {"query": "helper"}),
        ("semantic_search_nodes", {"query": "zz_none", "detail_level": "minimal"}),
        ("list_graph_stats", {}),
        ("get_docs_section", {"section_name": "trust"}),
    ],
)
def test_native_tools_leave_the_hint_session_untouched(
    git_repo: Path, tool: str, arguments: dict[str, Any]
) -> None:
    """These tools record nothing in the `dagayn.hints` session, so their Rust
    versions record nothing either. A Rust tool that calls `generate_hints`
    records into the same session (`_core.HintSession`) and is checked by a
    mixed-session test instead."""
    import dagayn.tools as tools
    from dagayn.hints import get_session, reset_session

    reset_session()
    getattr(tools, tool)(repo_root=str(git_repo), **arguments)
    session = get_session()
    assert list(session.tools_called) == []
    assert session.files_touched == set()
    assert session.nodes_queried == set()


REVIEW_TRACE = "answered review_tool in Rust"


def _session_both(
    repo: Path, calls: list[tuple[str, dict[str, Any]]], **extra: str
) -> tuple[list[Any], list[Any], str]:
    """Each side's results for *calls* in one session, with ``_runtime.pid``
    checked against the server process and then dropped."""
    sides = []
    rust_stderr = ""
    for env in ({}, {"DAGAYN_PYTHON_CLI": "1"}):
        session = Session(repo, **env, **extra)
        session.open()
        results = []
        for index, (name, arguments) in enumerate(calls, 1):
            session.send(
                {
                    "id": index,
                    "method": "tools/call",
                    "params": {"name": name, "arguments": arguments},
                }
            )
            result = session.read()["result"]
            content = result.get("structuredContent")
            if isinstance(content, dict) and "_runtime" in content:
                assert json.loads(result["content"][0]["text"]) == content
                assert content["_runtime"]["pid"] == session.proc.pid
                content["_runtime"]["pid"] = 0
                del result["content"]  # the same, pid included
            results.append(result)
        status, _, stderr = session.close()
        assert status == 0
        if not env:
            rust_stderr = stderr
        sides.append(results)
    return sides[0], sides[1], rust_stderr


def _commit(repo: Path, message: str) -> None:
    identity = {
        "GIT_AUTHOR_NAME": "t",
        "GIT_AUTHOR_EMAIL": "t@example.invalid",
        "GIT_COMMITTER_NAME": "t",
        "GIT_COMMITTER_EMAIL": "t@example.invalid",
    }
    for args in (["add", "-A"], ["commit", "-q", "-m", message]):
        subprocess.run(["git", *args], cwd=repo, env={**_env(), **identity}, check=True)


@pytest.mark.parametrize("change", ["none", "dirty", "committed", "wide"])
def test_review_answers_in_rust_as_python_does(git_repo: Path, change: str) -> None:
    if change in {"committed", "wide"}:
        (git_repo / "app.py").write_text(
            "def main():\n    return helper()\n\n\ndef helper():\n    return 1\n"
        )
        if change == "wide":
            # Enough callers for apply_output_budget to trim the impact lists.
            callers = "".join(f"def caller_{i}():\n    return helper()\n\n\n" for i in range(400))
            (git_repo / "many.py").write_text(f"from app import helper\n\n\n{callers}")
        _commit(git_repo, "edit")
        subprocess.run(
            [DAGAYN, "update", "--repo", git_repo], env=_env(), check=True, capture_output=True
        )
    elif change == "dirty":
        (git_repo / "test_app.py").write_text(
            "from app import main\n\n\ndef test_main():\n    main()\n\n"
        )
        (git_repo / "new.py").write_text("x = 1\n")
    review = "review_tool"
    calls: list[tuple[str, dict[str, Any]]] = [
        (review, {"mode": "affected_flows"}),
        (review, {"mode": "affected_flows", "base": "HEAD"}),
        (review, {"mode": "affected_flows", "changed_files": ["app.py"]}),
        (review, {"mode": "affected_flows", "changed_files": ["./app.py", "missing.py"]}),
        (review, {"mode": "affected_flows", "changed_files": []}),
        (review, {"mode": "affected_flows", "detail_level": "minimal", "max_depth": 1}),
        (review, {"mode": "impact"}),
        (review, {"mode": "impact", "detail_level": "minimal"}),
        (review, {"mode": "impact", "changed_files": ["app.py"], "max_nodes": 500}),
        (review, {"mode": "impact", "changed_files": ["app.py"], "max_nodes": 2}),
        (review, {"mode": "impact", "changed_files": ["./app.py", "/elsewhere/x.py", "gone.py"]}),
        (review, {"mode": "impact", "changed_files": [], "max_depth": 0}),
        (review, {}),
        (review, {"mode": "changes", "changed_files": ["app.py"]}),
        (
            review,
            {"mode": "changes", "changed_files": ["app.py", "gone.py"], "detail_level": "minimal"},
        ),
        (review, {"mode": "changes", "base": "HEAD", "max_depth": 1}),
        (review, {"mode": "changes", "changed_files": []}),
        (review, {"mode": "context"}),
        (review, {"mode": "context", "detail_level": "minimal"}),
        (review, {"mode": "context", "changed_files": ["app.py", "../x.py", "gone.py"]}),
        (review, {"mode": "context", "changed_files": ["app.py"], "max_lines_per_file": 2}),
        (review, {"mode": "context", "changed_files": ["app.py"], "include_source": False}),
        # Python answers this one, from the session the Rust calls updated.
        (review, {"mode": "changes", "changed_files": ["app.py"], "include_source": True}),
        (review, {"mode": "affected_flows", "changed_files": ["test_app.py"]}),
        (review, {"mode": "impact", "changed_files": ["test_app.py"], "detail_level": "verbose"}),
    ]
    rust, python, stderr = _session_both(git_repo, calls)
    # A single-commit repository has no HEAD~1, so changes on the default base
    # is Python's error -- except where no file changed, which it reports
    # before diffing.
    python_only = {"none": 2, "dirty": 3}.get(change, 0)
    assert stderr.count(REVIEW_TRACE) == len(calls) - 1 - python_only
    assert rust == python
    if change == "wide":
        trimmed = rust[8]["structuredContent"]
        assert trimmed["truncated"] is True
        assert "_truncation" in trimmed


@pytest.mark.parametrize(
    "arguments",
    [
        {"mode": "changes", "changed_files": ["app.py"], "detail_level": "verbose"},
        {"mode": "affected_flows", "base": "bad ref"},
        {"mode": "affected_flows", "detail_level": "full"},
        {"mode": "affected_flows", "changed_files": "app.py"},
        {"mode": "affected_flows", "max_nodes": "5"},
        {"mode": "affected_flows", "x": 1},
        {"mode": "context", "max_lines_per_file": "5"},
        {"mode": "impact", "max_depth": 1.5},
        {"mode": "impact", "base": "a b"},
    ],
)
def test_review_leaves_the_rest_to_python(git_repo: Path, arguments: dict[str, Any]) -> None:
    rust, python, stderr = _session_both(git_repo, [("review_tool", arguments)])
    assert REVIEW_TRACE not in stderr
    assert rust == python


FLOW_TRACE = "answered flow_tool in Rust"


@pytest.mark.parametrize("stale", [False, True])
def test_flow_tool_answers_in_rust_as_python_does(git_repo: Path, stale: bool) -> None:
    if stale:
        # The stored flow keeps member ids the update replaces.
        (git_repo / "app.py").write_text("def main():\n    return 1\n")
        subprocess.run(
            [DAGAYN, "update", "--skip-flows", "--repo", git_repo],
            env=_env(),
            check=True,
            capture_output=True,
        )
    flow = "flow_tool"
    calls: list[tuple[str, dict[str, Any]]] = [
        (flow, {}),
        (flow, {"sort_by": "name", "detail_level": "minimal"}),
        (flow, {"kind": "Function", "limit": 1}),
        (flow, {"kind": "Nope"}),
        (flow, {"mode": "get", "flow_id": 1}),
        (flow, {"mode": "get", "flow_id": 1, "include_source": True}),
        (flow, {"mode": "get", "flow_name": "MAI"}),
        (flow, {"mode": "get", "flow_id": 999}),
        # The request validation error.
        (flow, {"mode": "get"}),
        (flow, {"mode": "get", "flow_name": ""}),
    ]
    rust, python, stderr = _session_both(git_repo, calls)
    assert stderr.count(FLOW_TRACE) == len(calls)
    assert rust == python
    if stale:
        assert rust[4]["structuredContent"]["status"] == "degraded"


ARCHITECTURE_TRACE = "answered architecture_analysis_tool in Rust"


def test_architecture_metrics_answer_in_rust_as_python_does(git_repo: Path) -> None:
    (git_repo / "pkg").mkdir()
    (git_repo / "pkg" / "core.py").write_text(
        "from app import main\n\n\nclass Base:\n    pass\n\n\ndef run():\n    return main()\n"
    )
    (git_repo / "app.py").write_text(
        "from pkg.core import Base\n\n\ndef main():\n    return helper()\n\n\n"
        "def helper():\n    pass\n"
    )
    subprocess.run(
        [DAGAYN, "build", "--repo", git_repo], env=_env(), check=True, capture_output=True
    )
    arch = "architecture_analysis_tool"
    calls: list[tuple[str, dict[str, Any]]] = [
        (arch, {"mode": "adp_violations"}),
        (arch, {"mode": "adp_violations", "granularity": "file", "top_n": 0}),
        (arch, {"mode": "sdp_metrics", "granularity": "file"}),
        (arch, {"mode": "sdp_violations", "min_delta": 0}),
        (arch, {"mode": "sdp_violations", "dependency_profile": "implementation"}),
        (arch, {"mode": "sap_metrics", "detail_level": "verbose"}),
        (arch, {"mode": "sap_metrics", "scope_kind": "file", "unit_filter": ["pkg"]}),
        (arch, {"mode": "sap_violations", "min_distance": 0.1, "artifact_scope": "all"}),
        (arch, {"mode": "hubs"}),
        (arch, {"mode": "bridges", "artifact_scope": "all", "top_n": 2}),
        (arch, {"mode": "knowledge_gaps"}),
        (arch, {"mode": "knowledge_gaps", "artifact_scope": "docs", "top_n": 0}),
        (arch, {"mode": "surprising_connections", "artifact_scope": "all"}),
        (arch, {}),
        (arch, {"mode": "overview", "detail_level": "verbose", "top_n": 1}),
        (arch, {"mode": "communities", "detail_level": "standard", "sort_by": "name"}),
        (arch, {"mode": "communities", "min_size": 2, "top_n": 1}),
        (arch, {"mode": "community", "community_id": 1, "include_members": True}),
        (arch, {"mode": "community", "community_name": "NO-SUCH"}),
        # The request validation error.
        (arch, {"mode": "community"}),
    ]
    rust, python, stderr = _session_both(git_repo, calls)
    assert stderr.count(ARCHITECTURE_TRACE) == len(calls)
    assert rust == python
    assert rust[0]["structuredContent"]["count"] == 1


REFACTOR_TRACE = "answered refactor_tool in Rust"


def test_refactor_dead_code_and_suggest_answer_in_rust_as_python_does(git_repo: Path) -> None:
    (git_repo / "unused.py").write_text(
        "def orphan():\n    return 1\n\n\nclass Lonely:\n    pass\n\n\n"
        + "def big(a, b, c, d, e, f, is_x, use_y):\n"
        + "".join(f"    if a > {i}:\n        b = c + {i}\n" for i in range(40))
        + "    return b\n"
    )
    subprocess.run(
        [DAGAYN, "build", "--repo", git_repo], env=_env(), check=True, capture_output=True
    )
    refactor = "refactor_tool"
    calls: list[tuple[str, dict[str, Any]]] = [
        (refactor, {}),
        (refactor, {"mode": "suggest", "limit": 1}),
        (refactor, {"mode": "dead_code"}),
        (refactor, {"mode": "dead_code", "kind": "Class", "file_pattern": "unused"}),
        (refactor, {"mode": "dead_code", "limit": 0}),
    ]
    rust, python, stderr = _session_both(git_repo, calls)
    assert stderr.count(REFACTOR_TRACE) == len(calls)
    assert [r["structuredContent"] for r in rust] == [p["structuredContent"] for p in python]
    names = [d["name"] for d in rust[2]["structuredContent"]["dead_code"]]
    assert "orphan" in names


def test_rename_previewed_in_rust_is_applied_by_either_side(git_repo: Path) -> None:
    """The preview lands in the pending store both sides share: a dry run
    fastmcp has to coerce is Python's, the write is Rust's."""
    session = Session(git_repo)
    session.open()
    session.send(
        {
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "refactor_tool",
                "arguments": {"mode": "rename", "old_name": "helper", "new_name": "assist"},
            },
        }
    )
    preview = session.read()["result"]["structuredContent"]
    assert preview["status"] == "ok"
    refactor_id = preview["refactor_id"]
    for request_id, dry_run in ((2, "true"), (3, False)):
        session.send(
            {
                "id": request_id,
                "method": "tools/call",
                "params": {
                    "name": "apply_refactor_tool",
                    "arguments": {"refactor_id": refactor_id, "dry_run": dry_run},
                },
            }
        )
        applied = session.read()["result"]["structuredContent"]
        assert applied["status"] == "ok", applied
        if request_id == 2:
            assert "+def assist():" in applied["diffs"]["app.py"]
    status, _, stderr = session.close()
    assert status == 0
    assert REFACTOR_TRACE in stderr
    assert stderr.count(APPLY_TRACE) == 1
    assert BOOT_TRACE in stderr
    assert "def assist():" in (git_repo / "app.py").read_text()


APPLY_TRACE = "answered apply_refactor_tool in Rust"


def test_apply_dry_run_answers_in_rust_as_python_does(git_repo: Path) -> None:
    (git_repo / "app.py").write_text(
        "def main():\n    return helper()\n\n\n"
        + "".join(f"# filler {i}\n" for i in range(12))
        + "def helper():\n    return 'helper'  # helper\n"
    )
    subprocess.run(
        [DAGAYN, "build", "--repo", git_repo], env=_env(), check=True, capture_output=True
    )
    sides = []
    for env in ({}, {"DAGAYN_PYTHON_CLI": "1"}):
        session = Session(git_repo, **env)
        session.open()
        session.send(
            {
                "id": 1,
                "method": "tools/call",
                "params": {
                    "name": "refactor_tool",
                    "arguments": {"mode": "rename", "old_name": "helper", "new_name": "assist"},
                },
            }
        )
        refactor_id = session.read()["result"]["structuredContent"]["refactor_id"]
        results = []
        for request_id, arguments in enumerate(
            (
                {"refactor_id": refactor_id, "dry_run": True},
                {"refactor_id": "00000000", "dry_run": True},
                {"refactor_id": "00000000"},
            ),
            2,
        ):
            session.send(
                {
                    "id": request_id,
                    "method": "tools/call",
                    "params": {"name": "apply_refactor_tool", "arguments": arguments},
                }
            )
            results.append(session.read()["result"]["structuredContent"])
        status, _, stderr = session.close()
        assert status == 0
        if not env:
            assert stderr.count(APPLY_TRACE) == 3
        sides.append(results)
    assert sides[0] == sides[1]
    diff = sides[0][0]["diffs"]["app.py"]
    assert diff.startswith("--- a/app.py\n+++ b/app.py\n@@ -1,5 +1,5 @@\n")
    # The call and the definition are 15 lines apart: two hunks.
    assert diff.count("\n@@ ") == 2, diff
    assert "+def assist():\n" in diff


ENSURE_TRACE = "answered ensure_graph_tool in Rust"


@pytest.mark.parametrize("worktree", ["clean", "indexed_edit"])
def test_ensure_graph_answers_a_ready_graph_in_rust_as_python_does(
    git_repo: Path, worktree: str
) -> None:
    if worktree == "indexed_edit":
        (git_repo / "app.py").write_text("def main():\n    return 4\n")
        subprocess.run(
            [DAGAYN, "update", "--repo", git_repo], env=_env(), check=True, capture_output=True
        )
    calls: list[tuple[str, dict[str, Any]]] = [
        ("ensure_graph_tool", {}),
        ("ensure_graph_tool", {"force": False, "repo_root": str(git_repo)}),
    ]
    rust, python, stderr = _session_both(git_repo, calls)
    assert stderr.count(ENSURE_TRACE) == len(calls)
    assert BOOT_TRACE not in stderr
    for side in (rust, python):
        for result in side:
            result["structuredContent"]["elapsed_seconds"] = 0
    assert [r["structuredContent"] for r in rust] == [p["structuredContent"] for p in python]
    expected = "commit_synced" if worktree == "clean" else "worktree_ahead"
    assert rust[0]["structuredContent"]["sync"]["state"] == expected
    assert rust[0]["structuredContent"]["action"] == "noop"


def test_ensure_graph_leaves_a_refresh_to_python(git_repo: Path) -> None:
    (git_repo / "app.py").write_text("def main():\n    return 5\n")
    rust, _python, stderr = _call_both(git_repo, "ensure_graph_tool", {})
    assert ENSURE_TRACE not in stderr
    assert rust["structuredContent"]["phases"]["structure"] == "done"


@pytest.mark.parametrize("registry", [None, '{"repos": [{"path": "/a", "alias": "x"}]}'])
def test_maintenance_reads_answer_in_rust_as_python_does(
    git_repo: Path, tmp_path: Path, registry: str | None
) -> None:
    (git_repo / "big.py").write_text("def big():\n" + "    x = 1\n" * 60 + "    return x\n")
    subprocess.run(
        [DAGAYN, "build", "--repo", git_repo], env=_env(), check=True, capture_output=True
    )
    wiki = git_repo / ".dagayn" / "wiki"
    wiki.mkdir()
    (wiki / "app-main.md").write_text("# App\r\nmain\n")
    home = tmp_path / "home"
    (home / ".dagayn").mkdir(parents=True)
    if registry is not None:
        (home / ".dagayn" / "registry.json").write_text(registry)
    calls: list[tuple[str, dict[str, Any]]] = [
        ("find_large_functions_tool", {}),
        ("find_large_functions_tool", {"min_lines": 2, "kind": "Function", "limit": 2}),
        ("traverse_graph_tool", {"query": "helper"}),
        ("traverse_graph_tool", {"query": "main", "mode": "dfs", "depth": 2}),
        ("traverse_graph_tool", {"query": "main", "token_budget": 10}),
        ("traverse_graph_tool", {"query": "zzzqqq"}),
        ("get_suggested_questions_tool", {}),
        ("get_suggested_questions_tool", {"top_n": 1}),
        ("get_wiki_page_tool", {"community_name": "App Main"}),
        ("get_wiki_page_tool", {"community_name": "missing"}),
        ("list_repos_tool", {}),
    ]
    rust, python, stderr = _session_both(git_repo, calls, HOME=str(home))
    assert stderr.count(" in Rust") == len(calls)
    assert BOOT_TRACE not in stderr
    assert [r["structuredContent"] for r in rust] == [p["structuredContent"] for p in python]
    assert rust[8]["structuredContent"]["content"] == "# App\nmain\n"


def _fake_embedding_server(vector: list[float]) -> tuple[Any, int]:
    """An OpenAI-compatible ``/embeddings`` endpoint that embeds every query as *vector*."""
    import http.server
    import threading

    class Handler(http.server.BaseHTTPRequestHandler):
        def do_POST(self) -> None:  # noqa: N802 - http.server API
            self.rfile.read(int(self.headers.get("Content-Length") or 0))
            body = json.dumps({"data": [{"index": 0, "embedding": vector}]}).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, format: str, *args: Any) -> None:  # noqa: A002 - parent's name
            pass

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server, server.server_address[1]


def test_search_with_stored_vectors_answers_in_rust_as_python_does(git_repo: Path) -> None:
    import sqlite3
    import struct

    server, port = _fake_embedding_server([0.0, 1.0, 0.0, 0.0])
    provider = f"openai:fake-model@http://127.0.0.1:{port}/v1#dim=4#text=material"
    conn = sqlite3.connect(git_repo / ".dagayn" / "graph.db")
    conn.execute(
        "CREATE TABLE IF NOT EXISTS embeddings (qualified_name TEXT NOT NULL, vector BLOB NOT NULL,"
        " text_hash TEXT NOT NULL, provider TEXT NOT NULL, PRIMARY KEY (qualified_name, provider))"
    )
    for name, vector in (
        ("app.py::main", (1.0, 0.0, 0.0, 0.0)),
        ("app.py::helper", (0.0, 1.0, 0.0, 0.0)),
    ):
        conn.execute(
            "INSERT INTO embeddings VALUES (?, ?, 'h', ?)",
            (name, struct.pack("4f", *vector), provider),
        )
    conn.commit()
    conn.close()
    try:
        search = "semantic_search_nodes_tool"
        calls: list[tuple[str, dict[str, Any]]] = [
            (search, {"query": "something that assists"}),
            (search, {"query": "helper"}),
            (search, {"query": "main", "detail_level": "minimal"}),
            (search, {"query": "helper", "kind": "Function", "limit": 1}),
            ("traverse_graph_tool", {"query": "something that assists", "depth": 1}),
        ]
        rust, python, stderr = _session_both(git_repo, calls)
    finally:
        server.shutdown()
    assert stderr.count(" in Rust") == len(calls)
    assert BOOT_TRACE not in stderr
    assert [r["structuredContent"] for r in rust] == [p["structuredContent"] for p in python]
    first = rust[0]["structuredContent"]
    assert first["embedding_health"]["status"] == "degraded"
    assert first["results"][0]["qualified_name"] == "app.py::helper"
    assert rust[1]["structuredContent"]["search_mode"] == "hybrid"
    assert rust[4]["structuredContent"]["start_node"] == "app.py::helper"


def test_prompts_answer_from_the_recorded_replies_as_python_does(repo: Path) -> None:
    requests = [
        {"name": "review_changes"},
        {"name": "review_changes", "arguments": {"base": "main"}},
        {"name": "debug_issue", "arguments": {"description": ""}},
        {"name": "debug_issue", "arguments": {"description": 'flaky "login"\nretry'}},
        {"name": "architecture_map", "arguments": {"undeclared": "x"}},
        {"name": "pre_merge_check", "arguments": {"base": "origin/main"}},
    ]
    sides = []
    for env in ({}, {"DAGAYN_PYTHON_CLI": "1"}):
        session = Session(repo, **env)
        session.open()
        results = []
        for request_id, params in enumerate(requests, 1):
            session.send({"id": request_id, "method": "prompts/get", "params": params})
            results.append(session.read()["result"])
        status, _, stderr = session.close()
        assert status == 0
        if not env:
            assert BOOT_TRACE not in stderr
            assert stderr.count("answered prompt") == len(requests)
        sides.append(results)
    assert sides[0] == sides[1]
    assert 'flaky "login"\nretry' in sides[0][3]["messages"][0]["content"]["text"]


def test_postprocess_answers_in_rust_as_python_does(git_repo: Path) -> None:
    calls: list[tuple[str, dict[str, Any]]] = [
        ("run_postprocess_tool", {}),
        ("run_postprocess_tool", {"flows": False, "fts": False}),
    ]
    rust, python, stderr = _session_both(git_repo, calls)
    assert stderr.count("answered run_postprocess_tool in Rust") == len(calls)
    assert BOOT_TRACE not in stderr
    assert [r["structuredContent"] for r in rust] == [p["structuredContent"] for p in python]
    assert rust[0]["structuredContent"]["summary"] == "Post-processing complete."


def test_postprocess_after_the_python_server_starts_is_its(git_repo: Path) -> None:
    """A native writer next to Python's SQLite connections could remove the
    WAL index under them; once the backend runs, the call is relayed."""
    session = Session(git_repo)
    session.open()
    for request_id, (name, arguments) in enumerate(
        (("ensure_graph_tool", {"force": True}), ("run_postprocess_tool", {})), 1
    ):
        session.send(
            {
                "id": request_id,
                "method": "tools/call",
                "params": {"name": name, "arguments": arguments},
            }
        )
        assert session.read()["result"]["structuredContent"]["status"] == "ok"
    status, _, stderr = session.close()
    assert status == 0
    assert BOOT_TRACE in stderr
    assert "answered run_postprocess_tool in Rust" not in stderr
