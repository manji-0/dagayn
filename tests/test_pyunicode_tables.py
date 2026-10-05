"""The Rust tools' copy of Python's Unicode classes
(``crates/dagayn-tools/src/pyunicode.rs``) is this interpreter's answer for
every code point: ``re``'s ``\\w`` and ``\\d``, ``str.isprintable()``, and
``str.casefold()``."""

from __future__ import annotations

import re
import unicodedata
from collections.abc import Callable
from pathlib import Path

import pytest

#: The Unicode version the tables were taken from; other interpreters' classes
#: differ in the code points added since.
UNIDATA = "16.0.0"

TABLES = Path(__file__).resolve().parent.parent / "crates/dagayn-tools/src/pyunicode.rs"

_WORD = re.compile(r"\w")
_DIGIT = re.compile(r"\d")


def _ranges(member: Callable[[str], bool]) -> list[tuple[int, int]]:
    ranges: list[tuple[int, int]] = []
    start: int | None = None
    for cp in range(0x110000):
        if member(chr(cp)):
            if start is None:
                start = cp
        elif start is not None:
            ranges.append((start, cp - 1))
            start = None
    if start is not None:
        ranges.append((start, 0x10FFFF))
    return ranges


def _table(source: str, name: str) -> list[tuple[int, int]]:
    body = re.search(rf"const {name}: &\[\(u32, u32\)\] = &\[(.*?)\];", source, re.S)
    assert body is not None, name
    return [
        (int(a, 16), int(b, 16))
        for a, b in re.findall(r"\(0x([0-9A-Fa-f]+),\s*0x([0-9A-Fa-f]+)\)", body.group(1))
    ]


def _render(name: str, ranges: list[tuple[int, int]]) -> str:
    rows = [f"(0x{a:04X}, 0x{b:04X})," for a, b in ranges]
    lines = ["    " + " ".join(rows[i : i + 4]) for i in range(0, len(rows), 4)]
    header = ["#[rustfmt::skip]", f"pub(crate) const {name}: &[(u32, u32)] = &["]
    return "\n".join([*header, *lines, "];"])


@pytest.mark.skipif(
    unicodedata.unidata_version != UNIDATA,
    reason=f"the tables are Unicode {UNIDATA}'s (CPython 3.14)",
)
def test_the_rust_tables_are_this_pythons_unicode_classes() -> None:
    source = TABLES.read_text(encoding="utf-8")
    expected = {
        "WORD": _ranges(lambda c: _WORD.match(c) is not None),
        "DIGIT": _ranges(lambda c: _DIGIT.match(c) is not None),
        "NOT_PRINTABLE": _ranges(lambda c: not c.isprintable()),
    }
    stale = [name for name, ranges in expected.items() if _table(source, name) != ranges]
    regenerated = "\n\n".join(_render(name, expected[name]) for name in stale)
    assert not stale, f"replace these tables in {TABLES}:\n{regenerated}"


def _casefolds() -> list[tuple[int, list[int]]]:
    """Every code point ``str.casefold`` changes, with what it folds to."""
    return [
        (cp, [ord(c) for c in chr(cp).casefold()])
        for cp in range(0x110000)
        if chr(cp).casefold() != chr(cp)
    ]


def _casefold_table(source: str) -> list[tuple[int, list[int]]]:
    body = re.search(r"const CASEFOLD: &\[\(u32, \[u32; 3\]\)\] = &\[(.*?)\];", source, re.S)
    assert body is not None, "CASEFOLD"
    return [
        (int(cp, 16), [int(x, 16) for x in folded.split(",") if int(x.strip(), 16)])
        for cp, folded in re.findall(r"\(0x([0-9A-Fa-f]+),\s*\[([^\]]*)\]\)", body.group(1))
    ]


def _render_casefold(entries: list[tuple[int, list[int]]]) -> str:
    rows = [
        f"(0x{cp:04X}, [{', '.join(f'0x{x:04X}' for x in [*folded, 0, 0][:3])}]),"
        for cp, folded in entries
    ]
    lines = ["    " + " ".join(rows[i : i + 2]) for i in range(0, len(rows), 2)]
    header = ["#[rustfmt::skip]", "pub(crate) const CASEFOLD: &[(u32, [u32; 3])] = &["]
    return "\n".join([*header, *lines, "];"])


@pytest.mark.skipif(
    unicodedata.unidata_version != UNIDATA,
    reason=f"the tables are Unicode {UNIDATA}'s (CPython 3.14)",
)
def test_the_rust_casefold_table_is_this_pythons_casefold() -> None:
    expected = _casefolds()
    assert max(len(folded) for _, folded in expected) <= 3
    source = TABLES.read_text(encoding="utf-8")
    assert _casefold_table(source) == expected, (
        f"replace CASEFOLD in {TABLES}:\n{_render_casefold(expected)}"
    )
