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


@pytest.mark.parametrize(
    "arguments",
    [
        {"pattern": "callers_of", "target": "app.py::helper", "depth": 0},
        {"pattern": "callees_of", "target": "app.py::helper", "depth": 2},
        {"pattern": "no_such_pattern", "target": "app.py::helper"},
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
        {"query": "helper", "limit": 0},
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
    repo: Path, calls: list[tuple[str, dict[str, Any]]]
) -> tuple[list[Any], list[Any], str]:
    """Each side's results for *calls* in one session, with ``_runtime.pid``
    checked against the server process and then dropped."""
    sides = []
    rust_stderr = ""
    for env in ({}, {"DAGAYN_PYTHON_CLI": "1"}):
        session = Session(repo, **env)
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
        # Python's own validation error.
        (flow, {"mode": "get"}),
    ]
    rust, python, stderr = _session_both(git_repo, calls)
    assert stderr.count(FLOW_TRACE) == len(calls) - 1
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
        # Python's own validation error.
        (arch, {"mode": "community"}),
    ]
    rust, python, stderr = _session_both(git_repo, calls)
    assert stderr.count(ARCHITECTURE_TRACE) == len(calls) - 1
    assert rust == python
    assert rust[0]["structuredContent"]["count"] == 1
