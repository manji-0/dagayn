from __future__ import annotations

import re
from pathlib import Path
from typing import TYPE_CHECKING, Optional

if TYPE_CHECKING:
    from ..core import CodeParser

from ..test_detection import is_test_file as _is_test_file
from ..types import EdgeInfo, NodeInfo

_MARKDOWN_DIRECTIVE_RE = re.compile(
    r"<!--\s*(constrained-by|blocked-by|supersedes|derived-from)\s+(.+?)\s*-->",
    re.IGNORECASE,
)
_MARKDOWN_LINK_RE = re.compile(r"\[[^\]]+\]\(([^)]+)\)")
_MARKDOWN_REFERENCE_DEF_RE = re.compile(r"(?m)^\s*\[[^\]]+\]:\s*(\S+)")
_MARKDOWN_CODE_SPAN_RE = re.compile(r"`([^`\n]+)`")
_MARKDOWN_SYMBOL_RE = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)*$")
_MARKDOWN_SYMBOL_MIN_LEN = 3
_MARKDOWN_PLAIN_WORD_MIN_LEN = 10  # plain words (no _ or .) need longer to skip generic English


def parse(parser: "CodeParser", path: Path, source: bytes) -> tuple[list[NodeInfo], list[EdgeInfo]]:
    """Parse Markdown documents into section nodes and dependency edges."""
    file_path_str = str(path)
    test_file = _is_test_file(file_path_str)
    text = source.decode("utf-8", errors="replace")
    ts_parser = parser._get_parser("markdown")

    nodes: list[NodeInfo] = [
        NodeInfo(
            kind="File",
            name=file_path_str,
            file_path=file_path_str,
            line_start=1,
            line_end=source.count(b"\n") + 1,
            language="markdown",
            is_test=test_file,
        )
    ]
    edges: list[EdgeInfo] = []

    headings: list[dict[str, object]] = []
    if ts_parser:
        tree = ts_parser.parse(source)
        headings = _markdown_collect_headings(tree.root_node, source)

    if not headings:
        headings = _markdown_collect_headings_from_text(text)

    heading_stack: list[dict[str, object]] = []
    for heading in headings:
        level = int(str(heading["level"]))
        while heading_stack and int(str(heading_stack[-1]["level"])) >= level:
            heading_stack.pop()

        section_qname = f"{file_path_str}::{heading['slug']}"
        container = str(heading_stack[-1]["qname"]) if heading_stack else file_path_str
        nodes.append(
            NodeInfo(
                kind="Class",
                name=str(heading["slug"]),
                file_path=file_path_str,
                line_start=int(str(heading["line"])),
                line_end=int(str(heading["line"])),
                language="markdown",
                extra={
                    "markdown_kind": "section",
                    "display_name": str(heading["text"]),
                    "heading_level": level,
                },
            )
        )
        edges.append(
            EdgeInfo(
                kind="CONTAINS",
                source=container,
                target=section_qname,
                file_path=file_path_str,
                line=int(str(heading["line"])),
            )
        )
        heading_stack.append(
            {
                "level": level,
                "slug": heading["slug"],
                "line": heading["line"],
                "qname": section_qname,
            }
        )

    _extract_markdown_directives(path, text, file_path_str, headings, edges)
    _extract_markdown_links(path, text, file_path_str, headings, edges)
    _extract_markdown_code_spans(text, file_path_str, headings, edges)
    return nodes, _dedupe_markdown_edges(edges)


def _markdown_collect_headings(root, source: bytes) -> list[dict[str, object]]:
    """Collect headings in document order with GitHub-style unique slugs."""
    raw: list[dict[str, object]] = []

    def visit(node) -> None:
        if node.type in ("atx_heading", "setext_heading"):
            text = _markdown_heading_text(node, source)
            if text:
                raw.append(
                    {
                        "text": text,
                        "level": _markdown_heading_level(node),
                        "line": node.start_point[0] + 1,
                    }
                )
        for child in node.children:
            visit(child)

    visit(root)
    return _markdown_assign_heading_slugs(raw)


def _markdown_collect_headings_from_text(text: str) -> list[dict[str, object]]:
    """Fallback heading extraction when no Markdown parser is available."""
    raw: list[dict[str, object]] = []
    lines = text.splitlines()
    idx = 0
    while idx < len(lines):
        line = lines[idx]
        stripped = line.strip()
        if stripped.startswith("#"):
            marker = len(stripped) - len(stripped.lstrip("#"))
            if 1 <= marker <= 6 and len(stripped) > marker and stripped[marker] == " ":
                title = stripped[marker + 1 :].strip().rstrip("#").strip()
                if title:
                    raw.append(
                        {
                            "text": title,
                            "level": marker,
                            "line": idx + 1,
                        }
                    )
        elif idx + 1 < len(lines):
            underline = lines[idx + 1].strip()
            if stripped and underline and set(underline) <= {"="}:
                raw.append({"text": stripped, "level": 1, "line": idx + 1})
                idx += 1
            elif stripped and underline and set(underline) <= {"-"}:
                raw.append({"text": stripped, "level": 2, "line": idx + 1})
                idx += 1
        idx += 1
    return _markdown_assign_heading_slugs(raw)


def _markdown_assign_heading_slugs(
    raw_headings: list[dict[str, object]],
) -> list[dict[str, object]]:
    counts: dict[str, int] = {}
    assigned: set[str] = set()
    headings: list[dict[str, object]] = []

    for heading in raw_headings:
        base = _markdown_slugify(str(heading["text"]))
        n = counts.get(base, 0)
        if n == 0 and base not in assigned:
            slug = base
        else:
            k = max(1, n)
            while True:
                candidate = f"{base}-{k}"
                if candidate not in assigned:
                    slug = candidate
                    break
                k += 1
        counts[base] = n + 1
        assigned.add(slug)
        headings.append(
            {
                "text": heading["text"],
                "slug": slug,
                "level": heading["level"],
                "line": heading["line"],
            }
        )
    return headings


def _markdown_heading_level(node) -> int:
    for child in node.children:
        if child.type.startswith("atx_h") and child.type.endswith("_marker"):
            text = child.text.decode("utf-8", errors="replace")
            return len(text)
        if child.type == "setext_h1_underline":
            return 1
        if child.type == "setext_h2_underline":
            return 2
    return 1


def _markdown_heading_text(node, source: bytes) -> str:
    parts: list[str] = []
    for child in node.children:
        if child.type in {
            "atx_h1_marker",
            "atx_h2_marker",
            "atx_h3_marker",
            "atx_h4_marker",
            "atx_h5_marker",
            "atx_h6_marker",
            "setext_h1_underline",
            "setext_h2_underline",
        }:
            continue
        text = (
            source[child.start_byte : child.end_byte].decode("utf-8", errors="replace").strip()
        )
        if text:
            parts.append(text)
    return " ".join(parts).strip()


def _markdown_slugify(text: str) -> str:
    chars: list[str] = []
    for char in text:
        if char.isalnum():
            chars.append(char.lower())
        elif char in {" ", "-"}:
            chars.append("-")
        elif char == "_":
            chars.append("_")
    return "".join(chars)


def _markdown_section_for_line(
    line: int,
    file_path: str,
    headings: list[dict[str, object]],
) -> Optional[str]:
    section_slug: Optional[str] = None
    for heading in headings:
        if int(str(heading["line"])) > line:
            break
        section_slug = str(heading["slug"])
    if section_slug is None:
        return None
    return f"{file_path}::{section_slug}"


def _extract_markdown_directives(
    path: Path,
    text: str,
    file_path: str,
    headings: list[dict[str, object]],
    edges: list[EdgeInfo],
) -> None:
    for match in _MARKDOWN_DIRECTIVE_RE.finditer(text):
        kind = match.group(1).lower()
        raw_target = match.group(2).strip()
        line = text.count("\n", 0, match.start()) + 1
        source = _markdown_section_for_line(line, file_path, headings) or file_path
        target = _markdown_target(raw_target, path)
        if target is None:
            continue
        edges.append(
            EdgeInfo(
                kind="DEPENDS_ON",
                source=source,
                target=target,
                file_path=file_path,
                line=line,
                extra={"markdown_directive_kind": kind},
            )
        )
        if "::" in target:
            target_file = target.split("::", 1)[0]
        else:
            target_file = target
        if target_file != file_path:
            edges.append(
                EdgeInfo(
                    kind="IMPORTS_FROM",
                    source=file_path,
                    target=target_file,
                    file_path=file_path,
                    line=line,
                    extra={
                        "markdown_import_kind": "directive",
                        "markdown_directive_kind": kind,
                    },
                )
            )


def _extract_markdown_links(
    path: Path,
    text: str,
    file_path: str,
    headings: list[dict[str, object]],
    edges: list[EdgeInfo],
) -> None:
    for regex in (_MARKDOWN_LINK_RE, _MARKDOWN_REFERENCE_DEF_RE):
        for match in regex.finditer(text):
            raw_target = _markdown_normalize_link_target(match.group(1))
            if not raw_target or _markdown_is_external_target(raw_target):
                continue
            line = text.count("\n", 0, match.start()) + 1
            source = _markdown_section_for_line(line, file_path, headings) or file_path
            target = _markdown_target(raw_target, path)
            if target is None:
                continue
            if "::" in target:
                target_file, _target_section = target.split("::", 1)
                edges.append(
                    EdgeInfo(
                        kind="IMPORTS_FROM",
                        source=file_path,
                        target=target_file,
                        file_path=file_path,
                        line=line,
                        extra={"markdown_import_kind": "link"},
                    )
                )
                edges.append(
                    EdgeInfo(
                        kind="REFERENCES",
                        source=source,
                        target=target,
                        file_path=file_path,
                        line=line,
                        extra={"markdown_reference_kind": "link"},
                    )
                )
            elif target != file_path:
                edges.append(
                    EdgeInfo(
                        kind="IMPORTS_FROM",
                        source=file_path,
                        target=target,
                        file_path=file_path,
                        line=line,
                        extra={"markdown_import_kind": "link"},
                    )
                )


def _extract_markdown_code_spans(
    text: str,
    file_path: str,
    headings: list[dict[str, object]],
    edges: list[EdgeInfo],
) -> None:
    """Emit unresolved CROSS_ARTIFACT edges for inline code-span symbol refs.

    Only backtick spans that match the identifier-shape regex and meet the
    minimum length are emitted.  Resolution against actual graph nodes
    happens in the postprocess step (_resolve_markdown_artifact_refs).
    Fenced code blocks are not processed (too noisy for v1).
    """
    seen: set[tuple[str, str, int]] = set()
    for match in _MARKDOWN_CODE_SPAN_RE.finditer(text):
        sym = match.group(1).strip()
        if len(sym) < _MARKDOWN_SYMBOL_MIN_LEN:
            continue
        if not _MARKDOWN_SYMBOL_RE.match(sym):
            continue
        if "_" not in sym and "." not in sym and len(sym) < _MARKDOWN_PLAIN_WORD_MIN_LEN:
            continue
        line = text.count("\n", 0, match.start()) + 1
        source = _markdown_section_for_line(line, file_path, headings) or file_path
        key = (source, sym, line)
        if key in seen:
            continue
        seen.add(key)
        edges.append(
            EdgeInfo(
                kind="CROSS_ARTIFACT",
                source=source,
                target=f"<unresolved:{sym}>",
                file_path=file_path,
                line=line,
                extra={
                    "relationship_role": "describes_symbol",
                    "bridge_kind": "documentation",
                    "evidence_kind": "markdown_code_span",
                    "evidence_source": "code_span",
                    "source_language": "markdown",
                    "target_language": "unknown",
                    "confidence": 0.2,
                    "confidence_tier": "LOW",
                    "unresolved_target_name": sym,
                },
            )
        )


def _markdown_normalize_link_target(target: str) -> str:
    target = target.strip()
    if not target:
        return ""
    title_suffix = re.search(r"\s+(?:\"[^\"]*\"|'[^']*')\s*$", target)
    if title_suffix:
        target = target[: title_suffix.start()].rstrip()
    if target.startswith("<") and target.endswith(">"):
        target = target[1:-1].strip()
    return target


def _markdown_is_external_target(target: str) -> bool:
    lowered = target.lower()
    return lowered.startswith(("http://", "https://", "mailto:", "tel:"))


def _markdown_target(raw_target: str, source_file: Path) -> Optional[str]:
    raw_target = raw_target.strip()
    if not raw_target:
        return None
    if raw_target.startswith("#"):
        slug = _markdown_slugify(raw_target[1:].strip())
        return f"{source_file.resolve(strict=False)}::{slug}" if slug else None
    if raw_target.startswith("/"):
        return None

    path_part = raw_target
    section_part: Optional[str] = None
    if "#" in raw_target:
        path_part, section_part = raw_target.split("#", 1)
        section_part = section_part.strip()

    resolved = (source_file.parent / path_part).resolve(strict=False)
    target = str(resolved)
    if section_part:
        slug = _markdown_slugify(section_part)
        if not slug:
            return target
        return f"{target}::{slug}"
    return target


def _dedupe_markdown_edges(edges: list[EdgeInfo]) -> list[EdgeInfo]:
    seen: set[tuple[str, str, str, int]] = set()
    deduped: list[EdgeInfo] = []
    for edge in edges:
        key = (edge.kind, edge.source, edge.target, edge.line)
        if key in seen:
            continue
        seen.add(key)
        deduped.append(edge)
    return deduped
