"""Data types for the dagayn parser package."""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import NamedTuple, Optional

from ...state_types import GraphExtra


class CellInfo(NamedTuple):
    """Represents a single cell in a notebook with its language."""

    cell_index: int
    language: str
    source: str


# ---------------------------------------------------------------------------
# Data models for extracted entities
# ---------------------------------------------------------------------------


@dataclass
class NodeInfo:
    kind: str  # File, Class, Function, Type, Test
    name: str
    file_path: str
    line_start: int
    line_end: int
    language: str = ""
    parent_name: Optional[str] = None  # enclosing class/module
    params: Optional[str] = None
    return_type: Optional[str] = None
    modifiers: Optional[str] = None
    is_test: bool = False
    extra: GraphExtra = field(default_factory=dict)


@dataclass
class EdgeInfo:
    # CALLS, IMPORTS_FROM, INHERITS, IMPLEMENTS, CONTAINS,
    # TESTED_BY, DEPENDS_ON, REFERENCES, CROSS_ARTIFACT
    kind: str
    source: str  # qualified name or path
    target: str  # qualified name or path
    file_path: str
    line: int = 0
    extra: GraphExtra = field(default_factory=dict)


@dataclass(frozen=True)
class BridgePattern:
    """Language-agnostic descriptor of a cross-language bridge call site.

    `call_signature` is the canonical dotted-name as it appears in source
    (e.g. ``subprocess.run`` for Python, ``Runtime.getRuntime().exec`` for
    Java, ``child_process.exec`` for JS/TS, ``system`` for R). Aliased imports
    are not matched — only canonical forms.

    `relationship_role` and `bridge_kind` follow the metadata contract in
    ``docs/CROSS-LANGUAGE-EDGES-WIP.md``.
    """

    call_signature: str
    relationship_role: str
    bridge_kind: str
