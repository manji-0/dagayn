"""Terraform construct extraction for the CodeParser walker."""

from __future__ import annotations

import re
from typing import Optional

from .._base.protocol import CodeParser
from .._base.types import EdgeInfo, NodeInfo

_TERRAFORM_REFERENCE_RE = re.compile(
    r"\b(?:(data)\.([A-Za-z_][A-Za-z0-9_]*)\.([A-Za-z_][A-Za-z0-9_]*)"
    r"|((?:module|var|local|output|provider|check))\.([A-Za-z_][A-Za-z0-9_]*)"
    r"|([A-Za-z_][A-Za-z0-9_]*)\.([A-Za-z_][A-Za-z0-9_]*))\b"
)
_TERRAFORM_CALL_RE = re.compile(r"\b([A-Za-z_][A-Za-z0-9_]*)\s*\(")
_TERRAFORM_REFERENCE_SKIP_ROOTS = frozenset(
    {
        "count",
        "each",
        "ingress",
        "egress",
        "path",
        "self",
        "terraform",
    }
)
_TERRAFORM_CALL_SKIP = frozenset({"for", "if"})


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
    return _extract_terraform_constructs(
        parser,
        child,
        node_type,
        source,
        file_path,
        nodes,
        edges,
        enclosing_class,
        enclosing_func,
        import_map,
        defined_names,
        _depth,
    )


def _strip_tf_string(value: str) -> str:
    value = value.strip()
    if len(value) >= 2 and value[0] == value[-1] and value[0] in ("'", '"'):
        return value[1:-1]
    return value


def _terraform_field_text(node, field_name: str) -> Optional[str]:
    field_node = _terraform_field_node(node, field_name)
    if field_node is None:
        return None
    return field_node.text.decode("utf-8", errors="replace")


def _terraform_field_node(node, field_name: str):
    field_node = None
    try:
        field_node = node.child_by_field_name(field_name)
    except AttributeError:
        field_node = None
    return field_node


def _terraform_collect_references(
    text: str,
    caller: str,
    file_path: str,
    line: int,
    edges: list[EdgeInfo],
) -> None:
    seen: set[str] = set()
    for match in _TERRAFORM_REFERENCE_RE.finditer(text):
        target = None
        if match.group(1):
            target = f"data.{match.group(2)}.{match.group(3)}"
        elif match.group(4):
            target = f"{match.group(4)}.{match.group(5)}"
        else:
            root = match.group(6)
            name = match.group(7)
            if root in _TERRAFORM_REFERENCE_SKIP_ROOTS:
                continue
            target = f"resource.{root}.{name}"
        if not target or target == caller or target in seen:
            continue
        seen.add(target)
        edges.append(
            EdgeInfo(
                kind="REFERENCES",
                source=caller,
                target=target,
                file_path=file_path,
                line=line,
            )
        )


def _terraform_collect_calls(
    text: str,
    caller: str,
    file_path: str,
    line: int,
    edges: list[EdgeInfo],
) -> None:
    seen: set[str] = set()
    for match in _TERRAFORM_CALL_RE.finditer(text):
        name = match.group(1)
        if name in _TERRAFORM_CALL_SKIP or name in seen:
            continue
        seen.add(name)
        edges.append(
            EdgeInfo(
                kind="CALLS",
                source=caller,
                target=name,
                file_path=file_path,
                line=line,
            )
        )


def _terraform_scan_body(
    body_text: str,
    caller: str,
    file_path: str,
    line: int,
    edges: list[EdgeInfo],
) -> None:
    _terraform_collect_calls(body_text, caller, file_path, line, edges)
    _terraform_collect_references(body_text, caller, file_path, line, edges)


def _get_terraform_defined_name(node) -> Optional[str]:
    node_type = node.type
    if node_type in ("resource_block", "data_block", "ephemeral_block"):
        block_type = _terraform_field_text(node, "type")
        name = _terraform_field_text(node, "name")
        if block_type and name:
            prefix = node_type.removesuffix("_block")
            return f"{prefix}.{_strip_tf_string(block_type)}.{_strip_tf_string(name)}"
    if node_type in (
        "module_block",
        "provider_block",
        "variable_block",
        "output_block",
        "check_block",
    ):
        name = _terraform_field_text(node, "name")
        if name:
            prefix = {
                "module_block": "module",
                "provider_block": "provider",
                "variable_block": "var",
                "output_block": "output",
                "check_block": "check",
            }[node_type]
            return f"{prefix}.{_strip_tf_string(name)}"
    if node_type == "terraform_block":
        return "terraform"
    return None


def _extract_terraform_constructs(
    parser: "CodeParser",
    child,
    node_type: str,
    source: bytes,
    file_path: str,
    nodes: list[NodeInfo],
    edges: list[EdgeInfo],
    enclosing_class: Optional[str],
    enclosing_func: Optional[str],
    import_map: Optional[dict[str, str]],
    defined_names: Optional[set[str]],
    _depth: int,
) -> bool:
    del source, import_map, defined_names

    if node_type == "locals_block":
        body_text = _terraform_field_text(child, "body") or ""
        body_node = _terraform_field_node(child, "body")
        if body_node is None:
            return True
        for attr in body_node.children:
            if attr.type != "attribute":
                continue
            attr_name = _terraform_field_text(attr, "name")
            if not attr_name:
                continue
            node_name = f"local.{attr_name}"
            qualified = parser._qualify(node_name, file_path, None)
            nodes.append(
                NodeInfo(
                    kind="Function",
                    name=node_name,
                    file_path=file_path,
                    line_start=attr.start_point[0] + 1,
                    line_end=attr.end_point[0] + 1,
                    language="terraform",
                    extra={"terraform_kind": "local"},
                )
            )
            edges.append(
                EdgeInfo(
                    kind="CONTAINS",
                    source=file_path,
                    target=qualified,
                    file_path=file_path,
                    line=attr.start_point[0] + 1,
                )
            )
            attr_text = attr.text.decode("utf-8", errors="replace")
            _terraform_scan_body(
                attr_text,
                node_name,
                file_path,
                attr.start_point[0] + 1,
                edges,
            )
        return True

    if node_type in ("import_block", "moved_block", "removed_block"):
        body_text = _terraform_field_text(child, "body") or ""
        attrs: dict[str, str] = {}
        body_node = _terraform_field_node(child, "body")
        if body_node is not None:
            for attr in body_node.children:
                if attr.type != "attribute":
                    continue
                attr_name = _terraform_field_text(attr, "name")
                attr_value = _terraform_field_text(attr, "value")
                if attr_name and attr_value:
                    attrs[attr_name] = attr_value

        if node_type == "import_block":
            target = attrs.get("id") or attrs.get("to")
            if target:
                edges.append(
                    EdgeInfo(
                        kind="IMPORTS_FROM",
                        source=file_path,
                        target=_strip_tf_string(target),
                        file_path=file_path,
                        line=child.start_point[0] + 1,
                    )
                )
        elif node_type == "moved_block":
            from_target = attrs.get("from")
            to_target = attrs.get("to")
            if from_target and to_target:
                edges.append(
                    EdgeInfo(
                        kind="REFERENCES",
                        source=_strip_tf_string(from_target),
                        target=_strip_tf_string(to_target),
                        file_path=file_path,
                        line=child.start_point[0] + 1,
                        extra={"terraform_kind": "moved"},
                    )
                )
        elif node_type == "removed_block":
            from_target = attrs.get("from")
            if from_target:
                edges.append(
                    EdgeInfo(
                        kind="REFERENCES",
                        source=file_path,
                        target=_strip_tf_string(from_target),
                        file_path=file_path,
                        line=child.start_point[0] + 1,
                        extra={"terraform_kind": "removed"},
                    )
                )

        _terraform_scan_body(
            body_text,
            file_path,
            file_path,
            child.start_point[0] + 1,
            edges,
        )
        return True

    node_name = _get_terraform_defined_name(child)
    if node_name is None:
        return False

    kind = "Class"
    terraform_kind = node_type.removesuffix("_block")
    if node_type in ("variable_block", "output_block", "locals_block"):
        kind = "Function"
    elif node_type == "check_block":
        kind = "Test"

    qualified = parser._qualify(node_name, file_path, None)
    node = NodeInfo(
        kind=kind,
        name=node_name,
        file_path=file_path,
        line_start=child.start_point[0] + 1,
        line_end=child.end_point[0] + 1,
        language="terraform",
        is_test=node_type == "check_block",
        extra={"terraform_kind": terraform_kind},
    )
    nodes.append(node)
    edges.append(
        EdgeInfo(
            kind="CONTAINS",
            source=file_path,
            target=qualified,
            file_path=file_path,
            line=child.start_point[0] + 1,
        )
    )

    body_text = _terraform_field_text(child, "body") or child.text.decode(
        "utf-8",
        errors="replace",
    )
    _terraform_scan_body(
        body_text,
        node_name,
        file_path,
        child.start_point[0] + 1,
        edges,
    )

    if node_type == "module_block":
        body_node = _terraform_field_node(child, "body")
        if body_node is not None:
            for attr in body_node.children:
                if attr.type != "attribute":
                    continue
                attr_name = _terraform_field_text(attr, "name")
                attr_value = _terraform_field_text(attr, "value")
                if attr_name == "source" and attr_value:
                    edges.append(
                        EdgeInfo(
                            kind="IMPORTS_FROM",
                            source=qualified,
                            target=_strip_tf_string(attr_value),
                            file_path=file_path,
                            line=attr.start_point[0] + 1,
                        )
                    )

    if node_type == "terraform_block":
        for match in re.finditer(r"source\s*=\s*([\"'][^\"']+[\"'])", body_text):
            edges.append(
                EdgeInfo(
                    kind="DEPENDS_ON",
                    source=qualified,
                    target=_strip_tf_string(match.group(1)),
                    file_path=file_path,
                    line=child.start_point[0] + 1,
                )
            )

    return True
