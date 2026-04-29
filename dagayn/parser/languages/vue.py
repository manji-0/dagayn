from __future__ import annotations

from pathlib import Path

from .._base.protocol import CodeParser
from .._base.test_detection import is_test_file as _is_test_file
from .._base.types import EdgeInfo, NodeInfo


def parse(parser: "CodeParser", path: Path, source: bytes) -> tuple[list[NodeInfo], list[EdgeInfo]]:
    """Parse a Vue SFC by extracting <script> blocks and delegating to JS/TS."""
    vue_parser = parser._get_parser("vue")
    if not vue_parser:
        return [], []

    tree = vue_parser.parse(source)
    file_path_str = str(path)
    test_file = _is_test_file(file_path_str)

    all_nodes: list[NodeInfo] = [
        NodeInfo(
            kind="File",
            name=file_path_str,
            file_path=file_path_str,
            line_start=1,
            line_end=source.count(b"\n") + 1,
            language="vue",
            is_test=test_file,
        )
    ]
    all_edges: list[EdgeInfo] = []

    # Find script_element blocks in the Vue AST
    for child in tree.root_node.children:
        if child.type != "script_element":
            continue

        # Detect language from lang="ts" attribute
        script_lang = "javascript"
        start_tag = None
        raw_text_node = None
        for sub in child.children:
            if sub.type == "start_tag":
                start_tag = sub
            elif sub.type == "raw_text":
                raw_text_node = sub

        if start_tag:
            for attr in start_tag.children:
                if attr.type == "attribute":
                    attr_name = None
                    attr_value = None
                    for a in attr.children:
                        if a.type == "attribute_name":
                            attr_name = a.text.decode("utf-8", errors="replace")
                        elif a.type == "quoted_attribute_value":
                            for v in a.children:
                                if v.type == "attribute_value":
                                    attr_value = v.text.decode(
                                        "utf-8",
                                        errors="replace",
                                    )
                    if attr_name == "lang" and attr_value in ("ts", "typescript"):
                        script_lang = "typescript"

        if not raw_text_node:
            continue

        script_source = raw_text_node.text
        line_offset = raw_text_node.start_point[0]  # 0-based line of raw_text start

        # Parse the script block with the appropriate JS/TS parser
        script_parser = parser._get_parser(script_lang)
        if not script_parser:
            continue

        script_tree = script_parser.parse(script_source)

        # Collect imports and defined names from the script block
        import_map, defined_names = parser._collect_file_scope(
            script_tree.root_node,
            script_lang,
            script_source,
        )

        nodes: list[NodeInfo] = []
        edges: list[EdgeInfo] = []
        parser._extract_from_tree(
            script_tree.root_node,
            script_source,
            script_lang,
            file_path_str,
            nodes,
            edges,
            import_map=import_map,
            defined_names=defined_names,
        )

        # Adjust line numbers to account for position within the .vue file
        for node in nodes:
            node.line_start += line_offset
            node.line_end += line_offset
            node.language = "vue"
        for edge in edges:
            edge.line += line_offset

        all_nodes.extend(nodes)
        all_edges.extend(edges)

    # Generate TESTED_BY edges
    if test_file:
        test_qnames = set()
        for n in all_nodes:
            if n.is_test:
                qn = parser._qualify(n.name, n.file_path, n.parent_name)
                test_qnames.add(qn)
        for edge in list(all_edges):
            if edge.kind == "CALLS" and edge.source in test_qnames:
                all_edges.append(
                    EdgeInfo(
                        kind="TESTED_BY",
                        source=edge.target,
                        target=edge.source,
                        file_path=edge.file_path,
                        line=edge.line,
                    )
                )

    return all_nodes, all_edges
