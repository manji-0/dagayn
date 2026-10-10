"""Envelope helpers shared by the review, flow, and architecture dispatchers."""

from __future__ import annotations

from typing import cast

from ..contracts.state_types import seal_dispatcher_error, seal_dispatcher_ok
from ._common import ToolPayload, attach_answerability


def with_dispatch_metadata(
    result: ToolPayload,
    *,
    summary_label: str,
    mode: str,
    called_subtool: str,
    repo_root: str | None,
) -> ToolPayload:
    """Add dispatcher metadata without mutating the subtool response."""
    payload = dict(result)
    payload.setdefault("status", "ok")
    payload.setdefault("summary", f"{summary_label} mode {mode!r} completed.")
    payload["mode"] = mode
    payload["called_subtool"] = called_subtool
    attach_answerability(payload, repo_root)
    if payload.get("status") == "error":
        payload.setdefault("error", payload["summary"])
        return cast(ToolPayload, seal_dispatcher_error(payload))
    return cast(ToolPayload, seal_dispatcher_ok(payload))


def dispatch_error(message: str, *, mode: str, repo_root: str | None) -> ToolPayload:
    return cast(
        ToolPayload,
        seal_dispatcher_error(
            attach_answerability(
                {
                    "status": "error",
                    "summary": message,
                    "error": message,
                    "mode": mode,
                    "called_subtool": None,
                },
                repo_root,
            )
        ),
    )
