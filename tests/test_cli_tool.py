from __future__ import annotations

import argparse
import json

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
            "get_impact_radius_tool",
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
    assert args.tool_name == "get_impact_radius_tool"
    assert args.repo == "/tmp/repo"
    assert args.json_args == '{"changed_files": ["app.py"]}'
    assert args.arg == ["max_depth=3", 'detail_level="minimal"']


def test_tool_kwargs_parse_json_values():
    args = _parser().parse_args(
        [
            "tool",
            "list_flows_tool",
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
            "get_hub_nodes_tool",
            "--repo",
            "/tmp/repo",
            "--arg",
            "top_n=3",
        ]
    )

    tool.handle(args)

    assert calls == [{"repo_root": "/tmp/repo", "top_n": 3}]
    assert json.loads(capsys.readouterr().out)["summary"] == "done"


def test_handle_summary_format(monkeypatch, capsys):
    monkeypatch.setattr(
        tool,
        "_load_tool",
        lambda name: lambda **kwargs: {"status": "ok", "summary": "short"},
    )
    args = _parser().parse_args(["tool", "list_flows_tool", "--format", "summary"])

    tool.handle(args)

    assert capsys.readouterr().out == "short\n"


def test_handle_requires_tool_name(capsys):
    args = _parser().parse_args(["tool"])

    with pytest.raises(SystemExit) as exc:
        tool.handle(args)

    assert exc.value.code == 2
    assert "Specify a tool name" in capsys.readouterr().err
