"""Live worktree source slices for a single graph node."""

from __future__ import annotations

import hashlib
from pathlib import Path
from typing import Any

from ..embeddings_text import node_source_line_span
from ..graph import GraphNode, node_to_dict
from ._common import resolve_contained_path

SOURCE_OF_MAX_CHARS = 4000

_READ_ERROR_ESCAPES = "path_escapes_repo"
_READ_ERROR_MISSING_PATH = "missing_path"
_READ_ERROR_NOT_A_FILE = "not_a_file"
_READ_ERROR_UNREADABLE = "unreadable"


def read_live_node_source(
    node: GraphNode,
    *,
    repo_root: Path,
    max_chars: int = SOURCE_OF_MAX_CHARS,
) -> dict[str, Any]:
    """Return the live worktree span for *node*, capped at *max_chars*.

    Source is read from disk at query time. The graph is only a locator
    (path + line span + optional ``file_hash``). A hash mismatch still
    returns the live slice and sets ``source_stale`` so callers do not
    treat the graph span as authoritative.
    """
    limit = max(0, int(max_chars))
    payload = _base_payload(node, max_chars=limit)
    if not node.file_path:
        payload["read_error"] = _READ_ERROR_MISSING_PATH
        return payload

    path = resolve_contained_path(str(node.file_path), repo_root)
    if path is None:
        payload["read_error"] = _READ_ERROR_ESCAPES
        return payload
    if not path.is_file():
        payload["read_error"] = _READ_ERROR_NOT_A_FILE
        return payload

    try:
        raw = path.read_bytes()
    except OSError:
        payload["read_error"] = _READ_ERROR_UNREADABLE
        return payload

    live_hash = hashlib.sha256(raw).hexdigest()
    stored_hash = str(node.file_hash or "")
    payload["source_stale"] = bool(stored_hash) and stored_hash != live_hash

    text = raw.decode("utf-8", errors="replace")
    lines = text.splitlines()
    start, end = node_source_line_span(node, lines)
    span_text = "\n".join(lines[start:end])
    truncated = limit >= 0 and len(span_text) > limit
    source = span_text[:limit] if truncated else span_text
    payload["source"] = source
    payload["truncated"] = truncated
    payload["omitted_chars"] = max(0, len(span_text) - len(source))
    payload["omitted_lines"] = _omitted_lines(span_line_count=end - start, source=source)
    payload["span_line_start"] = start + 1 if lines else 0
    payload["span_line_end"] = end
    return payload


def _base_payload(node: GraphNode, *, max_chars: int) -> dict[str, Any]:
    payload = node_to_dict(node)
    payload.update(
        {
            "signature": node.signature,
            "params": node.params,
            "return_type": node.return_type,
            "source": "",
            "truncated": False,
            "source_stale": False,
            "read_error": None,
            "omitted_chars": 0,
            "omitted_lines": 0,
            "max_chars": max_chars,
            "span_line_start": node.line_start,
            "span_line_end": node.line_end,
        }
    )
    return payload


def _omitted_lines(*, span_line_count: int, source: str) -> int:
    if span_line_count <= 0:
        return 0
    if not source:
        return span_line_count
    kept_line_count = source.count("\n") + 1
    return max(0, span_line_count - kept_line_count)
