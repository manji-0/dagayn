"""tool command - run MCP tool implementations from the CLI."""

from __future__ import annotations

import argparse
import inspect
import json
import os
import sys
import time
from collections.abc import Callable
from contextlib import contextmanager, nullcontext
from importlib import import_module
from typing import Any

TOOL_REGISTRY: dict[str, str] = {
    "apply_refactor_tool": "dagayn.tools.refactor_tools:apply_refactor_func",
    "architecture_analysis_tool": "dagayn.tools.architecture_analysis:architecture_analysis_func",
    "build_or_update_graph_tool": "dagayn.tools.build:build_or_update_graph",
    "cross_repo_search_tool": "dagayn.tools.registry_tools:cross_repo_search_func",
    "embed_graph_tool": "dagayn.tools.docs:embed_graph",
    "find_large_functions_tool": "dagayn.tools.query:find_large_functions",
    "flow_tool": "dagayn.tools.flow_dispatcher:flow_func",
    "generate_wiki_tool": "dagayn.tools.docs:generate_wiki_func",
    "get_docs_section_tool": "dagayn.tools.docs:get_docs_section",
    "get_minimal_context_tool": "dagayn.tools.context:get_minimal_context",
    "get_suggested_questions_tool": "dagayn.tools.analysis_tools:get_suggested_questions_func",
    "get_wiki_page_tool": "dagayn.tools.docs:get_wiki_page_func",
    "list_graph_stats_tool": "dagayn.tools.query:list_graph_stats",
    "list_repos_tool": "dagayn.tools.registry_tools:list_repos_func",
    "query_graph_tool": "dagayn.tools.query:query_graph",
    "refactor_tool": "dagayn.tools.refactor_tools:refactor_func",
    "review_tool": "dagayn.tools.review_dispatcher:review_func",
    "run_postprocess_tool": "dagayn.tools.build:run_postprocess",
    "semantic_search_nodes_tool": "dagayn.tools.query:semantic_search_nodes",
    "traverse_graph_tool": "dagayn.tools.query:traverse_graph_func",
}

TOOL_ALIASES: dict[str, str] = {
    name.removesuffix("_tool"): name
    for name in TOOL_REGISTRY
    if name.endswith("_tool") and name != "refactor_tool"
}
TOOL_ALIASES["refactor"] = "refactor_tool"
TOOL_ALIASES["architecture_analysis"] = "architecture_analysis_tool"
TOOL_ALIASES["review"] = "review_tool"
TOOL_ALIASES["flow"] = "flow_tool"

_LOCAL_EMBEDDING_AWARE_TOOLS = {
    "semantic_search_nodes_tool",
    "traverse_graph_tool",
}


def register_command(sub: argparse._SubParsersAction) -> argparse.ArgumentParser:
    """Register the ``dagayn tool`` subcommand."""
    parser = sub.add_parser(
        "tool",
        help="Run a dagayn MCP tool implementation from the CLI.",
        description=(
            "Invoke any dagayn MCP tool by name without changing or restarting "
            "a running MCP server. Arguments are passed as JSON-compatible "
            "key/value pairs."
        ),
    )
    parser.add_argument("tool_name", nargs="?", help="MCP tool name, e.g. review_tool")
    parser.add_argument("--repo", default=None, help="Repository root passed as repo_root")
    parser.add_argument("--list", action="store_true", help="List available tool names and exit")
    parser.add_argument(
        "--arg",
        action="append",
        default=[],
        metavar="KEY=VALUE",
        help=(
            "Tool argument. VALUE is parsed as JSON when possible, so use "
            "--arg top_n=5, --arg include_source=true, or "
            "--arg 'changed_files=[\"src/app.py\"]'. Repeat for multiple args."
        ),
    )
    parser.add_argument(
        "--json-args",
        default=None,
        metavar="JSON",
        help="JSON object of tool arguments, e.g. '{\"top_n\": 5}'.",
    )
    parser.add_argument(
        "--format",
        choices=["json", "summary"],
        default="json",
        help="Output format (default: json).",
    )
    return parser


def _canonical_tool_name(name: str) -> str:
    if name in TOOL_REGISTRY:
        return name
    return TOOL_ALIASES.get(name, name)


def _load_tool(name: str) -> Callable[..., Any]:
    canonical = _canonical_tool_name(name)
    target = TOOL_REGISTRY.get(canonical)
    if target is None:
        known = ", ".join(sorted(TOOL_REGISTRY))
        raise KeyError(f"unknown tool {name!r}; expected one of: {known}")
    module_name, function_name = target.split(":", 1)
    module = import_module(module_name)
    func = getattr(module, function_name)
    if not callable(func):
        raise TypeError(f"{target} is not callable")
    return func


def _parse_value(raw: str) -> Any:
    try:
        return json.loads(raw)
    except json.JSONDecodeError:
        return raw


def _parse_arg_pair(raw: str) -> tuple[str, Any]:
    key, sep, value = raw.partition("=")
    if not sep or not key.strip():
        raise ValueError(f"expected KEY=VALUE, got {raw!r}")
    return key.strip(), _parse_value(value.strip())


def _tool_kwargs(args: argparse.Namespace) -> dict[str, Any]:
    kwargs: dict[str, Any] = {}
    if args.json_args:
        try:
            decoded = json.loads(args.json_args)
        except json.JSONDecodeError as exc:
            raise ValueError(f"--json-args must be a JSON object: {exc}") from exc
        if not isinstance(decoded, dict):
            raise ValueError("--json-args must be a JSON object")
        kwargs.update(decoded)
    for raw in args.arg:
        key, value = _parse_arg_pair(raw)
        kwargs[key] = value
    return kwargs


def _inject_repo_arg(func: Callable[..., Any], kwargs: dict[str, Any], repo: str | None) -> None:
    if not repo or "repo_root" in kwargs:
        return
    try:
        params = inspect.signature(func).parameters
    except (TypeError, ValueError):
        return
    if "repo_root" in params:
        kwargs["repo_root"] = repo


@contextmanager
def _maybe_local_embedding_server(tool_name: str, kwargs: dict[str, Any]):
    """Ensure persisted localhost embeddings are queryable for CLI tool calls."""
    canonical = _canonical_tool_name(tool_name)
    if canonical not in _LOCAL_EMBEDDING_AWARE_TOOLS:
        with nullcontext():
            yield
        return

    provider = kwargs.get("provider")
    if provider not in (None, "openai"):
        with nullcontext():
            yield
        return

    from .serve import _infer_persisted_local_embedding

    inferred = _infer_persisted_local_embedding(kwargs.get("repo_root"))
    if inferred is None:
        with nullcontext():
            yield
        return

    model = kwargs.get("model")
    if model is not None and model != inferred.model:
        with nullcontext():
            yield
        return

    from ...local_embeddings import local_embedding_server

    old_env = {
        key: os.environ.get(key)
        for key in (
            "CRG_OPENAI_API_KEY",
            "CRG_OPENAI_BASE_URL",
            "CRG_OPENAI_MAX_LENGTH",
            "CRG_OPENAI_MODEL",
        )
    }
    try:
        with local_embedding_server(
            inferred.level,
            runtime=inferred.runtime,
            port=inferred.port,
        ) as server:
            os.environ["CRG_OPENAI_API_KEY"] = "dagayn-local"
            os.environ["CRG_OPENAI_BASE_URL"] = server.base_url
            os.environ["CRG_OPENAI_MODEL"] = server.preset.model
            if server.preset.request_max_length is None:
                os.environ.pop("CRG_OPENAI_MAX_LENGTH", None)
            else:
                os.environ["CRG_OPENAI_MAX_LENGTH"] = str(server.preset.request_max_length)
            yield
    except RuntimeError as exc:
        print(f"dagayn tool: local embedding server unavailable: {exc}", file=sys.stderr)
        try:
            from ...search import _emb_failure_cache

            provider_name = f"openai:{inferred.model}@{inferred.base_url}"
            _emb_failure_cache[provider_name] = (time.monotonic(), str(exc))
        except Exception:  # noqa: BLE001  # nosec B110
            pass
        yield
    finally:
        for key, value in old_env.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value


def handle(args: argparse.Namespace) -> None:
    """Run the selected tool and print its result."""
    if args.list:
        for name in sorted(TOOL_REGISTRY):
            print(name)
        return

    if not args.tool_name:
        print("Specify a tool name, or use `dagayn tool --list`.", file=sys.stderr)
        sys.exit(2)

    try:
        func = _load_tool(args.tool_name)
        kwargs = _tool_kwargs(args)
        _inject_repo_arg(func, kwargs, args.repo)
        with _maybe_local_embedding_server(args.tool_name, kwargs):
            result = func(**kwargs)
            if inspect.isawaitable(result):
                import asyncio

                result = asyncio.run(result)
    except (KeyError, TypeError, ValueError) as exc:
        print(f"dagayn tool: {exc}", file=sys.stderr)
        sys.exit(2)

    if args.format == "summary" and isinstance(result, dict):
        print(result.get("summary", json.dumps(result, indent=2, default=str)))
    else:
        print(json.dumps(result, indent=2, default=str))
