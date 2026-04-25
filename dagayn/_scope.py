"""Scope key helpers shared by SDP, ADP, and SAP analysis."""

from __future__ import annotations

from collections import defaultdict
from pathlib import Path
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from .graph import GraphStore


def file_to_package(file_path: str) -> str:
    parent = Path(file_path).parent.as_posix()
    return "<root>" if parent == "." else parent


def node_file_to_scope_key(file_path: str, scope_kind: str) -> str | None:
    """Map a file path to a scope key for the given scope kind.

    Returns None if the scope kind is unsupported or file_path is empty.
    """
    if not file_path:
        return None
    if scope_kind == "file":
        return file_path
    if scope_kind in ("package", "directory"):
        return file_to_package(file_path)
    return None


def build_node_scope_maps(
    store: "GraphStore",
    scope_kind: str,
) -> tuple[dict[str, str], dict[str, str]]:
    """Return (qualified_to_scope, name_to_scope).

    qualified_to_scope maps node.qualified_name → scope_key for every node.
    name_to_scope maps node.name → scope_key only when exactly one unique
    scope contains a node with that name (ambiguous names are excluded).
    Used as a two-stage fallback: try qualified_name first, then bare name.
    """
    qualified_to_scope: dict[str, str] = {}
    name_scopes: dict[str, set[str]] = defaultdict(set)

    for node in store.get_all_nodes(exclude_files=False):
        sk = node_file_to_scope_key(node.file_path, scope_kind)
        if sk is None:
            continue
        qualified_to_scope[node.qualified_name] = sk
        name_scopes[node.name].add(sk)

    name_to_scope: dict[str, str] = {
        name: next(iter(scopes)) for name, scopes in name_scopes.items() if len(scopes) == 1
    }
    return qualified_to_scope, name_to_scope
