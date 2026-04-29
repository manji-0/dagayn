"""R construct extraction for the CodeParser walker."""

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
    return _extract_r_constructs(
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
    )


def _extract_r_constructs(
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
) -> bool:
    """Handle R-specific AST nodes (assignments and class-defining calls).

    Returns True if the child was fully handled and should be skipped
    by the main loop.
    """
    # R: function definitions via assignment
    if node_type == "binary_operator":
        handled = _handle_r_binary_operator(
            parser,
            child,
            source,
            language,
            file_path,
            nodes,
            edges,
            enclosing_class,
            enclosing_func,
            import_map,
            defined_names,
        )
        if handled:
            return True

    # R: setClass/setRefClass/setGeneric calls and imports
    if node_type == "call":
        handled = _handle_r_call(
            parser,
            child,
            source,
            language,
            file_path,
            nodes,
            edges,
            enclosing_class,
            enclosing_func,
            import_map,
            defined_names,
        )
        if handled:
            return True

    return False


# ------------------------------------------------------------------
# R-specific helpers
# ------------------------------------------------------------------


def _r_call_func_name(call_node) -> Optional[str]:
    """Extract the function name from an R call node."""
    for child in call_node.children:
        if child.type in ("identifier", "namespace_operator"):
            return child.text.decode("utf-8", errors="replace")
    return None


def _r_first_string_arg(call_node) -> Optional[str]:
    """Extract the first string argument value from an R call node."""
    for child in call_node.children:
        if child.type == "arguments":
            for arg in child.children:
                if arg.type == "argument":
                    for sub in arg.children:
                        if sub.type == "string":
                            for sc in sub.children:
                                if sc.type == "string_content":
                                    return sc.text.decode("utf-8", errors="replace")
            break
    return None


def _r_iter_args(call_node):
    """Yield (name_str, value_node) pairs from an R call's arguments."""
    for child in call_node.children:
        if child.type != "arguments":
            continue
        for arg in child.children:
            if arg.type != "argument":
                continue
            has_eq = any(sub.type == "=" for sub in arg.children)
            if has_eq:
                name = None
                value = None
                for sub in arg.children:
                    if sub.type == "identifier" and name is None:
                        name = sub.text.decode("utf-8", errors="replace")
                    elif sub.type not in ("=", ","):
                        value = sub
                yield (name, value)
            else:
                for sub in arg.children:
                    if sub.type not in (",",):
                        yield (None, sub)
                        break
        break


def _r_find_named_arg(call_node, arg_name: str):
    """Find a named argument's value node in an R call."""
    for name, value in _r_iter_args(call_node):
        if name == arg_name:
            return value
    return None


# ------------------------------------------------------------------
# R-specific handlers
# ------------------------------------------------------------------


def _handle_r_binary_operator(
    parser: "CodeParser",
    node,
    source: bytes,
    language: str,
    file_path: str,
    nodes: list[NodeInfo],
    edges: list[EdgeInfo],
    enclosing_class: Optional[str],
    enclosing_func: Optional[str],
    import_map: Optional[dict[str, str]],
    defined_names: Optional[set[str]],
) -> bool:
    """Handle R binary_operator nodes: name <- function(...) { ... }."""
    children = node.children
    if len(children) < 3:
        return False

    left, op, right = children[0], children[1], children[2]
    if op.type not in ("<-", "="):
        return False

    if right.type == "function_definition" and left.type == "identifier":
        name = left.text.decode("utf-8", errors="replace")
        is_test = _is_test_function(name, file_path)
        kind = "Test" if is_test else "Function"
        qualified = parser._qualify(name, file_path, enclosing_class)
        params = parser._get_params(right, language, source)

        nodes.append(
            NodeInfo(
                kind=kind,
                name=name,
                file_path=file_path,
                line_start=right.start_point[0] + 1,
                line_end=right.end_point[0] + 1,
                language=language,
                parent_name=enclosing_class,
                params=params,
                is_test=is_test,
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
                line=right.start_point[0] + 1,
            )
        )

        parser._extract_from_tree(
            right,
            source,
            language,
            file_path,
            nodes,
            edges,
            enclosing_class=enclosing_class,
            enclosing_func=name,
            import_map=import_map,
            defined_names=defined_names,
        )
        return True

    if right.type == "call" and left.type == "identifier":
        call_func = _r_call_func_name(right)
        if call_func in ("setRefClass", "setClass", "setGeneric"):
            assign_name = left.text.decode("utf-8", errors="replace")
            return _handle_r_class_call(
                parser,
                right,
                source,
                language,
                file_path,
                nodes,
                edges,
                enclosing_class,
                enclosing_func,
                import_map,
                defined_names,
                assign_name=assign_name,
            )

    return False


def _handle_r_call(
    parser: "CodeParser",
    node,
    source: bytes,
    language: str,
    file_path: str,
    nodes: list[NodeInfo],
    edges: list[EdgeInfo],
    enclosing_class: Optional[str],
    enclosing_func: Optional[str],
    import_map: Optional[dict[str, str]],
    defined_names: Optional[set[str]],
) -> bool:
    """Handle R call nodes for imports and class definitions."""
    func_name = _r_call_func_name(node)
    if not func_name:
        return False

    if func_name in ("library", "require", "source"):
        imports = parser._extract_import(node, language, source)
        for imp_target in imports:
            edges.append(
                EdgeInfo(
                    kind="IMPORTS_FROM",
                    source=file_path,
                    target=imp_target,
                    file_path=file_path,
                    line=node.start_point[0] + 1,
                )
            )
        return True

    if func_name in ("setRefClass", "setClass", "setGeneric"):
        return _handle_r_class_call(
            parser,
            node,
            source,
            language,
            file_path,
            nodes,
            edges,
            enclosing_class,
            enclosing_func,
            import_map,
            defined_names,
        )

    # Module-scope R calls attribute to the File node.
    call_name = parser._get_call_name(node, language, source)
    if call_name:
        caller = (
            parser._qualify(enclosing_func, file_path, enclosing_class)
            if enclosing_func
            else file_path
        )
        target = parser._resolve_call_target(
            call_name,
            file_path,
            language,
            import_map or {},
            defined_names or set(),
        )
        edges.append(
            EdgeInfo(
                kind="CALLS",
                source=caller,
                target=target,
                file_path=file_path,
                line=node.start_point[0] + 1,
            )
        )
        edges.extend(parser._detect_cross_language_bridge(node, language, file_path, caller))

    parser._extract_from_tree(
        node,
        source,
        language,
        file_path,
        nodes,
        edges,
        enclosing_class=enclosing_class,
        enclosing_func=enclosing_func,
        import_map=import_map,
        defined_names=defined_names,
    )
    return True


def _handle_r_class_call(
    parser: "CodeParser",
    node,
    source: bytes,
    language: str,
    file_path: str,
    nodes: list[NodeInfo],
    edges: list[EdgeInfo],
    enclosing_class: Optional[str],
    enclosing_func: Optional[str],
    import_map: Optional[dict[str, str]],
    defined_names: Optional[set[str]],
    assign_name: Optional[str] = None,
) -> bool:
    """Handle setClass/setRefClass/setGeneric calls -> Class nodes."""
    class_name = _r_first_string_arg(node) or assign_name
    if not class_name:
        return False

    qualified = parser._qualify(class_name, file_path, enclosing_class)
    nodes.append(
        NodeInfo(
            kind="Class",
            name=class_name,
            file_path=file_path,
            line_start=node.start_point[0] + 1,
            line_end=node.end_point[0] + 1,
            language=language,
            parent_name=enclosing_class,
        )
    )
    edges.append(
        EdgeInfo(
            kind="CONTAINS",
            source=file_path,
            target=qualified,
            file_path=file_path,
            line=node.start_point[0] + 1,
        )
    )

    methods_list = _r_find_named_arg(node, "methods")
    if methods_list is not None:
        extract_r_methods(
            parser,
            methods_list,
            source,
            language,
            file_path,
            nodes,
            edges,
            class_name,
            import_map,
            defined_names,
        )

    return True


def extract_r_methods(
    parser: "CodeParser",
    list_call,
    source: bytes,
    language: str,
    file_path: str,
    nodes: list[NodeInfo],
    edges: list[EdgeInfo],
    class_name: str,
    import_map: Optional[dict[str, str]],
    defined_names: Optional[set[str]],
) -> None:
    """Extract methods from a setRefClass methods = list(...) call."""
    for method_name, func_def in _r_iter_args(list_call):
        if not method_name or func_def is None:
            continue
        if func_def.type != "function_definition":
            continue

        qualified = parser._qualify(method_name, file_path, class_name)
        params = parser._get_params(func_def, language, source)
        nodes.append(
            NodeInfo(
                kind="Function",
                name=method_name,
                file_path=file_path,
                line_start=func_def.start_point[0] + 1,
                line_end=func_def.end_point[0] + 1,
                language=language,
                parent_name=class_name,
                params=params,
            )
        )
        edges.append(
            EdgeInfo(
                kind="CONTAINS",
                source=parser._qualify(class_name, file_path, None),
                target=qualified,
                file_path=file_path,
                line=func_def.start_point[0] + 1,
            )
        )
        parser._extract_from_tree(
            func_def,
            source,
            language,
            file_path,
            nodes,
            edges,
            enclosing_class=class_name,
            enclosing_func=method_name,
            import_map=import_map,
            defined_names=defined_names,
        )
