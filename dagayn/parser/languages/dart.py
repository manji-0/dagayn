"""Dart call-site detection for the CodeParser walker."""

from __future__ import annotations

from pathlib import Path
from typing import Optional

from .._base.protocol import CodeParser
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
    # Dart call detection is always a side-effect (no early continue).
    _extract_dart_calls_from_children(
        parser,
        child,
        source,
        file_path,
        edges,
        enclosing_class,
        enclosing_func,
    )
    return False


def _extract_dart_calls_from_children(
    parser: "CodeParser",
    parent,
    source: bytes,
    file_path: str,
    edges: list[EdgeInfo],
    enclosing_class: Optional[str],
    enclosing_func: Optional[str],
) -> None:
    """Detect Dart call sites from a parent node's children (#87 bug 1).

    tree-sitter-dart does not emit a single ``call_expression`` node for
    Dart calls.  Instead it produces ``identifier`` / method-selector
    siblings followed by a ``selector`` whose child is ``argument_part``:

        identifier "print"
        selector
          argument_part

    And for method calls like ``obj.foo()`` the middle selector is a
    ``unconditional_assignable_selector`` holding the method name:

        identifier "obj"
        selector
          unconditional_assignable_selector "."
            identifier "foo"
        selector
          argument_part

    This walker scans the immediate children of ``parent`` for either
    shape and emits a ``CALLS`` edge.  Nested calls are picked up as
    ``_extract_from_tree`` recurses into child nodes.
    """
    call_name: Optional[str] = None
    for sub in parent.children:
        if sub.type == "identifier":
            call_name = sub.text.decode("utf-8", errors="replace")
            continue
        if sub.type == "selector":
            # Case A: selector > unconditional_assignable_selector > identifier
            # (updates call_name to the method name)
            method_name: Optional[str] = None
            has_arguments = False
            for ssub in sub.children:
                if ssub.type == "unconditional_assignable_selector":
                    for ident in ssub.children:
                        if ident.type == "identifier":
                            method_name = ident.text.decode("utf-8", errors="replace")
                            break
                elif ssub.type == "argument_part":
                    has_arguments = True
            if method_name is not None:
                call_name = method_name
            if has_arguments and call_name:
                src_qn = (
                    parser._qualify(enclosing_func, file_path, enclosing_class)
                    if enclosing_func
                    else file_path
                )
                edges.append(
                    EdgeInfo(
                        kind="CALLS",
                        source=src_qn,
                        target=call_name,
                        file_path=file_path,
                        line=parent.start_point[0] + 1,
                    )
                )
                # After emitting for this call, clear call_name so we
                # don't re-emit on any trailing chained selector.
                call_name = None
            continue
        # Non-identifier, non-selector children don't change the
        # pending call name (``return``, ``await``, ``yield``, etc.)
        # but anything unexpected should reset it to avoid spurious
        # edges across unrelated siblings.
        if sub.type not in ("return", "await", "yield", "this", "const", "new"):
            call_name = None


def find_dart_pubspec_root(
    parser: "CodeParser",
    start: Path,
    pkg_name: str,
) -> Optional[Path]:
    """Walk up from ``start`` to find a ``pubspec.yaml`` whose ``name:``
    matches ``pkg_name``. Returns the directory containing that pubspec,
    or None if no match is found. Result is cached per (start, pkg_name)
    pair so repeated lookups within one parse pass are cheap.
    """
    import re

    cache_key = (str(start), pkg_name)
    cached = parser._dart_pubspec_cache.get(cache_key)
    if cached is not None or cache_key in parser._dart_pubspec_cache:
        return cached
    current = start
    # Avoid infinite loops on weird symlinks.
    for _ in range(20):
        pubspec = current / "pubspec.yaml"
        if pubspec.is_file():
            try:
                text = pubspec.read_text(encoding="utf-8", errors="replace")
            except OSError:
                text = ""
            m = re.search(r"^name:\s*([\w-]+)", text, re.MULTILINE)
            if m and m.group(1) == pkg_name:
                parser._dart_pubspec_cache[cache_key] = current
                return current
        if current.parent == current:
            break
        current = current.parent
    parser._dart_pubspec_cache[cache_key] = None
    return None
