"""Protocol definition for the dagayn parser package.

Defines the public API surface of CodeParser as a structural Protocol so
callers can depend on the interface rather than the concrete class.
"""

from __future__ import annotations

from pathlib import Path
from typing import Optional, Protocol, runtime_checkable

from ._base.types import EdgeInfo, NodeInfo


@runtime_checkable
class CodeParserProtocol(Protocol):
    """Public API for parsing source files into graph nodes and edges."""

    def detect_language(self, path: Path) -> Optional[str]: ...

    def parse_file(self, path: Path) -> tuple[list[NodeInfo], list[EdgeInfo]]: ...

    def parse_bytes(self, path: Path, source: bytes) -> tuple[list[NodeInfo], list[EdgeInfo]]: ...
