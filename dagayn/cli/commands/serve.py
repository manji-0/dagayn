"""serve command — argument registration and handler."""

from __future__ import annotations

import argparse


def register_command(sub: argparse._SubParsersAction) -> argparse.ArgumentParser:
    """Register the serve subcommand. Returns the subparser."""
    serve_cmd = sub.add_parser(
        "serve",
        help="Start MCP server (stdio by default, or HTTP on localhost with --http)",
    )
    serve_cmd.add_argument("--repo", default=None, help="Repository root (auto-detected)")
    serve_cmd.add_argument(
        "--tools",
        default=None,
        help=(
            "Comma-separated list of tool names to expose "
            "(e.g. query_graph_tool,semantic_search_nodes_tool). "
            "Unlisted tools are removed. Falls back to CRG_TOOLS env var. "
            "When unset, all tools are available."
        ),
    )
    serve_cmd.add_argument(
        "--http",
        action="store_true",
        help="Listen for MCP over Streamable HTTP on localhost (default port 5555)",
    )
    serve_cmd.add_argument(
        "--host",
        default=None,
        metavar="ADDR",
        help="Bind address for --http (default: 127.0.0.1)",
    )
    serve_cmd.add_argument(
        "--port",
        type=int,
        default=None,
        metavar="PORT",
        help="Port for --http (default: 5555)",
    )
    return serve_cmd


def handle(args: argparse.Namespace, serve_parser: argparse.ArgumentParser) -> None:
    """Start the MCP server."""
    from ...main import main as serve_main

    if args.port is not None and not args.http:
        serve_parser.error("--port requires --http")
    if args.host is not None and not args.http:
        serve_parser.error("--host requires --http")
    if args.http:
        host = args.host if args.host is not None else "127.0.0.1"
        port = args.port if args.port is not None else 5555
        serve_main(
            repo_root=args.repo,
            transport="streamable-http",
            host=host,
            port=port,
            tools=args.tools,
        )
    else:
        serve_main(repo_root=args.repo, tools=args.tools)
