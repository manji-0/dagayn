"""Elixir construct extraction for the CodeParser walker."""

from __future__ import annotations

from typing import TYPE_CHECKING, Optional

from ..test_detection import is_test_function as _is_test_function
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
    if node_type == "call":
        return _extract_elixir_constructs(
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
            _depth,
        )
    return False


def _elixir_call_identifier(node) -> Optional[str]:
    """Return the leading identifier of an Elixir ``call`` node.

    For ``def add(a, b)`` returns ``"def"``; for ``defmodule Calc``
    returns ``"defmodule"``; for ``IO.puts(msg)`` returns the dotted
    path's final identifier (``"puts"``); for ``alias Calculator``
    returns ``"alias"``.
    """
    if not node.children:
        return None
    first = node.children[0]
    if first.type == "identifier":
        return first.text.decode("utf-8", errors="replace")
    # Dotted calls: dot > left: alias "IO", right: identifier "puts"
    if first.type == "dot":
        for child in reversed(first.children):
            if child.type == "identifier":
                return child.text.decode("utf-8", errors="replace")
    return None


def _elixir_module_name(arguments) -> Optional[str]:
    """Extract a module name from a ``defmodule`` / ``alias`` / etc.
    arguments node. Supports ``Calc`` (single alias) and ``Foo.Bar``
    (dotted alias inside a `dot` node).
    """
    for child in arguments.children:
        if child.type == "alias":
            return child.text.decode("utf-8", errors="replace")
        if child.type == "dot":
            return child.text.decode("utf-8", errors="replace")
    return None


def _elixir_function_name_and_params(
    arguments,
    source: bytes,
) -> tuple[Optional[str], Optional[str]]:
    """Extract the function name and parameter list from a ``def``/
    ``defp``/``defmacro`` arguments node.

    The ``arguments`` of a ``def`` call wraps another ``call`` whose
    first child is the function's identifier and whose children
    (past the parens) are the parameters.
    """
    for child in arguments.children:
        if child.type == "call":
            name: Optional[str] = None
            for sub in child.children:
                if sub.type == "identifier" and name is None:
                    name = sub.text.decode("utf-8", errors="replace")
            # Parameter text is everything between the parens of
            # the inner call; source slice is simplest.
            params_text = child.text.decode("utf-8", errors="replace")
            # Strip the function name off the front.
            if name and params_text.startswith(name):
                params_text = params_text[len(name) :]
            return name, params_text
        if child.type == "identifier":
            # Zero-arity def like `def reset, do: ...` has no inner
            # call; just the identifier.
            return child.text.decode("utf-8", errors="replace"), None
    return None, None


def _extract_elixir_constructs(
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
    _depth: int,
) -> bool:
    """Handle every Elixir ``call`` node by dispatching on the leading
    identifier. See: #112

    Returns True if the node was fully handled (and the main loop
    should skip generic recursion); False to let the default dispatch
    continue (never used here — Elixir has no other node types).
    """
    ident = _elixir_call_identifier(node)
    if ident is None:
        return False

    # ---- defmodule Name do ... end ----------------------------------
    if ident == "defmodule":
        arguments = None
        do_block = None
        for sub in node.children:
            if sub.type == "arguments":
                arguments = sub
            elif sub.type == "do_block":
                do_block = sub
        if arguments is None:
            return False
        mod_name = _elixir_module_name(arguments)
        if mod_name is None:
            return False
        qualified = parser._qualify(mod_name, file_path, None)
        nodes.append(
            NodeInfo(
                kind="Class",
                name=mod_name,
                file_path=file_path,
                line_start=node.start_point[0] + 1,
                line_end=node.end_point[0] + 1,
                language=language,
                parent_name=None,
            )
        )
        # CONTAINS file -> module
        edges.append(
            EdgeInfo(
                kind="CONTAINS",
                source=file_path,
                target=qualified,
                file_path=file_path,
                line=node.start_point[0] + 1,
            )
        )
        if do_block is not None:
            parser._extract_from_tree(
                do_block,
                source,
                language,
                file_path,
                nodes,
                edges,
                enclosing_class=mod_name,
                enclosing_func=None,
                import_map=import_map,
                defined_names=defined_names,
                _depth=_depth + 1,
            )
        return True

    # ---- def / defp / defmacro / defmacrop -------------------------
    if ident in ("def", "defp", "defmacro", "defmacrop"):
        arguments = None
        do_block = None
        for sub in node.children:
            if sub.type == "arguments":
                arguments = sub
            elif sub.type == "do_block":
                do_block = sub
        if arguments is None:
            return False
        fn_name, params = _elixir_function_name_and_params(
            arguments,
            source,
        )
        if fn_name is None:
            return False
        is_test = _is_test_function(fn_name, file_path)
        kind = "Test" if is_test else "Function"
        qualified = parser._qualify(fn_name, file_path, enclosing_class)
        nodes.append(
            NodeInfo(
                kind=kind,
                name=fn_name,
                file_path=file_path,
                line_start=node.start_point[0] + 1,
                line_end=node.end_point[0] + 1,
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
                line=node.start_point[0] + 1,
            )
        )
        if do_block is not None:
            parser._extract_from_tree(
                do_block,
                source,
                language,
                file_path,
                nodes,
                edges,
                enclosing_class=enclosing_class,
                enclosing_func=fn_name,
                import_map=import_map,
                defined_names=defined_names,
                _depth=_depth + 1,
            )
        return True

    # ---- alias / import / require / use ----------------------------
    if ident in ("alias", "import", "require", "use"):
        for sub in node.children:
            if sub.type == "arguments":
                mod = _elixir_module_name(sub)
                if mod is not None:
                    edges.append(
                        EdgeInfo(
                            kind="IMPORTS_FROM",
                            source=file_path,
                            target=mod,
                            file_path=file_path,
                            line=node.start_point[0] + 1,
                        )
                    )
                break
        return True

    # ---- Everything else = a regular function/method call ----------
    # Module-scope calls attribute to the File node (same rule as the
    # generic _extract_calls path).
    # For dotted calls like `IO.puts(msg)`, prefer the dotted
    # identifier; for bare calls use the first identifier.
    call_name = ident
    caller = (
        parser._qualify(enclosing_func, file_path, enclosing_class) if enclosing_func else file_path
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
    # Recurse into arguments + do_block so nested calls are caught.
    for sub in node.children:
        if sub.type in ("arguments", "do_block"):
            parser._extract_from_tree(
                sub,
                source,
                language,
                file_path,
                nodes,
                edges,
                enclosing_class=enclosing_class,
                enclosing_func=enclosing_func,
                import_map=import_map,
                defined_names=defined_names,
                _depth=_depth + 1,
            )
    return True
