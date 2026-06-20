from __future__ import annotations

from os import PathLike
from typing import Any

class GraphStore:
    _pinned: bool
    _leases: int

    def __init__(self, db_path: str | PathLike[str]) -> None: ...
    def __getattr__(self, name: str) -> Any: ...

def filter_incremental_candidates(
    repo_root: str | PathLike[str],
    candidates: list[str],
    ignore_patterns: list[str],
) -> tuple[list[str], list[str]]: ...
def filter_parseable_files(
    repo_root: str | PathLike[str],
    candidates: list[str],
    ignore_patterns: list[str],
) -> list[str]: ...
def collect_parseable_files(
    repo_root: str | PathLike[str],
    recurse_submodules: bool | None = None,
) -> list[str]: ...
def parse_rust_owned_files_compact_json(
    repo_root: str | PathLike[str],
    file_paths: list[str],
) -> str: ...
def parse_rust_owned_file_compact_json(file_path: str, source: bytes) -> str: ...
def parse_markdown_compact_json(file_path: str, source: bytes) -> str: ...
def parse_terraform_compact_json(file_path: str, source: bytes) -> str: ...
def parse_rust_compact_json(file_path: str, source: bytes) -> str: ...
def parse_python_compact_json(file_path: str, source: bytes) -> str: ...
def embedding_search(
    db_path: str | PathLike[str],
    provider: str,
    query_vec: list[float],
    limit: int,
) -> list[tuple[str, float]]: ...
def embedding_search_prewarm(
    db_path: str | PathLike[str],
    provider: str,
) -> int: ...
