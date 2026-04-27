"""Solidity construct extraction for the CodeParser walker."""

from __future__ import annotations

from typing import TYPE_CHECKING, Optional

from ..types import EdgeInfo, NodeInfo

if TYPE_CHECKING:
    from ..core import CodeParser


def handle_node(
    parser: "CodeParser",
    child,
    node_type: str,
    source: bytes,
    language: str,
    file_path: str,
    nodes: list[NodeInfo],
    edges: list[EdgeInfo],
    enclosing_class: Optional[str],
    enclosing_func: Optional[str],
    import_map: Optional[dict[str, str]],
    defined_names: Optional[set[str]],
    _depth: int,
) -> bool:
    return _extract_solidity_constructs(
        parser,
        child,
        node_type,
        source,
        file_path,
        nodes,
        edges,
        enclosing_class,
        enclosing_func,
    )


def _extract_solidity_constructs(
    parser: "CodeParser",
    child,
    node_type: str,
    source: bytes,
    file_path: str,
    nodes: list[NodeInfo],
    edges: list[EdgeInfo],
    enclosing_class: Optional[str],
    enclosing_func: Optional[str],
) -> bool:
    """Handle Solidity-specific AST constructs (emit, state vars, etc.).

    Returns True if the child was fully handled and should skip
    default recursion.
    """
    # Emit statements: emit EventName(...) -> CALLS edge.
    # Module-scope emits attribute to the File node.
    if node_type == "emit_statement":
        for sub in child.children:
            if sub.type == "expression":
                for ident in sub.children:
                    if ident.type == "identifier":
                        caller = (
                            parser._qualify(
                                enclosing_func,
                                file_path,
                                enclosing_class,
                            )
                            if enclosing_func
                            else file_path
                        )
                        edges.append(
                            EdgeInfo(
                                kind="CALLS",
                                source=caller,
                                target=ident.text.decode(
                                    "utf-8",
                                    errors="replace",
                                ),
                                file_path=file_path,
                                line=child.start_point[0] + 1,
                            )
                        )
        # emit_statement falls through to default recursion
        return False

    # State variable declarations -> Function nodes (public ones
    # auto-generate getters, and all are critical for reviews)
    if node_type == "state_variable_declaration" and enclosing_class:
        var_name = None
        var_visibility = None
        var_mutability = None
        var_type = None
        for sub in child.children:
            if sub.type == "identifier":
                var_name = sub.text.decode(
                    "utf-8",
                    errors="replace",
                )
            elif sub.type == "visibility":
                var_visibility = sub.text.decode(
                    "utf-8",
                    errors="replace",
                )
            elif sub.type == "type_name":
                var_type = sub.text.decode(
                    "utf-8",
                    errors="replace",
                )
            elif sub.type in ("constant", "immutable"):
                var_mutability = sub.type
        if var_name:
            qualified = parser._qualify(
                var_name,
                file_path,
                enclosing_class,
            )
            nodes.append(
                NodeInfo(
                    kind="Function",
                    name=var_name,
                    file_path=file_path,
                    line_start=child.start_point[0] + 1,
                    line_end=child.end_point[0] + 1,
                    language="solidity",
                    parent_name=enclosing_class,
                    return_type=var_type,
                    modifiers=var_visibility,
                    extra={
                        "solidity_kind": "state_variable",
                        "mutability": var_mutability,
                    },
                )
            )
            edges.append(
                EdgeInfo(
                    kind="CONTAINS",
                    source=parser._qualify(
                        enclosing_class,
                        file_path,
                        None,
                    ),
                    target=qualified,
                    file_path=file_path,
                    line=child.start_point[0] + 1,
                )
            )
            return True
        return False

    # File-level and contract-level constant declarations
    if node_type == "constant_variable_declaration":
        var_name = None
        var_type = None
        for sub in child.children:
            if sub.type == "identifier":
                var_name = sub.text.decode(
                    "utf-8",
                    errors="replace",
                )
            elif sub.type == "type_name":
                var_type = sub.text.decode(
                    "utf-8",
                    errors="replace",
                )
        if var_name:
            qualified = parser._qualify(
                var_name,
                file_path,
                enclosing_class,
            )
            nodes.append(
                NodeInfo(
                    kind="Function",
                    name=var_name,
                    file_path=file_path,
                    line_start=child.start_point[0] + 1,
                    line_end=child.end_point[0] + 1,
                    language="solidity",
                    parent_name=enclosing_class,
                    return_type=var_type,
                    extra={"solidity_kind": "constant"},
                )
            )
            container = (
                parser._qualify(enclosing_class, file_path, None) if enclosing_class else file_path
            )
            edges.append(
                EdgeInfo(
                    kind="CONTAINS",
                    source=container,
                    target=qualified,
                    file_path=file_path,
                    line=child.start_point[0] + 1,
                )
            )
            return True
        return False

    # Using directives: using LibName for Type -> DEPENDS_ON edge
    if node_type == "using_directive":
        lib_name = None
        for sub in child.children:
            if sub.type == "type_alias":
                for ident in sub.children:
                    if ident.type == "identifier":
                        lib_name = ident.text.decode(
                            "utf-8",
                            errors="replace",
                        )
        if lib_name:
            source_name = (
                parser._qualify(
                    enclosing_class,
                    file_path,
                    None,
                )
                if enclosing_class
                else file_path
            )
            edges.append(
                EdgeInfo(
                    kind="DEPENDS_ON",
                    source=source_name,
                    target=lib_name,
                    file_path=file_path,
                    line=child.start_point[0] + 1,
                )
            )
        return True

    return False
