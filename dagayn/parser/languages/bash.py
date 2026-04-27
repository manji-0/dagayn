"""Bash source-command detection for the CodeParser walker."""

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
    if node_type == "command":
        return _extract_bash_source_command(parser, child, file_path, edges)
    return False


def _extract_bash_source_command(
    parser: "CodeParser",
    node,
    file_path: str,
    edges: list[EdgeInfo],
) -> bool:
    """Detect ``source foo.sh`` / ``. foo.sh`` and emit an IMPORTS_FROM
    edge. Returns True if handled (so the main loop skips recursing
    into this command). See: #197
    """
    command_name: Optional[str] = None
    args: list[str] = []
    for sub in node.children:
        if sub.type == "command_name":
            command_name = sub.text.decode("utf-8", errors="replace").strip()
        elif sub.type in ("word", "string", "raw_string") and command_name:
            txt = sub.text.decode("utf-8", errors="replace").strip()
            # Strip surrounding quotes if present
            if len(txt) >= 2 and txt[0] in ("'", '"') and txt[-1] == txt[0]:
                txt = txt[1:-1]
            if txt:
                args.append(txt)
    if command_name in ("source", ".") and args:
        target = args[0]
        # Try to resolve relative paths to real files
        resolved = parser._resolve_module_to_file(target, file_path, "bash")
        edges.append(
            EdgeInfo(
                kind="IMPORTS_FROM",
                source=file_path,
                target=resolved if resolved else target,
                file_path=file_path,
                line=node.start_point[0] + 1,
            )
        )
        return True
    return False
