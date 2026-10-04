"""Black-box MCP response snapshots: the parity oracle for the pure Rust port.

Builds a parity fixture with the ``dagayn`` CLI, starts ``dagayn serve`` over
stdio, calls a fixed list of read-only tools, and writes each payload as
canonical JSON. Nothing here imports dagayn, so the same harness checks any
server that speaks MCP: point ``DAGAYN_CLI_CMD`` / ``DAGAYN_MCP_SERVER_CMD`` at
another binary to compare it against the committed snapshots.

Usage:
    uv run python tools/mcp_snapshot.py --regenerate            # every fixture
    uv run python tools/mcp_snapshot.py --regenerate mixed      # one fixture
    uv run python tools/mcp_snapshot.py --check-determinism mixed

Snapshots live in ``tests/fixtures/parity/__mcp_snapshots__/``:
``<fixture>/<case>.json`` per tool call and ``tools_list.json`` for the
reduced tool and prompt listing.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import os
import re
import shlex
import shutil
import sqlite3
import subprocess
import sys
import tempfile
from collections.abc import Callable, Iterator, Mapping
from contextlib import contextmanager
from pathlib import Path
from typing import Any

REPO_ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO_ROOT))

# Client side only: keeps pydantic (under the mcp client) importable on early
# Python 3.14 builds. The server under test is a separate process.
import dagayn.contracts._python314_compat  # noqa: E402, F401

# isort: split
from mcp import ClientSession, StdioServerParameters  # noqa: E402
from mcp.client.stdio import stdio_client  # noqa: E402
from parity_export import export_db  # noqa: E402

PARITY_DIR = REPO_ROOT / "tests" / "fixtures" / "parity"
SNAPSHOT_DIR = PARITY_DIR / "__mcp_snapshots__"

#: (case name, tool, arguments) per fixture. Read-only tools only: building,
#: embedding, applying refactors, wiki generation, and cross-repo search write
#: state or reach outside the fixture.
_COMMON_CASES: list[tuple[str, str, dict[str, Any]]] = [
    ("minimal_context", "get_minimal_context_tool", {"task": "review the change"}),
    ("graph_stats", "list_graph_stats_tool", {}),
    ("architecture_overview", "architecture_analysis_tool", {"mode": "overview"}),
    ("architecture_communities", "architecture_analysis_tool", {"mode": "communities"}),
    ("flows", "flow_tool", {"mode": "list"}),
    ("refactor_suggest", "refactor_tool", {"mode": "suggest"}),
    ("dead_code", "refactor_tool", {"mode": "dead_code"}),
    ("suggested_questions", "get_suggested_questions_tool", {}),
    ("large_functions", "find_large_functions_tool", {"min_lines": 1}),
]

FIXTURE_CASES: dict[str, list[tuple[str, str, dict[str, Any]]]] = {
    "python_only": [
        ("callers_of", "query_graph_tool", {"pattern": "callers_of", "target": "create_user"}),
        ("callees_of", "query_graph_tool", {"pattern": "callees_of", "target": "run"}),
        ("importers_of", "query_graph_tool", {"pattern": "importers_of", "target": "models.py"}),
        ("source_of", "query_graph_tool", {"pattern": "source_of", "target": "get_email"}),
        ("children_of", "query_graph_tool", {"pattern": "children_of", "target": "models.py"}),
        ("search", "semantic_search_nodes_tool", {"query": "user email"}),
        (
            "rename_preview",
            "refactor_tool",
            {"mode": "rename", "old_name": "create_user", "new_name": "make_user"},
        ),
        (
            "review_context",
            "review_tool",
            {"mode": "context", "changed_files": ["services.py"]},
        ),
        ("impact", "review_tool", {"mode": "impact", "changed_files": ["models.py"]}),
        ("traverse", "traverse_graph_tool", {"query": "create_user"}),
    ],
    "mixed": [
        ("docs_for", "query_graph_tool", {"pattern": "docs_for", "target": "build_graph"}),
        (
            "ambiguous_target",
            "query_graph_tool",
            {"pattern": "implementations_of", "target": "Graph API"},
        ),
        ("search", "semantic_search_nodes_tool", {"query": "bucket"}),
        ("affected_flows", "review_tool", {"mode": "affected_flows", "changed_files": ["app.py"]}),
    ],
    "terraform_only": [
        ("children_of", "query_graph_tool", {"pattern": "children_of", "target": "main.tf"}),
        ("search", "semantic_search_nodes_tool", {"query": "output"}),
    ],
    "markdown_only": [
        ("search", "semantic_search_nodes_tool", {"query": "build"}),
        ("children_of", "query_graph_tool", {"pattern": "children_of", "target": "guide.md"}),
    ],
    "notebook": [
        (
            "file_summary",
            "query_graph_tool",
            {"pattern": "file_summary", "target": "analysis.ipynb"},
        ),
    ],
    "typescript": [
        ("tests_for", "query_graph_tool", {"pattern": "tests_for", "target": "decl"}),
        ("search", "semantic_search_nodes_tool", {"query": "barrel"}),
        (
            "review_changes",
            "review_tool",
            {"mode": "changes", "base": "HEAD", "changed_files": ["src/functions.ts"]},
        ),
        # The fixture is one commit: the default base HEAD~1 does not resolve.
        ("review_bad_base", "review_tool", {"mode": "changes", "changed_files": ["src/calls.ts"]}),
    ],
    "javascript": [
        ("children_of", "query_graph_tool", {"pattern": "children_of", "target": "helpers.js"}),
        ("search", "semantic_search_nodes_tool", {"query": "helper"}),
    ],
    "manifest_py_rust": [
        (
            "bridges_from",
            "query_graph_tool",
            {"pattern": "bridges_from", "target": "pyproject.toml"},
        ),
        (
            "cargo_bridges",
            "query_graph_tool",
            {"pattern": "bridges_from", "target": "rust/Cargo.toml"},
        ),
        (
            "importers_of",
            "query_graph_tool",
            {"pattern": "importers_of", "target": "rust/src/lib.rs"},
        ),
    ],
    "manifest_generated_client": [
        (
            "bridges_from",
            "query_graph_tool",
            {"pattern": "bridges_from", "target": "openapitools.json"},
        ),
        (
            "consumer_bridges",
            "query_graph_tool",
            {"pattern": "bridges_from", "target": "apps/web/package.json"},
        ),
    ],
}

#: Fixtures outside ``tests/fixtures/parity``; the rest are named after their directory.
FIXTURE_SOURCES: dict[str, Path] = {
    "manifest_py_rust": REPO_ROOT / "tests" / "fixtures" / "cross_artifact_manifest" / "py_rust",
    "manifest_generated_client": (
        REPO_ROOT / "tests" / "fixtures" / "cross_artifact_manifest" / "generated_client"
    ),
}

#: Keys whose values describe the run, not the graph.
_VOLATILE_KEYS = frozenset(
    {
        "db_path",
        "last_updated",
        "last_postprocessed_at",
        "built_at",
        "generated_at",
        "elapsed_ms",
        "elapsed_seconds",
        "duration_ms",
        "duration_seconds",
        "task_id",
        "version",
        "dagayn_version",
        "created_at",
        "fts_indexed_at",
        "updated_at",
        # Interpreter, binary path, and pid of the server process.
        "_runtime",
    }
)
_TIMESTAMP = re.compile(r"\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}:\d{2}")
#: A host path inside prose, e.g. an error message naming a file.
_HOST_PATH = re.compile(r"(?:^|[\s'\"(=:])/(?:Users|home|private|var|tmp|opt)/")
#: Session-scoped ids: replaced everywhere they appear, including prose.
_SESSION_ID_KEYS = ("refactor_id",)


def _cli_cmd() -> list[str]:
    raw = os.environ.get("DAGAYN_CLI_CMD")
    return shlex.split(raw) if raw else [sys.executable, "-m", "dagayn"]


#: fastmcp's own stdio loop, whatever ``DAGAYN_MCP_SERVER_CMD`` says.
PYTHON_SERVER_CMD = [sys.executable, "-m", "dagayn", "serve"]


def _server_cmd(repo: Path, tools: str | None, base: list[str] | None = None) -> list[str]:
    raw = os.environ.get("DAGAYN_MCP_SERVER_CMD")
    cmd = list(base) if base else shlex.split(raw) if raw else list(PYTHON_SERVER_CMD)
    cmd += ["--repo", str(repo)]
    if tools:
        cmd += ["--tools", tools]
    return cmd


def _isolated_env(home: Path) -> dict[str, str]:
    """Environment with no user config, registry, or embedding provider."""
    env = {
        key: value
        for key, value in os.environ.items()
        if not key.startswith(("DAGAYN_", "CRG_"))
        and not key.endswith("_API_KEY")
        and key not in {"HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME"}
    }
    env["HOME"] = str(home)
    return env


_GIT_IDENTITY = {
    "GIT_AUTHOR_NAME": "dagayn",
    "GIT_AUTHOR_EMAIL": "dagayn@example.invalid",
    "GIT_AUTHOR_DATE": "2026-01-01T00:00:00Z",
    "GIT_COMMITTER_NAME": "dagayn",
    "GIT_COMMITTER_EMAIL": "dagayn@example.invalid",
    "GIT_COMMITTER_DATE": "2026-01-01T00:00:00Z",
    "GIT_CONFIG_NOSYSTEM": "1",
}


def _git_commit_all(repo: Path, env: dict[str, str]) -> None:
    """Commit the fixture with a fixed identity and date: the SHA is reproducible."""
    git_env = {**env, **_GIT_IDENTITY}
    for args in (
        ["init", "-q", "-b", "main"],
        ["add", "-A"],
        ["commit", "-q", "--no-gpg-sign", "-m", "fixture"],
    ):
        subprocess.run(["git", *args], cwd=repo, env=git_env, check=True, capture_output=True)


@contextmanager
def built_fixture(name: str) -> Iterator[tuple[Path, dict[str, str]]]:
    """Copy fixture *name* to a temporary directory and build its graph."""
    with tempfile.TemporaryDirectory(prefix=f"mcp_snapshot_{name}_") as tmp:
        root = Path(tmp)
        repo = root / "repo"
        home = root / "home"
        home.mkdir()
        shutil.copytree(
            FIXTURE_SOURCES.get(name, PARITY_DIR / name),
            repo,
            ignore=shutil.ignore_patterns(".dagayn", ".git"),
        )
        env = _isolated_env(home)
        _git_commit_all(repo, env)
        subprocess.run(
            [*_cli_cmd(), "build", "--repo", str(repo)],
            check=True,
            capture_output=True,
            env=env,
            cwd=repo,
        )
        yield repo, env


def _payload(result: Any) -> Any:
    """The tool's JSON payload, independent of the server's MCP envelope."""
    if getattr(result, "structured_content", None) is not None:
        payload = result.structured_content
        # Servers wrap non-object returns as {"result": ...}.
        if isinstance(payload, dict) and set(payload) == {"result"}:
            return payload["result"]
        return payload
    texts = [item.text for item in result.content if getattr(item, "type", "") == "text"]
    joined = "".join(texts)
    try:
        return json.loads(joined)
    except json.JSONDecodeError:
        return joined


def normalize(value: Any, repo: Path, home: Path | None = None) -> Any:
    """Replace run-specific values with placeholders.

    Raises ``ValueError`` on an absolute path or timestamp that no rule
    covers, so a new volatile field surfaces instead of being frozen.
    """
    session_ids: dict[str, str] = {}

    def collect_ids(node: Any) -> None:
        if isinstance(node, dict):
            for key, item in node.items():
                if key in _SESSION_ID_KEYS and isinstance(item, str) and item:
                    session_ids[item] = f"<{key.upper()}>"
                collect_ids(item)
        elif isinstance(node, list):
            for item in node:
                collect_ids(item)

    collect_ids(value)
    roots = {str(repo.resolve()): "<REPO>", str(repo): "<REPO>"}
    if home is not None:
        roots.update({str(home.resolve()): "<HOME>", str(home): "<HOME>"})

    def norm_str(text: str, where: str) -> str:
        for session_id, placeholder in session_ids.items():
            text = text.replace(session_id, placeholder)
        for root in sorted(roots, key=len, reverse=True):
            text = text.replace(root, roots[root])
        if (text.startswith("/") and len(text) > 1) or _HOST_PATH.search(text):
            raise ValueError(f"unnormalized absolute path at {where}: {text!r}")
        if _TIMESTAMP.search(text):
            raise ValueError(f"unnormalized timestamp at {where}: {text!r}")
        return text

    def walk(node: Any, where: str) -> Any:
        if isinstance(node, dict):
            out: dict[str, Any] = {}
            for key, item in node.items():
                path = f"{where}.{key}"
                if key in _VOLATILE_KEYS and item is not None:
                    out[key] = "<VOLATILE>"
                else:
                    out[key] = walk(item, path)
            return out
        if isinstance(node, list):
            return [walk(item, f"{where}[{index}]") for index, item in enumerate(node)]
        if isinstance(node, str):
            return norm_str(node, where)
        return node

    return walk(value, "$")


def canonical(value: Any) -> str:
    return json.dumps(value, indent=2, sort_keys=True, ensure_ascii=False) + "\n"


async def _call_cases(
    repo: Path,
    env: dict[str, str],
    cases: list[tuple[str, str, dict[str, Any]]],
) -> dict[str, Any]:
    cmd = _server_cmd(repo, "all")
    params = StdioServerParameters(command=cmd[0], args=cmd[1:], env=env, cwd=str(repo))
    results: dict[str, Any] = {}
    async with stdio_client(params) as (read, write), ClientSession(read, write) as session:
        await session.initialize()
        for case, tool, arguments in cases:
            result = await session.call_tool(tool, arguments)
            payload = _payload(result)
            if result.is_error:
                payload = {"_error": payload}
            results[case] = normalize(payload, repo, Path(env["HOME"]))
    return results


def _reduce_schema(schema: Mapping[str, Any]) -> dict[str, Any]:
    """Parameter names, primitive types, and required set; schema dialect aside."""
    props = schema.get("properties") or {}

    def kind(spec: Mapping[str, Any]) -> Any:
        if "type" in spec:
            return spec["type"]
        options = spec.get("anyOf") or spec.get("oneOf") or []
        types = sorted({str(kind(option)) for option in options if option.get("type") != "null"})
        return types[0] if len(types) == 1 else types

    return {
        "params": {name: kind(spec) for name, spec in sorted(props.items())},
        "required": sorted(schema.get("required") or []),
    }


async def _list_surface(repo: Path, env: dict[str, str], tools: str | None) -> dict[str, Any]:
    cmd = _server_cmd(repo, tools)
    params = StdioServerParameters(command=cmd[0], args=cmd[1:], env=env, cwd=str(repo))
    async with stdio_client(params) as (read, write), ClientSession(read, write) as session:
        await session.initialize()
        listed = await session.list_tools()
        prompts = await session.list_prompts()
    return {
        "tools": {tool.name: _reduce_schema(tool.input_schema) for tool in listed.tools},
        "prompts": {
            prompt.name: sorted((arg.name, bool(arg.required)) for arg in prompt.arguments or [])
            for prompt in prompts.prompts
        },
    }


#: Raw JSON-RPC requests after the handshake, with the case name each reply is
#: frozen under. They pin what the MCP layer itself answers (schemas,
#: descriptions, prompt texts, argument errors, unknown names), which the
#: tool payload snapshots do not cover.
_PROTOCOL_REQUESTS: list[tuple[str, str, dict[str, Any]]] = [
    ("tools_list", "tools/list", {}),
    ("prompts_list", "prompts/list", {}),
    *[
        (case, "prompts/get", {"name": name, "arguments": arguments})
        for case, name, arguments in [
            ("prompt_review_changes", "review_changes", {}),
            ("prompt_review_changes_base", "review_changes", {"base": "main"}),
            ("prompt_architecture_map", "architecture_map", {}),
            ("prompt_debug_issue", "debug_issue", {"description": "flaky login"}),
            ("prompt_onboard_developer", "onboard_developer", {}),
            ("prompt_pre_merge_check", "pre_merge_check", {}),
        ]
    ],
    ("prompt_unknown", "prompts/get", {"name": "no_such_prompt", "arguments": {}}),
    (
        "call_missing_argument",
        "tools/call",
        {"name": "query_graph_tool", "arguments": {"pattern": "callers_of"}},
    ),
    (
        "call_bad_type",
        "tools/call",
        {
            "name": "query_graph_tool",
            "arguments": {"pattern": "callers_of", "target": "x", "depth": "two"},
        },
    ),
    (
        "call_coerced_type",
        "tools/call",
        {
            "name": "query_graph_tool",
            "arguments": {"pattern": "callers_of", "target": "nope", "depth": "2"},
        },
    ),
    (
        "call_unknown_argument",
        "tools/call",
        {
            "name": "query_graph_tool",
            "arguments": {"pattern": "callers_of", "target": "x", "bogus": 1},
        },
    ),
    ("call_unknown_tool", "tools/call", {"name": "no_such_tool", "arguments": {}}),
    ("ping", "ping", {}),
    ("resources_list", "resources/list", {}),
    ("resource_templates_list", "resources/templates/list", {}),
    ("logging_set_level", "logging/setLevel", {"level": "info"}),
    ("unknown_method", "no/such_method", {}),
]


def _client_init(version: str) -> dict[str, Any]:
    return {
        "protocolVersion": version,
        "capabilities": {},
        "clientInfo": {"name": "mcp-snapshot", "version": "0"},
    }


def _session(
    repo: Path,
    env: dict[str, str],
    tools: str | None,
    requests: list[tuple[str, str, dict[str, Any]]],
    base_cmd: list[str] | None = None,
) -> dict[str, Any]:
    """Send *requests* (``(case, method, params)``) to one server process in
    order and return each reply by case, without ``id`` and ``jsonrpc``. A
    ``notifications/*`` method is sent as a notification and has no reply."""
    cmd = _server_cmd(repo, tools, base_cmd)
    proc = subprocess.Popen(  # noqa: S603 - fixed argv, no shell
        cmd,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        env=env,
        cwd=str(repo),
        text=True,
    )
    assert proc.stdin is not None and proc.stdout is not None
    replies: dict[str, Any] = {}
    try:
        for request_id, (case, method, params) in enumerate(requests):
            message: dict[str, Any] = {"jsonrpc": "2.0", "method": method}
            if not method.startswith("notifications/"):
                message.update({"id": request_id, "params": params})
            proc.stdin.write(json.dumps(message) + "\n")
            proc.stdin.flush()
            if "id" not in message:
                continue
            while True:
                line = proc.stdout.readline()
                if not line:
                    raise RuntimeError(f"server closed the stream before answering {method}")
                reply = json.loads(line)
                if reply.get("id") == request_id:
                    replies[case] = {
                        key: value for key, value in reply.items() if key not in ("id", "jsonrpc")
                    }
                    break
    finally:
        proc.stdin.close()
        proc.wait(timeout=30)
    for reply in replies.values():
        # The implementation's own version, not part of the contract.
        server_info = (reply.get("result") or {}).get("serverInfo")
        if isinstance(server_info, dict) and "version" in server_info:
            server_info["version"] = "<VOLATILE>"
    return normalize(replies, repo, Path(env["HOME"]))


def _protocol_exchange(repo: Path, env: dict[str, str], tools: str | None) -> dict[str, Any]:
    """Every reply to :data:`_PROTOCOL_REQUESTS`, keyed by case, as raw JSON-RPC."""
    return _session(
        repo,
        env,
        tools,
        [
            ("initialize", "initialize", _client_init("2025-06-18")),
            ("initialized", "notifications/initialized", {}),
            *_PROTOCOL_REQUESTS,
        ],
    )


def _handshake_variants(repo: Path, env: dict[str, str]) -> dict[str, Any]:
    """Version negotiation and a request before ``initialize``, one process each."""
    return {
        "initialize_2024_11_05": _session(
            repo, env, None, [("reply", "initialize", _client_init("2024-11-05"))]
        )["reply"],
        "initialize_unknown_version": _session(
            repo, env, None, [("reply", "initialize", _client_init("1999-01-01"))]
        )["reply"],
        "request_before_initialize": _session(repo, env, None, [("reply", "tools/list", {})])[
            "reply"
        ],
    }


SURFACE_PATH = REPO_ROOT / "dagayn" / "server" / "mcp_surface.json"


def surface_from_python_server() -> str:
    """What the stdio front end in ``dagayn._core`` answers without Python:
    the fastmcp server's ``initialize`` (minus the version and the negotiated
    protocol revision), its full tool and prompt listings, and the
    ``prompts/get`` replies the front end fills in (:func:`_prompt_replies`)."""
    with built_fixture("python_only") as (repo, env):
        replies = _session(
            repo,
            env,
            "all",
            [
                ("initialize", "initialize", _client_init("2025-11-25")),
                ("initialized", "notifications/initialized", {}),
                ("tools", "tools/list", {}),
                ("prompts", "prompts/list", {}),
            ],
            base_cmd=PYTHON_SERVER_CMD,
        )
        prompts = replies["prompts"]["result"]["prompts"]
        prompt_replies = _prompt_replies(repo, env, prompts)
    initialize = replies["initialize"]["result"]
    surface = {
        "initialize": {
            "capabilities": initialize["capabilities"],
            "instructions": initialize.get("instructions"),
            "serverInfo": {"name": initialize["serverInfo"]["name"]},
        },
        "tools": replies["tools"]["result"]["tools"],
        "prompts": prompts,
        "prompt_replies": prompt_replies,
    }
    return json.dumps(surface, indent=1, ensure_ascii=False) + "\n"


#: Stands for an argument's value in a recorded ``prompts/get`` reply; the
#: front end substitutes the caller's value for it.
PROMPT_ARGUMENT_PLACEHOLDER = "\u0000dagayn-prompt-argument\u0000"


def _prompt_replies(
    repo: Path, env: dict[str, str], prompts: list[dict[str, Any]]
) -> dict[str, Any]:
    """Each prompt's ``prompts/get`` result without arguments and, per
    argument, with it empty (a prompt may substitute its own default) and set
    to :data:`PROMPT_ARGUMENT_PLACEHOLDER`."""
    requests: list[tuple[str, str, dict[str, Any]]] = [
        ("initialize", "initialize", _client_init("2025-11-25")),
        ("initialized", "notifications/initialized", {}),
    ]
    for prompt in prompts:
        name = prompt["name"]
        requests.append((f"{name}", "prompts/get", {"name": name, "arguments": {}}))
        for argument in prompt.get("arguments") or []:
            arg = argument["name"]
            for variant, value in (("empty", ""), ("template", PROMPT_ARGUMENT_PLACEHOLDER)):
                requests.append(
                    (
                        f"{name}/{arg}/{variant}",
                        "prompts/get",
                        {"name": name, "arguments": {arg: value}},
                    )
                )
    replies = _session(repo, env, "all", requests, base_cmd=PYTHON_SERVER_CMD)
    out: dict[str, Any] = {}
    for prompt in prompts:
        name = prompt["name"]
        entry: dict[str, Any] = {"default": replies[name]["result"], "arguments": {}}
        for argument in prompt.get("arguments") or []:
            arg = argument["name"]
            entry["arguments"][arg] = {
                variant: replies[f"{name}/{arg}/{variant}"]["result"]
                for variant in ("empty", "template")
            }
        out[name] = entry
    return out


def snapshot_protocol() -> str:
    """Raw MCP-layer replies for the default and the full tool surface."""
    with built_fixture("python_only") as (repo, env):
        surface = {
            "default": _protocol_exchange(repo, env, None),
            "all": _protocol_exchange(repo, env, "all"),
            "handshake": _handshake_variants(repo, env),
        }
    return canonical(surface)


def fixture_cases(name: str) -> list[tuple[str, str, dict[str, Any]]]:
    return [*_COMMON_CASES, *FIXTURE_CASES[name]]


def _graph_metadata(db_path: Path) -> dict[str, str]:
    conn = sqlite3.connect(db_path)
    try:
        return dict(conn.execute("SELECT key, value FROM metadata").fetchall())
    finally:
        conn.close()


def snapshot_fixture(name: str) -> dict[str, str]:
    """Canonical JSON text per case for fixture *name*.

    Besides the tool responses, ``graph`` (every node and edge) and
    ``metadata`` freeze the database the build wrote: it is the contract
    between whatever ran ``build`` and whatever serves it.
    """
    with built_fixture(name) as (repo, env):
        db_path = repo / ".dagayn" / "graph.db"
        snapshots = {
            "graph": export_db(db_path, entity_lines=True),
            "metadata": canonical(normalize(_graph_metadata(db_path), repo)),
        }
        results = asyncio.run(_call_cases(repo, env, fixture_cases(name)))
    snapshots.update({case: canonical(payload) for case, payload in results.items()})
    return snapshots


def snapshot_tools_list() -> str:
    with built_fixture("python_only") as (repo, env):
        surface = {
            "default": asyncio.run(_list_surface(repo, env, None)),
            "all": asyncio.run(_list_surface(repo, env, "all")),
        }
    return canonical(surface)


def _write(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def regenerate(names: list[str]) -> None:
    for name in names:
        snapshots = snapshot_fixture(name)
        target = SNAPSHOT_DIR / name
        if target.exists():
            shutil.rmtree(target)
        for case, text in snapshots.items():
            _write(target / f"{case}.json", text)
        print(f"{name}: {len(snapshots)} snapshots")
    _write(SNAPSHOT_DIR / "tools_list.json", snapshot_tools_list())
    print("tools_list.json")
    _write(SNAPSHOT_DIR / "protocol.json", snapshot_protocol())
    print("protocol.json")
    _write(SURFACE_PATH, surface_from_python_server())
    print(SURFACE_PATH.relative_to(REPO_ROOT))


def check_determinism(names: list[str], report: Callable[[str], None] = print) -> bool:
    ok = True
    for name in names:
        first = snapshot_fixture(name)
        second = snapshot_fixture(name)
        for case in first:
            if first[case] != second[case]:
                ok = False
                report(f"{name}/{case}: differs between two fresh builds")
    return ok


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n", 1)[0])
    parser.add_argument("fixtures", nargs="*", help="fixtures (default: all)")
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--regenerate", action="store_true")
    group.add_argument("--check-determinism", action="store_true")
    args = parser.parse_args()
    names = args.fixtures or list(FIXTURE_CASES)
    unknown = sorted(set(names) - set(FIXTURE_CASES))
    if unknown:
        parser.error(f"unknown fixtures: {', '.join(unknown)}")
    if args.regenerate:
        regenerate(names)
        return 0
    return 0 if check_determinism(names) else 1


if __name__ == "__main__":
    raise SystemExit(main())
