"""``dagayn serve`` over stdio with the Rust front end in ``dagayn._core``.

The front end answers ``initialize``, ``ping``, and the tool, prompt, and
resource listings from ``mcp_surface.json``, recorded from the fastmcp server
(``tools/mcp_snapshot.py --regenerate`` rewrites it, and
``tests/test_mcp_snapshots.py`` checks it), so a session starts without
importing fastmcp or the tools. Calls ``dagayn-tools`` answers in Rust
(``list_graph_stats_tool``, ``get_docs_section_tool``,
``get_minimal_context_tool``, most of ``query_graph_tool``,
``semantic_search_nodes_tool`` without embeddings, and ``review_tool``
``affected_flows``) never reach Python; the
first message it does not answer itself boots the fastmcp server of
:mod:`dagayn.server.main` in this process on a pipe pair
(:func:`dagayn.server.main.serve_on_fds`) and relays the session to it.
"""

from __future__ import annotations

import json
import os
import sys
import threading
from pathlib import Path
from typing import Any

SURFACE_PATH = Path(__file__).with_name("mcp_surface.json")


def serve_stdio(**config: Any) -> None:
    """Serve one stdio session; *config* is :func:`dagayn.server.main.configure`'s.

    Returns when stdin reaches EOF and the fastmcp server, if it was booted,
    has finished, so a local embedding sidecar around the call is stopped
    normally.
    """
    from .. import __version__
    from .._core import serve_mcp
    from ..runtime_identity import runtime_summary
    from .tool_allowlist import _resolve_tool_allow_list

    allowed = _resolve_tool_allow_list(tools=config.get("tools"))
    surface = SURFACE_PATH.read_text(encoding="utf-8")

    def boot(read_fd: int, write_fd: int) -> None:
        if os.environ.get("DAGAYN_MCP_TRACE"):
            print("dagayn: starting the Python MCP server", file=sys.stderr, flush=True)
        from . import main

        main.configure(**config)

        def serve() -> None:
            try:
                main.serve_on_fds(read_fd, write_fd)
            except BaseException:  # noqa: BLE001 - nothing else can report it
                # A request in flight would never get its reply, and the
                # client would wait on it; end the session as a crashed
                # fastmcp loop would.
                import traceback

                traceback.print_exc()
                os._exit(1)

        threading.Thread(target=serve, name="dagayn-mcp-backend", daemon=True).start()

    failures: list[BaseException] = []

    def run() -> None:
        try:
            serve_mcp(
                surface,
                sorted(allowed) if allowed is not None else None,
                __version__,
                boot,
                pinned_repo=config.get("repo_root"),
                # The parent of the package, which holds the packaged `docs/`
                # (`dagayn.tools.docs` looks there too).
                package_root=str(Path(__file__).resolve().parent.parent.parent),
                # get_minimal_context decides a local embedding refresh with it.
                local_embedding=config.get("local_embedding"),
                # Search embeds the query with these; the environment's
                # provider is checked in Rust as `get_provider` checks it.
                embedding_provider=config.get("embedding_provider"),
                embedding_model=config.get("embedding_model"),
                # `_runtime`, as the Python tools attach it.
                runtime=json.dumps(runtime_summary()),
            )
        except BaseException as exc:  # noqa: BLE001 - re-raised on the main thread
            failures.append(exc)

    # The main thread waits in short joins so Ctrl-C still raises here and
    # unwinds whatever the caller wrapped around the server.
    server = threading.Thread(target=run, name="dagayn-mcp-frontend", daemon=True)
    server.start()
    while server.is_alive():
        server.join(0.5)
    if failures:
        raise failures[0]
