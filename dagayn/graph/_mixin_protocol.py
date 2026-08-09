"""Internal typing contract shared by GraphStore mixins."""

from __future__ import annotations

import sqlite3
from pathlib import Path
from typing import TYPE_CHECKING, Any, Optional, Protocol

from .types import GraphEdge, GraphNode

if TYPE_CHECKING:
    from ..parser._base.types import EdgeInfo, NodeInfo


class GraphStoreMixinProtocol(Protocol):
    _conn: sqlite3.Connection
    _repo_root_cache: Any

    def _normalize_file_path_key(self, file_path: str | Path) -> str: ...

    def _normalize_qualified_key(self, qualified_name: str) -> str: ...

    def _make_qualified(self, node: NodeInfo) -> str: ...

    def _row_to_node(self, row: sqlite3.Row) -> GraphNode: ...

    def _row_to_edge(self, row: sqlite3.Row) -> GraphEdge: ...

    def _invalidate_cache(self) -> None: ...

    def _build_networkx_graph(self) -> Any: ...

    def get_metadata(self, key: str) -> Optional[str]: ...

    def get_nodes_by_qualified_names(
        self,
        qualified_names: list[str],
    ) -> dict[str, GraphNode]: ...

    def get_nodes_by_files(self, file_paths: list[str]) -> dict[str, list[GraphNode]]: ...

    def get_nodes_by_ids(self, node_ids: list[int]) -> dict[int, GraphNode]: ...

    def get_edges_by_endpoints(
        self,
        qualified_names: list[str],
    ) -> tuple[dict[str, list[GraphEdge]], dict[str, list[GraphEdge]]]: ...

    def get_edges_among(self, qualified_names: set[str]) -> list[GraphEdge]: ...

    def _batch_get_nodes(self, qualified_names: set[str]) -> list[GraphNode]: ...

    def remove_file_data(self, file_path: str, *, invalidate: bool = True) -> None: ...

    def remove_files_data(self, file_paths: list[str], *, invalidate: bool = True) -> None: ...

    def _bulk_insert_nodes(
        self,
        nodes: list[NodeInfo],
        fhash: str,
        mtime_ns: int = 0,
    ) -> None: ...

    def _bulk_insert_nodes_with_meta(
        self,
        nodes: list[tuple[NodeInfo, str, int]],
    ) -> None: ...

    def _bulk_insert_edges(self, edges: list[EdgeInfo]) -> None: ...
