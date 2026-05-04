"""Compatibility wrapper for the Rust-backed parser."""

from __future__ import annotations

import hashlib
import json
import re
from pathlib import Path
from typing import Any, Optional

from ._base.types import EdgeInfo, NodeInfo
from .dispatch import detect_language as _detect_language

_SQL_TABLE_RE = re.compile(
    r"(?:FROM|JOIN|INTO|CREATE\s+(?:OR\s+REPLACE\s+)?(?:TABLE|VIEW)|INSERT\s+OVERWRITE)"
    r"\s+((?:`[^`]+`|\w+)(?:\.(?:`[^`]+`|\w+))*)",
    re.IGNORECASE,
)


def file_hash(path: Path) -> str:
    """SHA-256 hash of file contents."""
    return hashlib.sha256(path.read_bytes()).hexdigest()


class CodeParser:
    """Parses source files through the native Rust parser shipped in the wheel."""

    def detect_language(self, path: Path) -> Optional[str]:
        return _detect_language(path)

    def parse_file(self, path: Path) -> tuple[list[NodeInfo], list[EdgeInfo]]:
        try:
            source = path.read_bytes()
        except (OSError, PermissionError):
            return [], []
        return self.parse_bytes(path, source)

    def parse_bytes(self, path: Path, source: bytes) -> tuple[list[NodeInfo], list[EdgeInfo]]:
        if self.detect_language(path) is None:
            return [], []

        try:
            from dagayn._core import parse_rust_owned_file_compact_json
        except ImportError as exc:
            raise RuntimeError(
                "dagayn._core is required for parsing. Install dagayn from a wheel "
                "or build the native extension with maturin."
            ) from exc

        parser_path = _parser_path(path)
        nodes, edges = json.loads(parse_rust_owned_file_compact_json(parser_path, source))
        return (
            _decode_nodes(nodes, parser_path=parser_path, display_path=str(path)),
            _decode_edges(edges, parser_path=parser_path, display_path=str(path)),
        )


def _decode_nodes(
    raw_nodes: list[list[Any]],
    *,
    parser_path: str,
    display_path: str,
) -> list[NodeInfo]:
    return [
        NodeInfo(
            kind=item[0],
            name=_normalize_path_string(item[1], parser_path, display_path),
            file_path=_normalize_path_string(item[2], parser_path, display_path),
            line_start=item[3],
            line_end=item[4],
            language=item[5],
            parent_name=item[6],
            params=item[7],
            return_type=item[8],
            modifiers=item[9],
            is_test=item[10],
            extra=item[11] or {},
        )
        for item in raw_nodes
    ]


def _decode_edges(
    raw_edges: list[list[Any]],
    *,
    parser_path: str,
    display_path: str,
) -> list[EdgeInfo]:
    return [
        EdgeInfo(
            kind=item[0],
            source=_normalize_path_string(item[1], parser_path, display_path),
            target=_normalize_path_string(item[2], parser_path, display_path),
            file_path=_normalize_path_string(item[3], parser_path, display_path),
            line=item[4],
            extra=item[5] or {},
        )
        for item in raw_edges
    ]


def _normalize_path_string(value: Any, parser_path: str, display_path: str) -> Any:
    if not isinstance(value, str):
        return value
    normalized = value
    if normalized.startswith("//") and not normalized.startswith("///"):
        normalized = normalized[1:]
    if display_path == parser_path:
        return normalized
    parser_dir = str(Path(parser_path).parent)
    display_dir = str(Path(display_path).parent)
    if normalized == parser_path:
        return display_path
    if normalized.startswith(f"{parser_path}::"):
        return f"{display_path}::{normalized.removeprefix(f'{parser_path}::')}"
    if parser_dir != ".":
        if normalized == parser_dir:
            return display_dir
        if normalized.startswith(f"{parser_dir}/"):
            return f"{display_dir}/{normalized.removeprefix(f'{parser_dir}/')}"
    return normalized


def _parser_path(path: Path) -> str:
    try:
        return str(path.resolve().relative_to(Path.cwd().resolve()))
    except (OSError, ValueError):
        return str(path)
