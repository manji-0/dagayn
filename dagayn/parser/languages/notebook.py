from __future__ import annotations

import json
import re
from pathlib import Path
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from ..core import CodeParser

from ..test_detection import is_test_file as _is_test_file
from ..types import CellInfo, EdgeInfo, NodeInfo

_SQL_TABLE_RE = re.compile(
    r"(?:FROM|JOIN|INTO|CREATE\s+(?:OR\s+REPLACE\s+)?(?:TABLE|VIEW)|INSERT\s+OVERWRITE)"
    r"\s+((?:`[^`]+`|\w+)(?:\.(?:`[^`]+`|\w+))*)",
    re.IGNORECASE,
)


def parse(parser: "CodeParser", path: Path, source: bytes) -> tuple[list[NodeInfo], list[EdgeInfo]]:
    """Parse a Jupyter notebook by extracting code cells."""
    try:
        nb = json.loads(source)
    except (json.JSONDecodeError, UnicodeDecodeError):
        return [], []

    # Determine kernel language
    kernel_lang = (
        nb.get("metadata", {}).get("kernelspec", {}).get("language")
        or nb.get("metadata", {}).get("language_info", {}).get("name")
        or "python"
    ).lower()

    # Only parse supported languages
    supported = {"python", "r"}
    if kernel_lang not in supported:
        return [], []

    # Build CellInfo list from code cells
    cells: list[CellInfo] = []
    magic_lang_map = {
        "%python": "python",
        "%sql": "sql",
        "%r": "r",
    }
    skip_magics = {"%scala", "%md", "%sh"}

    for cell_idx, cell in enumerate(nb.get("cells", [])):
        if cell.get("cell_type") != "code":
            continue
        lines = cell.get("source", [])
        if isinstance(lines, str):
            lines = lines.splitlines(keepends=True)
        if not lines:
            continue

        # Check first line for language-switching magic
        first_line = lines[0].strip()
        cell_lang = kernel_lang
        cell_lines = lines

        for magic, lang in magic_lang_map.items():
            if first_line == magic or first_line.startswith(magic + " "):
                cell_lang = lang
                cell_lines = lines[1:]  # strip magic line
                break
        else:
            # Check for skip magics
            for skip in skip_magics:
                if first_line == skip or first_line.startswith(skip + " "):
                    cell_lines = []
                    break

        # Filter %pip, ! lines from Python/R content (not SQL)
        if cell_lang in ("python", "r"):
            filtered = [ln for ln in cell_lines if not ln.lstrip().startswith(("%", "!"))]
        else:
            filtered = cell_lines
        if not filtered:
            continue

        cell_source = "".join(filtered)
        cells.append(CellInfo(cell_index=cell_idx, language=cell_lang, source=cell_source))

    if not cells:
        file_path_str = str(path)
        return [
            NodeInfo(
                kind="File",
                name=file_path_str,
                file_path=file_path_str,
                line_start=1,
                line_end=1,
                language=kernel_lang,
                is_test=_is_test_file(file_path_str),
            )
        ], []

    return _parse_notebook_cells(parser, path, cells, kernel_lang)


def _parse_notebook_cells(
    parser: "CodeParser",
    path: Path,
    cells: list[CellInfo],
    default_language: str,
) -> tuple[list[NodeInfo], list[EdgeInfo]]:
    """Parse notebook cells grouped by language.

    Args:
        parser: CodeParser instance.
        path: Notebook file path.
        cells: List of CellInfo with index, language, and source.
        default_language: Default language for the File node.
    """
    file_path_str = str(path)
    test_file = _is_test_file(file_path_str)

    # Group cells by language
    lang_cells: dict[str, list[CellInfo]] = {}
    for cell in cells:
        lang_cells.setdefault(cell.language, []).append(cell)

    all_nodes: list[NodeInfo] = []
    all_edges: list[EdgeInfo] = []

    # Track offsets per language for cell_index tagging.
    # Each language group is parsed independently by Tree-sitter,
    # so line numbers restart at 1 for each group.
    all_cell_offsets: list[tuple[int, int, int]] = []
    max_line = 1

    for lang, lang_group in lang_cells.items():
        if lang == "sql":
            # SQL: regex-based table extraction
            for cell in lang_group:
                for match in _SQL_TABLE_RE.finditer(cell.source):
                    table_name = match.group(1).replace("`", "")
                    all_edges.append(
                        EdgeInfo(
                            kind="IMPORTS_FROM",
                            source=file_path_str,
                            target=table_name,
                            file_path=file_path_str,
                            line=1,
                        )
                    )
            continue

        if lang not in ("python", "r"):
            continue

        ts_parser = parser._get_parser(lang)
        if not ts_parser:
            continue

        # Concatenate cells of this language.
        # Line numbers start at 1 for each language group because
        # Tree-sitter parses each concatenation independently.
        code_chunks: list[str] = []
        cell_offsets: list[tuple[int, int, int]] = []
        current_line = 1

        for cell in lang_group:
            cell_line_count = cell.source.count("\n") + (1 if not cell.source.endswith("\n") else 0)
            cell_offsets.append(
                (
                    cell.cell_index,
                    current_line,
                    current_line + cell_line_count - 1,
                )
            )
            code_chunks.append(cell.source)
            current_line += cell_line_count + 1

        concatenated = "\n".join(code_chunks)
        concat_bytes = concatenated.encode("utf-8")

        tree = ts_parser.parse(concat_bytes)

        import_map, defined_names = parser._collect_file_scope(
            tree.root_node,
            lang,
            concat_bytes,
        )
        parser._extract_from_tree(
            tree.root_node,
            concat_bytes,
            lang,
            file_path_str,
            all_nodes,
            all_edges,
            import_map=import_map,
            defined_names=defined_names,
        )

        all_cell_offsets.extend(cell_offsets)
        max_line = max(max_line, current_line)

    # Create File node
    file_node = NodeInfo(
        kind="File",
        name=file_path_str,
        file_path=file_path_str,
        line_start=1,
        line_end=max_line,
        language=default_language,
        is_test=test_file,
    )
    all_nodes.insert(0, file_node)

    # Resolve call targets
    all_edges = parser._resolve_call_targets(
        all_nodes,
        all_edges,
        file_path_str,
    )

    # Tag nodes with cell_index
    for node in all_nodes:
        if node.kind == "File":
            continue
        best_cell_idx: int | None = None
        best_overlap = -1
        for cell_idx, start, end in all_cell_offsets:
            overlap = min(node.line_end, end) - max(node.line_start, start) + 1
            if overlap > best_overlap and overlap > 0:
                best_overlap = overlap
                best_cell_idx = cell_idx
        if best_cell_idx is not None:
            node.extra["cell_index"] = best_cell_idx

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


def parse_databricks_py(
    parser: "CodeParser", path: Path, source: bytes
) -> tuple[list[NodeInfo], list[EdgeInfo]]:
    """Parse a Databricks .py notebook export."""
    text = source.decode("utf-8", errors="replace")

    # Strip the header line
    lines = text.split("\n")
    if lines and lines[0].strip() == "# Databricks notebook source":
        lines = lines[1:]

    # Split on COMMAND delimiters
    cell_chunks: list[list[str]] = [[]]
    for line in lines:
        if re.match(r"^# COMMAND\s*-+\s*$", line):
            cell_chunks.append([])
        else:
            cell_chunks[-1].append(line)

    # Classify each cell
    cells: list[CellInfo] = []
    magic_lang_map = {
        "# MAGIC %sql": "sql",
        "# MAGIC %r": "r",
    }
    skip_prefixes = ("# MAGIC %md", "# MAGIC %sh")

    for cell_idx, chunk in enumerate(cell_chunks):
        non_empty = [ln for ln in chunk if ln.strip()]
        if not non_empty:
            continue

        first_line = non_empty[0]

        # Check if all non-empty lines are MAGIC lines
        all_magic = all(ln.startswith("# MAGIC ") for ln in non_empty)

        # Detect language from the first MAGIC line (e.g. "# MAGIC %sql")
        cell_lang = None
        if all_magic:
            for prefix, lang in magic_lang_map.items():
                if first_line.startswith(prefix):
                    cell_lang = lang
                    break

        if cell_lang:
            # Strip "# MAGIC " prefix (8 chars) then skip the %lang directive line
            stripped = [ln[8:] if ln.startswith("# MAGIC ") else ln for ln in chunk]
            # Remove the first non-empty line if it's just the %lang directive
            stripped_non_empty = [ln for ln in stripped if ln.strip()]
            if stripped_non_empty and stripped_non_empty[0].strip().startswith("%"):
                # Drop the directive line from the source
                first_directive = stripped_non_empty[0]
                stripped = [ln for ln in stripped if ln != first_directive]
            cell_source = "\n".join(stripped)
            cells.append(
                CellInfo(
                    cell_index=cell_idx,
                    language=cell_lang,
                    source=cell_source,
                )
            )
            continue

        # Check for skip prefixes (md, sh)
        if all_magic and first_line.startswith(skip_prefixes):
            continue

        # Default: Python cell (mixed or no MAGIC)
        py_lines = [ln for ln in chunk if not ln.startswith("# MAGIC ")]
        cell_source = "\n".join(py_lines)
        cells.append(
            CellInfo(
                cell_index=cell_idx,
                language="python",
                source=cell_source,
            )
        )

    if not cells:
        file_path_str = str(path)
        file_node = NodeInfo(
            kind="File",
            name=file_path_str,
            file_path=file_path_str,
            line_start=1,
            line_end=1,
            language="python",
            is_test=_is_test_file(file_path_str),
        )
        file_node.extra["notebook_format"] = "databricks_py"
        return [file_node], []

    nodes, edges = _parse_notebook_cells(parser, path, cells, "python")

    # Tag File node with notebook_format
    for node in nodes:
        if node.kind == "File":
            node.extra["notebook_format"] = "databricks_py"
            break

    return nodes, edges
