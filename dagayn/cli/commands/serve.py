"""serve command — argument registration and handler."""

from __future__ import annotations

import argparse
import sqlite3
from pathlib import Path

from ._shared import _add_local_embedding_args

_REMOTE_EMBEDDING_CHOICES = ["none", "openai", "google", "minimax"]


def _embedding_provider_counts(db_path: Path) -> dict[str, int]:
    try:
        conn = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
        try:
            rows = conn.execute(
                "SELECT provider, COUNT(*) FROM embeddings GROUP BY provider"
            ).fetchall()
        finally:
            conn.close()
    except sqlite3.Error:
        return {}
    return {str(provider): int(count) for provider, count in rows}


def _infer_persisted_local_embedding(repo_root: str | None):
    if repo_root is None:
        from ...incremental import find_project_root

        root = find_project_root()
    else:
        root = Path(repo_root).expanduser().resolve()

    from ...incremental import get_db_path
    from ...local_embeddings import infer_local_embedding_provider

    db_path = get_db_path(root)
    provider_counts = _embedding_provider_counts(db_path)
    if len(provider_counts) != 1:
        return None
    provider_name = next(iter(provider_counts))
    return infer_local_embedding_provider(provider_name)


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
            "Unlisted tools are removed; use 'all' for every tool. "
            "Falls back to CRG_TOOLS env var."
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
    effective_local_embedding_port = args.local_embedding_port

    def _run(
        *,
        embedding_provider: str | None = remote_embedding,
        embedding_model: str | None = None,
        local_embedding_default: str | None = None,
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
                embedding_provider=embedding_provider,
                embedding_model=embedding_model,
                local_embedding=local_embedding_default,
                local_embedding_port=effective_local_embedding_port,
                local_embedding_bin=args.local_embedding_bin,
                keep_local_embedding_server=args.keep_local_embedding_server,
                local_embedding_timeout=args.local_embedding_timeout,
                local_embedding_request_timeout=args.local_embedding_request_timeout,
                local_embedding_batch_size=args.local_embedding_batch_size,
            )
        else:
            serve_main(
                repo_root=args.repo,
                tools=args.tools,
                embedding_provider=embedding_provider,
                embedding_model=embedding_model,
                local_embedding=local_embedding_default,
                local_embedding_port=effective_local_embedding_port,
                local_embedding_bin=args.local_embedding_bin,
                keep_local_embedding_server=args.keep_local_embedding_server,
                local_embedding_timeout=args.local_embedding_timeout,
                local_embedding_request_timeout=args.local_embedding_request_timeout,
                local_embedding_batch_size=args.local_embedding_batch_size,
            )

    inferred_local_embedding = None
    local_embedding = args.local_embedding
    if (not local_embedding or local_embedding == "none") and remote_embedding is None:
        inferred_local_embedding = _infer_persisted_local_embedding(args.repo)
        if inferred_local_embedding is not None:
            local_embedding = inferred_local_embedding.level
            effective_local_embedding_port = inferred_local_embedding.port

    if local_embedding and local_embedding != "none":
        from ...local_embeddings import local_embedding_server

        with local_embedding_server(
            local_embedding,
            port=effective_local_embedding_port,
            binary=args.local_embedding_bin,
            keep_running=args.keep_local_embedding_server,
            startup_timeout=args.local_embedding_timeout,
        ) as server:
            os.environ["CRG_OPENAI_API_KEY"] = "dagayn-local"
            os.environ["CRG_OPENAI_BASE_URL"] = server.base_url
            os.environ["CRG_OPENAI_MODEL"] = server.preset.model
            os.environ["CRG_OPENAI_BATCH_SIZE"] = str(args.local_embedding_batch_size)
            os.environ["CRG_OPENAI_TIMEOUT"] = str(args.local_embedding_request_timeout)
            _run(
                embedding_provider="openai",
                embedding_model=server.preset.model,
                local_embedding_default=local_embedding,
            )
    else:
        _run()
