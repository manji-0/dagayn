"""Value-reference and JSX-component extraction functions.

All functions operate on a CodeParser instance (passed as ``parser``) plus
tree-sitter nodes and auxiliary data.  Splitting them out keeps core.py
focused on orchestration.
"""

from __future__ import annotations

from typing import TYPE_CHECKING, Optional

from . import node_shape
from ._base.types import EdgeInfo

if TYPE_CHECKING:
    from .core import CodeParser

# ---------------------------------------------------------------------------
# Constants (previously CodeParser class attributes)
# ---------------------------------------------------------------------------

_PAIR_TYPES: frozenset[str] = frozenset({"pair"})
_ARRAY_TYPES: frozenset[str] = frozenset({"array", "list"})
_VALUE_REF_SKIP_NAMES: frozenset[str] = frozenset(
    {
        "true",
        "false",
        "null",
        "undefined",
        "None",
        "True",
        "False",
        "self",
        "this",
        "cls",
        "super",
    }
)

# ---------------------------------------------------------------------------
# JSX component extraction
# ---------------------------------------------------------------------------


def _extract_jsx_component_call(
    parser: "CodeParser",
    child,
    language: str,
    file_path: str,
    edges: list[EdgeInfo],
    enclosing_class: Optional[str],
    enclosing_func: Optional[str],
    import_map: Optional[dict[str, str]],
    defined_names: Optional[set[str]],
) -> None:
    """Emit a synthetic CALLS edge for JSX component usage."""
    target = _resolve_jsx_component_target(
        parser,
        child,
        language,
        file_path,
        import_map or {},
        defined_names or set(),
    )
    if not target:
        return

    caller = (
        node_shape._qualify(enclosing_func, file_path, enclosing_class)
        if enclosing_func
        else file_path
    )
    edges.append(
        EdgeInfo(
            kind="CALLS",
            source=caller,
            target=target,
            file_path=file_path,
            line=child.start_point[0] + 1,
        )
    )


def _resolve_jsx_component_target(
    parser: "CodeParser",
    node,
    language: str,
    file_path: str,
    import_map: dict[str, str],
    defined_names: set[str],
) -> Optional[str]:
    """Resolve a JSX component element to a call target."""
    component_ref = node_shape._get_jsx_component_reference(node)
    if component_ref is None:
        return None

    base_name, component_name = component_ref
    if base_name is None:
        return parser._resolve_call_target(
            component_name,
            file_path,
            language,
            import_map,
            defined_names,
        )

    if base_name in import_map:
        resolved = parser._resolve_imported_symbol(
            component_name,
            import_map[base_name],
            file_path,
            language,
        )
        if resolved:
            return resolved

    return component_name


# ---------------------------------------------------------------------------
# Value-reference extraction (function-as-value patterns)
# ---------------------------------------------------------------------------


def _extract_value_references(
    parser: "CodeParser",
    child,
    node_type: str,
    source: bytes,
    language: str,
    file_path: str,
    edges: list[EdgeInfo],
    enclosing_class: Optional[str],
    enclosing_func: Optional[str],
    import_map: Optional[dict[str, str]],
    defined_names: Optional[set[str]],
) -> None:
    """Emit REFERENCES edges for function-as-value patterns.

    Detects identifiers in value positions that likely refer to functions —
    object literal values, map property assignments, array elements, and
    callback arguments.
    """
    imap = import_map or {}
    dnames = defined_names or set()

    caller = (
        node_shape._qualify(enclosing_func, file_path, enclosing_class)
        if enclosing_func
        else file_path
    )

    if node_type in _PAIR_TYPES:
        _ref_from_pair(parser, child, source, language, file_path, caller, edges, imap, dnames)
        return

    if node_type == "shorthand_property_identifier" and language in (
        "javascript",
        "typescript",
        "tsx",
    ):
        name = child.text.decode("utf-8", errors="replace")
        _emit_reference_if_known(
            parser,
            name,
            language,
            file_path,
            caller,
            edges,
            imap,
            dnames,
            line=child.start_point[0] + 1,
        )
        return

    if node_type in ("assignment_expression", "augmented_assignment", "assignment"):
        _ref_from_assignment(
            parser, child, source, language, file_path, caller, edges, imap, dnames
        )
        return

    if node_type in _ARRAY_TYPES:
        _ref_from_array(parser, child, source, language, file_path, caller, edges, imap, dnames)
        return

    if node_type == "arguments":
        _ref_from_arguments(parser, child, source, language, file_path, caller, edges, imap, dnames)


def _emit_reference_if_known(
    parser: "CodeParser",
    name: str,
    language: str,
    file_path: str,
    caller: str,
    edges: list[EdgeInfo],
    import_map: dict[str, str],
    defined_names: set[str],
    line: int = 0,
) -> None:
    """Emit a REFERENCES edge if *name* is a known function/import."""
    if not name or name in _VALUE_REF_SKIP_NAMES:
        return
    if name.isupper() or len(name) <= 1:
        return
    if name not in defined_names and name not in import_map:
        return

    target = parser._resolve_call_target(
        name,
        file_path,
        language,
        import_map,
        defined_names,
    )
    edges.append(
        EdgeInfo(
            kind="REFERENCES",
            source=caller,
            target=target,
            file_path=file_path,
            line=line,
        )
    )


def _ref_from_pair(
    parser: "CodeParser",
    pair_node,
    source: bytes,
    language: str,
    file_path: str,
    caller: str,
    edges: list[EdgeInfo],
    import_map: dict[str, str],
    defined_names: set[str],
) -> None:
    """Extract a REFERENCES edge from an object/dict literal pair value."""
    children = pair_node.children
    value_node = None
    for ch in reversed(children):
        if ch.type not in (":", ",", "comment"):
            value_node = ch
            break
    if value_node is None:
        return
    if value_node.type == "identifier":
        name = value_node.text.decode("utf-8", errors="replace")
        _emit_reference_if_known(
            parser,
            name,
            language,
            file_path,
            caller,
            edges,
            import_map,
            defined_names,
            line=value_node.start_point[0] + 1,
        )


def _ref_from_assignment(
    parser: "CodeParser",
    assign_node,
    source: bytes,
    language: str,
    file_path: str,
    caller: str,
    edges: list[EdgeInfo],
    import_map: dict[str, str],
    defined_names: set[str],
) -> None:
    """Extract REFERENCES from ``obj.key = fnRef`` or ``obj['key'] = fnRef``."""
    children = assign_node.children
    if len(children) < 3:
        return
    lhs = children[0]
    if lhs.type not in (
        "member_expression",
        "subscript_expression",
        "attribute",
        "subscript",
    ):
        return
    rhs = None
    for ch in reversed(children):
        if ch.type not in ("=", ":", ",", "comment", "type_annotation"):
            rhs = ch
            break
    if rhs is None or rhs.type != "identifier":
        return
    name = rhs.text.decode("utf-8", errors="replace")
    _emit_reference_if_known(
        parser,
        name,
        language,
        file_path,
        caller,
        edges,
        import_map,
        defined_names,
        line=rhs.start_point[0] + 1,
    )


def _ref_from_array(
    parser: "CodeParser",
    array_node,
    source: bytes,
    language: str,
    file_path: str,
    caller: str,
    edges: list[EdgeInfo],
    import_map: dict[str, str],
    defined_names: set[str],
) -> None:
    """Extract REFERENCES from array/list elements that are identifiers."""
    for ch in array_node.children:
        if ch.type == "identifier":
            name = ch.text.decode("utf-8", errors="replace")
            _emit_reference_if_known(
                parser,
                name,
                language,
                file_path,
                caller,
                edges,
                import_map,
                defined_names,
                line=ch.start_point[0] + 1,
            )


def _ref_from_arguments(
    parser: "CodeParser",
    args_node,
    source: bytes,
    language: str,
    file_path: str,
    caller: str,
    edges: list[EdgeInfo],
    import_map: dict[str, str],
    defined_names: set[str],
) -> None:
    """Extract REFERENCES from identifier arguments (callbacks)."""
    for ch in args_node.children:
        if ch.type == "identifier":
            name = ch.text.decode("utf-8", errors="replace")
            _emit_reference_if_known(
                parser,
                name,
                language,
                file_path,
                caller,
                edges,
                import_map,
                defined_names,
                line=ch.start_point[0] + 1,
            )
