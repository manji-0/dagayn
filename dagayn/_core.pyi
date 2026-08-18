from __future__ import annotations

from os import PathLike
from typing import Any

class GraphStore:
    _pinned: bool
    _leases: int

    def __init__(self, db_path: str | PathLike[str]) -> None: ...
    def trace_flows_json(
        self,
        max_depth: int = 15,
        include_tests: bool = False,
        max_nodes: int = 512,
    ) -> str: ...
    def incremental_trace_flows_json(
        self,
        changed_files: list[str],
        max_depth: int = 15,
    ) -> int: ...
    def refresh_flow_criticalities(self) -> int: ...
    def detect_communities_json(self, min_size: int = 2) -> str: ...
    def incremental_detect_communities(
        self,
        changed_files: list[str],
        min_size: int = 2,
        pre_affected_count: int | None = None,
    ) -> int: ...
    def refresh_community_stats_json(self) -> str: ...
    def fts_query_json(self, query: str, limit: int = 50) -> str: ...
    def keyword_query_json(self, query: str, limit: int = 50) -> str: ...
    def embedding_search_json(
        self,
        provider_key: str,
        query_vec: list[float],
        limit: int = 50,
    ) -> str: ...
    def hybrid_search_json(
        self,
        query: str,
        emb_hits_json: str,
        embedding_health_json: str,
        kind: str = "",
        limit: int = 20,
        context_files_json: str = "[]",
        provider: str = "",
        model: str = "",
    ) -> str: ...
    def run_post_processing_json(
        self,
        manifest_extractor_id: str,
        manifest_nodes_json: str,
        manifest_edges_json: str,
        min_community_size: int = 2,
    ) -> str: ...
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
