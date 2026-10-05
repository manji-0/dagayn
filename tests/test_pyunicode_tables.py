"""The Rust tools' copy of Python's Unicode classes
(``crates/dagayn-tools/src/pyunicode.rs``) is this interpreter's answer for
every code point: ``re``'s ``\\w`` and ``\\d``, and ``str.isprintable()``."""

from __future__ import annotations

import re
from collections.abc import Callable
from pathlib import Path

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
