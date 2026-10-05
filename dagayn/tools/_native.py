"""Call a tool's Rust implementation (``dagayn_tools`` through ``_core``).

The Python tool bodies that defer to Rust open the graph with ``_get_store``
first, which resolves the repository and creates, migrates, or waits for the
graph, then call :func:`native_tool` with the arguments they were given.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path
from typing import Any

from ..runtime_identity import runtime_summary
from ..tool_surface import exposed_tool_names

#: The parent of the package, which holds the packaged ``docs/``.
_PACKAGE_ROOT = Path(__file__).resolve().parent.parent.parent


def _server_default(name: str) -> Any:
    """A ``dagayn serve`` default from :mod:`dagayn.server.main`, when it runs."""
    main = sys.modules.get("dagayn.server.main")
    return getattr(main, name, None) if main is not None else None


def native_tool(name: str, **arguments: Any) -> dict[str, Any]:
    """``name(**arguments)`` answered by Rust; omitted (``None``) arguments are
    left out, as an MCP client leaves them out. Raises ``RuntimeError`` when the
    Rust tool leaves the call to Python, which its callers no longer have."""
    from .. import _core

    allowed = exposed_tool_names()
    text = _core.call_tool(
        name,
        json.dumps({key: value for key, value in arguments.items() if value is not None}),
        allowed_tools=sorted(allowed) if allowed is not None else None,
        package_root=str(_PACKAGE_ROOT),
        local_embedding=_server_default("_default_local_embedding"),
        embedding_provider=_server_default("_default_embedding_provider"),
        embedding_model=_server_default("_default_embedding_model"),
        runtime=json.dumps(runtime_summary()),
    )
    if text is None:
        raise RuntimeError(f"{name} has no answer for these arguments")
    return json.loads(text)
