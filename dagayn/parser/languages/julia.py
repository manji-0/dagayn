"""Julia construct extraction for the CodeParser walker."""

from __future__ import annotations

from typing import Optional

from .._base.protocol import CodeParser
from .._base.test_detection import is_test_function as _is_test_function
from .._base.types import EdgeInfo, NodeInfo


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
    return _extract_julia_constructs(
        parser,
        child,
        node_type,
        source,
        language,
        file_path,
        nodes,
        edges,
        enclosing_class,
        enclosing_func,
        import_map,
        defined_names,
        _depth,
    )


def _julia_short_func_name(call_expr) -> Optional[str]:
    """Extract the function name from a Julia short-form function lhs."""
    for child in call_expr.children:
        if child.type == "identifier":
            return child.text.decode("utf-8", errors="replace")
        if child.type == "field_expression":
            for ident in reversed(child.children):
                if ident.type == "identifier":
                    return ident.text.decode("utf-8", errors="replace")
            return None
        if child.type == "parametrized_type_expression":
            for ident in child.children:
                if ident.type == "identifier":
                    return ident.text.decode("utf-8", errors="replace")
            return None
    return None


def _julia_string_arg(call_expr) -> Optional[str]:
    """Return the first string literal argument of a Julia call."""
    for child in call_expr.children:
        if child.type != "argument_list":
            continue
        for arg in child.children:
            if arg.type == "string_literal":
                for sub in arg.children:
                    if sub.type == "content":
                        return sub.text.decode("utf-8", errors="replace")
                raw = arg.text.decode("utf-8", errors="replace")
                return raw.strip('"').strip("'")
    return None


def _julia_call_first_identifier(call_expr) -> Optional[str]:
    """Return the first identifier of a Julia call expression."""
    for child in call_expr.children:
        if child.type == "identifier":
            return child.text.decode("utf-8", errors="replace")
    return None


def _julia_qualified_function_owner(node) -> Optional[str]:
    """Return the owner for a qualified Julia function like ``Base.show``."""
    signature = None
    if node.type == "assignment":
        lhs = node.children[0] if node.children else None
        if lhs is not None and lhs.type == "typed_expression":
            for sub in lhs.children:
                if sub.type == "call_expression":
                    lhs = sub
                    break
        signature = lhs
    else:
        for child in node.children:
            if child.type == "signature":
                signature = child
                break
    if signature is None:
        return None
    queue = [signature]
    while queue:
        current = queue.pop(0)
        if current.type == "field_expression":
            for child in current.children:
                if child.type == "identifier":
                    return child.text.decode("utf-8", errors="replace")
            return None
        queue.extend(list(current.children))
    return None


def _extract_julia_constructs(
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
    """Handle Julia-specific constructs the generic tables miss."""
    if node_type == "assignment":
        lhs = child.children[0] if child.children else None
        if lhs is not None and lhs.type == "typed_expression":
            for sub in lhs.children:
                if sub.type == "call_expression":
                    lhs = sub
                    break
        if lhs is not None and lhs.type == "call_expression":
            name = _julia_short_func_name(lhs)
            if name:
                is_test = _is_test_function(name, file_path, ())
                kind = "Test" if is_test else "Function"
                qualified = parser._qualify(name, file_path, enclosing_class)
                nodes.append(
                    NodeInfo(
                        kind=kind,
                        name=name,
                        file_path=file_path,
                        line_start=child.start_point[0] + 1,
                        line_end=child.end_point[0] + 1,
                        language=language,
                        parent_name=enclosing_class,
                        is_test=is_test,
                    )
                )
                container = (
                    parser._qualify(enclosing_class, file_path, None)
                    if enclosing_class
                    else file_path
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
                owner = _julia_qualified_function_owner(child)
                if owner:
                    edges.append(
                        EdgeInfo(
                            kind="REFERENCES",
                            source=qualified,
                            target=owner,
                            file_path=file_path,
                            line=child.start_point[0] + 1,
                        )
                    )
                seen_op = False
                for sub in child.children:
                    if not seen_op:
                        if sub.type == "operator":
                            seen_op = True
                        continue
                    parser._extract_from_tree(
                        sub,
                        source,
                        language,
                        file_path,
                        nodes,
                        edges,
                        enclosing_class=enclosing_class,
                        enclosing_func=name,
                        import_map=import_map,
                        defined_names=defined_names,
                        _depth=_depth + 1,
                    )
                return True

    if node_type == "call_expression":
        parent = child.parent
        if parent is not None and parent.type == "signature":
            return True
        if _julia_call_first_identifier(child) == "include":
            path_arg = _julia_string_arg(child)
            if path_arg:
                resolved = parser._resolve_module_to_file(
                    path_arg,
                    file_path,
                    language,
                )
                edges.append(
                    EdgeInfo(
                        kind="IMPORTS_FROM",
                        source=file_path,
                        target=resolved if resolved else path_arg,
                        file_path=file_path,
                        line=child.start_point[0] + 1,
                    )
                )
            return False

    if node_type in ("export_statement", "public_statement"):
        source_qual = (
            parser._qualify(enclosing_class, file_path, None) if enclosing_class else file_path
        )
        marker = "julia_export" if node_type == "export_statement" else "julia_public"
        for sub in child.children:
            if sub.type == "identifier":
                name = sub.text.decode("utf-8", errors="replace")
                edges.append(
                    EdgeInfo(
                        kind="REFERENCES",
                        source=source_qual,
                        target=name,
                        file_path=file_path,
                        line=child.start_point[0] + 1,
                        extra={marker: True},
                    )
                )
        return True

    if node_type == "macrocall_expression":
        macro_name = None
        for sub in child.children:
            if sub.type == "macro_identifier":
                for ident in sub.children:
                    if ident.type == "identifier":
                        macro_name = ident.text.decode("utf-8", errors="replace")
                        break
                break

        if macro_name == "enum":
            type_name: Optional[str] = None
            variant_identifiers: list = []
            for sub in child.children:
                if sub.type != "macro_argument_list":
                    continue
                for arg in sub.children:
                    if arg.type != "identifier":
                        continue
                    if type_name is None:
                        type_name = arg.text.decode("utf-8", errors="replace")
                    else:
                        variant_identifiers.append(arg)
                break
            if type_name:
                line_start = child.start_point[0] + 1
                line_end = child.end_point[0] + 1
                qualified_type = parser._qualify(type_name, file_path, enclosing_class)
                nodes.append(
                    NodeInfo(
                        kind="Class",
                        name=type_name,
                        file_path=file_path,
                        line_start=line_start,
                        line_end=line_end,
                        language=language,
                        parent_name=enclosing_class,
                        extra={"julia_kind": "enum"},
                    )
                )
                container = (
                    parser._qualify(enclosing_class, file_path, None)
                    if enclosing_class
                    else file_path
                )
                edges.append(
                    EdgeInfo(
                        kind="CONTAINS",
                        source=container,
                        target=qualified_type,
                        file_path=file_path,
                        line=line_start,
                    )
                )
                for variant in variant_identifiers:
                    vname = variant.text.decode("utf-8", errors="replace")
                    qualified_v = parser._qualify(vname, file_path, type_name)
                    nodes.append(
                        NodeInfo(
                            kind="Function",
                            name=vname,
                            file_path=file_path,
                            line_start=variant.start_point[0] + 1,
                            line_end=variant.end_point[0] + 1,
                            language=language,
                            parent_name=type_name,
                            extra={"julia_kind": "enum_variant"},
                        )
                    )
                    edges.append(
                        EdgeInfo(
                            kind="CONTAINS",
                            source=qualified_type,
                            target=qualified_v,
                            file_path=file_path,
                            line=variant.start_point[0] + 1,
                        )
                    )
            return True

        if macro_name == "testset":
            desc = None
            body_parent = None
            for sub in child.children:
                if sub.type != "macro_argument_list":
                    continue
                body_parent = sub
                for arg in sub.children:
                    if arg.type == "string_literal":
                        for c in arg.children:
                            if c.type == "content":
                                desc = c.text.decode("utf-8", errors="replace")
                                break
                        break
            line_no = child.start_point[0] + 1
            synth_base = f"testset:{desc}" if desc else "testset"
            synth_name = f"{synth_base}@L{line_no}"
            qualified = parser._qualify(synth_name, file_path, enclosing_class)
            nodes.append(
                NodeInfo(
                    kind="Test",
                    name=synth_name,
                    file_path=file_path,
                    line_start=child.start_point[0] + 1,
                    line_end=child.end_point[0] + 1,
                    language=language,
                    parent_name=enclosing_class,
                    is_test=True,
                )
            )
            container = (
                parser._qualify(enclosing_func, file_path, enclosing_class)
                if enclosing_func
                else file_path
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
            if body_parent is not None:
                parser._extract_from_tree(
                    body_parent,
                    source,
                    language,
                    file_path,
                    nodes,
                    edges,
                    enclosing_class=enclosing_class,
                    enclosing_func=synth_name,
                    import_map=import_map,
                    defined_names=defined_names,
                    _depth=_depth + 1,
                )
            return True

        return False

    return False
