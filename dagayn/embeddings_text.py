"""Text materialization helpers for embedding indexing."""

from __future__ import annotations

import functools
import logging
import os
import re
from pathlib import Path
from typing import TYPE_CHECKING

from .graph import GraphNode, GraphStore

if TYPE_CHECKING:
    from .embeddings_providers import EmbeddingProvider

logger = logging.getLogger(__name__)

_DEFAULT_SLOW_EMBED_BATCH_SECONDS = 10.0
_DEFAULT_SOURCE_CHARS = 2048
_DEFAULT_DOC_BODY_WEIGHT = 2
_EMBEDDING_TEXT_MODES = {"metadata", "body", "material", "structured", "narrative"}
_MARKDOWN_HEADING_RE = re.compile(r"^(#{1,6})\s+")
_DOC_BODY_KINDS = {"DocSection", "DocBody"}
_IDENTIFIER_RE = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")
_IDENT_BOUNDARY_RE = re.compile(r"(?<=[a-z0-9])(?=[A-Z])|(?<=[A-Z])(?=[A-Z][a-z])")
_CALL_RE = re.compile(
    r"\b([A-Za-z_][A-Za-z0-9_]*(?:::[A-Za-z_][A-Za-z0-9_]*)?"
    r"(?:\.[A-Za-z_][A-Za-z0-9_]*)?)\s*!?\s*\("
)
_ASSIGN_RE = re.compile(
    r"^\s*(?:let\s+|mut\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*(?::[^=]+)?=",
    re.MULTILINE,
)
_RETURN_RE = re.compile(r"\breturn\b\s*([^;\n]*)")
_BRANCH_RE = re.compile(r"\b(?:if|elif|else if|match|while)\b\s*([^{:\n]*)")
_LOOP_RE = re.compile(r"\bfor\s+(.+?)\s+in\s+([^:{\n]+)")
_STOP_IDENTIFIERS = {
    "and",
    "as",
    "bool",
    "break",
    "class",
    "continue",
    "def",
    "else",
    "false",
    "for",
    "fn",
    "if",
    "in",
    "let",
    "match",
    "mut",
    "none",
    "null",
    "or",
    "pub",
    "return",
    "self",
    "static",
    "str",
    "struct",
    "true",
    "while",
}
_TERM_EXPANSIONS = {
    "ast": "abstract syntax tree",
    "db": "database",
    "env": "environment",
    "fts": "full text search",
    "id": "identifier",
    "ids": "identifiers",
    "mcp": "model context protocol",
    "rrf": "reciprocal rank fusion",
    "sql": "sql database",
    "sqlite": "sqlite database",
}
_GRAPH_FACT_EDGE_KINDS = {
    "CALLS",
    "IMPORTS_FROM",
    "REFERENCES",
    "DEPENDS_ON",
    "INHERITS",
    "IMPLEMENTS",
    "TESTED_BY",
}


def _embedding_text_mode(text_mode: str | None = None) -> str:
    mode = (text_mode or os.environ.get("DAGAYN_EMBEDDING_TEXT_MODE") or "material").lower()
    if mode not in _EMBEDDING_TEXT_MODES:
        raise ValueError(
            "DAGAYN_EMBEDDING_TEXT_MODE must be one of: " + ", ".join(sorted(_EMBEDDING_TEXT_MODES))
        )
    return mode


def _embedding_source_chars() -> int:
    raw = os.environ.get("DAGAYN_EMBEDDING_SOURCE_CHARS")
    if raw is None:
        return _DEFAULT_SOURCE_CHARS
    try:
        return max(0, int(raw))
    except ValueError:
        logger.warning("Invalid DAGAYN_EMBEDDING_SOURCE_CHARS=%r; using default", raw)
        return _DEFAULT_SOURCE_CHARS


def _doc_embedding_body_weight() -> int:
    raw = os.environ.get("DAGAYN_DOC_EMBEDDING_BODY_WEIGHT")
    if raw is None:
        return _DEFAULT_DOC_BODY_WEIGHT
    try:
        return max(1, int(raw))
    except ValueError:
        logger.warning("Invalid DAGAYN_DOC_EMBEDDING_BODY_WEIGHT=%r; using default", raw)
        return _DEFAULT_DOC_BODY_WEIGHT


def _read_node_source_excerpt(
    node: GraphNode,
    *,
    source_root: Path | None = None,
    max_chars: int | None = None,
) -> str:
    """Read a bounded source span for embedding text, best-effort."""
    limit = _embedding_source_chars() if max_chars is None else max(0, max_chars)
    if limit <= 0:
        return ""

    file_path = Path(node.file_path)
    if not file_path.is_absolute():
        if source_root is None:
            return ""
        file_path = source_root / file_path
    try:
        lines = file_path.read_text(encoding="utf-8", errors="replace").splitlines()
    except OSError:
        return ""

    line_start = node.line_start or 1
    line_end = node.line_end or line_start
    start = max(int(line_start) - 1, 0)
    end = min(max(int(line_end), int(line_start)), len(lines))

    if node.kind == "DocSection":
        level = None
        if start < len(lines):
            match = _MARKDOWN_HEADING_RE.match(lines[start])
            if match:
                level = len(match.group(1))
        end = len(lines)
        for idx in range(start + 1, len(lines)):
            match = _MARKDOWN_HEADING_RE.match(lines[idx])
            if match and (level is None or len(match.group(1)) <= level):
                end = idx
                break

    return "\n".join(lines[start:end])[:limit]


def _material_base_text(node: GraphNode) -> str:
    parts = [node.name, node.qualified_name, str(node.file_path).replace("/", " ")]
    display_name = node.extra.get("display_name") if isinstance(node.extra, dict) else None
    if display_name:
        parts.append(str(display_name))
    if node.parent_name:
        parts.append(f"in {node.parent_name}")
    if node.language:
        parts.append(node.language)
    return " ".join(part for part in parts if part)


def _looks_like_comment_line(stripped: str) -> bool:
    return stripped.startswith(("#", "//", "///", "/*", "*", "--", '"""', "'''"))


def _clean_comment_line(stripped: str) -> str:
    cleaned = stripped
    for prefix in ("///", "//", "#", "/*", "*/", "*", "--", '"""', "'''"):
        if cleaned.startswith(prefix):
            cleaned = cleaned[len(prefix) :]
    return cleaned.strip(" */'\"")


def _comment_sentences_for_node(
    node: GraphNode,
    *,
    source_root: Path | None = None,
    max_chars: int | None = None,
) -> list[str]:
    limit = _embedding_source_chars() if max_chars is None else max(0, max_chars)
    if limit <= 0:
        return []

    file_path = Path(node.file_path)
    if not file_path.is_absolute():
        if source_root is None:
            return []
        file_path = source_root / file_path
    try:
        lines = file_path.read_text(encoding="utf-8", errors="replace").splitlines()
    except OSError:
        return []

    line_start = node.line_start or 1
    line_end = node.line_end or line_start
    start = max(int(line_start) - 1, 0)
    end = min(max(int(line_end), int(line_start)), len(lines))
    comments: list[str] = []

    idx = start - 1
    while idx >= 0:
        stripped = lines[idx].strip()
        if not stripped:
            idx -= 1
            continue
        if _looks_like_comment_line(stripped):
            comments.insert(0, _clean_comment_line(stripped))
            idx -= 1
            continue
        break

    for line in lines[start:end]:
        stripped = line.strip()
        if _looks_like_comment_line(stripped):
            comments.append(_clean_comment_line(stripped))

    text = "\n".join(comment for comment in comments if comment).strip()[:limit]
    if not text:
        return []
    return [part.strip() for part in re.split(r"(?<=[.!?。！？])\s+|\n+", text) if part.strip()]


def _node_to_material_text(
    node: GraphNode,
    *,
    source_root: Path | None = None,
) -> str:
    """Convert a node to the measured default embedding material."""
    base = _material_base_text(node)
    if node.kind in _DOC_BODY_KINDS:
        source_excerpt = _read_node_source_excerpt(node, source_root=source_root)
        return f"{base} {source_excerpt}" if source_excerpt else base

    if node.kind in {"Function", "Method", "Class"}:
        comments = _comment_sentences_for_node(node, source_root=source_root)
        if comments:
            return " ".join([base, *(f"{base} {comment}" for comment in comments)])
        return base

    return base


def _node_to_structured_text(
    node: GraphNode,
    *,
    source_root: Path | None = None,
) -> str:
    """Convert a node to labeled code-reference material for search experiments."""
    fields = [
        ("kind", node.kind),
        ("name", node.name),
        ("qualified", node.qualified_name),
        ("file", str(node.file_path).replace("/", " ")),
    ]
    display_name = node.extra.get("display_name") if isinstance(node.extra, dict) else None
    if display_name:
        fields.append(("display", str(display_name)))
    if node.parent_name:
        fields.append(("parent", node.parent_name))
    if node.language:
        fields.append(("language", node.language))
    if node.signature:
        fields.append(("signature", node.signature))
    if node.params:
        fields.append(("params", node.params))
    if node.return_type:
        fields.append(("returns", node.return_type))
    parts = [f"{key}: {value}" for key, value in fields if value]
    source_excerpt = _read_node_source_excerpt(node, source_root=source_root)
    if source_excerpt:
        parts.append(f"source:\n{source_excerpt}")
    return "\n".join(parts)


def _identifier_terms(value: str) -> list[str]:
    terms: list[str] = []
    seen: set[str] = set()
    for raw in _IDENTIFIER_RE.findall(value):
        pieces = _IDENT_BOUNDARY_RE.sub(" ", raw.replace("_", " ")).lower().split()
        for piece in pieces:
            if piece in _STOP_IDENTIFIERS or len(piece) < 2:
                continue
            for term in (piece, _TERM_EXPANSIONS.get(piece, "")):
                if term and term not in seen:
                    seen.add(term)
                    terms.append(term)
    return terms


def _limited_join(values: list[str], *, limit: int = 8) -> str:
    return ", ".join(values[:limit])


def _display_qualified_name(qualified_name: str) -> str:
    return qualified_name.rsplit("::", 1)[-1].rsplit("/", 1)[-1]


def _append_unique(values: list[str], value: str, seen: set[str]) -> None:
    if value and value not in seen:
        seen.add(value)
        values.append(value)


def _calls_from_source(source_excerpt: str) -> list[str]:
    calls: list[str] = []
    seen: set[str] = set()
    for match in _CALL_RE.finditer(source_excerpt):
        call = match.group(1).strip(".")
        line_start = source_excerpt.rfind("\n", 0, match.start()) + 1
        prefix = source_excerpt[line_start : match.start()].strip()
        if prefix.endswith(("def", "class", "fn", "pub fn")):
            continue
        if not call:
            continue
        last = call.rsplit(".", 1)[-1].rsplit("::", 1)[-1].lower()
        if last in _STOP_IDENTIFIERS:
            continue
        if call not in seen:
            seen.add(call)
            calls.append(call)
    return calls


def _assigned_names_from_source(source_excerpt: str) -> list[str]:
    names: list[str] = []
    seen: set[str] = set()
    for match in _ASSIGN_RE.finditer(source_excerpt):
        name = match.group(1)
        if name.lower() in _STOP_IDENTIFIERS or name in seen:
            continue
        seen.add(name)
        names.append(name)
    return names


def _return_terms_from_source(source_excerpt: str) -> list[str]:
    terms: list[str] = []
    seen: set[str] = set()
    for match in _RETURN_RE.finditer(source_excerpt):
        for term in _identifier_terms(match.group(1)):
            if term not in seen:
                seen.add(term)
                terms.append(term)
    return terms


def _branch_terms_from_source(source_excerpt: str) -> list[str]:
    terms: list[str] = []
    seen: set[str] = set()
    for match in _BRANCH_RE.finditer(source_excerpt):
        for term in _identifier_terms(match.group(1)):
            if term not in seen:
                seen.add(term)
                terms.append(term)
    return terms


def _loop_terms_from_source(source_excerpt: str) -> list[str]:
    terms: list[str] = []
    seen: set[str] = set()
    for match in _LOOP_RE.finditer(source_excerpt):
        for term in _identifier_terms(" ".join(match.groups())):
            if term not in seen:
                seen.add(term)
                terms.append(term)
    return terms


def _io_phrases_from_source(source_excerpt: str) -> list[str]:
    lowered = source_excerpt.lower()
    phrases: list[str] = []
    if any(token in lowered for token in ("read_text", "read_to_string", "open(", "select ")):
        phrases.append("reads data")
    if any(token in lowered for token in ("write_text", "std::fs::write", "insert ", "update ")):
        phrases.append("writes data")
    if any(
        token in lowered for token in (".execute(", "query_row", "sqlite", "select ", "insert ")
    ):
        phrases.append("uses sqlite database queries")
    if any(token in lowered for token in ("embed(", "embed_query", "embedding")):
        phrases.append("uses embedding model operations")
    if any(token in lowered for token in ("search", "fts", "match ")):
        phrases.append("performs search or ranking")
    return phrases


def _graph_fact_sentences(graph_facts: dict[str, list[str]] | None) -> list[str]:
    if not graph_facts:
        return []

    sentences: list[str] = []
    called = graph_facts.get("CALLS", [])
    if called:
        call_names = [f"`{_display_qualified_name(name)}`" for name in called]
        sentences.append(f"The graph says it calls {_limited_join(call_names)}.")
    imports = graph_facts.get("IMPORTS_FROM", [])
    if imports:
        import_names = [f"`{_display_qualified_name(name)}`" for name in imports]
        sentences.append(f"The graph says it imports from {_limited_join(import_names)}.")
    refs = graph_facts.get("REFERENCES", [])
    if refs:
        ref_names = [f"`{_display_qualified_name(name)}`" for name in refs]
        sentences.append(f"The graph says it references {_limited_join(ref_names)}.")
    deps = graph_facts.get("DEPENDS_ON", [])
    if deps:
        dep_names = [f"`{_display_qualified_name(name)}`" for name in deps]
        sentences.append(f"The graph says it depends on {_limited_join(dep_names)}.")
    inherited = graph_facts.get("INHERITS", [])
    if inherited:
        inherited_names = [f"`{_display_qualified_name(name)}`" for name in inherited]
        sentences.append(f"The graph says it inherits from {_limited_join(inherited_names)}.")
    implemented = graph_facts.get("IMPLEMENTS", [])
    if implemented:
        implemented_names = [f"`{_display_qualified_name(name)}`" for name in implemented]
        sentences.append(f"The graph says it implements {_limited_join(implemented_names)}.")
    tested_by = graph_facts.get("TESTED_BY", [])
    if tested_by:
        test_names = [f"`{_display_qualified_name(name)}`" for name in tested_by]
        sentences.append(f"The graph says it is tested by {_limited_join(test_names)}.")
    callers = graph_facts.get("called_by", [])
    if callers:
        caller_names = [f"`{_display_qualified_name(name)}`" for name in callers]
        sentences.append(f"The graph says it is called by {_limited_join(caller_names)}.")
    return sentences


def _graph_fact_terms(graph_facts: dict[str, list[str]] | None) -> list[str]:
    if not graph_facts:
        return []
    terms: list[str] = []
    seen: set[str] = set()
    for values in graph_facts.values():
        for value in values:
            for term in _identifier_terms(value):
                _append_unique(terms, term, seen)
    return terms


def _node_to_narrative_text(
    node: GraphNode,
    *,
    source_root: Path | None = None,
    graph_facts: dict[str, list[str]] | None = None,
) -> str:
    """Convert a node to deterministic natural-language static code facts."""
    source_excerpt = _read_node_source_excerpt(node, source_root=source_root)
    language = node.language or "code"
    kind = node.kind.lower()
    display_name = node.extra.get("display_name") if isinstance(node.extra, dict) else None
    subject = f"`{node.name}`"
    sentences = [
        f"This {language} {kind} {subject} is defined in `{node.file_path}`.",
        f"It is referenced as `{node.qualified_name}`.",
        "It is represented as static code facts for code search and AI explanation.",
    ]
    if display_name:
        sentences.append(f"It is also described as {display_name}.")
    if node.parent_name:
        sentences.append(f"It belongs to `{node.parent_name}`.")
    if node.params:
        sentences.append(f"It accepts parameters {node.params}.")
    if node.return_type:
        sentences.append(f"It declares return type `{node.return_type}`.")
    if node.signature:
        sentences.append(f"Its signature is `{node.signature}`.")

    name_terms = _identifier_terms(" ".join([node.name, node.qualified_name, node.signature or ""]))
    if name_terms:
        sentences.append(f"Its identifiers mention {_limited_join(name_terms)}.")

    sentences.extend(_graph_fact_sentences(graph_facts))
    graph_terms = _graph_fact_terms(graph_facts)
    if graph_terms:
        sentences.append(f"Its graph relationships mention {_limited_join(graph_terms, limit=16)}.")

    if source_excerpt:
        calls = _calls_from_source(source_excerpt)
        if calls:
            sentences.append(f"It calls {_limited_join([f'`{call}`' for call in calls])}.")
        assigned = _assigned_names_from_source(source_excerpt)
        if assigned:
            assigned_names = [f"`{name}`" for name in assigned]
            sentences.append(f"It defines or updates {_limited_join(assigned_names)}.")
        return_terms = _return_terms_from_source(source_excerpt)
        if return_terms:
            sentences.append(f"It returns values related to {_limited_join(return_terms)}.")
        branch_terms = _branch_terms_from_source(source_excerpt)
        if branch_terms:
            sentences.append(f"It branches on {_limited_join(branch_terms)}.")
        loop_terms = _loop_terms_from_source(source_excerpt)
        if loop_terms:
            sentences.append(f"It iterates over {_limited_join(loop_terms)}.")
        io_phrases = _io_phrases_from_source(source_excerpt)
        if io_phrases:
            sentences.append(f"It {_limited_join(io_phrases)}.")
        source_terms = _identifier_terms(source_excerpt)
        if source_terms:
            sentences.append(f"Its source mentions {_limited_join(source_terms, limit=16)}.")

    return " ".join(sentences)


def _node_to_text(
    node: GraphNode,
    *,
    source_root: Path | None = None,
    text_mode: str | None = None,
    graph_facts: dict[str, list[str]] | None = None,
) -> str:
    """Convert a node to a searchable text representation."""
    mode = _embedding_text_mode(text_mode)
    if mode == "material":
        return _node_to_material_text(node, source_root=source_root)
    if mode == "structured":
        return _node_to_structured_text(node, source_root=source_root)
    if mode == "narrative":
        return _node_to_narrative_text(node, source_root=source_root, graph_facts=graph_facts)

    parts = [node.name, node.qualified_name, str(node.file_path).replace("/", " ")]
    display_name = node.extra.get("display_name") if isinstance(node.extra, dict) else None
    if display_name:
        parts.append(str(display_name))
    if node.kind != "File":
        parts.append(node.kind.lower())
    if node.parent_name:
        parts.append(f"in {node.parent_name}")
    if node.signature:
        parts.append(node.signature)
    if node.params:
        parts.append(node.params)
    if node.return_type:
        parts.append(f"returns {node.return_type}")
    if node.language:
        parts.append(node.language)
    include_source = mode == "body" or node.kind in _DOC_BODY_KINDS
    if include_source:
        source_excerpt = _read_node_source_excerpt(node, source_root=source_root)
        if source_excerpt:
            repetitions = _doc_embedding_body_weight() if node.kind in _DOC_BODY_KINDS else 1
            if repetitions > 1:
                per_repetition = max(1, _embedding_source_chars() // repetitions)
                source_excerpt = source_excerpt[:per_repetition]
            parts.extend([source_excerpt] * repetitions)
    return " ".join(parts)


def _build_graph_facts_by_qualified_name(
    graph_store: GraphStore,
    nodes: list[GraphNode],
) -> dict[str, dict[str, list[str]]]:
    qns = [node.qualified_name for node in nodes]
    outgoing, incoming = graph_store.get_edges_by_endpoints(qns)
    facts_by_qn: dict[str, dict[str, list[str]]] = {}
    for qn in qns:
        facts: dict[str, list[str]] = {}
        seen_by_kind: dict[str, set[str]] = {}

        for edge in outgoing.get(qn, []):
            if edge.kind not in _GRAPH_FACT_EDGE_KINDS:
                continue
            values = facts.setdefault(edge.kind, [])
            seen = seen_by_kind.setdefault(edge.kind, set())
            _append_unique(values, _display_qualified_name(edge.target_qualified), seen)

        callers = facts.setdefault("called_by", [])
        seen_callers = seen_by_kind.setdefault("called_by", set())
        for edge in incoming.get(qn, []):
            if edge.kind == "CALLS":
                _append_unique(
                    callers,
                    _display_qualified_name(edge.source_qualified),
                    seen_callers,
                )
        if not callers:
            facts.pop("called_by", None)
        if facts:
            facts_by_qn[qn] = facts
    return facts_by_qn


def _slow_embed_batch_seconds() -> float:
    raw = os.environ.get("CRG_EMBEDDING_SLOW_BATCH_SECONDS")
    if raw is None:
        return _DEFAULT_SLOW_EMBED_BATCH_SECONDS
    try:
        return max(0.0, float(raw))
    except ValueError:
        return _DEFAULT_SLOW_EMBED_BATCH_SECONDS


@functools.lru_cache(maxsize=256)
def _embed_query_cached(provider: "EmbeddingProvider", query: str) -> list[float]:
    """Cache embed_query results keyed on (provider, query_text).

    Provider instances are compared by identity; the cache is naturally
    invalidated when a new EmbeddingStore (and therefore new provider
    instance) is created after a DB mtime change.
    """
    return provider.embed_query(query)
