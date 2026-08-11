"""Shared FTS5 row content builders for Python and rebuild paths."""

from __future__ import annotations

import json
import re
from pathlib import Path
from typing import Any

from ._fts_tokenize import segment_japanese_fts_text

_IDENT_BOUNDARY_RE = re.compile(r"(?<=[a-z0-9])(?=[A-Z])|(?<=[A-Z])(?=[A-Z][a-z])")
_IDENT_SPLIT_RE = re.compile(r"[^A-Za-z0-9]+")
_MARKDOWN_HEADING_RE = re.compile(r"^(#{1,6})\s+")


def identifier_search_text(*values: object) -> str:
    """Return identifier-friendly tokens for FTS (camel/snake/path split)."""
    tokens: list[str] = []
    for value in values:
        if not value:
            continue
        for chunk in _IDENT_SPLIT_RE.split(str(value)):
            if not chunk:
                continue
            tokens.extend(part.lower() for part in _IDENT_BOUNDARY_RE.sub(" ", chunk).split())
    return " ".join(tokens)


def structured_code_reference_text(
    *,
    kind: str,
    name: str,
    qualified_name: str,
    file_path: str,
    display_name: str = "",
    signature: str | None = None,
    source_excerpt: str = "",
) -> str:
    """Build the structured code-reference block indexed in ``doc_text``."""
    parts = [
        f"kind: {kind}",
        f"name: {name}",
        f"qualified: {qualified_name}",
        f"file: {file_path.replace('/', ' ')}",
    ]
    if display_name:
        parts.append(f"display: {display_name}")
    if signature:
        parts.append(f"signature: {signature}")
    if source_excerpt:
        parts.append(f"source:\n{source_excerpt}")
    return "\n".join(parts)


def _resolve_node_file(repo_root: Path | None, file_path_value: str) -> Path | None:
    file_path = Path(file_path_value)
    if not file_path.is_absolute():
        if repo_root is None:
            return None
        file_path = repo_root / file_path
    return file_path


def read_node_source_excerpt(
    repo_root: Path | None,
    *,
    kind: str,
    file_path: str,
    line_start: int | None,
    line_end: int | None,
    file_lines_cache: dict[Path, list[str] | None] | None = None,
) -> str:
    """Read a bounded source/doc span for FTS, best-effort and side-effect free."""
    resolved = _resolve_node_file(repo_root, file_path)
    if resolved is None:
        return ""
    if file_lines_cache is not None and resolved in file_lines_cache:
        lines = file_lines_cache[resolved]
    else:
        try:
            lines = resolved.read_text(encoding="utf-8", errors="replace").splitlines()
        except OSError:
            lines = None
        if file_lines_cache is not None:
            file_lines_cache[resolved] = lines
    if lines is None:
        return ""

    start_line = line_start or 1
    end_line = line_end or start_line
    start = max(int(start_line) - 1, 0)
    end = min(max(int(end_line), int(start_line)), len(lines))

    if kind == "DocSection":
        level = None
        if start < len(lines):
            match = _MARKDOWN_HEADING_RE.match(lines[start])
            if match:
                level = len(match.group(1))
        end = len(lines)
        for idx in range(start + 1, len(lines)):
            match = _MARKDOWN_HEADING_RE.match(lines[idx])
            if match and (level is None or len(match.group(1)) <= level):
                end = idx
                break

    return "\n".join(lines[start:end])[:4096]


def build_node_fts_values(
    *,
    kind: str,
    name: str,
    qualified_name: str,
    file_path: str,
    line_start: int | None,
    line_end: int | None,
    signature: str | None,
    extra: dict[str, Any] | str | None,
    repo_root: Path | None,
    file_lines_cache: dict[Path, list[str] | None] | None = None,
) -> tuple[str, str, str, str, str, str, str]:
    """Return FTS column values for a node row."""
    if isinstance(extra, str):
        try:
            extra_data = json.loads(extra or "{}")
        except (TypeError, json.JSONDecodeError):
            extra_data = {}
    else:
        extra_data = extra or {}

    display_name = str(extra_data.get("display_name", "") or "")
    signature_value = signature or ""
    identifier_tokens = identifier_search_text(name, qualified_name, file_path, display_name)
    source_excerpt = read_node_source_excerpt(
        repo_root,
        kind=kind,
        file_path=file_path,
        line_start=line_start,
        line_end=line_end,
        file_lines_cache=file_lines_cache,
    )
    structured_description = structured_code_reference_text(
        kind=kind,
        name=name,
        qualified_name=qualified_name,
        file_path=file_path,
        display_name=display_name,
        signature=signature_value or None,
        source_excerpt=source_excerpt,
    )
    doc_text = " ".join(
        part for part in (display_name, structured_description, source_excerpt) if part
    )
    doc_text = segment_japanese_fts_text(doc_text)
    return (
        name,
        qualified_name,
        file_path,
        signature_value,
        identifier_tokens,
        doc_text,
    )


def build_fts_insert_row(
    node_rowid: int,
    row: Any,
    repo_root: Path | None,
    file_lines_cache: dict[Path, list[str] | None] | None = None,
) -> tuple[int, str, str, str, str, str, str]:
    """Build a full ``nodes_fts`` insert tuple from a ``nodes`` table row."""
    values = build_node_fts_values(
        kind=row["kind"],
        name=row["name"],
        qualified_name=row["qualified_name"],
        file_path=row["file_path"],
        line_start=row["line_start"],
        line_end=row["line_end"],
        signature=row["signature"],
        extra=row["extra"],
        repo_root=repo_root,
        file_lines_cache=file_lines_cache,
    )
    return (node_rowid, *values)
