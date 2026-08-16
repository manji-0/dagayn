from __future__ import annotations

import argparse
import json
import os
from types import SimpleNamespace

import pytest

from dagayn.cli.commands import tool


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="command")
    tool.register_command(sub)
    return parser


def test_tool_parser_accepts_json_args_and_key_value_args():
    args = _parser().parse_args(
        [
            "tool",
            "review_tool",
            "--repo",
            "/tmp/repo",
            "--json-args",
            '{"changed_files": ["app.py"]}',
            "--arg",
            "max_depth=3",
            "--arg",
            'detail_level="minimal"',
        ]
    )

    assert args.command == "tool"
    assert args.tool_name == "review_tool"
    assert args.repo == "/tmp/repo"
    assert args.json_args == '{"changed_files": ["app.py"]}'
    assert args.arg == ["max_depth=3", 'detail_level="minimal"']


def test_tool_kwargs_parse_json_values():
    args = _parser().parse_args(
        [
            "tool",
            "flow_tool",
            "--arg",
            "limit=5",
            "--arg",
            "include_source=true",
            "--arg",
            "name=flow",
        ]
    )

    assert tool._tool_kwargs(args) == {
        "limit": 5,
        "include_source": True,
        "name": "flow",
    }


def test_handle_runs_tool_and_injects_repo_root(monkeypatch, capsys):
    calls: list[dict] = []

    def fake_tool(*, repo_root=None, top_n=10):
        calls.append({"repo_root": repo_root, "top_n": top_n})
        return {"status": "ok", "summary": "done", "top_n": top_n}

    monkeypatch.setattr(tool, "_load_tool", lambda name: fake_tool)
    args = _parser().parse_args(
        [
            "tool",
            "architecture_analysis_tool",
            "--repo",
            "/tmp/repo",
            "--arg",
            "top_n=3",
        ]
    )

    tool.handle(args)

    assert calls == [{"repo_root": "/tmp/repo", "top_n": 3}]
    assert json.loads(capsys.readouterr().out)["summary"] == "done"


def test_handle_starts_persisted_local_embedding_for_semantic_tool(monkeypatch, capsys):
    calls: list[dict] = []
    server_calls: list[dict] = []

    def fake_tool(*, repo_root=None):
        calls.append(
            {
                "repo_root": repo_root,
                "api_key": os.environ.get("CRG_OPENAI_API_KEY"),
                "base_url": os.environ.get("CRG_OPENAI_BASE_URL"),
                "max_length": os.environ.get("CRG_OPENAI_MAX_LENGTH"),
                "model": os.environ.get("CRG_OPENAI_MODEL"),
            }
        )
        return {"status": "ok", "summary": "done"}

    class FakeServer:
        base_url = "http://127.0.0.1:19090/v1"
        preset = SimpleNamespace(
            model="qwen3-embedding-0.6b-gguf-q8_0",
            request_max_length=None,
        )

    class FakeContext:
        def __enter__(self):
            return FakeServer()

        def __exit__(self, *_args):
            return None

    def fake_server(level, **kwargs):
        server_calls.append({"level": level, **kwargs})
        return FakeContext()

    monkeypatch.setattr(tool, "_load_tool", lambda name: fake_tool)
    monkeypatch.setattr(
        "dagayn.cli.commands.serve._infer_persisted_local_embedding",
        lambda repo_root: SimpleNamespace(
            level="low",
            runtime="llama",
            model="qwen3-embedding-0.6b-gguf-q8_0",
            base_url="http://127.0.0.1:19090/v1",
            port=19090,
        ),
    )
    monkeypatch.setattr("dagayn.local_embeddings.local_embedding_server", fake_server)
    monkeypatch.delenv("CRG_OPENAI_API_KEY", raising=False)
    monkeypatch.delenv("CRG_OPENAI_BASE_URL", raising=False)
    monkeypatch.delenv("CRG_OPENAI_MODEL", raising=False)

    args = _parser().parse_args(["tool", "semantic_search_nodes_tool", "--repo", "/tmp/repo"])
    tool.handle(args)

    assert server_calls == [{"level": "low", "runtime": "llama", "port": 19090}]
    assert calls == [
        {
            "repo_root": "/tmp/repo",
            "api_key": "dagayn-local",
            "base_url": "http://127.0.0.1:19090/v1",
            "max_length": None,
            "model": "qwen3-embedding-0.6b-gguf-q8_0",
        }
    ]
    assert json.loads(capsys.readouterr().out)["status"] == "ok"
    assert os.environ.get("CRG_OPENAI_API_KEY") is None


def test_handle_skips_local_embedding_for_explicit_cloud_provider(monkeypatch):
    calls: list[dict] = []

    def fake_tool(**kwargs):
        calls.append(kwargs)
        return {"status": "ok"}

    monkeypatch.setattr(tool, "_load_tool", lambda name: fake_tool)
    monkeypatch.setattr(
        "dagayn.cli.commands.serve._infer_persisted_local_embedding",
        lambda repo_root: (_ for _ in ()).throw(
            AssertionError("should not inspect persisted local embeddings")
        ),
    )

    args = _parser().parse_args(
        [
            "tool",
            "semantic_search_nodes_tool",
            "--arg",
            'provider="google"',
        ]
    )
    tool.handle(args)

    assert calls == [{"provider": "google"}]


def test_handle_rejects_unknown_kwarg_with_accepted_names(monkeypatch, capsys):
    def fake_tool(pattern, target, repo_root=None):
        return {"status": "ok"}

    monkeypatch.setattr(tool, "_load_tool", lambda name: fake_tool)

    args = _parser().parse_args(
        ["tool", "query_graph_tool", "--arg", "pattern=x", "--arg", "target=y", "--arg", "bogus=1"]
    )
    with pytest.raises(SystemExit) as excinfo:
        tool.handle(args)
    assert excinfo.value.code == 2
    err = capsys.readouterr().err
    assert "unknown argument(s) bogus" in err
    assert "accepted: pattern, repo_root, target" in err


def test_handle_allows_kwargs_only_tool(monkeypatch):
    calls: list[dict] = []

    def fake_tool(**kwargs):
        calls.append(kwargs)
        return {"status": "ok"}

    monkeypatch.setattr(tool, "_load_tool", lambda name: fake_tool)

    args = _parser().parse_args(["tool", "semantic_search_nodes_tool", "--arg", "provider=google"])
    tool.handle(args)

    assert calls == [{"provider": "google"}]


def test_handle_caches_local_embedding_start_failure(monkeypatch, capsys):
    from dagayn.search import _emb_failure_cache

    calls: list[dict] = []
    provider_name = "openai:qwen3-embedding-0.6b-gguf-q8_0@http://127.0.0.1:19090/v1"

    def fake_tool(**kwargs):
        calls.append(kwargs)
        return {"status": "ok"}

    monkeypatch.setattr(tool, "_load_tool", lambda name: fake_tool)
    monkeypatch.setattr(
        "dagayn.cli.commands.serve._infer_persisted_local_embedding",
        lambda repo_root: SimpleNamespace(
            level="low",
            runtime="llama",
            model="qwen3-embedding-0.6b-gguf-q8_0",
            base_url="http://127.0.0.1:19090/v1",
            port=19090,
        ),
    )
    monkeypatch.setattr(
        "dagayn.local_embeddings.local_embedding_server",
        lambda *_args, **_kwargs: (_ for _ in ()).throw(RuntimeError("missing binary")),
    )
    _emb_failure_cache.clear()

    args = _parser().parse_args(["tool", "semantic_search_nodes_tool"])
    tool.handle(args)

    assert calls == [{}]
    assert provider_name in _emb_failure_cache
    assert "missing binary" in capsys.readouterr().err


def test_tool_registry_uses_architecture_dispatcher_only():
    removed = {
        "get_architecture_overview_tool",
        "list_communities_tool",
        "get_community_tool",
        "get_hub_nodes_tool",
        "get_bridge_nodes_tool",
        "get_knowledge_gaps_tool",
        "get_surprising_connections_tool",
        "detect_adp_violations_tool",
        "compute_sdp_metrics_tool",
        "detect_sdp_violations_tool",
        "compute_sap_metrics_tool",
        "detect_sap_violations_tool",
    }

    assert "architecture_analysis_tool" in tool.TOOL_REGISTRY
    assert tool.TOOL_ALIASES["architecture_analysis"] == "architecture_analysis_tool"
    assert set(tool.TOOL_REGISTRY).isdisjoint(removed)


def test_tool_registry_uses_review_and_flow_dispatchers_only():
    removed = {
        "detect_changes_tool",
        "get_review_context_tool",
        "get_affected_flows_tool",
        "get_impact_radius_tool",
        "list_flows_tool",
        "get_flow_tool",
    }

    assert "review_tool" in tool.TOOL_REGISTRY
    assert "flow_tool" in tool.TOOL_REGISTRY
    assert tool.TOOL_ALIASES["review"] == "review_tool"
    assert tool.TOOL_ALIASES["flow"] == "flow_tool"
    assert set(tool.TOOL_REGISTRY).isdisjoint(removed)


def test_handle_summary_format(monkeypatch, capsys):
    monkeypatch.setattr(
        tool,
        "_load_tool",
        lambda name: lambda **kwargs: {"status": "ok", "summary": "short"},
    )
    args = _parser().parse_args(["tool", "flow_tool", "--format", "summary"])

    tool.handle(args)

    assert capsys.readouterr().out == "short\n"


def test_handle_requires_tool_name(capsys):
    args = _parser().parse_args(["tool"])

    with pytest.raises(SystemExit) as exc:
        tool.handle(args)

    assert exc.value.code == 2
    assert "Specify a tool name" in capsys.readouterr().err
