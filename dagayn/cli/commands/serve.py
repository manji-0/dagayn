"""serve command — argument registration and handler."""

from __future__ import annotations

import argparse

from ...tool_profiles import DEFAULT_TOOL_PROFILE, FULL_TOOL_PROFILE, TOOL_PROFILE_NAMES
from ._shared import _add_local_embedding_args

_REMOTE_EMBEDDING_CHOICES = ["none", "openai", "google", "minimax"]


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
            "Unlisted tools are removed. Overrides any tool profile. "
            "Falls back to CRG_TOOLS env var."
        ),
    )
    serve_cmd.add_argument(
        "--tool-profile",
        choices=TOOL_PROFILE_NAMES,
        default=None,
        help=(
            f"Named MCP tool profile (default: {DEFAULT_TOOL_PROFILE}; "
            f"use {FULL_TOOL_PROFILE} for all tools)."
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
    _add_local_embedding_args(serve_cmd)
    serve_cmd.add_argument(
        "--remote-embedding",
        choices=_REMOTE_EMBEDDING_CHOICES,
        default="none",
        help=(
            "Use this remote embedding provider by default for MCP semantic search "
            "(default: infer from environment when exactly one provider is configured)"
        ),
    )
    return serve_cmd


def handle(args: argparse.Namespace, serve_parser: argparse.ArgumentParser) -> None:
    """Start the MCP server, optionally managing a local embedding server."""
    import os

    from ...main import main as serve_main

    if args.port is not None and not args.http:
        serve_parser.error("--port requires --http")
    if args.host is not None and not args.http:
        serve_parser.error("--host requires --http")
    if args.local_embedding != "none" and args.remote_embedding != "none":
        serve_parser.error("--local-embedding and --remote-embedding are mutually exclusive")

    remote_embedding = args.remote_embedding if args.remote_embedding != "none" else None

    def _run(
        *,
        embedding_provider: str | None = remote_embedding,
        embedding_model: str | None = None,
    ) -> None:
        if args.http:
            host = args.host if args.host is not None else "127.0.0.1"
            port = args.port if args.port is not None else 5555
            serve_main(
                repo_root=args.repo,
                transport="streamable-http",
                host=host,
                port=port,
                tools=args.tools,
                tool_profile=args.tool_profile,
                embedding_provider=embedding_provider,
                embedding_model=embedding_model,
            )
        else:
            serve_main(
                repo_root=args.repo,
                tools=args.tools,
                tool_profile=args.tool_profile,
                embedding_provider=embedding_provider,
                embedding_model=embedding_model,
            )

    local_embedding = args.local_embedding
    if local_embedding and local_embedding != "none":
        from ...local_embeddings import local_embedding_server

        with local_embedding_server(
            local_embedding,
            port=args.local_embedding_port,
            binary=args.local_embedding_bin,
            keep_running=args.keep_local_embedding_server,
            startup_timeout=args.local_embedding_timeout,
        ) as server:
            os.environ["CRG_OPENAI_API_KEY"] = "dagayn-local"
            os.environ["CRG_OPENAI_BASE_URL"] = server.base_url
            os.environ["CRG_OPENAI_MODEL"] = server.preset.model
            os.environ["CRG_OPENAI_BATCH_SIZE"] = str(args.local_embedding_batch_size)
            os.environ["CRG_OPENAI_TIMEOUT"] = str(args.local_embedding_request_timeout)
            _run(embedding_provider="openai", embedding_model=server.preset.model)
    else:
        _run()
