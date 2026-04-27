from __future__ import annotations

import re
from pathlib import Path
from typing import TYPE_CHECKING, Optional

if TYPE_CHECKING:
    from ..core import CodeParser

from ..test_detection import is_test_file as _is_test_file
from ..test_detection import is_test_function as _is_test_function
from ..types import EdgeInfo, NodeInfo

_RESCRIPT_IDENT = r"[A-Za-z_][A-Za-z0-9_']*"

# `module Name =`, `module type Name =`, `module Name: {`, `module Name: (Sig) => {`
_RESCRIPT_MODULE_RE = re.compile(
    r"^\s*module\s+(?:type\s+)?([A-Z][A-Za-z0-9_']*)\s*[:=]",
    re.MULTILINE,
)

# Optional leading decorator block on the same line, e.g. `@deriving(foo)`.
_RESCRIPT_DECORATOR_PREFIX = r"(?:@[A-Za-z_][A-Za-z0-9_']*(?:\([^)]*\))?\s+)*"

# `let [rec] name` / `and name` — captures binding name. Multi-line decorators
# on prior lines don't interfere (they end with a newline and the anchor
# restarts on the next line); same-line decorators are tolerated.
_RESCRIPT_LET_RE = re.compile(
    rf"^\s*{_RESCRIPT_DECORATOR_PREFIX}"
    rf"(?:let\s+(?:rec\s+)?|and\s+)({_RESCRIPT_IDENT})\b",
    re.MULTILINE,
)

# `external name: sig = "..."`
_RESCRIPT_EXTERNAL_RE = re.compile(
    rf"^\s*{_RESCRIPT_DECORATOR_PREFIX}external\s+({_RESCRIPT_IDENT})\s*:",
    re.MULTILINE,
)

# `type name` / `type rec name` / `type name<'a>`
_RESCRIPT_TYPE_RE = re.compile(
    rf"^\s*{_RESCRIPT_DECORATOR_PREFIX}type\s+(?:rec\s+)?({_RESCRIPT_IDENT})\b",
    re.MULTILINE,
)

# `open Foo` / `include Foo.Bar`
_RESCRIPT_OPEN_RE = re.compile(
    r"^\s*(open|include)\s+([A-Z][A-Za-z0-9_'.]*)",
    re.MULTILINE,
)

# `module X = Foo.Bar` with no `{` body — a module alias/re-export. Distinct
# from `module X = { ... }` (handled by _RESCRIPT_MODULE_RE + brace scan).
_RESCRIPT_MODULE_ALIAS_RE = re.compile(
    r"^\s*module\s+([A-Z][A-Za-z0-9_']*)\s*=\s*"
    r"([A-Z][A-Za-z0-9_']*(?:\.[A-Za-z_][A-Za-z0-9_']*)*)\s*$",
    re.MULTILINE,
)

# JSX opening tag: `<Foo`, `<Foo.Bar`, `<Foo.Bar.Baz`. First segment must be
# Capitalized (lowercase tags are HTML elements, not ReScript components).
# The leading `<` must NOT be part of `=>`, `<=`, `<-`, or a generic-type
# parameter (we approximate by requiring the char before `<` to be space,
# newline, `{`, `(`, `,`, `>`, `}`, or BOF).
_RESCRIPT_JSX_RE = re.compile(
    r"(?:^|(?<=[\s{(,>}]))"
    r"<([A-Z][A-Za-z0-9_']*(?:\.[A-Z][A-Za-z0-9_']*)*)\b",
    re.MULTILINE,
)

# `@module("path")` — source module for an external binding
_RESCRIPT_MODULE_ATTR_RE = re.compile(
    r'@module\(\s*"([^"]+)"\s*\)',
)

# `Ident(`, `Mod.fn(` — anything that looks like a call site. Preceded by a
# non-identifier char to avoid matching suffixes of identifiers.
_RESCRIPT_CALL_RE = re.compile(
    rf"(?<![A-Za-z0-9_']){_RESCRIPT_IDENT}(?:\.{_RESCRIPT_IDENT})*\s*\(",
)

# Recompiled to grab the captured identifier sequence. We need a different
# regex with a capture group for matching:
_RESCRIPT_CALL_RE = re.compile(
    r"(?<![A-Za-z0-9_'])"
    r"([A-Za-z_][A-Za-z0-9_']*(?:\.[A-Za-z_][A-Za-z0-9_']*)*)"
    r"\s*\(",
)

# Reserved words + syntactic noise that should never be treated as names
# or as call targets.
_RESCRIPT_KEYWORDS = frozenset(
    {
        "let",
        "rec",
        "and",
        "type",
        "module",
        "open",
        "include",
        "external",
        "if",
        "else",
        "switch",
        "when",
        "match",
        "fun",
        "true",
        "false",
        "for",
        "while",
        "mutable",
        "try",
        "catch",
        "throw",
        "assert",
        "lazy",
        "do",
        "in",
        "of",
        "as",
        "exception",
        "private",
        "constraint",
        "with",
        "downto",
        "to",
        "unpack",
        "async",
        "await",
    }
)


def _strip_rescript_noise(text: str) -> str:
    """Replace ReScript comments and string/backtick content with spaces.

    Newlines are preserved so absolute offsets still map back to accurate
    line numbers. ReScript block comments may nest, so we track depth.
    """
    out: list[str] = []
    i = 0
    n = len(text)
    while i < n:
        c = text[i]
        nxt = text[i + 1] if i + 1 < n else ""
        # Line comment
        if c == "/" and nxt == "/":
            while i < n and text[i] != "\n":
                out.append(" ")
                i += 1
            continue
        # Nestable block comment
        if c == "/" and nxt == "*":
            depth = 1
            out.append("  ")
            i += 2
            while i < n and depth > 0:
                if i + 1 < n and text[i] == "/" and text[i + 1] == "*":
                    depth += 1
                    out.append("  ")
                    i += 2
                elif i + 1 < n and text[i] == "*" and text[i + 1] == "/":
                    depth -= 1
                    out.append("  ")
                    i += 2
                else:
                    out.append("\n" if text[i] == "\n" else " ")
                    i += 1
            continue
        # Double-quoted string — blank content, keep quotes + newlines.
        if c == '"':
            out.append('"')
            i += 1
            while i < n and text[i] != '"':
                if text[i] == "\\" and i + 1 < n:
                    out.append("  ")
                    i += 2
                    continue
                out.append("\n" if text[i] == "\n" else " ")
                i += 1
            if i < n:
                out.append('"')
                i += 1
            continue
        # Backtick template string — blank content, preserve newlines.
        if c == "`":
            out.append("`")
            i += 1
            while i < n and text[i] != "`":
                out.append("\n" if text[i] == "\n" else " ")
                i += 1
            if i < n:
                out.append("`")
                i += 1
            continue
        out.append(c)
        i += 1
    return "".join(out)


def _rescript_brace_depth_array(cleaned: str) -> list[int]:
    """Compute brace depth at every offset in `cleaned` (comment/string-stripped).

    Returned array has length len(cleaned); `depth[i]` is the depth
    immediately before the character at position i.
    """
    depth = [0] * (len(cleaned) + 1)
    d = 0
    for i, c in enumerate(cleaned):
        depth[i] = d
        if c == "{":
            d += 1
        elif c == "}":
            d = max(0, d - 1)
    depth[len(cleaned)] = d
    return depth


def _scan_rescript_modules(cleaned: str, offset_to_line) -> list[dict]:
    """Find `module Name = { ... }` blocks and their offset/line ranges.

    Returns dicts with name, start/end offsets, start/end lines, and parent
    module name (or None for top-level).
    """
    modules: list[dict] = []
    n = len(cleaned)
    # Module aliases (`module X = Foo.Bar`) also match _RESCRIPT_MODULE_RE but
    # have no brace body — skip them here to avoid the greedy `{`-scanner
    # swallowing the next unrelated block (e.g. a `let` body).
    alias_starts = {m.start() for m in _RESCRIPT_MODULE_ALIAS_RE.finditer(cleaned)}
    for match in _RESCRIPT_MODULE_RE.finditer(cleaned):
        if match.start() in alias_starts:
            continue
        name = match.group(1)
        header_start = match.start()
        # Find the first `{` after the header's `:` or `=`. To avoid grabbing
        # a `{` from an unrelated following statement, require that the chars
        # between `match.end()` and `brace_open` contain no definition-starting
        # keywords (`let`, `type`, `module`, `external`).
        brace_open = cleaned.find("{", match.end())
        if brace_open == -1:
            continue
        between = cleaned[match.end() : brace_open]
        if re.search(
            r"(?:^|\s)(?:let|type|module|external|and)\s",
            between,
        ):
            continue
        # Walk braces to find the matching close.
        depth = 1
        j = brace_open + 1
        while j < n and depth > 0:
            c = cleaned[j]
            if c == "{":
                depth += 1
            elif c == "}":
                depth -= 1
            j += 1
        brace_close = j - 1 if depth == 0 else n - 1
        modules.append(
            {
                "name": name,
                "start_off": header_start,
                "end_off": brace_close,
                "body_start_off": brace_open + 1,
                "start_line": offset_to_line(header_start),
                "end_line": offset_to_line(brace_close),
                "parent": None,
            }
        )

    # Parent = innermost strictly-containing module.
    for i, m in enumerate(modules):
        parent_name = None
        parent_start = -1
        for j, other in enumerate(modules):
            if i == j:
                continue
            if (
                other["start_off"] < m["start_off"]
                and other["end_off"] > m["end_off"]
                and other["start_off"] > parent_start
            ):
                parent_name = other["name"]
                parent_start = other["start_off"]
        m["parent"] = parent_name
    return modules


def parse(parser: "CodeParser", path: Path, source: bytes) -> tuple[list[NodeInfo], list[EdgeInfo]]:
    """Parse a ReScript `.res` or `.resi` file.

    Extracts modules, let bindings, types, external bindings, open/include
    imports, and function calls. Interface files (`.resi`) are flagged via
    ``File`` node ``extra["rescript_interface"]=True`` and skip call
    extraction since signatures have no call sites.
    """
    text = source.decode("utf-8", errors="replace")
    file_path_str = str(path)
    test_file = _is_test_file(file_path_str)
    is_interface = path.suffix.lower() == ".resi"

    # Strip comments and string/backtick literal content so downstream
    # regex matches are not fooled by code-looking text inside strings.
    # Newlines are preserved so offset→line mapping stays accurate.
    cleaned = _strip_rescript_noise(text)

    # Build offset → line index (1-based).
    line_starts = [0]
    for i, ch in enumerate(cleaned):
        if ch == "\n":
            line_starts.append(i + 1)

    def offset_to_line(off: int) -> int:
        lo, hi = 0, len(line_starts) - 1
        while lo < hi:
            mid = (lo + hi + 1) // 2
            if line_starts[mid] <= off:
                lo = mid
            else:
                hi = mid - 1
        return lo + 1

    nodes: list[NodeInfo] = []
    edges: list[EdgeInfo] = []

    file_extra: dict = {}
    if is_interface:
        file_extra["rescript_interface"] = True
    nodes.append(
        NodeInfo(
            kind="File",
            name=file_path_str,
            file_path=file_path_str,
            line_start=1,
            line_end=text.count("\n") + 1,
            language="rescript",
            is_test=test_file,
            extra=file_extra,
        )
    )

    # Modules with brace-matched offset ranges.
    modules = _scan_rescript_modules(cleaned, offset_to_line)
    depth_arr = _rescript_brace_depth_array(cleaned)

    def is_top_level(off: int, parent_mod: Optional[str]) -> bool:
        """True if offset is at file scope (depth 0) or directly inside
        `parent_mod`'s body (depth = module body depth)."""
        d = depth_arr[off] if off < len(depth_arr) else 0
        if parent_mod is None:
            return d == 0
        for m in modules:
            if m["name"] == parent_mod and m["start_off"] <= off <= m["end_off"]:
                expected = depth_arr[m["body_start_off"]]
                return d == expected
        return False

    for m in modules:
        nodes.append(
            NodeInfo(
                kind="Class",
                name=m["name"],
                file_path=file_path_str,
                line_start=m["start_line"],
                line_end=m["end_line"],
                language="rescript",
                parent_name=m["parent"],
                extra={"rescript_kind": "module"},
            )
        )

    def enclosing_module(off: int) -> Optional[str]:
        innermost_name = None
        innermost_start = -1
        for m in modules:
            if m["start_off"] <= off <= m["end_off"] and m["start_off"] > innermost_start:
                innermost_name = m["name"]
                innermost_start = m["start_off"]
        return innermost_name

    # First: let/and bindings — collect offsets so we can later compute
    # end offsets for call attribution.
    let_entries: list[dict] = []
    for match in _RESCRIPT_LET_RE.finditer(cleaned):
        name = match.group(1)
        if name in _RESCRIPT_KEYWORDS:
            continue
        off = match.start(1)
        parent = enclosing_module(off)
        if not is_top_level(off, parent):
            continue  # nested local `let` — not a structural node
        line_start = offset_to_line(off)
        is_test_fn = _is_test_function(name, file_path_str)
        let_entries.append(
            {
                "name": name,
                "start_off": off,
                "line_start": line_start,
                "parent": parent,
                "is_test": is_test_fn,
            }
        )

    # Sort by start_off, compute end_off as next same-or-outer-scope let start
    # or the closing brace of the enclosing module, or end of file.
    let_entries.sort(key=lambda e: e["start_off"])
    for i, entry in enumerate(let_entries):
        nxt = len(cleaned)
        for later in let_entries[i + 1 :]:
            nxt = later["start_off"]
            break
        # Clamp by enclosing module end if any
        if entry["parent"]:
            for m in modules:
                if (
                    m["name"] == entry["parent"]
                    and m["start_off"] <= entry["start_off"] <= m["end_off"]
                ):
                    nxt = min(nxt, m["end_off"])
                    break
        entry["end_off"] = max(nxt, entry["start_off"] + 1)
        entry["line_end"] = offset_to_line(entry["end_off"] - 1)

    for entry in let_entries:
        nodes.append(
            NodeInfo(
                kind="Test" if entry["is_test"] else "Function",
                name=entry["name"],
                file_path=file_path_str,
                line_start=entry["line_start"],
                line_end=entry["line_end"],
                language="rescript",
                parent_name=entry["parent"],
                is_test=entry["is_test"],
            )
        )

    # External bindings (also create IMPORTS_FROM edges for @module attrs).
    for match in _RESCRIPT_EXTERNAL_RE.finditer(cleaned):
        name = match.group(1)
        if name in _RESCRIPT_KEYWORDS:
            continue
        off = match.start(1)
        parent = enclosing_module(off)
        if not is_top_level(off, parent):
            continue
        line_start = offset_to_line(off)
        nodes.append(
            NodeInfo(
                kind="Function",
                name=name,
                file_path=file_path_str,
                line_start=line_start,
                line_end=line_start,
                language="rescript",
                parent_name=parent,
                extra={"rescript_external": True},
            )
        )
        # Look back up to 200 chars for a nearby @module("...") attr.
        # Read from the ORIGINAL text (not `cleaned`) so string literal
        # content like "fs" is preserved. Offsets are length-equivalent
        # because `_strip_rescript_noise` replaces with spaces/newlines.
        look_start = max(0, off - 200)
        snippet = text[look_start:off]
        for attr in _RESCRIPT_MODULE_ATTR_RE.finditer(snippet):
            edges.append(
                EdgeInfo(
                    kind="IMPORTS_FROM",
                    source=file_path_str,
                    target=attr.group(1),
                    file_path=file_path_str,
                    line=line_start,
                    extra={"rescript_import_kind": "external_module"},
                )
            )

    # Type definitions.
    for match in _RESCRIPT_TYPE_RE.finditer(cleaned):
        name = match.group(1)
        if name in _RESCRIPT_KEYWORDS:
            continue
        off = match.start(1)
        parent = enclosing_module(off)
        if not is_top_level(off, parent):
            continue
        line_start = offset_to_line(off)
        nodes.append(
            NodeInfo(
                kind="Type",
                name=name,
                file_path=file_path_str,
                line_start=line_start,
                line_end=line_start,
                language="rescript",
                parent_name=parent,
            )
        )

    # open / include statements.
    for match in _RESCRIPT_OPEN_RE.finditer(cleaned):
        kind = match.group(1)
        target = match.group(2)
        off = match.start()
        line = offset_to_line(off)
        edges.append(
            EdgeInfo(
                kind="IMPORTS_FROM",
                source=file_path_str,
                target=target,
                file_path=file_path_str,
                line=line,
                extra={"rescript_import_kind": kind},
            )
        )

    # Module aliases: `module X = Foo.Bar` (no brace body). These
    # re-export another module and are the second most common way ReScript
    # files reference each other (after JSX).
    for match in _RESCRIPT_MODULE_ALIAS_RE.finditer(cleaned):
        alias_name = match.group(1)
        target = match.group(2)
        off = match.start()
        # Skip if the alias was actually the header of a `module X = { ... }`
        # block already captured by `modules`. That scanner requires `{` to
        # follow, so a trailing-dot form like `module X = Foo.Bar` at EOL
        # never gets mistaken for a block.
        if any(m["start_off"] == off for m in modules):
            continue
        line = offset_to_line(off)
        edges.append(
            EdgeInfo(
                kind="IMPORTS_FROM",
                source=file_path_str,
                target=target,
                file_path=file_path_str,
                line=line,
                extra={
                    "rescript_import_kind": "module_alias",
                    "alias_name": alias_name,
                },
            )
        )

    # JSX component usage: `<Foo />`, `<Foo.Bar />`. The root module is
    # what matters for cross-file dependency tracking (importers_of);
    # the specific component is the CALLS target for finer queries.
    if not is_interface:
        for match in _RESCRIPT_JSX_RE.finditer(cleaned):
            target = match.group(1)
            off = match.start(1)
            root = target.split(".", 1)[0]
            line = offset_to_line(off)
            edges.append(
                EdgeInfo(
                    kind="IMPORTS_FROM",
                    source=file_path_str,
                    target=root,
                    file_path=file_path_str,
                    line=line,
                    extra={"rescript_import_kind": "jsx"},
                )
            )
            # Attribute a CALLS edge to the enclosing let, so
            # callers_of(<Foo.Bar />) can find the caller.
            caller = None
            caller_parent = None
            for entry in let_entries:
                if entry["start_off"] <= off < entry["end_off"]:
                    caller = entry["name"]
                    caller_parent = entry["parent"]
                elif entry["start_off"] > off:
                    break
            if caller is not None:
                edges.append(
                    EdgeInfo(
                        kind="CALLS",
                        source=parser._qualify(
                            caller,
                            file_path_str,
                            caller_parent,
                        ),
                        target=target,
                        file_path=file_path_str,
                        line=line,
                        extra={"rescript_call_kind": "jsx"},
                    )
                )

    # Calls — interface files have no call sites, skip.
    if not is_interface and let_entries:
        for match in _RESCRIPT_CALL_RE.finditer(cleaned):
            target = match.group(1)
            off = match.start(1)
            top = target.split(".", 1)[0]
            if top in _RESCRIPT_KEYWORDS or target in _RESCRIPT_KEYWORDS:
                continue
            # Find enclosing let by offset range.
            caller = None
            caller_parent = None
            for entry in let_entries:
                if entry["start_off"] <= off < entry["end_off"]:
                    caller = entry["name"]
                    caller_parent = entry["parent"]
                elif entry["start_off"] > off:
                    break
            if caller is None:
                continue
            # Skip the definition site itself: `let name = ...` where
            # name(x) is actually the definition header, not a call.
            if caller == target and off == next(
                (e["start_off"] for e in let_entries if e["name"] == caller),
                -1,
            ):
                continue
            line = offset_to_line(off)
            source_qn = parser._qualify(caller, file_path_str, caller_parent)
            edges.append(
                EdgeInfo(
                    kind="CALLS",
                    source=source_qn,
                    target=target,
                    file_path=file_path_str,
                    line=line,
                )
            )

    # CONTAINS edges: each module node contains its members.
    for n in nodes:
        if n.kind in ("Function", "Type", "Test") and n.parent_name:
            edges.append(
                EdgeInfo(
                    kind="CONTAINS",
                    source=parser._qualify(n.parent_name, file_path_str, None),
                    target=parser._qualify(n.name, file_path_str, n.parent_name),
                    file_path=file_path_str,
                    line=n.line_start,
                )
            )

    # Tag modules whose member functions are all externals as JS bindings.
    # (e.g. `module TextEncoder = { type encoder; @new external ... }`)
    member_funcs: dict[str, list[NodeInfo]] = {}
    for n in nodes:
        if n.kind == "Function" and n.parent_name:
            member_funcs.setdefault(n.parent_name, []).append(n)
    for mod_node in nodes:
        if mod_node.kind != "Class":
            continue
        members = member_funcs.get(mod_node.name, [])
        if members and all(m.extra.get("rescript_external") for m in members):
            mod_node.extra["rescript_kind"] = "js_binding"

    # Dedupe IMPORTS_FROM edges by (source, target). The same `open X`
    # can appear multiple times legitimately (e.g. reopened within
    # different scopes), and include+open of the same module produces
    # two edges; collapse them.
    seen_imports: set[tuple[str, str]] = set()
    deduped_edges: list[EdgeInfo] = []
    for e in edges:
        if e.kind == "IMPORTS_FROM":
            key = (e.source, e.target)
            if key in seen_imports:
                continue
            seen_imports.add(key)
        deduped_edges.append(e)
    edges = deduped_edges

    edges = parser._resolve_call_targets(nodes, edges, file_path_str)

    if test_file:
        test_qnames = set()
        for n in nodes:
            if n.is_test:
                qn = parser._qualify(n.name, n.file_path, n.parent_name)
                test_qnames.add(qn)
        for edge in list(edges):
            if edge.kind == "CALLS" and edge.source in test_qnames:
                edges.append(
                    EdgeInfo(
                        kind="TESTED_BY",
                        source=edge.target,
                        target=edge.source,
                        file_path=edge.file_path,
                        line=edge.line,
                    )
                )

    return nodes, edges
