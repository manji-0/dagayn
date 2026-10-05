# Ruff's parser for the Python extractor

## Question

Should the Python extractor in `crates/dagayn-parser/src/python/` (plain
modules, Jupyter notebooks, marimo apps, Databricks exports) parse with
Ruff's parser (`ruff_python_parser` + `ruff_python_ast`) instead of the
tree-sitter Python grammar? This is GitHub issue #181.

Verdict: **yes.** On the same corpora the Ruff-based extractor emits the same
graph except for the differences classified below, all of them deliberate,
recovers more definitions and calls from broken files, and runs the whole
Python extraction about 5.8 times faster.

## Crates and toolchain

- `ruff_python_parser`, `ruff_python_ast`, and `ruff_text_size` `=0.0.16`
  from crates.io (MIT, published by Astral as internal components of Ruff).
  Every 0.0.x release may break the API, so the versions are pinned with `=`
  and upgraded on purpose.
- 0.0.16 needs Rust 1.97, so `rust-toolchain.toml` moved from 1.95.0 to
  1.98.0 (the newest stable) rather than pinning the older 0.0.10.
- Line numbers come from a line index over the source text (`\n` only, as
  tree-sitter rows and `util::line_count` count them); Ruff's nodes carry
  byte ranges only. A source that is not UTF-8 is decoded lossily before it
  is parsed, which keeps every line where it was.
- Ruff's parser has no parent pointers: the walker carries the enclosing
  class's bases (for `super().m()`), and reads decorators from the
  definition itself.

## How it was compared

While both extractors existed (the commit that added the Ruff one kept the
tree-sitter one beside it, selected with `DAGAYN_PYTHON_PARSER`), the
example `crates/dagayn-parser/examples/python_parser_diff.rs` parsed every
`.py`, `.pyi`, and `.ipynb` file of a directory with both through
`RustOwnedParser` and listed, per file, the nodes and edges only one of them
emitted and the node fields that differed. The switch-over removed both the
tree-sitter extractor and the example. Corpora:

| Corpus | Files | Files that differ | Nodes (tree-sitter / Ruff) | Edges (tree-sitter / Ruff) |
|---|---|---|---|---|
| this repository with its root (`dagayn/`, `tests/`, fixtures and notebooks) | 282 | 11 | 5,932 / 5,932 | 45,532 / 45,532 |
| `.venv/lib/python3.14/site-packages`, no root | 6,609 | 268 | 93,573 / 93,571 | 679,171 / 679,236 |

A third corpus measured error recovery: 551 files sampled from both (seed
181), each broken one of three ways (1-4 lines deleted, a line cut in half,
an unclosed `(` appended to a line). Each extractor's definitions
(`kind|parent|name`) and calls (`source|target`) on the broken file were
scored against those of the intact file:

| Mutation | Definitions recall / precision, tree-sitter | Ruff | Calls recall / precision, tree-sitter | Ruff |
|---|---|---|---|---|
| lines deleted | 0.974 / 0.997 | 0.978 / 0.998 | 0.974 / 0.987 | 0.979 / 0.994 |
| line cut in half | 0.895 / 0.976 | 0.973 / 0.993 | 0.922 / 0.934 | 0.973 / 0.976 |
| unclosed `(` | 0.921 / 0.960 | 0.971 / 0.983 | 0.929 / 0.932 | 0.959 / 0.958 |

Ruff recovers better on 65 of the files and tree-sitter on 18.

## Differences, classified

Every difference on the two real corpora falls into one of these classes.
None is a regression; the regressions the first runs showed are listed
under "Fixed while porting".

| Class | Count (repo / site-packages) | Verdict |
|---|---|---|
| A class or function's `line_end` no longer counts the comment lines after its last statement (`# fmt: on`, a section banner for the next test) | 4 / 246 | improvement |
| A receiver typed through a union annotation (`x: dict[str, int] \| None`, `lock: AbstractContextManager[None] \| None = ...`) resolves its member call (`dict.get`, `contextlib.AbstractContextManager.__enter__`, `CaptureBase.start`) instead of `receiver_unknown`; tree-sitter parses such annotations as `union_type`, which the extractor never read | 6 / 139 | improvement |
| Calls tree-sitter never saw: inside `[*map(...)]` / `{*range(...)}` displays, through a parenthesized callee (`(ctypes.c_int)(x)`), and in `type(x).attr = ...`, which tree-sitter misparses as a `type` alias statement | 0 / 48 | improvement |
| `type(x).attr = ...` no longer yields a bogus Type node `(x).attr` | 0 / 2 | improvement |
| A subprocess bridge whose first argument follows a comment (`subprocess.run(  # noqa` ...) reads the argument instead of falling back to a `<dynamic:...>` target | 7 / 2 | improvement |
| A dict value or assignment value in parentheses (`"KEY": (\n handler\n)`) is a REFERENCES edge like the unparenthesized one | 0 / 19 | improvement |
| `return_type` of a parenthesized return annotation (`-> (\n tuple[...] \| None\n)`) is the annotation without the parentheses | 0 / 16 | improvement |
| TESTED_BY edges follow the CALLS changes above | 0 / 4 | follows |

Expected differences the issue listed, and what was decided:

- **Decorated definitions' `line_start`.** Ruff's range of a decorated
  `def`/`class` starts at the first decorator. The extractor keeps the line
  of the `def` / `async` / `class` keyword (found by skipping the decorators,
  comments and blank lines after the last decorator), as tree-sitter gave it,
  so node spans, `nearest_documentation_source`, and notebook cell tagging do
  not move; decorators stay in `extra.decorators`. No difference on the
  corpora.
- **Decoded string literals.** Strings are read through Ruff's decoded value:
  escapes are decoded (`"C:\\x"` is `C:\x`, not `C:\\x`), implicit
  concatenation is one string (`"SELECT * " "FROM t"`), and a bytes literal
  is decoded lossily. This affects bridge targets, wasm module paths, lazy
  export tables, marimo's `_unparsable_cell` source, and the SQL table
  imports of notebooks (`spark.sql`, `mo.sql`): a query split over
  concatenated literals now matches as one. An f-string keeps only its
  literal parts, as before, but an f-string that interpolates a value is no
  longer a fixed bridge target (`open(f"{root}/x.txt")` was `/x.txt`; it is
  now a `<dynamic:...>` target). No difference on the corpora.
- **Error recovery.** See the mutation table. One part needed help: after a
  syntax error (an unexpected indent, typically), Ruff's parser keeps the
  rest of the file in the body of the definition the error is in. The walker
  takes a statement indented no deeper than the keyword of an enclosing
  definition as outside it, and that definition's `line_end` stops at its
  last statement indented deeper. Before this, recall on the deleted-lines
  mutation was 0.790 (definitions) and 0.727 (calls). Only files with syntax
  errors take this path.
- **Python 2 sources** (`print "x"`, `exec "..."`) give the same result.

Smaller deliberate changes, with no hits on the corpora:

- `type X[T] = ...` names the alias `X` (tree-sitter gave `X[T]`).
- `a = b = Foo()` binds `b` only, as tree-sitter's nested assignment did;
  `self.a = self.b = Foo()` types `b` only.
- `from __future__ import ...` stays out of the graph (tree-sitter parsed it
  as a statement of its own, which the extractor never read).
- A `{key: handler for ...}` comprehension's value is a REFERENCES edge, as
  tree-sitter's `pair` node made it.

## Fixed while porting

Regressions the first differential runs showed, fixed before the switch:

- `from __future__ import annotations` produced an IMPORTS_FROM edge in every
  file and made `annotations` an import alias (REFERENCES to it).
- A marimo cell's body was cut from the first statement's first character,
  so the dedent saw no common indentation and Ruff nested the rest of the
  cell in its first definition. The cell source now starts at the line.
- The re-nesting after syntax errors above.

## Performance

Release build, macOS arm64, `RustOwnedParser::parse_file_in_repo` over all
files, best of two runs, maximum resident set size from `/usr/bin/time -l`
(it includes the sources the benchmark holds in memory):

| Corpus | tree-sitter | Ruff |
|---|---|---|
| site-packages (6,609 files, 72 MB) | 7.17 s, 123 MB | 1.23 s, 113 MB |
| this repository (282 files, 3.6 MB) | 0.45 s, 17.5 MB | 0.08 s, 14.8 MB |

Ruff's parser is recursive; it overflows the 8 MB main-thread stack at
about 50,000 chained binary operators on one line (`1+1+...`), where the
tree-sitter extractor's walk already overflowed at 5,000.
