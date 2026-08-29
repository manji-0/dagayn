"""Protocol definitions for the dagayn graph package.

Defines the public API surface of GraphStore as structural Protocols.
Callers that need only a subset of the store can depend on a narrower
Protocol rather than on the concrete GraphStore class, decoupling them
from the SQLite implementation details.
"""

from __future__ import annotations

from pathlib import Path
from typing import TYPE_CHECKING, Optional, Protocol, runtime_checkable

from .types import (
    FlowAdjacency,
    GraphEdge,
    GraphNode,
    GraphStats,
    ImpactRadiusResult,
    SubgraphResult,
    TransitiveTestRecord,
)

if TYPE_CHECKING:
    from dagayn.parser._base.types import EdgeInfo, NodeInfo
    from .types import FtsQueryResult


@runtime_checkable
class GraphStoreProtocol(Protocol):
    """Core storage and basic query operations."""

    def close(self) -> None: ...

    def get_repo_root(self) -> Optional[Path]: ...

    def get_metadata(self, key: str) -> Optional[str]: ...

    def set_metadata(self, key: str, value: str) -> None: ...

    def upsert_node(self, node: NodeInfo, file_hash: str = "") -> int: ...

    def upsert_edge(self, edge: EdgeInfo) -> int: ...

    def remove_file_data(self, file_path: str) -> None: ...

    def store_file_nodes_edges(
        self,
        file_path: str,
        nodes: list[NodeInfo],
        edges: list[EdgeInfo],
        fhash: str = "",
        mtime_ns: int = 0,
    ) -> None: ...

    def store_file_batch(
        self,
        batch: list[tuple[str, list[NodeInfo], list[EdgeInfo], str, int]],
    ) -> None: ...

    def commit(self) -> None: ...

    def rollback(self) -> None: ...

    def get_node(self, qualified_name: str) -> Optional[GraphNode]: ...

    def get_nodes_by_qualified_names(
        self,
        qualified_names: list[str],
    ) -> dict[str, GraphNode]: ...

    def get_nodes_by_file(self, file_path: str) -> list[GraphNode]: ...

    def get_nodes_by_files(self, file_paths: list[str]) -> dict[str, list[GraphNode]]: ...

    def get_all_nodes(self, exclude_files: bool = True) -> list[GraphNode]: ...

    def get_all_files(self) -> list[str]: ...

    def get_edges_by_source(self, qualified_name: str) -> list[GraphEdge]: ...

    def get_edges_by_target(self, qualified_name: str) -> list[GraphEdge]: ...

    def get_edges_among(self, qualified_names: set[str]) -> list[GraphEdge]: ...

    def search_nodes(self, query: str, limit: int = 20) -> list[GraphNode]: ...

    def fts_query(self, query: str, limit: int = 50) -> "FtsQueryResult": ...

    def keyword_query(self, query: str, limit: int = 50) -> list[tuple[int, float]]: ...

    def get_nodes_by_ids(self, node_ids: list[int]) -> dict[int, GraphNode]: ...

    def get_stats(self) -> GraphStats: ...

    def get_nodes_by_size(
        self,
        min_lines: int = 50,
        max_lines: int | None = None,
        kind: str | None = None,
        file_path_pattern: str | None = None,
        limit: int = 50,
    ) -> list[GraphNode]: ...


@runtime_checkable
class GraphQueryProtocol(Protocol):
    """Impact analysis and advanced graph traversal operations."""

    def get_impact_radius(
        self,
        changed_files: list[str],
        max_depth: int = ...,
        max_nodes: int = ...,
    ) -> ImpactRadiusResult: ...

    def get_impact_radius_sql(
        self,
        changed_files: list[str],
        max_depth: int = ...,
        max_nodes: int = ...,
    ) -> ImpactRadiusResult: ...

    def get_transitive_tests(
        self,
        qualified_name: str,
        max_depth: int = 1,
    ) -> list[TransitiveTestRecord]: ...

    def resolve_bare_call_targets(self) -> int: ...

    def resolve_bare_inheritance_targets(self) -> int: ...

    def get_subgraph(self, qualified_names: list[str]) -> SubgraphResult: ...

    def load_flow_adjacency(self) -> FlowAdjacency: ...

    def get_outgoing_targets(self, source_qns: list[str]) -> list[str]: ...

    def get_incoming_sources(self, target_qns: list[str]) -> list[str]: ...

    def get_flow_qualified_names_for_flows(self, flow_ids: list[int]) -> dict[int, set[str]]: ...
